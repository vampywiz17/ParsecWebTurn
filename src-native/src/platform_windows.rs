//! Documented Win32 Unicode clipboard, default browser and message-box APIs.
use crate::platform::{Desktop, MAX_TEXT};
use windows_sys::Win32::{
    Foundation::*,
    System::{Com::*, DataExchange::*, Memory::*},
    UI::{Shell::*, WindowsAndMessaging::*},
};

pub struct NativeDesktop(pub usize);
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}
struct Clipboard;
impl Drop for Clipboard {
    fn drop(&mut self) {
        unsafe {
            CloseClipboard();
        }
    }
}
struct Locked(HGLOBAL);
impl Drop for Locked {
    fn drop(&mut self) {
        unsafe {
            GlobalUnlock(self.0);
        }
    }
}
impl NativeDesktop {
    fn owner(&self) -> HWND {
        self.0 as HWND
    }
    fn clipboard(&self) -> Option<Clipboard> {
        // Match the focused local UI use case. Never read the system clipboard
        // from an unfocused background session or from offline probes.
        unsafe {
            if GetForegroundWindow() == self.owner() && OpenClipboard(self.owner()) != 0 {
                Some(Clipboard)
            } else {
                None
            }
        }
    }
}

impl Desktop for NativeDesktop {
    fn read_text(&self) -> Option<String> {
        let _clipboard = self.clipboard()?;
        unsafe {
            let handle = GetClipboardData(13); // CF_UNICODETEXT
            if handle.is_null() {
                return None;
            }
            let size = GlobalSize(handle);
            if !(2..=(MAX_TEXT + 1) * 2).contains(&size) || !size.is_multiple_of(2) {
                return None;
            }
            let ptr = GlobalLock(handle).cast::<u16>();
            if ptr.is_null() {
                return None;
            }
            let _locked = Locked(handle);
            // The clipboard is open, the handle locked, and its size bounded.
            let chars = std::slice::from_raw_parts(ptr, size / 2);
            let end = chars.iter().position(|c| *c == 0)?;
            String::from_utf16(&chars[..end]).ok()
        }
    }
    fn write_text(&self, text: &str) -> bool {
        let Some(_clipboard) = self.clipboard() else {
            return false;
        };
        let chars = wide(text);
        unsafe {
            let handle = GlobalAlloc(GMEM_MOVEABLE, chars.len() * 2);
            if handle.is_null() {
                return false;
            }
            let ptr = GlobalLock(handle).cast::<u16>();
            if ptr.is_null() {
                GlobalFree(handle);
                return false;
            }
            std::ptr::copy_nonoverlapping(chars.as_ptr(), ptr, chars.len());
            GlobalUnlock(handle);
            if EmptyClipboard() == 0 || SetClipboardData(13, handle).is_null() {
                GlobalFree(handle);
                return false;
            }
            // SetClipboardData transfers ownership to Windows on success.
            true
        }
    }
    fn open_url(&self, url: &str) -> bool {
        unsafe {
            let init = CoInitializeEx(
                std::ptr::null(),
                (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
            );
            if init < 0 {
                return false;
            }
            let target = wide(url);
            let verb = wide("open");
            let result = ShellExecuteW(
                self.owner(),
                verb.as_ptr(),
                target.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                SW_SHOWNORMAL,
            ) as isize;
            CoUninitialize();
            result > 32
        }
    }
    fn alert(&self, title: &str, message: &str) -> bool {
        unsafe {
            MessageBoxW(
                self.owner(),
                wide(message).as_ptr(),
                wide(title).as_ptr(),
                MB_OK | MB_ICONINFORMATION,
            ) != 0
        }
    }
}
