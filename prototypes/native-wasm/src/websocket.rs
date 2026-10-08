//! Text signaling bridge for the pinned Matoya ABI. Offline by default.
use anyhow::{bail, Context, Result};
use futures_util::{SinkExt, StreamExt};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{mpsc, Arc, Condvar, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::{mpsc as command, oneshot};
use tokio_tungstenite::tungstenite::{
    client::IntoClientRequest, protocol::WebSocketConfig, Message,
};
use wasmtime::{Caller, Val};

pub const MAX_MESSAGE: usize = 64 * 1024;
const LIMIT: Duration = Duration::from_secs(5);
const KEEPALIVE: Duration = Duration::from_secs(60);

#[derive(Default)]
struct Inbox {
    messages: VecDeque<String>,
    bytes: usize,
    closed: bool,
    failed: bool,
    code: u16,
}
struct Shared {
    inbox: Mutex<Inbox>,
    changed: Condvar,
}
struct WorkerGuard(Arc<Shared>);
impl Drop for WorkerGuard {
    fn drop(&mut self) {
        let closed = self
            .0
            .inbox
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .closed;
        if !closed {
            self.0.finish(1006, true);
        }
    }
}
impl Shared {
    fn new() -> Self {
        Self {
            inbox: Default::default(),
            changed: Condvar::new(),
        }
    }
    fn push(&self, text: String) -> bool {
        let mut inbox = self.inbox.lock().unwrap_or_else(|e| e.into_inner());
        if text.len() > MAX_MESSAGE
            || text.contains('\0')
            || inbox.messages.len() >= 16
            || inbox.bytes + text.len() > 1024 * 1024
        {
            return false;
        }
        inbox.bytes += text.len();
        inbox.messages.push_back(text);
        self.changed.notify_all();
        true
    }
    fn finish(&self, code: u16, failed: bool) {
        let mut inbox = self.inbox.lock().unwrap_or_else(|e| e.into_inner());
        inbox.closed = true;
        inbox.failed = failed;
        inbox.code = code;
        if failed {
            inbox.messages.clear();
            inbox.bytes = 0;
        }
        self.changed.notify_all();
    }
}
struct Write {
    text: String,
    expires: Instant,
    ack: mpsc::SyncSender<bool>,
}
struct Socket {
    shared: Arc<Shared>,
    writes: command::Sender<Write>,
    cancel: Mutex<Option<oneshot::Sender<()>>>,
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
}
impl Socket {
    fn start(
        url: String,
        timeout: Duration,
        heartbeat: Duration,
    ) -> std::result::Result<Arc<Self>, u16> {
        let shared = Arc::new(Shared::new());
        let state = shared.clone();
        let (writes, incoming) = command::channel(8);
        let (cancel, cancellation) = oneshot::channel();
        let (ready, outcome) = mpsc::sync_channel(1);
        let worker = std::thread::Builder::new()
            .name("parsec-ws".into())
            .spawn(move || {
                let _guard = WorkerGuard(state.clone());
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build();
                match runtime {
                    Ok(runtime) => runtime.block_on(run(
                        url,
                        timeout,
                        heartbeat,
                        state.clone(),
                        incoming,
                        cancellation,
                        ready,
                    )),
                    Err(_) => {
                        let _ = ready.send(Err(0));
                        state.finish(1006, true);
                    }
                }
            })
            .map_err(|_| 0u16)?;
        let socket = Arc::new(Self {
            shared,
            writes,
            cancel: Mutex::new(Some(cancel)),
            worker: Mutex::new(Some(worker)),
        });
        match outcome.recv_timeout(timeout + Duration::from_millis(100)) {
            Ok(Ok(())) => Ok(socket),
            Ok(Err(status)) => {
                socket.stop();
                Err(status)
            }
            Err(_) => {
                socket.stop();
                Err(0)
            }
        }
    }
    fn cancel(&self) {
        if let Some(cancel) = self.cancel.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = cancel.send(());
        }
    }
    fn stop(&self) {
        self.cancel();
        let worker = self.worker.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(worker) = worker {
            let _ = worker.join();
        }
    }
    fn write(&self, text: String) -> bool {
        if text.len() > MAX_MESSAGE
            || self
                .shared
                .inbox
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .closed
        {
            return false;
        }
        let (ack, result) = mpsc::sync_channel(1);
        if self
            .writes
            .try_send(Write {
                text,
                expires: Instant::now() + LIMIT,
                ack,
            })
            .is_err()
        {
            return false;
        }
        result
            .recv_timeout(LIMIT + Duration::from_millis(100))
            .unwrap_or(false)
    }
    fn read(
        &self,
        memory: &crate::memory::GuestMemory,
        output: u32,
        size: usize,
        timeout: Duration,
    ) -> Result<i32> {
        let deadline = Instant::now() + timeout;
        let mut inbox = self.shared.inbox.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(text) = inbox.messages.front() {
                if text.len() + 1 > size {
                    return Ok(3);
                }
                memory.c_string(output, size, text)?;
                let size = text.len();
                inbox.messages.pop_front();
                inbox.bytes -= size;
                return Ok(0);
            }
            if inbox.closed {
                return Ok(if inbox.failed { 3 } else { 1 });
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(2);
            }
            inbox = self
                .shared
                .changed
                .wait_timeout(inbox, remaining)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }
}
impl Drop for Socket {
    fn drop(&mut self) {
        self.stop();
    }
}

async fn run(
    url: String,
    timeout: Duration,
    heartbeat: Duration,
    state: Arc<Shared>,
    mut writes: command::Receiver<Write>,
    mut cancellation: oneshot::Receiver<()>,
    ready: mpsc::SyncSender<std::result::Result<(), u16>>,
) {
    let config = WebSocketConfig::default()
        .write_buffer_size(0)
        .max_message_size(Some(MAX_MESSAGE))
        .max_frame_size(Some(MAX_MESSAGE))
        .max_write_buffer_size(MAX_MESSAGE * 2);
    let connect = async {
        let request = url.into_client_request()?;
        tokio_tungstenite::connect_async_with_config(request, Some(config), false).await
    };
    let connected = tokio::select! {
        biased;
        _ = &mut cancellation => { state.finish(1000,false); return; }
        result = tokio::time::timeout(timeout,connect) => result,
    };
    let mut ws = match connected {
        Ok(Ok((ws, _))) => {
            let _ = ready.send(Ok(()));
            ws
        }
        error => {
            let status = match error {
                Ok(Err(tokio_tungstenite::tungstenite::Error::Http(response))) => {
                    response.status().as_u16()
                }
                _ => 0,
            };
            let _ = ready.send(Err(status));
            state.finish(1006, true);
            return;
        }
    };
    let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + heartbeat, heartbeat);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let (code, failed) = loop {
        tokio::select! {
            biased;
            _ = &mut cancellation => {
                let _ = tokio::time::timeout(Duration::from_secs(1),async {
                    ws.close(Some(tokio_tungstenite::tungstenite::protocol::CloseFrame {
                        code: tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::Normal,
                        reason: "".into(),
                    })).await?;
                    while let Some(message) = ws.next().await {
                        if matches!(message,Ok(Message::Close(_)) | Err(_)) { break; }
                    }
                    Ok::<(),tokio_tungstenite::tungstenite::Error>(())
                }).await;
                break (1000,false);
            }
            request = writes.recv() => {
                let Some(request) = request else { break (1000,false); };
                let remaining=request.expires.saturating_duration_since(Instant::now());
                if remaining.is_zero() { let _=request.ack.send(false); continue; }
                let ok = matches!(tokio::time::timeout(remaining,ws.send(Message::Text(request.text.into()))).await,Ok(Ok(())));
                let _ = request.ack.send(ok);
                if !ok { break (1006,true); }
            }
            message = ws.next() => {
                match message {
                    Some(Ok(Message::Text(text))) => { if !state.push(text.to_string()) { break (1009,true); } }
                    Some(Ok(Message::Close(frame))) => {
                        let code = frame.map(|f|u16::from(f.code)).unwrap_or(1000);
                        let _ = tokio::time::timeout(LIMIT,ws.flush()).await;
                        break (code,false);
                    }
                    Some(Ok(Message::Ping(_))) => {
                        if !matches!(tokio::time::timeout(LIMIT,ws.flush()).await,Ok(Ok(()))) { break (1006,true); }
                    }
                    Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(_)) => { break (1003,true); }
                    Some(Err(tokio_tungstenite::tungstenite::Error::Capacity(_))) => { break (1009,true); }
                    Some(Err(tokio_tungstenite::tungstenite::Error::Protocol(_))) => { break (1002,true); }
                    Some(Err(_)) | None => { break (1006,true); }
                }
            }
            _ = ticker.tick() => {
                if !matches!(tokio::time::timeout(LIMIT,ws.send(Message::Text("__ping__".into()))).await,Ok(Ok(()))) { break (1006,true); }
            }
        }
    };
    state.finish(code, failed);
}

#[derive(Default)]
struct Registry {
    next: u32,
    sockets: BTreeMap<u32, Arc<Socket>>,
}
pub struct Network {
    port: Option<u16>,
    heartbeat: Duration,
    registry: Mutex<Registry>,
}
impl Default for Network {
    fn default() -> Self {
        Self {
            port: None,
            heartbeat: KEEPALIVE,
            registry: Mutex::new(Registry {
                next: 1,
                sockets: BTreeMap::new(),
            }),
        }
    }
}
impl Network {
    pub fn diagnostic(port: u16, heartbeat: Duration) -> Self {
        Self {
            port: Some(port),
            heartbeat,
            ..Default::default()
        }
    }
    fn allowed(&self, url: &reqwest::Url) -> bool {
        self.port.is_some()
            && url.scheme() == "ws"
            && url.host_str() == Some("127.0.0.1")
            && url.port_or_known_default() == self.port
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none()
    }
    fn connect(&self, url: reqwest::Url, timeout: Duration) -> std::result::Result<u32, u16> {
        if !self.allowed(&url) {
            return Err(0);
        }
        let mut registry = self.registry.lock().unwrap_or_else(|e| e.into_inner());
        if registry.sockets.len() >= 4 || registry.next == u32::MAX {
            return Err(0);
        }
        let id = registry.next;
        registry.next += 1;
        let socket = Socket::start(url.to_string(), timeout, self.heartbeat)?;
        registry.sockets.insert(id, socket);
        Ok(id)
    }
    fn socket(&self, id: u32) -> Option<Arc<Socket>> {
        self.registry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .sockets
            .get(&id)
            .cloned()
    }
    fn destroy(&self, id: u32) {
        let socket = self
            .registry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .sockets
            .remove(&id);
        if let Some(socket) = socket {
            socket.stop();
        }
    }
    pub fn active_handles(&self) -> usize {
        self.registry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .sockets
            .len()
    }
}
impl Drop for Network {
    fn drop(&mut self) {
        // Signal every socket first, so teardown budgets run concurrently.
        let registry = self.registry.get_mut().unwrap_or_else(|e| e.into_inner());
        for socket in registry.sockets.values() {
            socket.cancel();
        }
        registry.sockets.clear();
    }
}

fn arg(args: &[Val], index: usize) -> Result<u32> {
    Ok(args
        .get(index)
        .and_then(Val::i32)
        .context("WebSocket ABI requires i32")? as u32)
}
fn timeout(ms: u32) -> Duration {
    Duration::from_millis(u64::from(ms)).min(LIMIT)
}
pub fn dispatch(
    caller: &mut Caller<'_, crate::host::HostState>,
    name: &str,
    args: &[Val],
    out: &mut [Val],
) -> Result<()> {
    let memory = caller.data().memory.clone();
    let network = caller.data().websocket.clone();
    match name {
        "MTY_WebSocketConnect" => {
            let status = arg(args, 4)?;
            if status != 0 {
                memory.write(status, &0u16.to_le_bytes())?;
            }
            out[0] = Val::I32(0);
            let input: Result<reqwest::Url> = (|| {
                // Pinned browser ignores headers/proxy; this stage refuses
                // unsupported nonempty values instead of fabricating support.
                for index in [1, 2] {
                    let p = arg(args, index)?;
                    if p != 0 && !memory.string(p, 16 * 1024)?.is_empty() {
                        bail!("WebSocket headers/proxy unsupported");
                    }
                }
                Ok(reqwest::Url::parse(&memory.string(arg(args, 0)?, 8192)?)?)
            })();
            if let Ok(url) = input {
                let ms = arg(args, 3)?;
                let limit = if ms == 0 { LIMIT } else { timeout(ms) };
                match network.connect(url, limit) {
                    Ok(id) => {
                        if status != 0 {
                            memory.write(status, &101u16.to_le_bytes())?;
                        }
                        out[0] = Val::I32(id as i32);
                    }
                    Err(code) => {
                        if status != 0 {
                            memory.write(status, &code.to_le_bytes())?;
                        }
                    }
                }
            }
        }
        "MTY_WebSocketRead" => {
            let output = arg(args, 2)?;
            let size = arg(args, 3)? as usize;
            if size > MAX_MESSAGE + 1 {
                bail!("WebSocket read buffer exceeds limit");
            }
            memory.range(output, size)?;
            let code = match network.socket(arg(args, 0)?) {
                Some(socket) => socket.read(&memory, output, size, timeout(arg(args, 1)?))?,
                None => 3,
            };
            out[0] = Val::I32(code);
        }
        "MTY_WebSocketWrite" => {
            out[0] = Val::I32(0);
            if let Some(socket) = network.socket(arg(args, 0)?) {
                if let Ok(text) = memory.string(arg(args, 1)?, MAX_MESSAGE + 1) {
                    out[0] = Val::I32(i32::from(socket.write(text)));
                }
            }
        }
        "MTY_WebSocketGetCloseCode" => {
            let code = network
                .socket(arg(args, 0)?)
                .map(|s| {
                    s.shared
                        .inbox
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .code
                })
                .unwrap_or(0);
            out[0] = Val::I32(i32::from(code));
        }
        "MTY_WebSocketDestroy" => {
            let output = arg(args, 0)?;
            if output != 0 {
                let id = memory.u32(output)?;
                network.destroy(id);
                memory.set_u32(output, 0)?;
            }
        }
        _ => bail!("Unknown WebSocket import"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inbox_and_origin_limits_fail_without_reusing_data() {
        let inbox = Shared::new();
        assert!(!inbox.push("x".repeat(MAX_MESSAGE + 1)));
        assert!(!inbox.push("a\0b".into()));
        for _ in 0..16 {
            assert!(inbox.push(String::new()));
        }
        assert!(!inbox.push(String::new()));
        inbox.finish(1009, true);
        assert!(inbox.inbox.lock().unwrap().messages.is_empty());
        let url = reqwest::Url::parse("ws://127.0.0.1:1234/path").unwrap();
        assert!(!Network::default().allowed(&url));
        let network = Network::diagnostic(1234, KEEPALIVE);
        assert!(network.allowed(&url));
        for url in [
            "ws://localhost:1234",
            "ws://127.0.0.1:1235",
            "wss://127.0.0.1:1234",
            "ws://u@127.0.0.1:1234",
        ] {
            assert!(!network.allowed(&reqwest::Url::parse(url).unwrap()));
        }
    }
}
