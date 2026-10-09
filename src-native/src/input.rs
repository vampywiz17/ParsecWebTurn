//! PC scan codes -> UI Events code names registered by the pinned Matoya core.
//! Printable text is delivered separately by Win32 WM_CHAR, preserving layout.
pub fn code(vk: u32, scan: u8, extended: bool) -> Option<&'static str> {
    let special = match vk {
        0x08 => "Backspace",
        0x09 => "Tab",
        0x0D if extended => "NumpadEnter",
        0x0D => "Enter",
        0x10 if scan == 0x36 => "ShiftRight",
        0x10 => "ShiftLeft",
        0x11 if extended => "ControlRight",
        0x11 => "ControlLeft",
        0x12 if extended => "AltRight",
        0x12 => "AltLeft",
        0x14 => "CapsLock",
        0x1B => "Escape",
        0x20 => "Space",
        0x21 if extended => "PageUp",
        0x22 if extended => "PageDown",
        0x23 if extended => "End",
        0x24 if extended => "Home",
        0x25 if extended => "ArrowLeft",
        0x26 if extended => "ArrowUp",
        0x27 if extended => "ArrowRight",
        0x28 if extended => "ArrowDown",
        0x2D if extended => "Insert",
        0x2E if extended => "Delete",
        0x5B => "MetaLeft",
        0x5C => "MetaRight",
        0x5D => "ContextMenu",
        0x90 => "NumLock",
        0x91 => "ScrollLock",
        0x13 => "Pause",
        0x2C => "PrintScreen",
        0x6F => "NumpadDivide",
        _ => "",
    };
    if !special.is_empty() {
        return Some(special);
    }
    if (0x70..=0x87).contains(&vk) {
        return Some(
            [
                "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12", "F13",
                "F14", "F15", "F16", "F17", "F18", "F19", "F20", "F21", "F22", "F23", "F24",
            ][(vk - 0x70) as usize],
        );
    }
    if extended {
        return None;
    }
    Some(match scan {
        0x02 => "Digit1",
        0x03 => "Digit2",
        0x04 => "Digit3",
        0x05 => "Digit4",
        0x06 => "Digit5",
        0x07 => "Digit6",
        0x08 => "Digit7",
        0x09 => "Digit8",
        0x0A => "Digit9",
        0x0B => "Digit0",
        0x0C => "Minus",
        0x0D => "Equal",
        0x10 => "KeyQ",
        0x11 => "KeyW",
        0x12 => "KeyE",
        0x13 => "KeyR",
        0x14 => "KeyT",
        0x15 => "KeyY",
        0x16 => "KeyU",
        0x17 => "KeyI",
        0x18 => "KeyO",
        0x19 => "KeyP",
        0x1A => "BracketLeft",
        0x1B => "BracketRight",
        0x1E => "KeyA",
        0x1F => "KeyS",
        0x20 => "KeyD",
        0x21 => "KeyF",
        0x22 => "KeyG",
        0x23 => "KeyH",
        0x24 => "KeyJ",
        0x25 => "KeyK",
        0x26 => "KeyL",
        0x27 => "Semicolon",
        0x28 => "Quote",
        0x29 => "Backquote",
        0x2B => "Backslash",
        0x2C => "KeyZ",
        0x2D => "KeyX",
        0x2E => "KeyC",
        0x2F => "KeyV",
        0x30 => "KeyB",
        0x31 => "KeyN",
        0x32 => "KeyM",
        0x33 => "Comma",
        0x34 => "Period",
        0x35 => "Slash",
        0x37 => "NumpadMultiply",
        0x47 => "Numpad7",
        0x48 => "Numpad8",
        0x49 => "Numpad9",
        0x4A => "NumpadSubtract",
        0x4B => "Numpad4",
        0x4C => "Numpad5",
        0x4D => "Numpad6",
        0x4E => "NumpadAdd",
        0x4F => "Numpad1",
        0x50 => "Numpad2",
        0x51 => "Numpad3",
        0x52 => "Numpad0",
        0x53 => "NumpadDecimal",
        0x56 => "IntlBackslash",
        _ => return None,
    })
}

#[derive(Default)]
pub struct TextDecoder(Option<u16>);
impl TextDecoder {
    pub fn push(&mut self, unit: u16) -> Option<char> {
        if (0xD800..=0xDBFF).contains(&unit) {
            self.0 = Some(unit);
            return None;
        }
        let high = self.0.take();
        let code = if (0xDC00..=0xDFFF).contains(&unit) {
            let high = high?;
            0x10000 + ((u32::from(high) - 0xD800) << 10) + u32::from(unit) - 0xDC00
        } else {
            u32::from(unit)
        };
        char::from_u32(code).filter(|c| !c.is_control())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keys_preserve_physical_position_and_navigation() {
        assert_eq!(code(0x09, 0x0F, false), Some("Tab"));
        assert_eq!(code(0x5A, 0x15, false), Some("KeyY")); // Hungarian layout
        assert_eq!(code(0x56, 0x2F, false), Some("KeyV"));
        assert_eq!(code(0x11, 0x1D, true), Some("ControlRight"));
        assert_eq!(code(0x0D, 0x1C, true), Some("NumpadEnter"));
        assert_eq!(code(0x25, 0x4B, false), Some("Numpad4"));
        assert_eq!(code(0x25, 0x4B, true), Some("ArrowLeft"));
        assert_eq!(code(0, 0, false), None);
    }
    #[test]
    fn unicode_text_combines_surrogates_and_omits_control_duplicates() {
        let mut d = TextDecoder::default();
        assert_eq!(d.push(0xD83E), None);
        assert_eq!(d.push(0xDD80), Some('🦀'));
        assert_eq!(d.push(9), None);
        assert_eq!(d.push(22), None); // Ctrl+V is a key, not text
        assert_eq!(d.push('ő' as u16), Some('ő'));
        assert_eq!(d.push(0xDC00), None);
    }
}
