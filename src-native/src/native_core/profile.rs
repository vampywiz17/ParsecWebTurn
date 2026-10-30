//! Native-only preferences. Session protection uses existing CurrentUser DPAPI.
use anyhow::{ensure, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};

pub use crate::session_preferences::{Resolution, Video, MAX_BANDWIDTH_MBPS, MAX_FPS};

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Profile {
    pub email: String,
    pub encrypted_session: String,
    pub last_peer: String,
    pub hide_session_button: bool,
    pub video: Video,
}

impl Video {
    pub fn mode(&self) -> crate::input_state::Mode {
        crate::input_state::Mode {
            mouse: matches!(self.immersive, 1 | 3),
            keyboard: matches!(self.immersive, 1 | 2),
        }
    }
    pub fn request(&self, client_size: (u32, u32)) -> Result<bytes::Bytes> {
        self.validate()?;
        let mut values = serde_json::Map::new();
        let (w, h) = match self.resolution {
            Resolution::Host => (0, 0),
            Resolution::Client => client_size,
            Resolution::Fixed(w, h) => (w, h),
        };
        ensure!(
            w <= 16384 && h <= 16384,
            "Client display dimensions exceed limit"
        );
        values.insert("resolutionX".into(), w.into());
        values.insert("resolutionY".into(), h.into());
        if let Some(n) = self.bandwidth_mbps {
            values.insert("encoderMaxBitrate".into(), n.into());
        }
        if let Some(n) = self.fps {
            values.insert("encoderFPS".into(), n.into());
        }
        if let Some(n) = self.constant_fps {
            values.insert("fullFPS".into(), n.into());
        }
        if let Some(n) = self.rotated {
            values.insert("rotated".into(), n.into());
        }
        // Same owner-video contract used by the existing core and lowlat daemon.
        // No codec flags here: the BUD adapter preserves its negotiated capability.
        crate::control::text(17, 11, &serde_json::json!({"video":[values]}).to_string())
    }
}

impl Profile {
    pub fn session(&self) -> Result<Option<super::account::Session>> {
        if self.encrypted_session.is_empty() {
            return Ok(None);
        }
        ensure!(
            self.encrypted_session.len() <= 32768,
            "Stored session exceeds limit"
        );
        let raw = STANDARD.decode(&self.encrypted_session)?;
        let plain = zeroize::Zeroizing::new(crate::secure_storage::protect(&raw, false)?);
        super::account::Session::from_token(std::str::from_utf8(&plain)?).map(Some)
    }
    pub fn encrypted(token: &str) -> Result<String> {
        Ok(STANDARD.encode(crate::secure_storage::protect(token.as_bytes(), true)?))
    }
}

impl super::sign_in::SessionStore for crate::connection_settings::Manager {
    fn save(&self, email: &str, token: &str) -> Result<()> {
        let blob = Profile::encrypted(token)?;
        self.update_native(|profile| {
            profile.email = email.into();
            profile.encrypted_session = blob;
        })
    }
    fn clear(&self, forget_email: bool) -> Result<()> {
        self.update_native(|profile| {
            profile.encrypted_session.clear();
            if forget_email {
                profile.email.clear();
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn protected_session_and_video_requests_do_not_store_passwords() {
        for (mode, mouse, keyboard) in [
            (0, false, false),
            (1, true, true),
            (2, false, true),
            (3, true, false),
        ] {
            let v = Video {
                immersive: mode,
                ..Default::default()
            };
            assert_eq!(v.mode(), crate::input_state::Mode { mouse, keyboard });
            assert!(!v.has_host_settings());
        }
        let token = "fixture-session-not-a-real-account";
        let p = Profile {
            encrypted_session: Profile::encrypted(token).unwrap(),
            ..Default::default()
        };
        let json = serde_json::to_string(&p).unwrap();
        assert!(!json.contains(token));
        assert!(!json.contains("password"));
        assert_eq!(p.session().unwrap().unwrap().token(), token);
        assert!(Profile {
            encrypted_session: "not-base64".into(),
            ..Default::default()
        }
        .session()
        .is_err());
        let v = Video {
            resolution: Resolution::Client,
            fps: Some(60),
            constant_fps: Some(true),
            bandwidth_mbps: Some(20),
            ..Default::default()
        };
        let bytes = v.request((2560, 1440)).unwrap();
        assert_eq!(&bytes[4..8], &11i32.to_be_bytes());
        let value: serde_json::Value = serde_json::from_slice(&bytes[13..bytes.len() - 1]).unwrap();
        assert_eq!(value["video"][0]["resolutionX"], 2560);
        assert_eq!(value["video"][0]["encoderMaxBitrate"], 20);
        assert_eq!(value["video"][0]["fullFPS"], true);
        let preserve = Video::default().request((1, 1)).unwrap();
        let value: serde_json::Value =
            serde_json::from_slice(&preserve[13..preserve.len() - 1]).unwrap();
        assert_eq!(value["video"][0]["resolutionX"], 0);
        assert!(value["video"][0].get("encoderFPS").is_none());
        assert!(Video {
            fps: Some(241),
            ..Default::default()
        }
        .validate()
        .is_err());
    }

    #[test]
    fn rotation_stays_with_compressed_frame_and_uses_oriented_input_extents() {
        let mut queue = crate::video_output::Queue::default();
        assert!(queue.submit(&[1, 2, 3], true, false, 42));
        queue.set_last_packet(1, true);
        let frame = queue.pop().unwrap();
        assert_eq!((frame.timestamp_100ns, frame.rotation), (42, 1));
        let source = crate::video_windows::display_extents(1920, 1080, frame.rotation);
        assert_eq!(source, (1080, 1920));
        let viewport = crate::viewport::Viewport {
            source,
            client: (540, 960),
        };
        assert_eq!(viewport.map(270, 480), Some((540, 960)));
        assert_eq!(
            crate::video_windows::display_extents(1920, 1080, 2),
            (1920, 1080)
        );
    }
    #[test]
    fn ordinary_settings_save_cannot_restore_a_signed_out_session() {
        let path = std::env::temp_dir().join(format!(
            "evoparseq-profile-{}-{}.json",
            std::process::id(),
            crate::bud_attempt::native_attempt_id().unwrap()
        ));
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
        let _cleanup = Cleanup(path.clone());
        let manager = crate::connection_settings::Manager::open(path.clone());
        use crate::native_core::sign_in::SessionStore;
        SessionStore::save(
            manager.as_ref(),
            "fixture@example.invalid",
            "fixture-session",
        )
        .unwrap();
        assert_eq!(
            manager
                .view()
                .0
                .native_core
                .session()
                .unwrap()
                .unwrap()
                .token(),
            "fixture-session"
        );
        let mut stale = manager.view().0;
        stale.native_core.hide_session_button = true;
        SessionStore::clear(manager.as_ref(), false).unwrap();
        manager.save(stale, "", "", false, false).unwrap();
        assert!(manager.view().0.native_core.encrypted_session.is_empty());
        assert_eq!(
            manager.view().0.native_core.email,
            "fixture@example.invalid"
        );
        assert!(manager.native_session_button_hidden());
        assert!(
            crate::connection_settings::Manager::open(path.clone()).native_session_button_hidden()
        );
        assert!(!std::fs::read_to_string(&path)
            .unwrap()
            .contains("fixture-session"));
        SessionStore::clear(manager.as_ref(), true).unwrap();
        assert!(manager.view().0.native_core.email.is_empty());
    }
}
