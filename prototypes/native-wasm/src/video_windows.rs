//! Documented Media Foundation H.264 -> D3D11 NV12 -> DXGI presentation.
//! COM and all GPU objects are created, used and released on one worker thread.
use crate::video_output::{Frame, Queue, Snapshot};
use std::{
    mem::ManuallyDrop,
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};
use windows::{
    core::Interface,
    Win32::{
        Foundation::{HMODULE, HWND, RECT},
        Graphics::{
            Direct3D::*,
            Direct3D10::ID3D10Multithread,
            Direct3D11::*,
            Dxgi::{Common::*, *},
        },
        Media::MediaFoundation::*,
        System::{Com::*, Variant::*},
    },
};

#[derive(Debug)]
struct Failure {
    stage: &'static str,
    hresult: Option<String>,
}
type Result<T> = std::result::Result<T, Failure>;
fn api<T>(stage: &'static str, value: windows::core::Result<T>) -> Result<T> {
    value.map_err(|e| Failure {
        stage,
        hresult: Some(format!("0x{:08X}", e.code().0 as u32)),
    })
}
fn unavailable(stage: &'static str) -> Failure {
    Failure {
        stage,
        hresult: None,
    }
}

type Shared = Arc<(Mutex<Queue>, Condvar)>;
pub struct Pipeline {
    shared: Shared,
    started: Instant,
}
impl Pipeline {
    pub fn start(window: Arc<crate::window::Window>) -> Self {
        Self::start_mode(window, false)
    }
    fn start_mode(window: Arc<crate::window::Window>, verify_synthetic_pixels: bool) -> Self {
        let shared = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        shared
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .report
            .enabled = true;
        let state = shared.clone();
        // Guard HWND destruction even if thread startup or device creation fails.
        window
            .active_contexts
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        let activity = Activity(window.clone());
        let error_state = shared.clone();
        let spawned = std::thread::Builder::new()
            .name("native-video-d3d11".into())
            .spawn(move || {
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run(&state, &window, verify_synthetic_pixels)
                }));
                let mut q = state.0.lock().unwrap_or_else(|e| e.into_inner());
                match outcome {
                    Ok(Err(failure)) => q.fail(failure.stage, failure.hresult),
                    Err(_) => q.fail("video-worker-panic", None),
                    Ok(Ok(())) => {}
                }
                q.clear();
                q.stopped = true;
                q.report.worker_finished = true;
                q.report.resources_released = true;
                window.set_video_ready(false);
                *window
                    .video_report
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = Some(q.report.clone());
                drop(activity);
            });
        if spawned.is_err() {
            let mut q = error_state.0.lock().unwrap_or_else(|e| e.into_inner());
            q.fail("video-worker-start", None);
            q.stopped = true;
            q.report.worker_finished = true;
            q.report.resources_released = true;
        }
        Self {
            shared,
            started: Instant::now(),
        }
    }
    pub fn submit(&self, bytes: &[u8], info: crate::video_stream::PacketInfo) -> bool {
        let timestamp = i64::try_from(self.started.elapsed().as_nanos() / 100).unwrap_or(i64::MAX);
        let accepted = self
            .shared
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .submit(bytes, info.idr, info.parameters, timestamp);
        self.shared.1.notify_one();
        accepted
    }
    pub fn snapshot(&self) -> Snapshot {
        self.shared
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .report
            .clone()
    }
    pub fn stop(&self) {
        self.shared
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .stopped = true;
        self.shared.1.notify_all();
    }
}
impl Drop for Pipeline {
    fn drop(&mut self) {
        self.stop();
    }
}
struct Activity(Arc<crate::window::Window>);
impl Drop for Activity {
    fn drop(&mut self) {
        self.0
            .active_contexts
            .fetch_sub(1, std::sync::atomic::Ordering::Release);
    }
}

fn update(shared: &Shared, f: impl FnOnce(&mut Snapshot)) {
    f(&mut shared.0.lock().unwrap_or_else(|e| e.into_inner()).report);
}
fn run(
    shared: &Shared,
    window: &Arc<crate::window::Window>,
    verify_synthetic_pixels: bool,
) -> Result<()> {
    let mut session = None;
    loop {
        let frame = {
            let mut q = shared.0.lock().unwrap_or_else(|e| e.into_inner());
            while q.report.queue_depth == 0
                && !q.stopped
                && q.report.failure_stage.is_none()
                && !window.closing.load(std::sync::atomic::Ordering::Acquire)
            {
                q = shared
                    .1
                    .wait_timeout(q, Duration::from_millis(100))
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
            }
            if q.stopped
                || q.report.failure_stage.is_some()
                || window.closing.load(std::sync::atomic::Ordering::Acquire)
            {
                break;
            }
            q.pop().ok_or_else(|| unavailable("video-queue-state"))?
        };
        if session.is_none() {
            let hwnd = window
                .ensure_video_surface()
                .map_err(|_| unavailable("video-surface-create"))?;
            let native = unsafe { Session::create(HWND(hwnd as *mut _), verify_synthetic_pixels)? };
            update(shared, |s| {
                s.decoder_initialized = true;
                s.low_latency_request_accepted = native.low_latency_request_accepted;
                s.decoder = Some("Media Foundation H.264");
                s.renderer = Some("D3D11 VideoProcessor / DXGI");
                s.adapter = Some(native.renderer.adapter.clone());
            });
            session = Some(native);
        }
        let feed_started = Instant::now();
        let result = unsafe { session.as_mut().unwrap().feed(&frame, shared, window) };
        update(shared, |s| {
            s.decode_feed_max_us = s
                .decode_feed_max_us
                .max(feed_started.elapsed().as_micros().min(u64::MAX as u128) as u64)
        });
        result?;
        *window
            .video_report
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(
            shared
                .0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .report
                .clone(),
        );
    }
    // Release the transform before the device manager, renderer and MF runtime.
    drop(session);
    Ok(())
}

struct Runtime;
impl Runtime {
    unsafe fn start() -> Result<Self> {
        api(
            "video-com-start",
            CoInitializeEx(None, COINIT_MULTITHREADED).ok(),
        )?;
        if let Err(e) = api("video-mf-start", MFStartup(MF_VERSION, MFSTARTUP_FULL)) {
            CoUninitialize();
            return Err(e);
        }
        Ok(Self)
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        unsafe {
            let _ = MFShutdown();
            CoUninitialize();
        }
    }
}

struct Session {
    low_latency_request_accepted: bool,
    decoder: IMFTransform,
    _manager: IMFDXGIDeviceManager,
    renderer: Renderer,
    _runtime: Runtime,
}
impl Session {
    unsafe fn create(hwnd: HWND, verify_synthetic_pixels: bool) -> Result<Self> {
        let runtime = Runtime::start()?;
        let mut renderer = Renderer::create(hwnd)?;
        renderer.verify_synthetic_pixels = verify_synthetic_pixels;
        let mut token = 0;
        let mut manager = None;
        api(
            "video-dxgi-manager",
            MFCreateDXGIDeviceManager(&mut token, &mut manager),
        )?;
        let manager = manager.ok_or_else(|| unavailable("video-dxgi-manager-null"))?;
        api(
            "video-dxgi-reset",
            manager.ResetDevice(&renderer.device, token),
        )?;
        let decoder: IMFTransform = api(
            "video-decoder-create",
            CoCreateInstance(&CMSH264DecoderMFT, None, CLSCTX_INPROC_SERVER),
        )?;
        let attrs = api("video-decoder-attributes", decoder.GetAttributes())?;
        if api(
            "video-decoder-d3d11-aware",
            attrs.GetUINT32(&MF_SA_D3D11_AWARE),
        )? == 0
        {
            return Err(unavailable("video-decoder-not-d3d11-aware"));
        }
        // The documented Microsoft H.264 decoder property uses VT_UI4 (unlike
        // other codecs' VT_BOOL). Request through ICodecAPI; rejection is optional.
        let low_latency_request_accepted = decoder.cast::<ICodecAPI>().is_ok_and(|codec| {
            let value = VARIANT {
                Anonymous: VARIANT_0 {
                    Anonymous: ManuallyDrop::new(VARIANT_0_0 {
                        vt: VT_UI4,
                        Anonymous: VARIANT_0_0_0 { ulVal: 1 },
                        ..Default::default()
                    }),
                },
            };
            codec.SetValue(&CODECAPI_AVLowLatencyMode, &value).is_ok()
        });
        api(
            "video-decoder-set-manager",
            decoder.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, manager.as_raw() as usize),
        )?;
        let input = api("video-input-type-create", MFCreateMediaType())?;
        api(
            "video-input-major",
            input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video),
        )?;
        api(
            "video-input-h264",
            input.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264),
        )?;
        api("video-input-type", decoder.SetInputType(0, &input, 0))?;
        let mut result = Self {
            low_latency_request_accepted,
            decoder,
            _manager: manager,
            renderer,
            _runtime: runtime,
        };
        result.output_type()?;
        api(
            "video-begin-streaming",
            result
                .decoder
                .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0),
        )?;
        api(
            "video-start-stream",
            result
                .decoder
                .ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0),
        )?;
        Ok(result)
    }
    unsafe fn output_type(&mut self) -> Result<()> {
        for index in 0..32 {
            let media = api(
                "video-output-enumerate",
                self.decoder.GetOutputAvailableType(0, index),
            )?;
            if api("video-output-subtype", media.GetGUID(&MF_MT_SUBTYPE))? == MFVideoFormat_NV12 {
                api(
                    "video-output-set-nv12",
                    self.decoder.SetOutputType(0, &media, 0),
                )?;
                let stream = api(
                    "video-output-stream-info",
                    self.decoder.GetOutputStreamInfo(0),
                )?;
                if stream.dwFlags
                    & ((MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0
                        | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0) as u32)
                    == 0
                {
                    return Err(unavailable("video-output-requires-cpu-buffer"));
                }
                return Ok(());
            }
        }
        Err(unavailable("video-output-nv12-unavailable"))
    }
    unsafe fn feed(
        &mut self,
        frame: &Frame,
        shared: &Shared,
        window: &crate::window::Window,
    ) -> Result<()> {
        let buffer = api(
            "video-input-buffer",
            MFCreateMemoryBuffer(frame.bytes.len() as u32),
        )?;
        let mut ptr = std::ptr::null_mut();
        api("video-input-lock", buffer.Lock(&mut ptr, None, None))?;
        if ptr.is_null() {
            let _ = buffer.Unlock();
            return Err(unavailable("video-input-null-buffer"));
        }
        std::ptr::copy_nonoverlapping(frame.bytes.as_ptr(), ptr, frame.bytes.len());
        api("video-input-unlock", buffer.Unlock())?;
        api(
            "video-input-length",
            buffer.SetCurrentLength(frame.bytes.len() as u32),
        )?;
        let sample = api("video-input-sample", MFCreateSample())?;
        api("video-input-add-buffer", sample.AddBuffer(&buffer))?;
        api(
            "video-input-timestamp",
            sample.SetSampleTime(frame.timestamp_100ns),
        )?;
        if frame.idr {
            api(
                "video-input-clean-point",
                sample.SetUINT32(&MFSampleExtension_CleanPoint, 1),
            )?;
        }
        let mut result = self.decoder.ProcessInput(0, &sample, 0);
        if result
            .as_ref()
            .err()
            .is_some_and(|e| e.code() == MF_E_NOTACCEPTING)
        {
            self.output(shared, window)?;
            result = self.decoder.ProcessInput(0, &sample, 0);
        }
        api("video-process-input", result)?;
        update(shared, |s| {
            s.frames_submitted = s.frames_submitted.saturating_add(1)
        });
        self.output(shared, window)
    }
    unsafe fn output(&mut self, shared: &Shared, window: &crate::window::Window) -> Result<()> {
        for _ in 0..64 {
            let mut out = MFT_OUTPUT_DATA_BUFFER {
                dwStreamID: 0,
                pSample: ManuallyDrop::new(None),
                dwStatus: 0,
                pEvents: ManuallyDrop::new(None),
            };
            let mut status = 0;
            let result = self
                .decoder
                .ProcessOutput(0, std::slice::from_mut(&mut out), &mut status);
            // These COM fields are caller-owned on both success and failure.
            let sample = ManuallyDrop::take(&mut out.pSample);
            drop(ManuallyDrop::take(&mut out.pEvents));
            match result {
                Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(()),
                Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                    self.output_type()?;
                    continue;
                }
                other => api("video-process-output", other)?,
            }
            let sample = sample.ok_or_else(|| unavailable("video-output-null-sample"))?;
            let buffer = api("video-output-buffer", sample.GetBufferByIndex(0))?;
            let dxgi: IMFDXGIBuffer = api("video-output-not-dxgi", buffer.cast())?;
            let mut raw = std::ptr::null_mut();
            api(
                "video-output-texture",
                dxgi.GetResource(&ID3D11Texture2D::IID, &mut raw),
            )?;
            if raw.is_null() {
                return Err(unavailable("video-output-null-texture"));
            }
            let texture = ID3D11Texture2D::from_raw(raw);
            let subresource = api("video-output-subresource", dxgi.GetSubresourceIndex())?;
            let media = api(
                "video-output-current-type",
                self.decoder.GetOutputCurrentType(0),
            )?;
            let size = api("video-output-size", media.GetUINT64(&MF_MT_FRAME_SIZE))?;
            let width = (size >> 32) as u32;
            let height = size as u32;
            if width == 0 || height == 0 || width > 8192 || height > 8192 {
                return Err(unavailable("video-output-invalid-size"));
            }
            let mut source = RECT {
                left: 0,
                top: 0,
                right: width as i32,
                bottom: height as i32,
            };
            let mut aperture = [0u8; std::mem::size_of::<MFVideoArea>()];
            let mut aperture_size = 0;
            if media
                .GetBlob(
                    &MF_MT_MINIMUM_DISPLAY_APERTURE,
                    &mut aperture,
                    Some(&mut aperture_size),
                )
                .is_ok()
                && aperture_size as usize == aperture.len()
            {
                let area = std::ptr::read_unaligned(aperture.as_ptr().cast::<MFVideoArea>());
                if area.OffsetX.fract != 0 || area.OffsetY.fract != 0 {
                    return Err(unavailable("video-fractional-aperture"));
                }
                let left = i32::from(area.OffsetX.value);
                let top = i32::from(area.OffsetY.value);
                let right = left
                    .checked_add(area.Area.cx)
                    .ok_or_else(|| unavailable("video-aperture-overflow"))?;
                let bottom = top
                    .checked_add(area.Area.cy)
                    .ok_or_else(|| unavailable("video-aperture-overflow"))?;
                if left < 0
                    || top < 0
                    || right <= left
                    || bottom <= top
                    || right > width as i32
                    || bottom > height as i32
                {
                    return Err(unavailable("video-invalid-aperture"));
                }
                source = RECT {
                    left,
                    top,
                    right,
                    bottom,
                };
            }
            let matrix = media.GetUINT32(&MF_MT_YUV_MATRIX).ok().filter(|v| *v != 0);
            let nominal = media
                .GetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE)
                .ok()
                .filter(|v| *v != 0);
            if matrix.is_some_and(|v| v > 2) || nominal.is_some_and(|v| v > 2) {
                return Err(unavailable("video-unsupported-color-space"));
            }
            let color =
                (if matrix.unwrap_or(1) == 1 { 4 } else { 0 }) | (nominal.unwrap_or(2) << 4);
            update(shared, |s| {
                s.frames_decoded = s.frames_decoded.saturating_add(1);
                s.gpu_surface_output = true;
                s.width = Some((source.right - source.left) as u32);
                s.height = Some((source.bottom - source.top) as u32);
                s.color_matrix = matrix;
                s.nominal_range = nominal;
            });
            let presented = self
                .renderer
                .present(&texture, subresource, source, color, shared)?;
            update(shared, |s| {
                s.synthetic_pixel_variation_verified = self.renderer.synthetic_pixels_verified
            });
            update(shared, |s| match presented {
                Presentation::Presented => {
                    s.frames_presented = s.frames_presented.saturating_add(1)
                }
                Presentation::Busy => s.presentations_busy = s.presentations_busy.saturating_add(1),
                Presentation::NotVisible => {
                    s.presentations_not_visible = s.presentations_not_visible.saturating_add(1)
                }
            });
            if presented == Presentation::Presented {
                window.set_video_ready(true);
            }
        }
        Err(unavailable("video-output-iteration-limit"))
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        unsafe {
            let _ = self.decoder.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0);
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Presentation {
    Presented,
    Busy,
    NotVisible,
}
fn presentation_status(status: windows::core::HRESULT) -> Result<Presentation> {
    if status == DXGI_ERROR_WAS_STILL_DRAWING {
        return Ok(Presentation::Busy);
    }
    api("video-present", status.ok())?;
    Ok(if status == windows::core::HRESULT(0) {
        Presentation::Presented
    } else {
        Presentation::NotVisible
    })
}

struct ProcessorCache {
    enumeration: ID3D11VideoProcessorEnumerator,
    processor: ID3D11VideoProcessor,
    key: (u32, u32, u32, u32),
}

struct Renderer {
    device: ID3D11Device,
    video_device: ID3D11VideoDevice,
    video_context: ID3D11VideoContext,
    swap: IDXGISwapChain1,
    hwnd: HWND,
    adapter: String,
    verify_synthetic_pixels: bool,
    synthetic_pixels_verified: bool,
    size: (u32, u32),
    processor: Option<ProcessorCache>,
}
impl Renderer {
    unsafe fn create(hwnd: HWND) -> Result<Self> {
        let mut device = None;
        let mut context = None;
        api(
            "video-d3d11-device",
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_VIDEO_SUPPORT | D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            ),
        )?;
        let device = device.ok_or_else(|| unavailable("video-d3d11-null-device"))?;
        let context = context.ok_or_else(|| unavailable("video-d3d11-null-context"))?;
        let multithread: ID3D10Multithread = api("video-d3d11-multithread", device.cast())?;
        // Return value is the previous protection state, not success/failure.
        let _ = multithread.SetMultithreadProtected(true);
        let video_device = api("video-d3d11-video-device", device.cast())?;
        let video_context = api("video-d3d11-video-context", context.cast())?;
        let dxgi: IDXGIDevice = api("video-dxgi-device", device.cast())?;
        let adapter = api("video-adapter", dxgi.GetAdapter())?;
        let desc = api("video-adapter-desc", adapter.GetDesc())?;
        let adapter_name = String::from_utf16_lossy(&desc.Description)
            .trim_end_matches('\0')
            .to_owned();
        let factory: IDXGIFactory2 = api("video-dxgi-factory", adapter.GetParent())?;
        let size = (1024, 720);
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: size.0,
            Height: size.1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            AlphaMode: DXGI_ALPHA_MODE_IGNORE,
            ..Default::default()
        };
        let swap = api(
            "video-swapchain",
            factory.CreateSwapChainForHwnd(&device, hwnd, &desc, None, None),
        )?;
        api(
            "video-disable-dxgi-alt-enter",
            factory.MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER),
        )?;
        Ok(Self {
            device,
            video_device,
            video_context,
            swap,
            hwnd,
            adapter: adapter_name,
            verify_synthetic_pixels: false,
            synthetic_pixels_verified: false,
            size,
            processor: None,
        })
    }
    // Offline synthetic fixture only. Live paths never map/read pixel buffers.
    unsafe fn verify_pixels(&self, back: &ID3D11Texture2D, size: (u32, u32)) -> Result<bool> {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        back.GetDesc(&mut desc);
        desc.Usage = D3D11_USAGE_STAGING;
        desc.BindFlags = 0;
        desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        desc.MiscFlags = 0;
        let mut staging = None;
        api(
            "video-probe-staging",
            self.device.CreateTexture2D(&desc, None, Some(&mut staging)),
        )?;
        let staging = staging.ok_or_else(|| unavailable("video-probe-staging-null"))?;
        let context = api("video-probe-context", self.device.GetImmediateContext())?;
        context.CopyResource(&staging, back);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        api(
            "video-probe-map",
            context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)),
        )?;
        let valid = !mapped.pData.is_null() && mapped.RowPitch >= size.0 * 4;
        let mut colors = std::collections::BTreeSet::new();
        if valid {
            for row in 1..8u32 {
                for col in 1..8u32 {
                    let x = col * size.0 / 8;
                    let y = row * size.1 / 8;
                    let p = (mapped.pData as *const u8).add((y * mapped.RowPitch + x * 4) as usize);
                    colors.insert([*p, *p.add(1), *p.add(2)]);
                }
            }
        }
        context.Unmap(&staging, 0);
        Ok(valid && colors.len() > 8)
    }

    unsafe fn present(
        &mut self,
        texture: &ID3D11Texture2D,
        subresource: u32,
        source: RECT,
        color: u32,
        shared: &Shared,
    ) -> Result<Presentation> {
        let width = (source.right - source.left) as u32;
        let height = (source.bottom - source.top) as u32;
        let mut td = D3D11_TEXTURE2D_DESC::default();
        texture.GetDesc(&mut td);
        if td.Format != DXGI_FORMAT_NV12
            || td.MipLevels != 1
            || td.ArraySize == 0
            || subresource >= td.ArraySize
            || width > td.Width
            || height > td.Height
        {
            return Err(unavailable("video-output-texture-layout"));
        }
        let mut rect = windows_sys::Win32::Foundation::RECT::default();
        if windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect(self.hwnd.0, &mut rect) == 0 {
            return Err(unavailable("video-surface-size"));
        }
        let size = (
            (rect.right - rect.left).max(1) as u32,
            (rect.bottom - rect.top).max(1) as u32,
        );
        if size != self.size {
            api(
                "video-swapchain-resize",
                self.swap.ResizeBuffers(
                    2,
                    size.0,
                    size.1,
                    DXGI_FORMAT_UNKNOWN,
                    DXGI_SWAP_CHAIN_FLAG(0),
                ),
            )?;
            self.size = size;
            self.processor = None;
        }
        let key = (td.Width, td.Height, size.0, size.1);
        if self.processor.as_ref().is_none_or(|p| p.key != key) {
            let desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
                InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
                InputFrameRate: DXGI_RATIONAL {
                    Numerator: 60,
                    Denominator: 1,
                },
                InputWidth: td.Width,
                InputHeight: td.Height,
                OutputFrameRate: DXGI_RATIONAL {
                    Numerator: 60,
                    Denominator: 1,
                },
                OutputWidth: size.0,
                OutputHeight: size.1,
                Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
            };
            let enumeration = api(
                "video-processor-enumerate",
                self.video_device.CreateVideoProcessorEnumerator(&desc),
            )?;
            let processor = api(
                "video-processor-create",
                self.video_device.CreateVideoProcessor(&enumeration, 0),
            )?;
            self.processor = Some(ProcessorCache {
                enumeration,
                processor,
                key,
            });
        }
        let cached = self.processor.as_ref().unwrap();
        let enumeration = &cached.enumeration;
        let processor = &cached.processor;
        let input_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
            ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPIV {
                    MipSlice: 0,
                    ArraySlice: subresource,
                },
            },
            ..Default::default()
        };
        let mut input = None;
        api(
            "video-processor-input-view",
            self.video_device.CreateVideoProcessorInputView(
                texture,
                enumeration,
                &input_desc,
                Some(&mut input),
            ),
        )?;
        let back: ID3D11Texture2D = api("video-backbuffer", self.swap.GetBuffer(0))?;
        let output_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
            ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
            },
        };
        let mut output = None;
        api(
            "video-processor-output-view",
            self.video_device.CreateVideoProcessorOutputView(
                &back,
                enumeration,
                &output_desc,
                Some(&mut output),
            ),
        )?;
        let output = output.ok_or_else(|| unavailable("video-processor-null-output"))?;
        // Preserve aspect ratio and letterbox on the GPU, without CPU pixels.
        let [left, top, right, bottom] = crate::viewport::Viewport {
            source: (width, height),
            client: size,
        }
        .rect()
        .ok_or_else(|| unavailable("video-viewport"))?;
        let target = RECT {
            left,
            top,
            right,
            bottom,
        };
        let full = RECT {
            left: 0,
            top: 0,
            right: size.0 as i32,
            bottom: size.1 as i32,
        };
        self.video_context
            .VideoProcessorSetOutputTargetRect(processor, true, Some(&full));
        self.video_context
            .VideoProcessorSetStreamSourceRect(processor, 0, true, Some(&source));
        self.video_context
            .VideoProcessorSetStreamDestRect(processor, 0, true, Some(&target));
        self.video_context.VideoProcessorSetStreamFrameFormat(
            processor,
            0,
            D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
        );
        // Standard video default: BT.709 limited input, full-range RGB output.
        self.video_context.VideoProcessorSetStreamColorSpace(
            processor,
            0,
            &D3D11_VIDEO_PROCESSOR_COLOR_SPACE { _bitfield: color },
        );
        self.video_context.VideoProcessorSetOutputColorSpace(
            processor,
            &D3D11_VIDEO_PROCESSOR_COLOR_SPACE { _bitfield: 0 },
        );
        let mut stream = D3D11_VIDEO_PROCESSOR_STREAM {
            Enable: true.into(),
            pInputSurface: ManuallyDrop::new(input),
            ..Default::default()
        };
        let result = self.video_context.VideoProcessorBlt(
            processor,
            &output,
            0,
            std::slice::from_ref(&stream),
        );
        ManuallyDrop::drop(&mut stream.pInputSurface);
        api("video-processor-blt", result)?;
        if self.verify_synthetic_pixels && !self.synthetic_pixels_verified {
            self.synthetic_pixels_verified = self.verify_pixels(&back, size)?;
        }
        // Never pace reference-picture decoding against display refresh. A busy
        // swap chain skips only this display submission, not encoded pictures.
        let started = Instant::now();
        let status = self.swap.Present(0, DXGI_PRESENT_DO_NOT_WAIT);
        update(shared, |s| {
            s.present_max_us = s
                .present_max_us
                .max(started.elapsed().as_micros().min(u64::MAX as u128) as u64)
        });
        presentation_status(status)
    }
}

/// No account, network or real content: create and release the actual GPU path.
pub fn probe(sustained: bool) -> anyhow::Result<serde_json::Value> {
    use std::sync::atomic::Ordering;
    let window = crate::window::Window::create_video_probe()?;
    let pipeline = Pipeline::start_mode(window.clone(), true);
    let fixture = include_bytes!("../fixtures/synthetic-1920x1080.h264");
    let mut starts = Vec::new();
    for i in 0..fixture.len().saturating_sub(4) {
        if fixture[i..].starts_with(&[0, 0, 0, 1]) && fixture[i + 4] & 31 == 9 {
            starts.push(i);
        }
    }
    anyhow::ensure!(starts.len() == 8, "synthetic fixture access unit count");
    starts.push(fixture.len());
    let mut inspector = crate::video_stream::Inspector::default();
    let cycles = if sustained { 128 } else { 1 };
    'feeding: for _ in 0..cycles {
        for pair in starts.windows(2) {
            let bytes = &fixture[pair[0]..pair[1]];
            let info = inspector
                .receive(bytes)
                .ok_or_else(|| anyhow::anyhow!("invalid synthetic Annex B fixture"))?;
            pipeline.submit(bytes, info);
            std::thread::sleep(if sustained {
                Duration::from_micros(8333)
            } else {
                Duration::from_millis(40)
            });
            if pipeline.snapshot().failure_stage.is_some() {
                break 'feeding;
            }
        }
    }
    let framing = inspector.snapshot();
    anyhow::ensure!(
        framing.idr_messages > 0 && framing.sps_profile_idc == Some(100),
        "synthetic high-profile reference-picture fixture"
    );
    let until = Instant::now() + Duration::from_secs(12);
    while Instant::now() < until {
        let report = pipeline.snapshot();
        if report.failure_stage.is_some()
            || (report.frames_decoded == cycles * 8
                && report.frames_presented > 0
                && report.synthetic_pixel_variation_verified)
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    pipeline.stop();
    let until = Instant::now() + Duration::from_secs(3);
    while !pipeline.snapshot().worker_finished && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(10));
    }
    let report = pipeline.snapshot();
    let released = window.active_contexts.load(Ordering::Acquire) == 0;
    if released {
        window.close();
    }
    let until = Instant::now() + Duration::from_secs(1);
    while !window.handle().is_null() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(
        serde_json::json!({"scope":"synthetic-native-h264-d3d11", "external_requests_enabled":false,"real_account_used":false,
        "cpu_readback_live_enabled":false,"sustained":sustained,"synthetic_fixture_frames":cycles*8,"synthetic_fixture_idr_frames":framing.idr_messages,"video":report,
        "native_window_released":window.handle().is_null(),"gpu_resources_released":released}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn busy_or_occluded_presentation_is_not_a_decoder_failure() {
        assert_eq!(
            presentation_status(windows::core::HRESULT(0)).unwrap(),
            Presentation::Presented
        );
        assert_eq!(
            presentation_status(DXGI_ERROR_WAS_STILL_DRAWING).unwrap(),
            Presentation::Busy
        );
        assert_eq!(
            presentation_status(windows::Win32::Foundation::DXGI_STATUS_OCCLUDED).unwrap(),
            Presentation::NotVisible
        );
        let error = presentation_status(DXGI_ERROR_DEVICE_REMOVED).unwrap_err();
        assert_eq!(error.stage, "video-present");
        assert!(error.hresult.is_some());
    }
}
