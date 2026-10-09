//! Borderless fullscreen through documented Win32 APIs; called on the UI thread.
use anyhow::{bail, Result};
use windows_sys::Win32::{Foundation::*, Graphics::Gdi::*, UI::WindowsAndMessaging::*};

struct Saved {
    style: isize,
    placement: WINDOWPLACEMENT,
}
#[derive(Default)]
pub struct State {
    saved: Option<Saved>,
}
impl State {
    pub fn active(&self) -> bool {
        self.saved.is_some()
    }
    pub fn set(&mut self, hwnd: HWND, enable: bool) -> Result<bool> {
        if enable == self.active() {
            return Ok(enable);
        }
        // SAFETY: HWND belongs to the calling UI thread. Placement and monitor
        // structures have the documented size, and are not retained by Windows.
        unsafe {
            if enable {
                let mut placement = WINDOWPLACEMENT {
                    length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
                    ..std::mem::zeroed()
                };
                let mut monitor = MONITORINFO {
                    cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                    ..std::mem::zeroed()
                };
                if GetWindowPlacement(hwnd, &mut placement) == 0
                    || GetMonitorInfoW(
                        MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST),
                        &mut monitor,
                    ) == 0
                {
                    bail!("fullscreen geometry unavailable");
                }
                let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
                if style & WS_VISIBLE as isize == 0 {
                    placement.showCmd = SW_HIDE as u32;
                }
                set_style(hwnd, style & !(WS_OVERLAPPEDWINDOW as isize))?;
                let rect = monitor.rcMonitor;
                if SetWindowPos(
                    hwnd,
                    std::ptr::null_mut(),
                    rect.left,
                    rect.top,
                    rect.right - rect.left,
                    rect.bottom - rect.top,
                    SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
                ) == 0
                {
                    let _ = set_style(hwnd, style);
                    SetWindowPlacement(hwnd, &placement);
                    SetWindowPos(
                        hwnd,
                        std::ptr::null_mut(),
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
                    );
                    bail!("fullscreen resize failed");
                }
                self.saved = Some(Saved { style, placement });
            } else if let Some(saved) = &self.saved {
                set_style(hwnd, saved.style)?;
                if SetWindowPlacement(hwnd, &saved.placement) == 0
                    || SetWindowPos(
                        hwnd,
                        std::ptr::null_mut(),
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
                    ) == 0
                {
                    bail!("window placement restore failed");
                }
                self.saved = None;
            }
        }
        Ok(self.active())
    }
}
unsafe fn set_style(hwnd: HWND, style: isize) -> Result<()> {
    SetLastError(0);
    if SetWindowLongPtrW(hwnd, GWL_STYLE, style) == 0 && GetLastError() != 0 {
        bail!("window style change failed");
    }
    Ok(())
}

#[cfg(any(test, feature = "diagnostics"))]
pub fn probe() -> Result<serde_json::Value> {
    struct OwnedWindow(HWND);
    impl Drop for OwnedWindow {
        fn drop(&mut self) {
            unsafe {
                DestroyWindow(self.0);
            }
        }
    }
    // A hidden stock window exercises geometry/placement without a GPU,
    // account, clipboard, injected UI clicks, or changes to any existing window.
    let name: Vec<u16> = "STATIC\0".encode_utf16().collect();
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            name.as_ptr(),
            name.as_ptr(),
            WS_OVERLAPPEDWINDOW,
            80,
            80,
            640,
            480,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null(),
        )
    };
    if hwnd.is_null() {
        bail!("fullscreen probe window unavailable");
    }
    let window = OwnedWindow(hwnd);
    let mut original: RECT = unsafe { std::mem::zeroed() };
    unsafe {
        if GetWindowRect(window.0, &mut original) == 0 {
            bail!("probe geometry missing");
        }
    }
    let original_style = unsafe { GetWindowLongPtrW(window.0, GWL_STYLE) };
    let mut state = State::default();
    if state.set(window.0, false)? {
        bail!("idle exit entered fullscreen");
    }
    for _ in 0..5 {
        if !state.set(window.0, true)? || !state.set(window.0, true)? {
            bail!("fullscreen not entered");
        }
        let mut rect: RECT = unsafe { std::mem::zeroed() };
        let mut monitor: MONITORINFO = unsafe { std::mem::zeroed() };
        monitor.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        unsafe {
            if GetWindowRect(window.0, &mut rect) == 0
                || GetMonitorInfoW(
                    MonitorFromWindow(window.0, MONITOR_DEFAULTTONEAREST),
                    &mut monitor,
                ) == 0
            {
                bail!("probe monitor missing");
            }
        }
        let bounds = |r: RECT| (r.left, r.top, r.right, r.bottom);
        if bounds(rect) != bounds(monitor.rcMonitor)
            || unsafe { GetWindowLongPtrW(window.0, GWL_STYLE) } & WS_OVERLAPPEDWINDOW as isize != 0
        {
            bail!("fullscreen is not borderless monitor size");
        }
        if state.set(window.0, false)? || state.set(window.0, false)? {
            bail!("fullscreen not exited");
        }
        unsafe {
            if GetWindowRect(window.0, &mut rect) == 0 {
                bail!("restored geometry missing");
            }
        }
        if bounds(rect) != bounds(original)
            || unsafe { GetWindowLongPtrW(window.0, GWL_STYLE) } != original_style
        {
            bail!("window geometry/style not restored");
        }
    }
    drop(window);
    guest_probe()?;
    Ok(
        serde_json::json!({"schema":1,"scope":"isolated-native-fullscreen-and-guest-import", "fullscreen_roundtrips":5,"native_geometry_restored":true,"native_style_restored":true,"idempotent_requests_verified":true,"guest_import_verified":true,"window_event_feedback_verified":false,"real_account_used":false,"parsec_host_connected":false}),
    )
}
fn guest_probe() -> Result<()> {
    let mut config = wasmtime::Config::new();
    config
        .wasm_threads(true)
        .consume_fuel(true)
        .epoch_interruption(true);
    let engine = wasmtime::Engine::new(&config)?;
    let module = wasmtime::Module::new(
        &engine,
        r#"(module
        (import "env" "memory" (memory 1 1 shared))
        (import "env" "web_set_fullscreen" (func $set (param i32)))
        (func (export "test") i32.const 0 call $set i32.const 1 call $set i32.const 0 call $set))"#,
    )?;
    let (mut store, instance) = crate::instantiate(&engine, &module)?;
    instance
        .get_typed_func::<(), ()>(&mut store, "test")?
        .call(&mut store, ())?;
    if store.data().boundary.is_some()
        || store.data().calls.get("env::web_set_fullscreen") != Some(&3)
    {
        bail!("guest fullscreen import failed");
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    #[test]
    fn inactive_optional_capabilities_can_be_released_but_not_acquired() {
        let mut config = wasmtime::Config::new();
        config
            .wasm_threads(true)
            .consume_fuel(true)
            .epoch_interruption(true);
        let engine = wasmtime::Engine::new(&config).unwrap();
        for name in ["web_set_pointer_lock", "web_set_kb_grab"] {
            let module = wasmtime::Module::new(
                &engine,
                format!(
                    r#"(module
                (import "env" "memory" (memory 1 1 shared))
                (import "env" "{name}" (func $set (param i32)))
                (func (export "set") (param i32) local.get 0 call $set))"#
                ),
            )
            .unwrap();
            let (mut store, instance) = crate::instantiate(&engine, &module).unwrap();
            let set = instance
                .get_typed_func::<i32, ()>(&mut store, "set")
                .unwrap();
            for _ in 0..3 {
                set.call(&mut store, 0).unwrap();
            }
            assert!(store.data().boundary.is_none());
            assert!(set.call(&mut store, 1).is_err());
            assert_eq!(
                store.data().boundary.as_deref(),
                Some(format!("env::{name}").as_str())
            );
        }
    }
    #[test]
    fn native_fullscreen_restores_geometry_and_guest_import_does_not_trap() {
        let report = super::probe().unwrap();
        assert_eq!(report["native_geometry_restored"], true);
        assert_eq!(report["guest_import_verified"], true);
        assert_eq!(report["parsec_host_connected"], false);
    }
}
