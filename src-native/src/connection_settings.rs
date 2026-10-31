//! Persisted native connection settings and RTCIceServer configuration.
#[cfg(test)]
use crate::turn_configuration::parse_cloudflare;
use crate::turn_configuration::{cloudflare_request, validate_urls};
use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, io::Write, path::PathBuf, sync::Mutex, time::Instant};

use std::{io::Read, sync::Arc};
use webrtc::ice_transport::ice_server::RTCIceServer;

pub const PARSEC_STUN: &str = "stun:stun.parsec.gg:3478";
/// Eligibility policy only; actual profile/output support still requires
/// the video worker's device capability checks.
pub fn full_chroma_vendor_allowed(vendor: u32) -> bool {
    matches!(vendor, 0x10de | 0x8086)
}
const MAX_FILE: usize = 65536;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PresentationMode {
    #[default]
    Automatic,
    LowLatency,
    Vsync,
    Vrr,
}

impl PresentationMode {
    pub const ALL: [Self; 4] = [Self::Automatic, Self::LowLatency, Self::Vsync, Self::Vrr];
    pub fn label(self) -> &'static str {
        match self {
            Self::Automatic => "Automatic (existing behavior)",
            Self::LowLatency => "Low latency (one queued presentation)",
            Self::Vsync => "VSync (synchronize to display refresh)",
            Self::Vrr => "VRR (experimental; compatible display required)",
        }
    }
    pub fn from_index(index: usize) -> Self {
        Self::ALL.get(index).copied().unwrap_or_default()
    }
    pub fn low_latency(self) -> bool {
        matches!(self, Self::LowLatency | Self::Vrr)
    }
    pub fn timing_status(self, queue_control: bool, tearing: bool) -> &'static str {
        match (self, queue_control, tearing) {
            (Self::Automatic, _, _) => "Automatic",
            (Self::LowLatency, true, _) => "Low latency; maximum queued presentations: 1",
            (Self::LowLatency, false, _) => "Low latency unavailable; automatic timing",
            (Self::Vsync, true, _) => "VSync; maximum queued presentations: 1",
            (Self::Vsync, false, _) => "VSync; queue control unavailable",
            (Self::Vrr, true, true) => {
                "VRR path enabled; queue limit: 1; panel activation unverified"
            }
            (Self::Vrr, false, true) => {
                "VRR path enabled; queue control unavailable; panel activation unverified"
            }
            (Self::Vrr, true, false) => "VRR unavailable; using low-latency timing",
            (Self::Vrr, false, false) => "VRR unavailable; using automatic timing",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VsrRuntimeConsent {
    #[default]
    Ask,
    Allowed,
    Declined,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuPreference {
    pub name: String,
    pub vendor_id: u32,
    pub device_id: u32,
    pub subsystem_id: u32,
    pub revision: u32,
    pub luid: u64,
}
impl GpuPreference {
    pub fn same_hardware(&self, other: &Self) -> bool {
        (
            self.vendor_id,
            self.device_id,
            self.subsystem_id,
            self.revision,
        ) == (
            other.vendor_id,
            other.device_id,
            other.subsystem_id,
            other.revision,
        )
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub native_core: crate::native_core::profile::Profile,
    #[cfg(feature = "bud")]
    #[serde(alias = "experimentalBud")]
    pub bud: bool,
    #[cfg(feature = "bud")]
    pub experimental_hid_mouse: bool,
    #[cfg(feature = "bud")]
    pub experimental_hid_keyboard: bool,
    pub prefer_hevc: bool,
    pub presentation_mode: PresentationMode,
    /// Optional RTX Video quality: 0 off, 1..=4 low through ultra.
    pub nvidia_vsr: u8,
    pub intel_ai_vsr: bool,
    pub vsr_runtime_consent: VsrRuntimeConsent,
    pub experimental_ten_bit: bool,
    pub experimental_chroma_444: bool,
    pub event_logging: bool,
    pub experimental_tcp_qos: bool,
    #[serde(alias = "experimentalAudioBuffer")]
    pub custom_audio_buffer: bool,
    pub audio_min_buffer_ms: u32,
    pub audio_max_buffer_ms: u32,
    #[serde(alias = "experimentalRawAudio")]
    pub allow_raw_audio: bool,
    pub experimental_surround_audio: bool,
    #[cfg(feature = "vendor-video")]
    pub experimental_zero_copy: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_gpu: Option<GpuPreference>,
    pub provider: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub custom_urls: Vec<String>,
    pub stun_urls: Vec<String>,
    pub turn_urls: Vec<String>,
    pub stun_only: bool,
    pub force_relay: bool,
    pub custom_username: String,
    pub encrypted_custom_password: String,
    pub turn_key_id: String,
    pub encrypted_api_token: String,
    pub cache_credentials: bool,
    pub media_diagnostics: bool,
    pub ttl: u32,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}
impl Settings {
    pub fn ai_upscale_quality(&self, vendor: Option<u32>) -> u8 {
        if cfg!(feature = "vendor-video")
            && vendor == Some(0x8086)
            && self.intel_ai_vsr
            && !self.experimental_ten_bit
            && !self.experimental_chroma_444
        {
            1
        } else {
            self.nvidia_vsr_quality(vendor)
        }
    }
    pub fn nvidia_vsr_quality(&self, vendor: Option<u32>) -> u8 {
        if cfg!(nvidia_vsr_sdk) && vendor == Some(0x10de) {
            self.nvidia_vsr.min(4)
        } else {
            0
        }
    }
}
impl Default for Settings {
    // Legacy defaults apply to existing JSON; a new native installation uses fresh().
    fn default() -> Self {
        Self {
            native_core: Default::default(),
            #[cfg(feature = "bud")]
            bud: false,
            #[cfg(feature = "bud")]
            experimental_hid_mouse: false,
            #[cfg(feature = "bud")]
            experimental_hid_keyboard: false,
            prefer_hevc: false,
            presentation_mode: PresentationMode::Automatic,
            nvidia_vsr: 0,
            intel_ai_vsr: false,
            vsr_runtime_consent: VsrRuntimeConsent::Ask,
            experimental_ten_bit: false,
            experimental_chroma_444: false,
            event_logging: false,
            experimental_tcp_qos: false,
            custom_audio_buffer: false,
            audio_min_buffer_ms: 75,
            audio_max_buffer_ms: 150,
            allow_raw_audio: false,
            experimental_surround_audio: false,
            #[cfg(feature = "vendor-video")]
            experimental_zero_copy: false,
            video_gpu: None,
            provider: "cloudflare".into(),
            custom_urls: vec![],
            stun_urls: vec![],
            turn_urls: vec![],
            stun_only: false,
            force_relay: false,
            custom_username: String::new(),
            encrypted_custom_password: String::new(),
            turn_key_id: String::new(),
            encrypted_api_token: String::new(),
            cache_credentials: false,
            media_diagnostics: false,
            ttl: 86400,
            extra: BTreeMap::new(),
        }
    }
}
impl Settings {
    #[cfg(feature = "bud")]
    pub fn hid_mouse_enabled(&self) -> bool {
        self.bud && self.experimental_hid_mouse
    }

    #[cfg(feature = "bud")]
    pub fn hid_keyboard_enabled(&self) -> bool {
        self.bud && self.experimental_hid_keyboard
    }

    fn fresh() -> Self {
        Self {
            provider: "custom".into(),
            ..Self::default()
        }
    }
    fn normalize(&mut self) {
        if self.provider.is_empty() {
            self.provider = "cloudflare".into();
        }
        if self.ttl == 0 {
            self.ttl = 86400;
        }
        if self.stun_urls.is_empty() && self.turn_urls.is_empty() {
            for url in &self.custom_urls {
                if url.trim().to_ascii_lowercase().starts_with("stun") {
                    self.stun_urls.push(url.trim().into());
                } else {
                    self.turn_urls.push(url.trim().into());
                }
            }
        }
        self.custom_urls.clear();
    }
    pub fn secret(&self) -> Result<String> {
        if self.stun_only || (self.provider == "custom" && self.turn_urls.is_empty()) {
            return Ok(String::new());
        }
        unprotect(if self.provider == "custom" {
            &self.encrypted_custom_password
        } else {
            &self.encrypted_api_token
        })
    }
    pub fn validate(&self) -> Result<()> {
        if self.custom_audio_buffer
            && (self.audio_max_buffer_ms == 0
                || self.audio_max_buffer_ms > 500
                || self.audio_min_buffer_ms > self.audio_max_buffer_ms)
        {
            bail!("Audio buffer must satisfy 0 <= minimum <= maximum <= 500 ms, with maximum above zero");
        }
        if self.force_relay && self.stun_only {
            bail!("Force TURN relay cannot be combined with STUN only");
        }
        if self.force_relay && self.provider == "custom" && self.turn_urls.is_empty() {
            bail!("Force TURN relay requires at least one TURN server");
        }
        if !matches!(self.provider.as_str(), "custom" | "cloudflare") {
            bail!("Choose Custom servers or Cloudflare TURN");
        }
        validate_urls(&self.stun_urls, false)?;
        // Dormant TURN settings survive STUN-only mode unchanged, including secrets.
        if self.stun_only {
            return Ok(());
        }
        let secret = self.secret()?;
        if self.provider == "custom" {
            validate_urls(&self.turn_urls, true)?;
            if !self.turn_urls.is_empty()
                && (self.custom_username.trim().is_empty() || secret.is_empty())
            {
                bail!("TURN requires a username and password");
            }
        } else {
            if self.turn_key_id.is_empty()
                || self.turn_key_id.len() > 256
                || !self
                    .turn_key_id
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
            {
                bail!("Enter a valid Cloudflare TURN Key ID");
            }
            if secret.trim().is_empty() || secret.contains(['\r', '\n']) {
                bail!("Enter a valid Cloudflare API token");
            }
            if !(60..=172800).contains(&self.ttl) {
                bail!("Credential lifetime must be 60Ă˘â‚¬â€ś172800 seconds");
            }
        }
        Ok(())
    }
    pub fn stun_servers(&self) -> Vec<RTCIceServer> {
        let urls = if self.stun_urls.is_empty() {
            // Preserve the old explicit Cloudflare STUN-only configuration.
            vec![if self.stun_only && self.provider == "cloudflare" {
                "stun:stun.cloudflare.com:3478"
            } else {
                PARSEC_STUN
            }
            .into()]
        } else {
            self.stun_urls.clone()
        };
        vec![RTCIceServer {
            urls,
            ..Default::default()
        }]
    }
    fn custom_servers(&self) -> Result<Vec<RTCIceServer>> {
        let mut servers = self.stun_servers();
        if !self.stun_only && !self.turn_urls.is_empty() {
            servers.push(RTCIceServer {
                urls: self.turn_urls.clone(),
                username: self.custom_username.clone(),
                credential: self.secret()?,
            });
        }
        Ok(servers)
    }
}

fn unprotect(value: &str) -> Result<String> {
    if value.is_empty() {
        return Ok(String::new());
    }
    let bytes = STANDARD
        .decode(value)
        .context("Invalid saved credential encoding")?;
    String::from_utf8(crate::secure_storage::protect(&bytes, false)?)
        .context("Cannot read saved credential for this Windows user")
}
fn protect(value: &str) -> Result<String> {
    Ok(STANDARD.encode(crate::secure_storage::protect(value.as_bytes(), true)?))
}

pub struct Manager {
    path: PathBuf,
    state: Mutex<(Settings, Option<String>)>,
    cache: Mutex<Option<Cached>>,
}
struct Cached {
    key: [u8; 32],
    issued: Instant,
    servers: Vec<RTCIceServer>,
}

impl Manager {
    pub fn directory(&self) -> PathBuf {
        self.path
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .to_path_buf()
    }
    pub fn open(path: PathBuf) -> Arc<Self> {
        let loaded = (|| -> Result<Settings> {
            let file = match std::fs::File::open(&path) {
                Ok(f) => f,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Settings::fresh()),
                Err(_) => bail!("Cannot read settings.json"),
            };
            let mut raw = Vec::new();
            file.take(MAX_FILE as u64 + 1).read_to_end(&mut raw)?;
            if raw.len() > MAX_FILE {
                bail!("settings.json exceeds 64 KiB");
            }
            let mut settings: Settings = serde_json::from_slice(&raw)
                .context("Invalid settings.json; the existing file has been preserved")?;
            settings.normalize();
            Ok(settings)
        })();
        let state = match loaded {
            Ok(s) => (s, None),
            Err(_) => (
                Settings::fresh(),
                Some(
                    "Cannot read settings.json. Fix the file or save valid settings to replace it."
                        .into(),
                ),
            ),
        };
        Arc::new(Self {
            path,
            state: Mutex::new(state),
            cache: Mutex::new(None),
        })
    }
    pub fn view(&self) -> (Settings, Option<String>) {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner()).clone();
        state
    }

    pub fn native_session_button_hidden(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .0
            .native_core
            .hide_session_button
    }
    pub fn save(
        &self,
        mut settings: Settings,
        password: &str,
        token: &str,
        forget_password: bool,
        forget_token: bool,
    ) -> Result<()> {
        settings.normalize();
        if forget_password {
            settings.encrypted_custom_password.clear();
        }
        if forget_token {
            settings.encrypted_api_token.clear();
        }
        if !password.is_empty() {
            settings.encrypted_custom_password = protect(password)?;
        }
        if !token.is_empty() {
            settings.encrypted_api_token = protect(token)?;
        }
        settings.validate()?;

        let mut current = self.state.lock().unwrap_or_else(|e| e.into_inner());

        {
            let hide_button = settings.native_core.hide_session_button;
            settings.native_core = current.0.native_core.clone();
            settings.native_core.hide_session_button = hide_button;
        }
        let raw = serde_json::to_vec_pretty(&settings)?;
        if raw.len() > MAX_FILE {
            bail!("Settings exceed 64 KiB");
        }
        atomic_write(&self.path, &raw)?;
        let logging = settings.event_logging;

        {
            *current = (settings, None);
        }

        *self.cache.lock().unwrap_or_else(|e| e.into_inner()) = None;
        crate::event_log::configure(logging)
            .context("Settings saved, but the event log could not be opened")?;
        Ok(())
    }

    pub fn update_native(
        &self,
        update: impl FnOnce(&mut crate::native_core::profile::Profile),
    ) -> Result<()> {
        self.update_local(|settings| {
            update(&mut settings.native_core);
            settings.native_core.video.validate()
        })
    }
    /// Local setup consent must be saveable before TURN credentials exist.
    pub fn update_vsr_consent(&self, consent: VsrRuntimeConsent) -> Result<()> {
        self.update_local(|settings| {
            settings.vsr_runtime_consent = consent;
            Ok(())
        })
    }
    fn update_local(&self, update: impl FnOnce(&mut Settings) -> Result<()>) -> Result<()> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        anyhow::ensure!(
            state.1.is_none(),
            "Fix settings.json before saving local preferences"
        );
        let mut settings = state.0.clone();
        update(&mut settings)?;
        let raw = serde_json::to_vec_pretty(&settings)?;
        anyhow::ensure!(raw.len() <= MAX_FILE, "Settings exceed 64 KiB");
        atomic_write(&self.path, &raw)?;
        state.0 = settings;
        Ok(())
    }
    #[cfg(test)]
    pub async fn resolve(&self) -> Result<Vec<RTCIceServer>> {
        let (settings, error) = self.view();
        if error.is_some() {
            bail!("Fix connection settings before connecting");
        }
        self.resolve_settings(&settings).await
    }
    /// Explicit BUD adapter: freeze preferences and credentials together.
    #[cfg(all(windows, feature = "bud"))]
    pub async fn resolve_bud(&self) -> Result<(Settings, Vec<RTCIceServer>)> {
        let (settings, error) = self.view();
        if error.is_some() {
            bail!("Fix connection settings before connecting");
        }
        let servers = if settings.force_relay {
            self.resolve_settings(&settings).await?
        } else {
            settings.stun_servers()
        };
        Ok((settings, servers))
    }

    #[cfg(all(windows, feature = "bud"))]
    pub async fn resolve_bud_relay(&self, settings: &Settings) -> Result<Vec<RTCIceServer>> {
        if settings.stun_only {
            bail!("TURN is disabled by STUN-only settings");
        }
        self.resolve_settings(settings).await
    }

    /// Resolve credentials and routing from the same settings snapshot.
    pub async fn resolve_configuration(
        &self,
    ) -> Result<webrtc::peer_connection::configuration::RTCConfiguration> {
        use webrtc::peer_connection::{
            configuration::RTCConfiguration, policy::ice_transport_policy::RTCIceTransportPolicy,
        };
        let (settings, error) = self.view();
        if error.is_some() {
            bail!("Fix connection settings before connecting");
        }
        Ok(RTCConfiguration {
            ice_servers: self.resolve_settings(&settings).await?,
            ice_transport_policy: if settings.force_relay {
                RTCIceTransportPolicy::Relay
            } else {
                RTCIceTransportPolicy::All
            },
            ..Default::default()
        })
    }
    async fn resolve_settings(&self, settings: &Settings) -> Result<Vec<RTCIceServer>> {
        use sha2::{Digest, Sha256};
        settings.validate()?;
        if settings.stun_only || settings.provider == "custom" {
            return settings.custom_servers();
        }
        let secret = settings.secret()?;
        let key: [u8; 32] = Sha256::digest(serde_json::to_vec(&(
            &settings.turn_key_id,
            &secret,
            settings.ttl,
        ))?)
        .into();
        if settings.cache_credentials {
            let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(cache) = cache.as_ref().filter(|c| {
                c.key == key
                    && c.issued.elapsed().as_secs()
                        < u64::from(settings.ttl.saturating_sub((settings.ttl / 4).max(30)))
            }) {
                let mut servers = settings.stun_servers();
                servers.extend(cache.servers.clone());
                return Ok(servers);
            }
        }
        let issued = Instant::now();
        let endpoint = format!(
            "https://rtc.live.cloudflare.com/v1/turn/keys/{}/credentials/generate-ice-servers",
            settings.turn_key_id
        );
        let servers = cloudflare_request(&endpoint, &secret, settings.ttl).await?;
        if settings.cache_credentials {
            *self.cache.lock().unwrap_or_else(|e| e.into_inner()) = Some(Cached {
                key,
                issued,
                servers: servers.clone(),
            });
        }
        let mut result = settings.stun_servers();
        result.extend(servers);
        Ok(result)
    }
}

fn atomic_write(path: &std::path::Path, raw: &[u8]) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };
    let mut random = [0; 16];
    getrandom::fill(&mut random)
        .map_err(|_| anyhow::anyhow!("Cannot allocate settings transaction"))?;
    let name: String = random.iter().map(|b| format!("{b:02x}")).collect();
    let temporary = path.with_file_name(format!("settings-{name}.tmp"));
    let result = (|| -> Result<()> {
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&temporary).context("Cannot write settings beside the executable; use --data-dir with a writable folder")?;
        file.write_all(raw)?;
        file.sync_all()?;
        drop(file);
        let from: Vec<_> = temporary.as_os_str().encode_wide().chain([0]).collect();
        let to: Vec<_> = path.as_os_str().encode_wide().chain([0]).collect();
        if unsafe {
            MoveFileExW(
                from.as_ptr(),
                to.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            bail!("Cannot replace settings.json; existing settings were preserved");
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use std::time::Duration;
    #[cfg(feature = "bud")]
    #[test]
    fn promoted_settings_accept_old_keys_and_save_current_names() {
        let settings: Settings = serde_json::from_str(
            r#"{"provider":"custom","experimentalBud":true,"experimentalAudioBuffer":true,"experimentalRawAudio":true,"audioMinBufferMs":20,"audioMaxBufferMs":60,"experimentalSurroundAudio":true}"#,
        )
        .unwrap();
        assert!(settings.bud && settings.custom_audio_buffer && settings.allow_raw_audio);
        assert!(settings.experimental_surround_audio);
        assert_eq!(
            (settings.audio_min_buffer_ms, settings.audio_max_buffer_ms),
            (20, 60)
        );
        settings.validate().unwrap();
        let saved = serde_json::to_value(&settings).unwrap();
        for key in ["bud", "customAudioBuffer", "allowRawAudio"] {
            assert_eq!(saved[key], true);
        }
        for key in [
            "experimentalBud",
            "experimentalAudioBuffer",
            "experimentalRawAudio",
        ] {
            assert!(saved.get(key).is_none());
        }
        let restored: Settings = serde_json::from_value(saved).unwrap();
        assert!(restored.bud && restored.custom_audio_buffer && restored.allow_raw_audio);
    }
    #[cfg(feature = "bud")]
    #[test]
    fn hid_input_preference_roundtrips_without_enabling_webrtc_input() {
        let mut s: Settings = serde_json::from_str("{}").unwrap();
        assert!(!s.hid_mouse_enabled());
        s.experimental_hid_mouse = true;
        assert!(!s.hid_mouse_enabled());
        let restored: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert!(restored.experimental_hid_mouse);
        assert!(!restored.hid_mouse_enabled());
        s.bud = true;
        assert!(s.hid_mouse_enabled());
    }

    #[test]
    fn audio_defaults_and_limits_preserve_automatic_settings() {
        let mut settings = super::Settings::fresh();
        assert!(!settings.custom_audio_buffer);
        assert!(!settings.allow_raw_audio);
        assert_eq!(
            (settings.audio_min_buffer_ms, settings.audio_max_buffer_ms),
            (75, 150)
        );
        settings.custom_audio_buffer = true;
        settings.audio_min_buffer_ms = 20;
        settings.audio_max_buffer_ms = 60;
        assert!(settings.validate().is_ok());
        settings.audio_min_buffer_ms = 61;
        assert!(settings.validate().is_err());
        settings.audio_min_buffer_ms = 0;
        settings.audio_max_buffer_ms = 0;
        assert!(settings.validate().is_err());
        settings.audio_max_buffer_ms = 501;
        assert!(settings.validate().is_err());
        settings.custom_audio_buffer = false;
        assert!(settings.validate().is_ok());
    }
    #[test]
    fn legacy_settings_keep_existing_presentation_and_modes_roundtrip() {
        assert_eq!(
            serde_json::from_str::<Settings>("{}")
                .unwrap()
                .presentation_mode,
            PresentationMode::Automatic
        );
        for (index, mode) in PresentationMode::ALL.into_iter().enumerate() {
            assert_eq!(PresentationMode::from_index(index), mode);
            assert!(!mode.label().is_empty());
            let settings = Settings {
                presentation_mode: mode,
                ..Default::default()
            };
            let restored: Settings =
                serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
            assert_eq!(restored.presentation_mode, mode);
        }
        assert_eq!(
            PresentationMode::from_index(usize::MAX),
            PresentationMode::Automatic
        );
        assert!(PresentationMode::Vrr.low_latency());
        assert!(PresentationMode::Vrr
            .timing_status(true, true)
            .contains("unverified"));
        assert!(PresentationMode::Vrr
            .timing_status(true, false)
            .contains("unavailable"));
    }
    use super::*;

    #[tokio::test]
    async fn relay_policy_is_persisted_and_cannot_fall_back_to_direct() {
        use webrtc::peer_connection::policy::ice_transport_policy::RTCIceTransportPolicy;
        let dir = Directory::new();
        let manager = Manager::open(dir.file());
        let mut s = Settings::fresh();
        assert!(!s.force_relay);
        s.force_relay = true;
        assert!(s.validate().is_err()); // no TURN server
        s.turn_urls = vec![
            "turns:relay.example.invalid:443".into(),
            "turn:relay.example.invalid:3478?transport=udp".into(),
            "turn:relay.example.invalid:3478?transport=tcp".into(),
        ];
        s.custom_username = "fixture".into();
        manager
            .save(s, "fixture-password", "", false, false)
            .unwrap();
        let manager = Manager::open(dir.file());
        assert!(manager.view().0.force_relay);
        let config = manager.resolve_configuration().await.unwrap();
        assert_eq!(config.ice_transport_policy, RTCIceTransportPolicy::Relay);
        assert_eq!(config.ice_servers[0].urls, [PARSEC_STUN]);
        assert_eq!(config.ice_servers[1].urls, manager.view().0.turn_urls);
        let mut s = manager.view().0;
        s.stun_only = true;
        assert!(s.validate().is_err());
        s.stun_only = false;
        s.force_relay = false;
        manager.save(s, "", "", false, false).unwrap();
        let config = manager.resolve_configuration().await.unwrap();
        assert_eq!(config.ice_transport_policy, RTCIceTransportPolicy::All);
        assert_eq!(config.ice_servers[1].urls, manager.view().0.turn_urls);
        assert!(!serde_json::from_str::<Settings>("{}").unwrap().force_relay);
    }
    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            let mut nonce = [0; 16];
            getrandom::fill(&mut nonce).unwrap();
            let name: String = nonce.iter().map(|b| format!("{b:02x}")).collect();
            let path = std::env::temp_dir().join(format!("evoparseq-settings-{name}"));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn file(&self) -> PathBuf {
            self.0.join("settings.json")
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn fresh_install_uses_parsec_without_turn_or_file_write() {
        let dir = Directory::new();
        let manager = Manager::open(dir.file());
        let settings = manager.view().0;
        assert_eq!(settings.provider, "custom");
        assert_eq!(settings.stun_servers()[0].urls, [PARSEC_STUN]);
        assert_eq!(settings.custom_servers().unwrap().len(), 1);
        assert!(!dir.file().exists());
    }

    #[test]
    fn legacy_split_migration_and_unknown_fields_round_trip() {
        let dir = Directory::new();
        std::fs::write(dir.file(), br#"{"provider":"custom","stunOnly":true,"customUrls":[" STUN:example.org:3478 ","turn:example.org:3478"],"futureOption":{"keep":true}}"#).unwrap();
        let manager = Manager::open(dir.file());
        let s = manager.view().0;
        assert_eq!(s.stun_urls, ["STUN:example.org:3478"]);
        assert_eq!(s.turn_urls, ["turn:example.org:3478"]);
        manager.save(s, "", "", false, false).unwrap();
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.file()).unwrap()).unwrap();
        assert_eq!(value["futureOption"]["keep"], true);
        assert!(value.get("customUrls").is_none());
        let mut s: Settings = serde_json::from_str(
            r#"{"customUrls":["stun:old.example"],"stunUrls":["stun:new.example"]}"#,
        )
        .unwrap();
        s.normalize();
        assert_eq!(s.stun_urls, ["stun:new.example"]);
        assert!(s.custom_urls.is_empty());
    }

    #[tokio::test]
    async fn stun_only_never_requests_cloudflare_or_decrypts_dormant_secret() {
        let dir = Directory::new();
        let manager = Manager::open(dir.file());
        let s = Settings {
            stun_only: true,
            encrypted_api_token: "not-base64".into(),
            ..Settings::default()
        };
        manager.save(s, "", "", false, false).unwrap();
        let servers = manager.resolve().await.unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].urls, ["stun:stun.cloudflare.com:3478"]);
        assert!(servers[0].credential.is_empty());
    }

    #[test]
    fn saved_secrets_use_legacy_current_user_dpapi_and_survive_blank_edits() {
        let dir = Directory::new();
        let manager = Manager::open(dir.file());
        let s = Settings {
            turn_urls: vec!["turns:localhost:5349?transport=tcp".into()],
            custom_username: "fixture".into(),
            ..Settings::fresh()
        };
        manager
            .save(s, "synthetic-password", "", false, false)
            .unwrap();
        let s = manager.view().0;
        let blob = s.encrypted_custom_password.clone();
        // The old client's reader is Base64 -> DPAPI CurrentUser, no entropy.
        let decrypted =
            crate::secure_storage::protect(&STANDARD.decode(&blob).unwrap(), false).unwrap();
        assert_eq!(decrypted, b"synthetic-password");
        manager.save(s, "", "", false, false).unwrap();
        assert_eq!(
            Manager::open(dir.file()).view().0.encrypted_custom_password,
            blob
        );
        assert!(!String::from_utf8(std::fs::read(dir.file()).unwrap())
            .unwrap()
            .contains("synthetic-password"));
        let mut s = manager.view().0;
        s.stun_only = true;
        manager.save(s, "", "", true, false).unwrap();
        assert!(manager.view().0.encrypted_custom_password.is_empty());
    }

    #[test]
    fn invalid_file_and_invalid_edit_preserve_original_bytes() {
        let dir = Directory::new();
        std::fs::write(dir.file(), b"malformed").unwrap();
        let manager = Manager::open(dir.file());
        assert!(manager.view().1.is_some());
        let mut s = Settings::fresh();
        s.stun_urls = vec!["https://not-stun.example".into()];
        assert!(manager.save(s, "", "", false, false).is_err());
        assert_eq!(std::fs::read(dir.file()).unwrap(), b"malformed");
    }

    #[tokio::test]
    async fn credential_request_uses_post_and_preserves_generated_tls_urls() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            loop {
                let mut buffer = [0; 1024];
                let n = socket.read(&mut buffer).await.unwrap();
                assert_ne!(n, 0);
                request.extend_from_slice(&buffer[..n]);
                if request.ends_with(b"{\"ttl\":60}") {
                    break;
                }
                assert!(request.len() < 8192);
            }
            let body = br#"{"iceServers":[{"urls":"turns:localhost:5349?transport=tcp","username":"fixture","credential":"fixture-password"}]}"#;
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            socket.write_all(body).await.unwrap();
            String::from_utf8(request).unwrap()
        });
        let servers =
            cloudflare_request(&format!("http://{address}/fixture"), "synthetic-token", 60)
                .await
                .unwrap();
        assert_eq!(servers[0].urls, ["turns:localhost:5349?transport=tcp"]);
        let request = server.await.unwrap();
        assert!(request.starts_with("POST /fixture HTTP/1.1"));
        assert!(request
            .to_ascii_lowercase()
            .contains("authorization: bearer synthetic-token"));
        assert!(request.ends_with(r#"{"ttl":60}"#));
    }

    #[tokio::test]
    async fn credential_service_redirects_are_not_followed() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let _ = socket.read(&mut [0; 8192]).await.unwrap();
            socket.write_all(format!("HTTP/1.1 302 Found\r\nLocation: http://{address}/redirected\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
            drop(socket);
            assert!(
                tokio::time::timeout(Duration::from_millis(200), listener.accept())
                    .await
                    .is_err()
            );
        });
        let error = cloudflare_request(&format!("http://{address}/fixture"), "synthetic-token", 60)
            .await
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("HTTP 302"));
        assert!(!error.contains("synthetic-token"));
        server.await.unwrap();
    }

    #[test]
    fn supported_turn_transports_are_validated_without_forcing_relay() {
        for url in [
            "turn:example.org:3478",
            "turn:example.org:3478?transport=tcp",
            "turns:example.org:443",
            "turns:example.org:5349?transport=tcp",
        ] {
            validate_urls(&[url.into()], true).unwrap();
        }
        for url in [
            "turns:example.org?transport=udp",
            "turn:example.org:0",
            "turn:user:password@example.org",
            "turn://example.org",
        ] {
            assert!(validate_urls(&[url.into()], true).is_err(), "{url}");
        }
        assert!(validate_urls(&["stuns:example.org".into()], false).is_err());
        let servers = parse_cloudflare(br#"{"iceServers":[{"urls":["stun:stun.example.org"]},{"urls":["turn:example.org:3478?transport=udp","turn:example.org:3478?transport=tcp","turns:example.org:443?transport=tcp"],"username":"fixture","credential":"fixture-password"}]}"#).unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].urls.len(), 3);
        assert!(parse_cloudflare(
            br#"{"iceServers":[{"urls":"turns:example.org","username":"fixture"}]}"#
        )
        .is_err());
    }
}

#[cfg(test)]
mod runtime_consent_tests {
    use super::*;
    #[test]
    fn consent_can_be_saved_before_network_configuration_without_losing_profile() {
        let dir =
            std::env::temp_dir().join(format!("evoparseq-vsr-consent-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        let mut settings = Settings::default();
        settings.native_core.encrypted_session = "opaque-existing-protected-session".into();
        std::fs::write(&path, serde_json::to_vec(&settings).unwrap()).unwrap();
        let manager = Manager::open(path.clone());
        assert!(manager.view().0.validate().is_err());
        manager
            .update_vsr_consent(VsrRuntimeConsent::Allowed)
            .unwrap();
        let restored = Manager::open(path).view().0;
        assert_eq!(restored.vsr_runtime_consent, VsrRuntimeConsent::Allowed);
        assert_eq!(
            restored.native_core.encrypted_session,
            settings.native_core.encrypted_session
        );
        std::fs::remove_file(dir.join("settings.json")).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
}
