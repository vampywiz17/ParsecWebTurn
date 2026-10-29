//! Shared local shortcut policy and UTF-16 decoding; native events stay in adapters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shortcut {
    Menu,
    Statistics,
    Fullscreen,
    ReleaseInput,
    Disconnect,
    SecureAttention,
}

#[cfg(not(target_os = "macos"))]
pub fn shortcut_binding(code: &str) -> Option<(i32, Shortcut)> {
    match code {
        "KeyM" | "KeyO" => Some((3, Shortcut::Menu)),
        "KeyS" => Some((3, Shortcut::Statistics)),
        "KeyW" => Some((3, Shortcut::Fullscreen)),
        "KeyR" => Some((3, Shortcut::ReleaseInput)),
        "KeyQ" => Some((3, Shortcut::Disconnect)),
        "End" => Some((6, Shortcut::SecureAttention)),
        _ => None,
    }
}
#[cfg(not(target_os = "macos"))]
pub fn shortcut(code: &str, modifiers: i32) -> Option<Shortcut> {
    let (required, action) = shortcut_binding(code)?;
    (modifiers & 15 == required).then_some(action)
}
/// Official Parsec macOS defaults (Shift 1, Control 2, Option 4, Command 8).
/// Parsec's Command+Shift+S adds a screen; this client keeps S for stats.
#[cfg(target_os = "macos")]
pub fn shortcut(code: &str, modifiers: i32) -> Option<Shortcut> {
    match (code, modifiers & 15) {
        ("KeyM", 9) => Some(Shortcut::Menu),
        ("KeyS", 9) => Some(Shortcut::Statistics),
        ("KeyW", 9) => Some(Shortcut::Fullscreen),
        ("KeyZ", 12) => Some(Shortcut::ReleaseInput),
        ("Backquote", 12) | ("KeyQ" | "KeyW", 8) => Some(Shortcut::Disconnect),
        ("Backspace", 6) => Some(Shortcut::SecureAttention),
        _ => None,
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ShortcutRoute {
    Forward,
    Consume,
    Activate(Shortcut),
}

#[derive(Default)]
pub struct Shortcuts(std::collections::BTreeSet<&'static str>);
impl Shortcuts {
    /// Consume the trigger's repeats and release even if modifiers change.
    /// A key already held remotely must never become a local shortcut on repeat.
    pub fn route(
        &mut self,
        code: &'static str,
        down: bool,
        repeated: bool,
        action: Option<Shortcut>,
    ) -> ShortcutRoute {
        if !down {
            return if self.0.remove(code) {
                ShortcutRoute::Consume
            } else {
                ShortcutRoute::Forward
            };
        }
        if self.0.contains(code) {
            return ShortcutRoute::Consume;
        }
        if !repeated {
            if let Some(action) = action {
                self.0.insert(code);
                return ShortcutRoute::Activate(action);
            }
        }
        ShortcutRoute::Forward
    }
    pub fn clear(&mut self) {
        self.0.clear();
    }
}

#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Default)]
pub struct TextDecoder(Option<u16>);
#[cfg_attr(not(windows), allow(dead_code))]
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
    #[cfg(target_os = "macos")]
    #[test]
    fn mac_shortcuts_follow_parsec_defaults_and_ignore_lock_toggles() {
        for (key, mods, action) in [
            ("KeyM", 9, Shortcut::Menu),
            ("KeyS", 9, Shortcut::Statistics),
            ("KeyW", 9, Shortcut::Fullscreen),
            ("KeyZ", 12, Shortcut::ReleaseInput),
            ("Backquote", 12, Shortcut::Disconnect),
            ("KeyQ", 8, Shortcut::Disconnect),
            ("KeyW", 8, Shortcut::Disconnect),
            ("Backspace", 6, Shortcut::SecureAttention),
        ] {
            assert_eq!(shortcut(key, mods), Some(action));
            assert_eq!(shortcut(key, mods | 48), Some(action));
        }
        // Former Ctrl+Shift chords and plain Command copy/paste reach the host.
        for (key, mods) in [
            ("KeyM", 3),
            ("KeyQ", 3),
            ("End", 6),
            ("KeyC", 8),
            ("KeyZ", 8),
        ] {
            assert_eq!(shortcut(key, mods), None);
        }
    }
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn shortcuts_reserve_exact_chords_and_ignore_lock_toggles() {
        for (key, mods, action) in [
            ("KeyM", 3, Shortcut::Menu),
            ("KeyO", 3, Shortcut::Menu),
            ("KeyS", 3, Shortcut::Statistics),
            ("KeyW", 3, Shortcut::Fullscreen),
            ("KeyR", 3, Shortcut::ReleaseInput),
            ("KeyQ", 3, Shortcut::Disconnect),
            ("End", 6, Shortcut::SecureAttention),
        ] {
            assert_eq!(shortcut(key, mods), Some(action));
            assert_eq!(shortcut(key, mods | 48), Some(action));
            for extra in [4, 8] {
                if mods & extra == 0 {
                    assert_eq!(shortcut(key, mods | extra), None);
                }
            }
        }
        for (key, mods) in [
            ("Delete", 6),
            ("Tab", 4),
            ("Enter", 4),
            ("F8", 0),
            ("F11", 0),
            ("KeyE", 3),
            ("KeyR", 8),
        ] {
            assert_eq!(shortcut(key, mods), None);
        }
    }
    #[test]
    fn shortcut_repeats_and_release_stay_local_without_stealing_held_remote_keys() {
        let mut keys = Shortcuts::default();
        let full = Some(Shortcut::Fullscreen);
        assert_eq!(keys.route("KeyW", true, true, full), ShortcutRoute::Forward);
        assert_eq!(
            keys.route("KeyW", false, false, full),
            ShortcutRoute::Forward
        );
        assert_eq!(
            keys.route("KeyW", true, false, full),
            ShortcutRoute::Activate(Shortcut::Fullscreen)
        );
        assert_eq!(keys.route("KeyW", true, true, None), ShortcutRoute::Consume);
        assert_eq!(
            keys.route("KeyW", false, false, None),
            ShortcutRoute::Consume
        );
        assert_eq!(
            keys.route("KeyW", false, false, None),
            ShortcutRoute::Forward
        );
        assert_eq!(
            keys.route("KeyW", true, false, full),
            ShortcutRoute::Activate(Shortcut::Fullscreen)
        );
        keys.clear();
        assert_eq!(
            keys.route("KeyW", true, false, None),
            ShortcutRoute::Forward
        );
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
