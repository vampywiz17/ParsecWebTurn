//! Documented AppKit general pasteboard for Unicode text, used on the main thread.
use crate::platform::{Desktop, MAX_TEXT};
use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
use objc2_foundation::NSString;
use std::sync::{atomic::Ordering, Weak};

pub struct NativeDesktop(pub Weak<crate::window::Window>);

impl Desktop for NativeDesktop {
    fn read_text(&self) -> Option<String> {
        // Match the Windows contract: only the focused stream window reads.
        let window = self.0.upgrade()?;
        if !window.has_focus.load(Ordering::Acquire) {
            return None;
        }
        crate::window_macos::on_main(|_| {
            let text = NSPasteboard::generalPasteboard()
                .stringForType(unsafe { NSPasteboardTypeString })?
                .to_string();
            (text.len() <= MAX_TEXT).then_some(text)
        })
    }

    fn write_text(&self, text: &str) -> bool {
        let Some(window) = self.0.upgrade() else {
            return false;
        };
        if text.len() > MAX_TEXT {
            return false;
        }
        crate::window_macos::on_main(|_| {
            let pasteboard = NSPasteboard::generalPasteboard();
            pasteboard.clearContents();
            let written = pasteboard
                .setString_forType(&NSString::from_str(text), unsafe { NSPasteboardTypeString });
            // A remote copy must not be echoed back as a local change.
            window
                .pasteboard_seen
                .store(pasteboard.changeCount(), Ordering::Release);
            written
        })
    }
}
