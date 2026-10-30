//! AAudio pulls the common PCM queue on its own real-time callback.
use crate::audio_stream::{CallbackReader, NativeReceiver};
use anyhow::{ensure, Result};
use ndk_sys as ndk;
use std::{
    cell::UnsafeCell,
    ffi::c_void,
    sync::atomic::{AtomicBool, AtomicI32, Ordering},
};

struct Callback {
    reader: UnsafeCell<CallbackReader>,
    stopping: AtomicBool,
    error: AtomicI32,
}
impl Callback {
    fn fail(&self, code: i32) {
        let _ = self
            .error
            .compare_exchange(0, code, Ordering::Relaxed, Ordering::Relaxed);
    }
}
unsafe extern "C" fn data(
    _stream: *mut ndk::AAudioStream,
    user: *mut c_void,
    output: *mut c_void,
    frames: i32,
) -> i32 {
    // NDK owns this writable stereo i16 buffer for this callback only.
    // Boxed userData stays alive until close has stopped native callbacks.
    if user.is_null() || output.is_null() || frames <= 0 {
        return ndk::AAUDIO_CALLBACK_RESULT_STOP as i32;
    }
    let state = &*user.cast::<Callback>();
    let Some(samples) = (frames as usize).checked_mul(2) else {
        return ndk::AAUDIO_CALLBACK_RESULT_STOP as i32;
    };
    let output = std::slice::from_raw_parts_mut(output.cast::<i16>(), samples);
    if state.stopping.load(Ordering::Relaxed) {
        output.fill(0);
        return ndk::AAUDIO_CALLBACK_RESULT_STOP as i32;
    }
    // AAudio documents serialized callbacks. Only this callback accesses the
    // reader; other threads use atomic counters. No locks, allocation, stream
    // commands, network or decoder work run on this real-time thread.
    let result = (&mut *state.reader.get()).render(output);
    if result.invalid_layout {
        state.fail(ndk::AAUDIO_ERROR_INVALID_FORMAT);
        return ndk::AAUDIO_CALLBACK_RESULT_STOP as i32;
    }
    ndk::AAUDIO_CALLBACK_RESULT_CONTINUE as i32
}
unsafe extern "C" fn error(_stream: *mut ndk::AAudioStream, user: *mut c_void, code: i32) {
    if let Some(state) = user.cast::<Callback>().as_ref() {
        state.fail(code);
    }
}
pub struct Output {
    stream: *mut ndk::AAudioStream,
    callback: Option<Box<Callback>>,
}
#[derive(serde::Serialize)]
pub struct Snapshot {
    state: String,
    frames_written: i64,
    frames_read: i64,
    xruns: i32,
    buffer_frames: i32,
    capacity_frames: i32,
    device_id: i32,
}
impl Output {
    pub fn open(receiver: NativeReceiver) -> Result<Self> {
        let mut callback = Box::new(Callback {
            reader: UnsafeCell::new(receiver.callback_reader()?),
            stopping: AtomicBool::new(false),
            error: AtomicI32::new(0),
        });
        unsafe {
            let mut builder = std::ptr::null_mut();
            ensure!(
                ndk::AAudio_createStreamBuilder(&mut builder) == 0 && !builder.is_null(),
                "Audio builder failed"
            );
            ndk::AAudioStreamBuilder_setDirection(builder, ndk::AAUDIO_DIRECTION_OUTPUT as i32);
            ndk::AAudioStreamBuilder_setSharingMode(
                builder,
                ndk::AAUDIO_SHARING_MODE_SHARED as i32,
            );
            ndk::AAudioStreamBuilder_setPerformanceMode(
                builder,
                ndk::AAUDIO_PERFORMANCE_MODE_LOW_LATENCY as i32,
            );
            ndk::AAudioStreamBuilder_setUsage(builder, ndk::AAUDIO_USAGE_GAME as i32);
            ndk::AAudioStreamBuilder_setContentType(builder, ndk::AAUDIO_CONTENT_TYPE_MOVIE as i32);
            ndk::AAudioStreamBuilder_setFormat(builder, ndk::AAUDIO_FORMAT_PCM_I16);
            ndk::AAudioStreamBuilder_setChannelCount(builder, 2);
            ndk::AAudioStreamBuilder_setSampleRate(builder, 48000);
            let user = (&mut *callback as *mut Callback).cast();
            ndk::AAudioStreamBuilder_setDataCallback(builder, Some(data), user);
            ndk::AAudioStreamBuilder_setErrorCallback(builder, Some(error), user);
            let mut stream = std::ptr::null_mut();
            let result = ndk::AAudioStreamBuilder_openStream(builder, &mut stream);
            ndk::AAudioStreamBuilder_delete(builder);
            ensure!(
                result == 0 && !stream.is_null(),
                "Audio output could not open ({result})"
            );
            let output = Self {
                stream,
                callback: Some(callback),
            };
            ensure!(
                ndk::AAudioStream_getSampleRate(stream) == 48000
                    && ndk::AAudioStream_getChannelCount(stream) == 2
                    && ndk::AAudioStream_getFormat(stream) == ndk::AAUDIO_FORMAT_PCM_I16,
                "Audio format not supported"
            );
            let burst = ndk::AAudioStream_getFramesPerBurst(stream);
            if burst > 0 {
                let capacity = ndk::AAudioStream_getBufferCapacityInFrames(stream);
                let result = ndk::AAudioStream_setBufferSizeInFrames(
                    stream,
                    burst.saturating_mul(2).min(capacity),
                );
                ensure!(
                    result > 0,
                    "Audio device buffer configuration failed ({result})"
                );
            }
            let result = ndk::AAudioStream_requestStart(stream);
            ensure!(result == 0, "Audio output could not start ({result})");
            Ok(output)
        }
    }
    pub fn check(&self) -> Result<()> {
        let code = self
            .callback
            .as_ref()
            .unwrap()
            .error
            .load(Ordering::Relaxed);
        ensure!(
            code == 0,
            "Android audio output failed ({code}); check the output device and reconnect"
        );
        Ok(())
    }
    pub fn snapshot(&self) -> Snapshot {
        unsafe {
            Snapshot {
                state: std::ffi::CStr::from_ptr(ndk::AAudio_convertStreamStateToText(
                    ndk::AAudioStream_getState(self.stream),
                ))
                .to_string_lossy()
                .into_owned(),
                frames_written: ndk::AAudioStream_getFramesWritten(self.stream),
                frames_read: ndk::AAudioStream_getFramesRead(self.stream),
                xruns: ndk::AAudioStream_getXRunCount(self.stream),
                buffer_frames: ndk::AAudioStream_getBufferSizeInFrames(self.stream),
                capacity_frames: ndk::AAudioStream_getBufferCapacityInFrames(self.stream),
                device_id: ndk::AAudioStream_getDeviceId(self.stream),
            }
        }
    }
}
impl Drop for Output {
    fn drop(&mut self) {
        self.callback
            .as_ref()
            .unwrap()
            .stopping
            .store(true, Ordering::Relaxed);
        unsafe {
            // Close from the control thread, never from the native callbacks.
            ndk::AAudioStream_requestStop(self.stream);
            let result = ndk::AAudioStream_close(self.stream);
            if result < 0 {
                crate::event_log::record(
                    "ERROR",
                    "android_audio_close_failed",
                    serde_json::json!({"code":result}),
                );
                // A failed close does not prove callbacks have stopped. Retain
                // userData rather than free memory a callback may still use.
                Box::leak(self.callback.take().unwrap());
            }
        }
    }
}
