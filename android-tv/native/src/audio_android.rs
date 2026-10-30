//! AAudio owns device buffering; the existing Rust Opus pipeline owns the only
//! application PCM queue. Writes are nonblocking and never delay video delivery.
use anyhow::{ensure, Result};
use ndk_sys as ndk;

pub struct Output(*mut ndk::AAudioStream);
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
            ndk::AAudioStreamBuilder_setFormat(builder, ndk::AAUDIO_FORMAT_PCM_I16 as i32);
            ndk::AAudioStreamBuilder_setChannelCount(builder, 2);
            ndk::AAudioStreamBuilder_setSampleRate(builder, 48000);
            let mut stream = std::ptr::null_mut();
            let result = ndk::AAudioStreamBuilder_openStream(builder, &mut stream);
            ndk::AAudioStreamBuilder_delete(builder);
            ensure!(
                result == 0 && !stream.is_null(),
                "Audio output could not open ({result})"
            );
            let output = Self(stream);
            ensure!(
                ndk::AAudioStream_getSampleRate(stream) == 48000
                    && ndk::AAudioStream_getChannelCount(stream) == 2
                    && ndk::AAudioStream_getFormat(stream) == ndk::AAUDIO_FORMAT_PCM_I16 as i32,
                "Audio format not supported"
            );
            let burst = ndk::AAudioStream_getFramesPerBurst(stream);
            if burst > 0 {
                ndk::AAudioStream_setBufferSizeInFrames(stream, burst.saturating_mul(2));
            }
            let result = ndk::AAudioStream_requestStart(stream);
            ensure!(result == 0, "Audio output could not start ({result})");
            Ok(output)
        }
    }
    pub fn write(&self, samples: &[i16]) -> Result<usize> {
        let frames = i32::try_from(samples.len() / 2)?;
        let written =
            unsafe { ndk::AAudioStream_write(self.0, samples.as_ptr().cast(), frames, 0) };
        ensure!(
            written >= 0,
            "Audio output stopped ({written}); reconnect after checking the output device"
        );
        Ok(written as usize * 2)
    }
}
impl Drop for Output {
    fn drop(&mut self) {
        unsafe {
            ndk::AAudioStream_close(self.0);
        }
    }
}
