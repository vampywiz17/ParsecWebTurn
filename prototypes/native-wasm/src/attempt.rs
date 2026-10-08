//! Managed native offer creation behind parsec_web_new_attempt. The guest's
//! private ABI is isolated; no account, signaling server or remote host is used.
use crate::{memory::GuestMemory, signaling::Description, transport::CHANNELS};
use anyhow::{bail, Context, Result};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc, Arc, Condvar, Mutex,
    },
    time::Duration,
};
static ACTIVE_WORKERS: AtomicUsize = AtomicUsize::new(0);
struct Permit;
impl Drop for Permit {
    fn drop(&mut self) {
        ACTIVE_WORKERS.fetch_sub(1, Ordering::SeqCst);
    }
}
use webrtc::{
    api::{setting_engine::SettingEngine, APIBuilder},
    data_channel::data_channel_init::RTCDataChannelInit,
    ice::network_type::NetworkType,
    peer_connection::{
        configuration::RTCConfiguration, peer_connection_state::RTCPeerConnectionState,
    },
};

#[derive(Clone)]
pub struct Output {
    memory: GuestMemory,
    strings: [u32; 3],
    capacity: usize,
    sync: u32,
    error: u32,
}

impl Output {
    pub fn new(
        memory: GuestMemory,
        strings: [u32; 3],
        capacity: usize,
        sync: u32,
        error: u32,
    ) -> Result<Self> {
        if !(2..=4096).contains(&capacity) {
            bail!("invalid attempt output capacity");
        }
        memory.sync_word(sync)?;
        let mut ranges = Vec::new();
        for ptr in strings {
            ranges.push(memory.range(ptr, capacity)?);
        }
        ranges.push(memory.range(sync, 4)?);
        ranges.push(memory.range(error, 4)?);
        for (i, a) in ranges.iter().enumerate() {
            for b in &ranges[i + 1..] {
                if a.start < b.end && b.start < a.end {
                    bail!("overlapping attempt output buffers");
                }
            }
        }
        Ok(Self {
            memory,
            strings,
            capacity,
            sync,
            error,
        })
    }
    pub fn finish(&self, credentials: Option<&Description>) -> Result<bool> {
        // Validate every size before writing any credential. Failure leaves the
        // buffers empty, so stale credentials cannot look like a valid offer.
        let values = credentials.map(|d| {
            [
                &d.credentials.ufrag,
                &d.credentials.password,
                &d.credentials.fingerprint,
            ]
        });
        let fits = values
            .as_ref()
            .is_some_and(|values| values.iter().all(|v| v.len() < self.capacity));
        for (i, ptr) in self.strings.iter().enumerate() {
            self.memory.c_string(
                *ptr,
                self.capacity,
                if fits {
                    values.as_ref().unwrap()[i]
                } else {
                    ""
                },
            )?;
        }
        self.memory.set_u32(self.error, u32::from(!fits))?;
        self.memory.signal(self.sync)?;
        Ok(fits)
    }
}

#[derive(Default)]
struct Progress {
    ready: bool,
    closed: bool,
    failed: bool,
    channels: usize,
}

struct Completion {
    output: Option<Output>,
    progress: Progress,
}

pub struct Attempt {
    cancel: Option<mpsc::Sender<()>>,
    completion: Arc<Mutex<Completion>>,
    finished: Arc<(Mutex<bool>, Condvar)>,
}

impl Attempt {
    pub fn spawn(output: Output) -> Result<Self> {
        let mut active = ACTIVE_WORKERS.load(Ordering::SeqCst);
        loop {
            if active >= 8 {
                bail!("native offer worker limit reached");
            }
            match ACTIVE_WORKERS.compare_exchange(
                active,
                active + 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => break,
                Err(current) => active = current,
            }
        }
        let permit = Permit;
        let (tx, rx) = mpsc::channel();
        let completion = Arc::new(Mutex::new(Completion {
            output: Some(output),
            progress: Default::default(),
        }));
        let finished = Arc::new((Mutex::new(false), Condvar::new()));
        let shared = completion.clone();
        let done = finished.clone();
        std::thread::Builder::new()
            .name("parsec-native-offer".into())
            .spawn(move || {
                let _permit = permit;
                let result = worker(&shared, rx);
                let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
                if result.is_err() {
                    state.progress.failed = true;
                    if let Some(output) = state.output.take() {
                        let _ = output.finish(None);
                    }
                }
                *done.0.lock().unwrap_or_else(|e| e.into_inner()) = true;
                done.1.notify_all();
            })
            .context("starting native offer worker")?;
        Ok(Self {
            cancel: Some(tx),
            completion,
            finished,
        })
    }

    pub fn cancel(&mut self) {
        // Serialize publication and cancellation. Taking the output lease
        // prevents the old worker from writing into reused guest buffers.
        let mut state = self.completion.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(output) = state.output.take() {
            if output.finish(None).is_err() {
                state.progress.failed = true;
            }
        }
        drop(state);
        if let Some(tx) = self.cancel.take() {
            let _ = tx.send(());
        }
    }

    pub fn failed(&self) -> bool {
        self.completion
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .progress
            .failed
    }

    pub fn snapshot(&self) -> serde_json::Value {
        let state = self.completion.lock().unwrap_or_else(|e| e.into_inner());
        serde_json::json!({ "offer_ready": state.progress.ready, "peer_closed": state.progress.closed, "failed": state.progress.failed, "negotiated_channels": state.progress.channels, "worker_finished": *self.finished.0.lock().unwrap_or_else(|e| e.into_inner()), "host_connected": false })
    }

    pub fn wait_finished(&self, duration: Duration) -> Result<()> {
        let done = self
            .finished
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("offer completion lock poisoned"))?;
        let (done, _) = self
            .finished
            .1
            .wait_timeout_while(done, duration, |done| !*done)
            .map_err(|_| anyhow::anyhow!("offer completion wait failed"))?;
        if !*done {
            bail!("native offer worker did not finish within the deadline");
        }
        Ok(())
    }
}

impl Drop for Attempt {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn worker(shared: &Arc<Mutex<Completion>>, cancel: mpsc::Receiver<()>) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let mut settings = SettingEngine::default();
    settings.set_network_types(vec![NetworkType::Udp4]);
    let api = APIBuilder::new().with_setting_engine(settings).build();
    let peer = runtime.block_on(async {
        tokio::time::timeout(
            Duration::from_secs(5),
            api.new_peer_connection(RTCConfiguration::default()),
        )
        .await
    })??;
    let prepared = runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(5), async {
            let mut channels = Vec::new();
            for (id, label) in CHANNELS {
                channels.push(
                    peer.create_data_channel(
                        label,
                        Some(RTCDataChannelInit {
                            negotiated: Some(id),
                            ordered: Some(true),
                            ..Default::default()
                        }),
                    )
                    .await?,
                );
            }
            let offer = peer.create_offer(None).await?;
            let description = Description::from_sdp(&offer.sdp)?;
            Ok::<_, anyhow::Error>((offer, description, channels))
        })
        .await
    });
    let outcome = (|| -> Result<()> {
        let (offer, description, channels) = prepared??;
        let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(output) = state.output.take() {
            state.progress.ready = output.finish(Some(&description))?;
            state.progress.failed = !state.progress.ready;
            state.progress.channels = channels.len();
        }
        let ready = state.progress.ready;
        drop(state);
        // Retain the real offer/peer/channels for the following begin_p2p
        // integration. M3b intentionally does not install a remote description.
        if ready {
            if matches!(
                cancel.recv_timeout(Duration::from_secs(30)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ) {
                bail!("native pending offer expired");
            }
        }
        drop((offer, channels));
        Ok(())
    })();
    let closed = runtime
        .block_on(async { tokio::time::timeout(Duration::from_secs(2), peer.close()).await });
    let is_closed =
        matches!(closed, Ok(Ok(()))) && peer.connection_state() == RTCPeerConnectionState::Closed;
    shared
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .progress
        .closed = is_closed;
    if !is_closed {
        bail!("native offer peer did not close within the deadline");
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signaling::Credentials;
    use wasmtime::{Config, Engine, MemoryType, SharedMemory};
    fn memory() -> GuestMemory {
        let mut config = Config::new();
        config.wasm_threads(true);
        GuestMemory(
            SharedMemory::new(&Engine::new(&config).unwrap(), MemoryType::shared(1, 1)).unwrap(),
        )
    }
    #[test]
    fn outputs_reject_aliases_and_incomplete_credentials_before_publication() {
        let m = memory();
        assert!(Output::new(m.clone(), [100, 200, 700], 256, 64, 80).is_err());
        assert!(Output::new(m.clone(), [100, 400, 700], 256, 65, 80).is_err());
        assert!(Output::new(m.clone(), [100, 400, 700], 256, 64, 702).is_err());
        let output = Output::new(m.clone(), [100, 400, 700], 128, 64, 80).unwrap();
        let d = Description {
            credentials: Credentials {
                ufrag: "abcd".into(),
                password: "x".repeat(256),
                fingerprint: format!("sha-256 {}", ["AB"; 32].join(":")),
            },
            mid: "0".into(),
        };
        assert!(!output.finish(Some(&d)).unwrap());
        assert_eq!(m.u32(80).unwrap(), 1);
        assert_eq!(m.string(100, 128).unwrap(), "");
        assert_eq!(m.string(400, 128).unwrap(), "");
        assert_eq!(m.string(700, 128).unwrap(), "");
    }
    #[test]
    fn cancellation_prevents_writes_into_reused_guest_buffers() {
        let m = memory();
        let output = Output::new(m.clone(), [100, 400, 700], 256, 64, 80).unwrap();
        let mut attempt = Attempt::spawn(output).unwrap();
        attempt.cancel();
        m.sync_word(64).unwrap().store(0, Ordering::SeqCst);
        for ptr in [100, 400, 700] {
            m.c_string(ptr, 256, "reused-buffer").unwrap();
        }
        m.set_u32(80, 12345).unwrap();
        attempt.wait_finished(Duration::from_secs(12)).unwrap();
        for ptr in [100, 400, 700] {
            assert_eq!(m.string(ptr, 256).unwrap(), "reused-buffer");
        }
        assert_eq!(m.u32(80).unwrap(), 12345);
        assert_eq!(attempt.snapshot()["peer_closed"], true);
        assert_eq!(attempt.snapshot()["host_connected"], false);
    }
}
