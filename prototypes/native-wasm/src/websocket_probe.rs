//! Controlled guest and loopback server, not real Parsec signaling/login.
use anyhow::{bail, Context, Result};
use futures_util::{SinkExt, StreamExt};
use std::{net::TcpListener, sync::Arc, time::Duration};
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::{
    handshake::server::ErrorResponse,
    protocol::{frame::coding::CloseCode, CloseFrame},
    Message,
};
use wasmtime::{Config, Engine, Module};

struct Server {
    port: u16,
    stop: Option<oneshot::Sender<()>>,
    worker: Option<std::thread::JoinHandle<Result<usize>>>,
}
impl Server {
    fn start() -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        listener.set_nonblocking(true)?;
        let (stop, cancellation) = oneshot::channel();
        let worker = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            runtime.block_on(async {
                let listener=tokio::net::TcpListener::from_std(listener)?;
                tokio::select! {
                    biased;
                    _ = cancellation => bail!("WebSocket fixture cancelled before completion"),
                    result = tokio::time::timeout(Duration::from_secs(8),serve(listener)) => result.context("WebSocket fixture deadline")?,
                }
            })
        });
        Ok(Self {
            port,
            stop: Some(stop),
            worker: Some(worker),
        })
    }
    fn finish(&mut self) -> Result<usize> {
        self.worker
            .take()
            .context("Fixture already finished")?
            .join()
            .map_err(|_| anyhow::anyhow!("WebSocket fixture panicked"))?
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
async fn serve(listener: tokio::net::TcpListener) -> Result<usize> {
    let (stream, _) = listener.accept().await?;
    let mut ws = tokio_tungstenite::accept_async(stream).await?;
    ws.send(Message::Ping(b"fixture-ping".as_slice().into()))
        .await?;
    ws.send(Message::Text("árvíz ✓".into())).await?;
    ws.send(Message::Text("".into())).await?;
    let (mut pong, mut keepalive, mut text) = (false, false, false);
    while !(pong && keepalive && text) {
        match ws.next().await.context("Fixture client disconnected")?? {
            Message::Pong(bytes) if bytes.as_ref() == b"fixture-ping" => pong = true,
            Message::Text(value) if value == "__ping__" => keepalive = true,
            Message::Text(value) if value == "hello ✓" => text = true,
            _ => bail!("Unexpected fixture client message"),
        }
    }
    ws.send(Message::Text("bye".into())).await?;
    ws.close(Some(CloseFrame {
        code: CloseCode::Normal,
        reason: "fixture done".into(),
    }))
    .await?;
    let mut closed = false;
    while let Some(message) = ws.next().await {
        match message? {
            Message::Close(Some(frame)) if frame.code == CloseCode::Normal => {
                closed = true;
                break;
            }
            Message::Text(value) if value == "__ping__" => {}
            _ => bail!("Unexpected closing message"),
        }
    }
    if !closed {
        bail!("Fixture close not acknowledged");
    }

    let (stream, _) = listener.accept().await?;
    let mut ws = tokio_tungstenite::accept_async(stream).await?;
    loop {
        match ws
            .next()
            .await
            .context("Destroy fixture disconnected without close")??
        {
            Message::Close(_) => {
                ws.flush().await?;
                break;
            }
            Message::Text(value) if value == "__ping__" => {}
            _ => bail!("Unexpected destroy fixture message"),
        }
    }

    let (stream, _) = listener.accept().await?;
    let mut ws = tokio_tungstenite::accept_async(stream).await?;
    let _ = ws
        .send(Message::Text(
            "x".repeat(crate::websocket::MAX_MESSAGE + 1).into(),
        ))
        .await;
    while let Some(message) = ws.next().await {
        if message.is_err() || matches!(message, Ok(Message::Close(_))) {
            break;
        }
    }

    let (stream, _) = listener.accept().await?;
    let rejected = tokio_tungstenite::accept_hdr_async(stream, |_, _| {
        Err(ErrorResponse::builder()
            .status(403)
            .body(Some("fixture rejection".into()))
            .unwrap())
    })
    .await;
    if rejected.is_ok() {
        bail!("Rejected handshake succeeded");
    }
    Ok(4)
}

pub fn probe() -> Result<serde_json::Value> {
    let mut server = Server::start()?;
    let mut config = Config::new();
    config
        .wasm_threads(true)
        .consume_fuel(true)
        .epoch_interruption(true);
    let engine = Engine::new(&config)?;
    let module = Module::new(
        &engine,
        r#"(module
        (import "env" "memory" (memory 2 2 shared))
        (import "env" "MTY_WebSocketConnect" (func $connect (param i32 i32 i32 i32 i32) (result i32)))
        (import "env" "MTY_WebSocketRead" (func $read (param i32 i32 i32 i32) (result i32)))
        (import "env" "MTY_WebSocketWrite" (func $write (param i32 i32) (result i32)))
        (import "env" "MTY_WebSocketGetCloseCode" (func $code (param i32) (result i32)))
        (import "env" "MTY_WebSocketDestroy" (func $destroy (param i32)))
        (func (export "connect") (result i32) (local $id i32)
            i32.const 16 i32.const 0 i32.const 0 i32.const 1000 i32.const 404 call $connect local.set $id
            i32.const 400 local.get $id i32.store local.get $id)
        (func (export "read") (param $id i32) (param $timeout i32) (param $out i32) (param $size i32) (result i32)
            local.get $id local.get $timeout local.get $out local.get $size call $read)
        (func (export "write") (param i32) (result i32) local.get 0 i32.const 1024 call $write)
        (func (export "code") (param i32) (result i32) local.get 0 call $code)
        (func (export "destroy") i32.const 400 call $destroy))"#,
    )?;
    let (mut store, instance) = crate::instantiate(&engine, &module)?;
    let memory = store.data().memory.clone();
    let connect = instance.get_typed_func::<(), i32>(&mut store, "connect")?;
    let read = instance.get_typed_func::<(i32, i32, i32, i32), i32>(&mut store, "read")?;
    let write = instance.get_typed_func::<i32, i32>(&mut store, "write")?;
    let code = instance.get_typed_func::<i32, i32>(&mut store, "code")?;
    let destroy = instance.get_typed_func::<(), ()>(&mut store, "destroy")?;
    memory.c_string(16, 300, &format!("ws://127.0.0.1:{}/fixture", server.port))?;
    if connect.call(&mut store, ())? != 0 {
        bail!("Offline WebSocket policy failed");
    }
    let network = Arc::new(crate::websocket::Network::diagnostic(
        server.port,
        Duration::from_millis(50),
    ));
    store.data_mut().websocket = network.clone();
    let id = connect.call(&mut store, ())?;
    if id <= 0 || memory.read(404, 2)? != 101u16.to_le_bytes() {
        bail!("WebSocket upgrade failed");
    }
    if read.call(&mut store, (id, 1000, -1, 128)).is_ok() {
        bail!("Invalid WebSocket output accepted");
    }
    if read.call(&mut store, (id, 1000, 2048, 2))? != 3 {
        bail!("Small WebSocket buffer accepted");
    }
    if read.call(&mut store, (id, 1000, 2048, 128))? != 0 || memory.string(2048, 128)? != "árvíz ✓"
    {
        bail!("Unicode WebSocket retry failed");
    }
    if read.call(&mut store, (id, 1000, 2048, 128))? != 0 || memory.string(2048, 128)? != "" {
        bail!("Empty text frame lost");
    }
    memory.c_string(2048, 128, "sentinel")?;
    if read.call(&mut store, (id, 10, 2048, 128))? != 2 || memory.string(2048, 128)? != "sentinel" {
        bail!("Empty queue timeout failed");
    }
    memory.c_string(1024, 128, "hello ✓")?;
    if write.call(&mut store, id)? != 1 {
        bail!("WebSocket write failed");
    }
    if read.call(&mut store, (id, 1000, 2048, 128))? != 0 || memory.string(2048, 128)? != "bye" {
        bail!("Final queued message lost");
    }
    if read.call(&mut store, (id, 1000, 2048, 128))? != 1 || code.call(&mut store, id)? != 1000 {
        bail!("Remote close status failed");
    }
    destroy.call(&mut store, ())?;
    if memory.u32(400)? != 0
        || read.call(&mut store, (id, 0, 2048, 128))? != 3
        || write.call(&mut store, id)? != 0
    {
        bail!("Destroyed handle stayed valid");
    }
    destroy.call(&mut store, ())?;

    let next = connect.call(&mut store, ())?;
    if next <= id {
        bail!("WebSocket handle recycled");
    }
    destroy.call(&mut store, ())?;
    if network.active_handles() != 0 {
        bail!("Destroyed socket leaked");
    }

    let oversized = connect.call(&mut store, ())?;
    if oversized <= next
        || read.call(&mut store, (oversized, 1000, 2048, 128))? != 3
        || code.call(&mut store, oversized)? != 1009
    {
        bail!("WebSocket message limit not enforced");
    }
    destroy.call(&mut store, ())?;
    if connect.call(&mut store, ())? != 0 || memory.read(404, 2)? != 403u16.to_le_bytes() {
        bail!("Upgrade failure status lost");
    }
    if network.active_handles() != 0 {
        bail!("Failed connect leaked handle");
    }
    let connections = server.finish()?;
    Ok(serde_json::json!({
        "schema":1,"scope":"controlled-wasm-native-loopback-websocket",
        "fixture_connections_verified":connections,"server_closed":true,
        "unicode_roundtrip_verified":true,"empty_text_verified":true,
        "rfc_ping_pong_verified":true,"application_keepalive_verified":true,
        "read_timeout_verified":true,"buffer_retry_verified":true,
        "queued_before_close_verified":true,"remote_close_code_verified":true,
        "destroy_cleanup_verified":true,"stale_handles_verified":true,
        "message_limit_verified":true,"upgrade_failure_verified":true,
        "offline_policy_verified":true,
        "original_parsec_guest_signaling_exercised":false,
        "authentication_integrated":false,"external_requests_enabled":false,
        "parsec_host_connected":false,"video_decoded":false
    }))
}

#[cfg(test)]
mod tests {
    #[test]
    fn native_websocket_imports_exchange_text_and_close_without_real_login() {
        let report = super::probe().unwrap();
        assert_eq!(report["fixture_connections_verified"], 4);
        assert_eq!(report["destroy_cleanup_verified"], true);
        assert_eq!(report["application_keepalive_verified"], true);
        assert_eq!(report["authentication_integrated"], false);
    }
}
