//! AAudio owns device buffering; the existing Rust Opus pipeline owns the only
//! application PCM queue. Writes are nonblocking and never delay video delivery.
use anyhow::{ensure, Result};
use ndk_sys as ndk;

pub struct Output {
    stream: *mut ndk::AAudioStream,
    started: bool,
}
#[derive(serde::Serialize)]
pub struct Snapshot {
    state: String,
    frames_written: i64,
    frames_read: i64,
    xruns: i32,
    buffer_frames: i32,
    capacity_frames: i32,
}
impl Output {
    pub fn open() -> Result<Self> {
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
            ndk::AAudioStreamBuilder_setFormat(builder, ndk::AAUDIO_FORMAT_PCM_I16);
            ndk::AAudioStreamBuilder_setChannelCount(builder, 2);
            ndk::AAudioStreamBuilder_setSampleRate(builder, 48000);
            // This backend is a bounded, nonblocking writer, not an AAudio
            // real-time data callback. Give one 20-ms Opus block room to play
            // between worker wakeups; two tiny bursts can underrun repeatedly.
            ndk::AAudioStreamBuilder_setBufferCapacityInFrames(builder, 1920);
            let mut stream = std::ptr::null_mut();
            let result = ndk::AAudioStreamBuilder_openStream(builder, &mut stream);
            ndk::AAudioStreamBuilder_delete(builder);
            ensure!(
                result == 0 && !stream.is_null(),
                "Audio output could not open ({result})"
            );
            let output = Self {
                stream,
                started: false,
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
                ndk::AAudioStream_setBufferSizeInFrames(
                    stream,
                    burst.saturating_mul(2).max(960).min(capacity),
                );
            }
            Ok(output)
        }
    }
    pub fn write(&mut self, samples: &[i16]) -> Result<usize> {
        let frames = i32::try_from(samples.len() / 2)?;
        let written =
            unsafe { ndk::AAudioStream_write(self.stream, samples.as_ptr().cast(), frames, 0) };
        ensure!(
            written >= 0,
            "Audio output stopped ({written}); reconnect after checking the output device"
        );
        // Prime with actual PCM before starting; opening during signaling used
        // to start an empty output seconds before the first audio could arrive.
        if written > 0 && !self.started {
            let result = unsafe { ndk::AAudioStream_requestStart(self.stream) };
            ensure!(result == 0, "Audio output could not start ({result})");
            self.started = true;
        }
        Ok(written as usize * 2)
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
            }
        }
    }
}
impl Drop for Output {
    fn drop(&mut self) {
        unsafe {
            ndk::AAudioStream_close(self.stream);
        }
    }
}
