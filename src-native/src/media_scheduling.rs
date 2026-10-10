//! Documented, thread-scoped MMCSS registration. Failure keeps normal scheduling.
use std::{marker::PhantomData, rc::Rc};
use windows::{
    core::PCWSTR,
    Win32::{Foundation::HANDLE, System::Threading::*},
};

pub struct Registration {
    handle: HANDLE,
    // AvRevertMmThreadCharacteristics must run on the registering thread.
    _thread_bound: PhantomData<Rc<()>>,
}
impl Registration {
    pub fn audio() -> Option<Self> {
        Self::register(windows::core::w!("Pro Audio"))
    }
    pub fn video() -> Option<Self> {
        Self::register(windows::core::w!("Playback"))
    }
    fn register(task: PCWSTR) -> Option<Self> {
        let mut index = 0;
        // SAFETY: task is static and index is writable; registration stays local.
        let handle = unsafe { AvSetMmThreadCharacteristicsW(task, &mut index) }.ok()?;
        Some(Self {
            handle,
            _thread_bound: PhantomData,
        })
    }
}
impl Drop for Registration {
    fn drop(&mut self) {
        // SAFETY: this non-Send guard remains on the registering thread.
        unsafe {
            let _ = AvRevertMmThreadCharacteristics(self.handle);
        }
    }
}
