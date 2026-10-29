//! Documented Core Audio default-output AudioUnit for decoded 48 kHz PCM.
//! The default-output unit follows the system's output device changes itself.
use crate::audio_layout::Layout;
use anyhow::{bail, Result};
use serde::Serialize;
use std::{
    collections::VecDeque,
    ffi::c_void,
    sync::{Arc, Mutex},
};

/// Same report fields as the Windows backend, for the shared statistics view.
#[derive(Clone, Default, Serialize)]
pub struct Snapshot {
    pub layout: Option<Layout>,
    pub minimum_ms: u32,
    pub maximum_ms: u32,
    pub custom_buffer: bool,
    pub low_latency_device: bool,
    pub device_capacity_frames: u32,
    pub device_opened: bool,
    pub frames_queued: u64,
    pub frames_written: u64,
    pub frames_dropped: u64,
    pub underruns: u64,
    pub queued_frames: usize,
    pub device_padding: u32,
    pub failure_hresult: Option<String>,
    pub resources_released: bool,
    pub recovering: bool,
    pub recovery_attempts: u64,
    pub recoveries: u64,
    pub last_device_error: Option<String>,
}

mod ffi {
    use std::ffi::c_void;
    pub type OSStatus = i32;
    pub type AudioUnit = *mut c_void;
    pub const OUTPUT: u32 = u32::from_be_bytes(*b"auou");
    pub const DEFAULT_OUTPUT: u32 = u32::from_be_bytes(*b"def ");
    pub const APPLE: u32 = u32::from_be_bytes(*b"appl");
    pub const LINEAR_PCM: u32 = u32::from_be_bytes(*b"lpcm");
    pub const SIGNED_INTEGER: u32 = 1 << 2;
    pub const PACKED: u32 = 1 << 3;
    pub const STREAM_FORMAT: u32 = 8;
    pub const SET_RENDER_CALLBACK: u32 = 23;
    pub const SCOPE_INPUT: u32 = 1;

    #[repr(C)]
    pub struct AudioComponentDescription {
        pub component_type: u32,
        pub component_sub_type: u32,
        pub component_manufacturer: u32,
        pub component_flags: u32,
        pub component_flags_mask: u32,
    }
    #[repr(C)]
    pub struct AudioStreamBasicDescription {
        pub sample_rate: f64,
        pub format_id: u32,
        pub format_flags: u32,
        pub bytes_per_packet: u32,
        pub frames_per_packet: u32,
        pub bytes_per_frame: u32,
        pub channels_per_frame: u32,
        pub bits_per_channel: u32,
        pub reserved: u32,
    }
    #[repr(C)]
    pub struct AudioBuffer {
        pub number_channels: u32,
        pub data_byte_size: u32,
        pub data: *mut c_void,
    }
    #[repr(C)]
    pub struct AudioBufferList {
        pub number_buffers: u32,
        pub buffers: [AudioBuffer; 1],
    }
    pub type RenderCallback = unsafe extern "C" fn(
        *mut c_void,
        *mut u32,
        *const c_void,
        u32,
        u32,
        *mut AudioBufferList,
    ) -> OSStatus;
    #[repr(C)]
    pub struct AURenderCallbackStruct {
        pub input_proc: RenderCallback,
        pub input_proc_ref_con: *mut c_void,
    }
    #[link(name = "AudioToolbox", kind = "framework")]
    extern "C" {
        pub fn AudioComponentFindNext(
            component: *mut c_void,
            description: *const AudioComponentDescription,
        ) -> *mut c_void;
        pub fn AudioComponentInstanceNew(component: *mut c_void, out: *mut AudioUnit) -> OSStatus;
        pub fn AudioComponentInstanceDispose(unit: AudioUnit) -> OSStatus;
        pub fn AudioUnitSetProperty(
            unit: AudioUnit,
            id: u32,
            scope: u32,
            element: u32,
            data: *const c_void,
            size: u32,
        ) -> OSStatus;
        pub fn AudioUnitInitialize(unit: AudioUnit) -> OSStatus;
        pub fn AudioUnitUninitialize(unit: AudioUnit) -> OSStatus;
        pub fn AudioOutputUnitStart(unit: AudioUnit) -> OSStatus;
        pub fn AudioOutputUnitStop(unit: AudioUnit) -> OSStatus;
    }
}

#[derive(Default)]
struct State {
    report: Snapshot,
    pcm: VecDeque<i16>,
    channels: usize,
    min_frames: usize,
    // Silence until the minimum buffer is queued again after an underrun.
    primed: bool,
    stopped: bool,
}

pub struct Playback {
    pub layout: Layout,
    shared: Arc<Mutex<State>>,
    unit: Mutex<Option<Unit>>,
    max_frames: usize,
}

/// The AudioUnit plus the callback's strong reference, released after disposal.
struct Unit {
    unit: ffi::AudioUnit,
    callback_state: *const Mutex<State>,
}
// Core Audio units may be stopped and disposed from any thread.
unsafe impl Send for Unit {}

impl Playback {
    pub fn create_layout(
        window: Arc<crate::window::Window>,
        min_buffer: u32,
        max_buffer: u32,
        layout: Layout,
    ) -> Result<Self> {
        let custom = window.audio_buffer();
        let (min_buffer, max_buffer) = custom.unwrap_or((min_buffer, max_buffer));
        if min_buffer > max_buffer || max_buffer == 0 || max_buffer > 500 {
            bail!("invalid audio latency limits");
        }
        let shared = Arc::new(Mutex::new(State {
            report: Snapshot {
                layout: Some(layout),
                minimum_ms: min_buffer,
                maximum_ms: max_buffer,
                custom_buffer: custom.is_some(),
                ..Default::default()
            },
            channels: usize::from(layout.channels),
            min_frames: min_buffer as usize * 48,
            ..Default::default()
        }));
        crate::event_log::record(
            "INFO",
            "audio_output_configured",
            serde_json::json!({
                "minimum_ms":min_buffer,"maximum_ms":max_buffer,"custom":custom.is_some(),
                "channels":layout.channels,"channel_mask":layout.mask
            }),
        );
        let unit = match unsafe { open(&shared, u32::from(layout.channels)) } {
            Ok(unit) => unit,
            Err(error) => {
                crate::event_log::record(
                    "ERROR",
                    "audio_output_failed",
                    serde_json::json!({"reason":error.to_string()}),
                );
                return Err(error);
            }
        };
        shared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .report
            .device_opened = true;
        Ok(Self {
            layout,
            shared,
            unit: Mutex::new(Some(unit)),
            max_frames: max_buffer as usize * 48,
        })
    }
    pub fn queue(&self, pcm: &[i16]) {
        let channels = usize::from(self.layout.channels);
        if !pcm.len().is_multiple_of(channels) {
            return;
        }
        let frames = pcm.len() / channels;
        let mut s = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        if s.stopped || s.pcm.len() / channels + frames > self.max_frames {
            s.report.frames_dropped += frames as u64;
            return;
        }
        s.pcm.extend(pcm);
        s.report.frames_queued += frames as u64;
        s.report.queued_frames = s.pcm.len() / channels;
    }
    /// The default-output unit handles device changes without a reopen.
    pub fn take_recovery_flush(&self) -> bool {
        false
    }
    pub fn snapshot(&self) -> Snapshot {
        self.shared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .report
            .clone()
    }
    pub fn stop(&self) {
        {
            let mut s = self.shared.lock().unwrap_or_else(|e| e.into_inner());
            s.stopped = true;
            s.pcm.clear();
        }
        // The render callback takes the state lock; never stop the unit while holding it.
        if let Some(unit) = self.unit.lock().unwrap_or_else(|e| e.into_inner()).take() {
            unsafe {
                ffi::AudioOutputUnitStop(unit.unit);
                ffi::AudioUnitUninitialize(unit.unit);
                ffi::AudioComponentInstanceDispose(unit.unit);
                drop(Arc::from_raw(unit.callback_state));
            }
            let mut s = self.shared.lock().unwrap_or_else(|e| e.into_inner());
            s.report.resources_released = true;
            s.report.device_opened = false;
            s.report.queued_frames = 0;
        }
    }
}
impl Drop for Playback {
    fn drop(&mut self) {
        self.stop();
    }
}

unsafe fn open(state: &Arc<Mutex<State>>, channels: u32) -> Result<Unit> {
    let description = ffi::AudioComponentDescription {
        component_type: ffi::OUTPUT,
        component_sub_type: ffi::DEFAULT_OUTPUT,
        component_manufacturer: ffi::APPLE,
        component_flags: 0,
        component_flags_mask: 0,
    };
    let component = ffi::AudioComponentFindNext(std::ptr::null_mut(), &description);
    if component.is_null() {
        bail!("default audio output unavailable");
    }
    let mut unit = std::ptr::null_mut();
    let status = ffi::AudioComponentInstanceNew(component, &mut unit);
    if status != 0 || unit.is_null() {
        bail!("AudioComponentInstanceNew failed: {status}");
    }
    let format = ffi::AudioStreamBasicDescription {
        sample_rate: 48_000.0,
        format_id: ffi::LINEAR_PCM,
        format_flags: ffi::SIGNED_INTEGER | ffi::PACKED,
        bytes_per_packet: channels * 2,
        frames_per_packet: 1,
        bytes_per_frame: channels * 2,
        channels_per_frame: channels,
        bits_per_channel: 16,
        reserved: 0,
    };
    let callback_state = Arc::into_raw(state.clone());
    let callback = ffi::AURenderCallbackStruct {
        input_proc: render,
        input_proc_ref_con: callback_state as *mut c_void,
    };
    let fail = |stage: &str, status: i32| {
        ffi::AudioComponentInstanceDispose(unit);
        drop(Arc::from_raw(callback_state));
        anyhow::anyhow!("{stage} failed: {status}")
    };
    let status = ffi::AudioUnitSetProperty(
        unit,
        ffi::STREAM_FORMAT,
        ffi::SCOPE_INPUT,
        0,
        (&format as *const ffi::AudioStreamBasicDescription).cast(),
        size_of::<ffi::AudioStreamBasicDescription>() as u32,
    );
    if status != 0 {
        return Err(fail("AudioUnit stream format", status));
    }
    let status = ffi::AudioUnitSetProperty(
        unit,
        ffi::SET_RENDER_CALLBACK,
        ffi::SCOPE_INPUT,
        0,
        (&callback as *const ffi::AURenderCallbackStruct).cast(),
        size_of::<ffi::AURenderCallbackStruct>() as u32,
    );
    if status != 0 {
        return Err(fail("AudioUnit render callback", status));
    }
    let status = ffi::AudioUnitInitialize(unit);
    if status != 0 {
        return Err(fail("AudioUnitInitialize", status));
    }
    let status = ffi::AudioOutputUnitStart(unit);
    if status != 0 {
        ffi::AudioUnitUninitialize(unit);
        return Err(fail("AudioOutputUnitStart", status));
    }
    Ok(Unit {
        unit,
        callback_state,
    })
}

unsafe extern "C" fn render(
    state: *mut c_void,
    _flags: *mut u32,
    _time: *const c_void,
    _bus: u32,
    frames: u32,
    data: *mut ffi::AudioBufferList,
) -> ffi::OSStatus {
    if data.is_null() || (*data).number_buffers == 0 {
        return 0;
    }
    let buffer = &mut (*data).buffers[0];
    let out = std::slice::from_raw_parts_mut(
        buffer.data.cast::<i16>(),
        buffer.data_byte_size as usize / 2,
    );
    let state = &*(state as *const Mutex<State>);
    let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
    fill(&mut s, out, frames as usize);
    0
}

fn fill(s: &mut State, out: &mut [i16], frames: usize) {
    let wanted = (frames * s.channels).min(out.len());
    if !s.primed && s.pcm.len() / s.channels.max(1) >= s.min_frames.max(1) {
        s.primed = true;
    }
    let mut written = 0;
    if s.primed {
        for (slot, sample) in out[..wanted]
            .iter_mut()
            .zip(s.pcm.drain(..wanted.min(s.pcm.len())))
        {
            *slot = sample;
            written += 1;
        }
        if written < wanted {
            s.report.underruns += 1;
            s.primed = false;
        }
    }
    out[written..].fill(0);
    s.report.frames_written += (written / s.channels.max(1)) as u64;
    s.report.queued_frames = s.pcm.len() / s.channels.max(1);
}

/// Output follows the decoded stream's layout; Core Audio maps stereo to the device.
pub fn device_layout() -> Result<Layout> {
    Ok(Layout::STEREO)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn playback_waits_for_minimum_buffer_and_reprimes_after_underrun() {
        let mut s = State {
            channels: 2,
            min_frames: 4,
            ..Default::default()
        };
        let mut out = [7i16; 8];
        s.pcm.extend([1, 1, 2, 2]);
        fill(&mut s, &mut out, 4);
        assert_eq!(out, [0; 8], "below minimum: silence, queue kept");
        assert_eq!(s.pcm.len(), 4);
        s.pcm.extend([3, 3, 4, 4, 5, 5]);
        fill(&mut s, &mut out, 4);
        assert_eq!(out, [1, 1, 2, 2, 3, 3, 4, 4]);
        fill(&mut s, &mut out, 4);
        assert_eq!(out, [5, 5, 0, 0, 0, 0, 0, 0]);
        assert_eq!(s.report.underruns, 1);
        assert!(!s.primed);
        assert_eq!(s.report.frames_written, 5);
    }
}
