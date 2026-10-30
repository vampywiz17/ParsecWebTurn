//! NDK asynchronous MediaCodec -> original Android Surface. No decoded output
//! buffer is mapped, copied to the CPU, uploaded, or passed across JNI.
use crate::{
    codec_initialization::{Geometry, Initialization},
    video_output::{NativeOperation, Queue},
    video_stream::{Codec, PacketInfo, ParameterCache},
};
use anyhow::{ensure, Context, Result};
use ndk_sys as ndk;
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    ffi::{c_void, CString},
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex, OnceLock,
    },
    thread,
    time::Instant,
};

static ACTIVE: AtomicUsize = AtomicUsize::new(0);
struct Lease;
impl Drop for Lease {
    fn drop(&mut self) {
        ACTIVE.fetch_sub(1, Ordering::AcqRel);
    }
}

pub struct NativeWindow(pub *mut ndk::ANativeWindow);
// ANativeWindow is a documented reference-counted cross-thread producer endpoint.
unsafe impl Send for NativeWindow {}
impl Drop for NativeWindow {
    fn drop(&mut self) {
        unsafe {
            ndk::ANativeWindow_release(self.0);
        }
    }
}

struct MediaFormat(*mut ndk::AMediaFormat);
impl Drop for MediaFormat {
    fn drop(&mut self) {
        unsafe {
            ndk::AMediaFormat_delete(self.0);
        }
    }
}

#[derive(Clone, Deserialize)]
pub struct Choice {
    pub name: String,
    pub low_latency: bool,
}
#[derive(Clone, Deserialize)]
pub struct Choices {
    pub avc: Choice,
    pub hevc: Option<Choice>,
    pub main10: bool,
}

#[derive(Default)]
struct Counters {
    input_callbacks: AtomicU64,
    output_callbacks: AtomicU64,
    inputs: AtomicU64,
    outputs: AtomicU64,
    skipped: AtomicU64,
    stage: AtomicU32,
    decoder: Mutex<Option<String>>,
    width: AtomicU32,
    height: AtomicU32,
    depth: AtomicU32,
    hevc: AtomicBool,
    error: Mutex<Option<String>>,
}
impl Counters {
    fn fail(&self, stage: &str, code: i32) {
        let mut error = self.error.lock().unwrap_or_else(|e| e.into_inner());
        if error.is_none() {
            *error = Some(format!("{stage} ({code})"));
        }
    }
    fn check(&self, stage: &str, code: i32) -> Result<()> {
        if code == 0 {
            return Ok(());
        }
        // Publish before stop/delete, which may itself block inside a driver.
        self.fail(stage, code);
        anyhow::bail!("{stage} ({code})")
    }
}
#[derive(Serialize)]
pub struct Snapshot {
    pub input_callbacks: u64,
    pub output_callbacks: u64,
    pub inputs: u64,
    pub submitted: u64,
    pub stage: &'static str,
    pub decoder: Option<String>,
    pub queued: usize,
    pub dropped: u64,
    pub skipped: u64,
    pub queue_wait_ms: f64,
    pub recovery_reason: Option<&'static str>,
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub codec: &'static str,
    pub error: Option<String>,
}
struct State {
    queue: Mutex<Queue>,
    thread: OnceLock<thread::Thread>,
    counters: Arc<Counters>,
}
pub struct Pipeline {
    state: Arc<State>,
    started: Instant,
}
impl Pipeline {
    pub fn start(window: NativeWindow, choices: Choices) -> Result<Arc<Self>> {
        ensure!(ACTIVE.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire).is_ok(),
            "The previous video decoder is still closing. Retry shortly; restart the app if it persists.");
        let lease = Lease;
        let state = Arc::new(State {
            queue: Mutex::new(Queue::with_display_policy(
                crate::video_output::DisplayCatchUpPolicy::Adaptive,
            )),
            thread: OnceLock::new(),
            counters: Arc::default(),
        });
        let worker = state.clone();
        thread::Builder::new()
            .name("android-video".into())
            .spawn(move || {
                let _lease = lease;
                let _ = worker.thread.set(thread::current());
                if let Err(error) = run(&worker, &window, choices) {
                    let mut slot = worker
                        .counters
                        .error
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    if slot.is_none() {
                        *slot = Some(error.to_string());
                    }
                }
                // window and callback contexts remain owned here until MediaCodec
                // stop/delete finish; a stuck driver is never force-freed/reused.
            })
            .context("Could not start the Android video worker")?;
        Ok(Arc::new(Self {
            state,
            started: Instant::now(),
        }))
    }
    pub fn submit(&self, bytes: &[u8], info: PacketInfo) -> bool {
        let mut q = self.state.queue.lock().unwrap_or_else(|e| e.into_inner());
        let mut data = q.take_buffer();
        data.extend_from_slice(bytes);
        let timestamp = self
            .started
            .elapsed()
            .as_micros()
            .saturating_mul(10)
            .min(i64::MAX as u128) as i64;
        let accepted = q.submit_codec(data, info.idr, info.parameters, timestamp, info.codec);
        if accepted {
            q.set_last_packet(0, info.picture);
        }
        drop(q);
        if let Some(t) = self.state.thread.get() {
            t.unpark();
        }
        accepted
    }
    pub fn stop(&self) {
        self.state
            .queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .stopped = true;
        if let Some(t) = self.state.thread.get() {
            t.unpark();
        }
    }
    pub fn take_keyframe_request(&self) -> bool {
        let mut queue = self.state.queue.lock().unwrap_or_else(|e| e.into_inner());
        queue.check_native_call(Instant::now());
        queue.take_keyframe_request(Instant::now()).is_some()
    }
    pub fn snapshot(&self) -> Snapshot {
        let c = &self.state.counters;
        let native_error = c.error.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let mut queue = self.state.queue.lock().unwrap_or_else(|e| e.into_inner());
        queue.check_native_call(Instant::now());
        let error = native_error.or_else(|| queue.report.failure_stage.map(str::to_owned));
        Snapshot {
            input_callbacks: c.input_callbacks.load(Ordering::Relaxed),
            output_callbacks: c.output_callbacks.load(Ordering::Relaxed),
            inputs: c.inputs.load(Ordering::Relaxed),
            submitted: c.outputs.load(Ordering::Relaxed),
            stage: match c.stage.load(Ordering::Relaxed) {
                1 => "creating codec",
                2 => "configuring Surface",
                3 => "starting codec",
                4 => "waiting for input buffer",
                5 => "submitting compressed input",
                6 => "releasing output to Surface",
                _ => "waiting for stream parameters",
            },
            decoder: c.decoder.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            queued: queue.report.queue_depth,
            dropped: queue.report.frames_dropped,
            skipped: c.skipped.load(Ordering::Relaxed),
            queue_wait_ms: queue.report.local_timing.compressed_wait.latest_ms,
            recovery_reason: queue.report.recovery_reason,
            width: c.width.load(Ordering::Relaxed),
            height: c.height.load(Ordering::Relaxed),
            depth: c.depth.load(Ordering::Relaxed),
            codec: if c.hevc.load(Ordering::Relaxed) {
                "HEVC"
            } else {
                "H.264"
            },
            error,
        }
    }
}
impl Drop for Pipeline {
    fn drop(&mut self) {
        self.stop();
    }
}

struct Callbacks {
    slots: Mutex<VecDeque<usize>>,
    outputs: Mutex<crate::decode_state::OutputIndices>,
    thread: thread::Thread,
    counters: Arc<Counters>,
    stopping: AtomicBool,
}
unsafe extern "C" fn input(_codec: *mut ndk::AMediaCodec, data: *mut c_void, index: i32) {
    let c = unsafe { &*data.cast::<Callbacks>() };
    if c.stopping.load(Ordering::Acquire) {
        return;
    }
    c.counters.input_callbacks.fetch_add(1, Ordering::Relaxed);
    let mut slots = c.slots.lock().unwrap_or_else(|e| e.into_inner());
    if index < 0 || slots.len() >= 128 {
        c.counters.fail("video-input-slots", index);
    } else {
        slots.push_back(index as usize);
    }
    drop(slots);
    c.thread.unpark();
}
unsafe extern "C" fn output(
    _codec: *mut ndk::AMediaCodec,
    data: *mut c_void,
    index: i32,
    info: *mut ndk::AMediaCodecBufferInfo,
) {
    let c = unsafe { &*data.cast::<Callbacks>() };
    if c.stopping.load(Ordering::Acquire) {
        return;
    }
    c.counters.output_callbacks.fetch_add(1, Ordering::Relaxed);
    if index < 0 || info.is_null() {
        c.counters.fail("video-output-index", index);
        c.thread.unpark();
        return;
    }
    let render = unsafe { (*info).flags } & ndk::AMEDIACODEC_BUFFER_FLAG_CODEC_CONFIG == 0;
    if !c
        .outputs
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(index as usize, render)
    {
        c.counters.fail("video-output-index-capacity", index);
    }
    // No synchronous MediaCodec API on its internal callback looper. In
    // particular, releaseOutputBuffer waits for work on that same looper.
    c.thread.unpark();
}
unsafe extern "C" fn format(
    _codec: *mut ndk::AMediaCodec,
    data: *mut c_void,
    f: *mut ndk::AMediaFormat,
) {
    if f.is_null() {
        return;
    }
    // The NDK gives the callback a new format copy. Release it even when
    // stopping; it is metadata only, never a decoded output buffer.
    let _owned = MediaFormat(f);
    let c = unsafe { &*data.cast::<Callbacks>() };
    if c.stopping.load(Ordering::Acquire) {
        return;
    }
    let get = |key: &std::ffi::CStr| -> Option<i32> {
        let mut v = 0;
        unsafe { ndk::AMediaFormat_getInt32(f, key.as_ptr(), &mut v) }.then_some(v)
    };
    let width = get(c"crop-right")
        .zip(get(c"crop-left"))
        .and_then(|(r, l)| r.checked_sub(l)?.checked_add(1))
        .or_else(|| get(c"width"));
    let height = get(c"crop-bottom")
        .zip(get(c"crop-top"))
        .and_then(|(b, t)| b.checked_sub(t)?.checked_add(1))
        .or_else(|| get(c"height"));
    if let (Some(w), Some(h)) = (width, height) {
        if (1..=8192).contains(&w) && (1..=8192).contains(&h) {
            c.counters.width.store(w as u32, Ordering::Relaxed);
            c.counters.height.store(h as u32, Ordering::Relaxed);
        }
    }
}
unsafe extern "C" fn error(
    _codec: *mut ndk::AMediaCodec,
    data: *mut c_void,
    code: ndk::media_status_t,
    _action: i32,
    _detail: *const std::ffi::c_char,
) {
    let c = unsafe { &*data.cast::<Callbacks>() };
    if !c.stopping.load(Ordering::Acquire) {
        c.counters.fail("video-codec-callback", code.0);
        c.thread.unpark();
    }
}

struct Decoder {
    codec: *mut ndk::AMediaCodec,
    kind: Codec,
    callbacks: Box<Callbacks>,
    geometry: Geometry,
}
impl Decoder {
    fn open(
        window: &NativeWindow,
        choice: &Choice,
        config: Initialization,
        codec: Codec,
        counters: Arc<Counters>,
    ) -> Result<Self> {
        ensure!(
            !choice.name.is_empty() && choice.name.len() <= 256,
            "Invalid hardware decoder name"
        );
        let name = CString::new(choice.name.as_str())?;
        *counters.decoder.lock().unwrap_or_else(|e| e.into_inner()) = Some(choice.name.clone());
        counters.stage.store(1, Ordering::Relaxed);
        let native = unsafe { ndk::AMediaCodec_createCodecByName(name.as_ptr()) };
        ensure!(
            !native.is_null(),
            "Could not open the declared Android hardware decoder"
        );
        let mut decoder = Self {
            codec: native,
            kind: codec,
            geometry: config.geometry,
            callbacks: Box::new(Callbacks {
                slots: Mutex::new(VecDeque::with_capacity(128)),
                outputs: Mutex::default(),
                thread: thread::current(),
                counters: counters.clone(),
                stopping: AtomicBool::new(false),
            }),
        };
        let callbacks = ndk::AMediaCodecOnAsyncNotifyCallback {
            onAsyncInputAvailable: Some(input),
            onAsyncOutputAvailable: Some(output),
            onAsyncFormatChanged: Some(format),
            onAsyncError: Some(error),
        };
        let result = unsafe {
            ndk::AMediaCodec_setAsyncNotifyCallback(
                native,
                callbacks,
                (&mut *decoder.callbacks as *mut Callbacks).cast(),
            )
            .0
        };
        counters.check("video-async-setup", result)?;
        let f = unsafe { ndk::AMediaFormat_new() };
        ensure!(!f.is_null(), "MediaCodec format allocation failed");
        let owned = MediaFormat(f);
        unsafe {
            ndk::AMediaFormat_setString(
                f,
                c"mime".as_ptr(),
                match codec {
                    Codec::H264 => c"video/avc",
                    Codec::Hevc => c"video/hevc",
                }
                .as_ptr(),
            );
            ndk::AMediaFormat_setInt32(f, c"width".as_ptr(), config.geometry.width as i32);
            ndk::AMediaFormat_setInt32(f, c"height".as_ptr(), config.geometry.height as i32);
            ndk::AMediaFormat_setInt32(
                f,
                c"max-input-size".as_ptr(),
                crate::control::MAX_CHANNEL_MESSAGE as i32,
            );
            ndk::AMediaFormat_setInt32(f, c"priority".as_ptr(), 0);
            if choice.low_latency {
                ndk::AMediaFormat_setInt32(f, c"low-latency".as_ptr(), 1);
            }
            ndk::AMediaFormat_setBuffer(
                f,
                c"csd-0".as_ptr(),
                config.csd0.as_ptr().cast(),
                config.csd0.len(),
            );
            if !config.csd1.is_empty() {
                ndk::AMediaFormat_setBuffer(
                    f,
                    c"csd-1".as_ptr(),
                    config.csd1.as_ptr().cast(),
                    config.csd1.len(),
                );
            }
        }
        counters.stage.store(2, Ordering::Relaxed);
        let result =
            unsafe { ndk::AMediaCodec_configure(native, f, window.0, std::ptr::null_mut(), 0).0 };
        drop(owned);
        counters.check("video-surface-configuration", result)?;
        counters
            .width
            .store(config.geometry.visible_width, Ordering::Relaxed);
        counters
            .height
            .store(config.geometry.visible_height, Ordering::Relaxed);
        counters
            .depth
            .store(config.geometry.depth, Ordering::Relaxed);
        counters.hevc.store(codec == Codec::Hevc, Ordering::Relaxed);
        counters.stage.store(3, Ordering::Relaxed);
        let result = unsafe { ndk::AMediaCodec_start(native).0 };
        counters.check("video-codec-start", result)?;
        counters.stage.store(4, Ordering::Relaxed);
        Ok(decoder)
    }
    fn present(&self) -> Result<()> {
        crate::decode_state::drain_outputs(&self.callbacks.outputs, |output| {
            self.callbacks.counters.stage.store(6, Ordering::Relaxed);
            let result = unsafe {
                ndk::AMediaCodec_releaseOutputBuffer(self.codec, output.index, output.render).0
            };
            self.callbacks
                .counters
                .check("video-surface-release", result)?;
            if output.render {
                self.callbacks
                    .counters
                    .outputs
                    .fetch_add(1, Ordering::Relaxed);
            }
            if output.skipped {
                self.callbacks
                    .counters
                    .skipped
                    .fetch_add(1, Ordering::Relaxed);
            }
            self.callbacks.counters.stage.store(4, Ordering::Relaxed);
            Ok(())
        })
    }
    fn feed(&mut self, frame: &crate::video_output::Frame) -> Result<bool> {
        let Some(index) = self
            .callbacks
            .slots
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pop_front()
        else {
            return Ok(false);
        };
        self.callbacks.counters.stage.store(5, Ordering::Relaxed);
        let mut capacity = 0;
        let buffer = unsafe { ndk::AMediaCodec_getInputBuffer(self.codec, index, &mut capacity) };
        if buffer.is_null() || capacity < frame.bytes.len() {
            let message = format!(
                "video-input-capacity: {} compressed bytes, buffer {capacity}",
                frame.bytes.len()
            );
            let mut error = self
                .callbacks
                .counters
                .error
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if error.is_none() {
                *error = Some(message.clone());
            }
            anyhow::bail!("{message}");
        }
        unsafe {
            std::ptr::copy_nonoverlapping(frame.bytes.as_ptr(), buffer, frame.bytes.len());
        }
        let flags = if frame.parameters && !frame.picture {
            ndk::AMEDIACODEC_BUFFER_FLAG_CODEC_CONFIG
        } else if frame.idr {
            ndk::AMEDIACODEC_BUFFER_FLAG_KEY_FRAME
        } else {
            0
        };
        let result = unsafe {
            ndk::AMediaCodec_queueInputBuffer(
                self.codec,
                index,
                0,
                frame.bytes.len(),
                (frame.timestamp_100ns.max(0) / 10) as u64,
                flags,
            )
            .0
        };
        self.callbacks
            .counters
            .check("video-compressed-input", result)?;
        self.callbacks
            .counters
            .inputs
            .fetch_add(1, Ordering::Relaxed);
        self.callbacks.counters.stage.store(4, Ordering::Relaxed);
        Ok(true)
    }
}
impl Drop for Decoder {
    fn drop(&mut self) {
        self.callbacks.stopping.store(true, Ordering::Release);
        unsafe {
            ndk::AMediaCodec_stop(self.codec);
            ndk::AMediaCodec_delete(self.codec);
        }
        // Callbacks/context and Surface stay alive through stop/delete.
    }
}

// Setup is an explicitly bounded native operation, not three failed decodes.
// The queue stays bounded and requests a fresh IDR if setup falls behind.
// A stalled driver keeps owning its resources; no forced free/cancellation.
struct Setup<'a>(&'a State);
impl<'a> Setup<'a> {
    fn begin(state: &'a State) -> Self {
        let mut queue = state.queue.lock().unwrap_or_else(|e| e.into_inner());
        queue.optional_setup(true);
        queue.begin_native_call(NativeOperation::Setup, Instant::now());
        Self(state)
    }
}
impl Drop for Setup<'_> {
    fn drop(&mut self) {
        let mut queue = self.0.queue.lock().unwrap_or_else(|e| e.into_inner());
        queue.finish_native_call(Instant::now());
        queue.optional_setup(false);
    }
}

fn run(state: &State, window: &NativeWindow, choices: Choices) -> Result<()> {
    let mut cache = ParameterCache::default();
    let mut decoder: Option<Decoder> = None;
    let mut pending: Option<crate::video_output::Frame> = None;
    let mut prepared = false;
    let mut waiting_idr = true;
    let mut previous_codec = None;
    let mut generation = 0;
    loop {
        {
            let mut queue = state.queue.lock().unwrap_or_else(|e| e.into_inner());
            if queue.stopped || queue.report.failure_stage.is_some() {
                return Ok(());
            }
            if crate::decode_state::retire_pending(&mut queue, &mut generation, &mut pending) {
                prepared = false;
                waiting_idr = true;
            }
            if pending.is_none() {
                pending = queue.pop();
                prepared = false;
            }
        }
        // Drain even when no new compressed input arrived, but honor local
        // cancellation before rendering any remaining native output indices.
        if let Some(decoder) = decoder.as_ref() {
            decoder.present()?;
        }
        if state
            .counters
            .error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
        {
            return Ok(());
        }
        let Some(frame) = &pending else {
            thread::park();
            continue;
        };
        if !prepared && previous_codec != Some(frame.codec) {
            cache = ParameterCache::default();
            previous_codec = Some(frame.codec);
        }
        if !prepared && frame.parameters {
            cache.observe(&frame.bytes, frame.codec);
            if let Some(config) = cache.decoder_initialization(frame.codec) {
                if decoder
                    .as_ref()
                    .is_none_or(|d| d.geometry != config.geometry || d.kind != frame.codec)
                {
                    let setup = Setup::begin(state);
                    drop(decoder.take());
                    let choice = match frame.codec {
                        Codec::H264 => &choices.avc,
                        Codec::Hevc => choices
                            .hevc
                            .as_ref()
                            .context("Host sent HEVC without a hardware decoder")?,
                    };
                    decoder = Some(Decoder::open(
                        window,
                        choice,
                        config,
                        frame.codec,
                        state.counters.clone(),
                    )?);
                    drop(setup);
                    waiting_idr = true;
                    {
                        let mut queue = state.queue.lock().unwrap_or_else(|e| e.into_inner());
                        // A queued IDR already terminates the reference gap.
                        // Re-arming ingress gating here would drop its following
                        // deltas while the worker is still opening the codec.
                        let needs_keyframe = !frame.idr && !queue.has_queued_idr();
                        queue.begin_format_change(Instant::now(), needs_keyframe);
                    }
                    // CSD in MediaFormat is submitted automatically by start().
                    if !frame.picture {
                        let frame = pending.take().unwrap();
                        state
                            .queue
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .recycle(frame.bytes);
                        continue;
                    }
                    // Recheck the queue generation after a slow native setup
                    // before submitting the access unit retained across it.
                    prepared = true;
                    continue;
                }
            }
        }
        prepared = true;
        if frame.picture && waiting_idr {
            if !frame.idr {
                let frame = pending.take().unwrap();
                state
                    .queue
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .recycle(frame.bytes);
                continue;
            }
            waiting_idr = false;
            state
                .queue
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .recovered();
        }
        if let Some(d) = decoder.as_mut() {
            if !d.feed(frame)? {
                thread::park();
                continue;
            }
        } else if frame.picture {
            anyhow::bail!("Video parameters incomplete; reconnect to request a fresh keyframe");
        }
        let frame = pending.take().unwrap();
        state
            .queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .recycle(frame.bytes);
    }
}
