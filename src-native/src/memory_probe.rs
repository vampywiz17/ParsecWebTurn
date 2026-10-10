//! Isolated diagnostics: numeric counters for this process, no memory contents.
use serde::Serialize;
use windows::Win32::System::{
    ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX},
    Threading::GetCurrentProcess,
};

#[derive(Clone, Serialize)]
pub struct Snapshot {
    pub working_set_bytes: usize,
    pub peak_working_set_bytes: usize,
    pub private_commit_bytes: usize,
}
pub fn snapshot() -> Option<Snapshot> {
    unsafe {
        let mut counters = PROCESS_MEMORY_COUNTERS_EX {
            cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
            ..Default::default()
        };
        GetProcessMemoryInfo(
            GetCurrentProcess(),
            std::ptr::from_mut(&mut counters).cast(),
            counters.cb,
        )
        .ok()?;
        Some(Snapshot {
            working_set_bytes: counters.WorkingSetSize,
            peak_working_set_bytes: counters.PeakWorkingSetSize,
            private_commit_bytes: counters.PrivateUsage,
        })
    }
}

#[derive(Serialize)]
pub struct Profile {
    pub ahead_of_time: bool,
    pub module_load_ms: u128,
    pub before_load: Option<Snapshot>,
    pub after_load: Option<Snapshot>,
    pub guest_linear_bytes: usize,
}
