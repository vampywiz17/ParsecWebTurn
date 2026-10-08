//! Encrypted synthetic credential exchange through the actual guest imports.
//! No Parsec authentication, real credentials, external traffic or trust bypass.
use anyhow::{bail, Context, Result};
use futures_util::{SinkExt, StreamExt};
use std::{net::TcpListener, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::oneshot,
};
use tokio_tungstenite::tungstenite::Message;
use wasmtime::{Config, Engine, Module};

struct Server {
    port: u16,
    root: Vec<u8>,
    stop: Option<oneshot::Sender<()>>,
    worker: Option<std::thread::JoinHandle<Result<usize>>>,
}
impl Server {
    fn start(valid_name: bool) -> Result<Self> {
        let cert = rcgen::generate_simple_self_signed(vec![if valid_name {
            "127.0.0.1"
        } else {
            "fixture.invalid"
        }
        .into()])?;
        let root = cert.cert.der().to_vec();
        let key = rustls::pki_types::PrivatePkcs8KeyDer::from(cert.key_pair.serialize_der());
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(vec![cert.cert.der().clone()], key.into())?;
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        listener.set_nonblocking(true)?;
        let (stop, cancel) = oneshot::channel();
        let worker = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            runtime.block_on(async {
                let listener = tokio::net::TcpListener::from_std(listener)?;
                tokio::select! {
                    _ = cancel => bail!("TLS fixture cancelled"),
                    result = tokio::time::timeout(Duration::from_secs(12), serve(listener,acceptor,valid_name)) => result.context("TLS fixture deadline")?,
                }
            })
        });
        Ok(Self {
            port,
            root,
            stop: Some(stop),
            worker: Some(worker),
        })
    }
    fn finish(&mut self) -> Result<usize> {
        self.worker
            .take()
            .context("TLS fixture already finished")?
            .join()
            .map_err(|_| anyhow::anyhow!("TLS fixture worker panicked"))?
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
async fn serve(
    listener: tokio::net::TcpListener,
    acceptor: tokio_rustls::TlsAcceptor,
    valid_name: bool,
) -> Result<usize> {
    let attempts = if valid_name { 4 } else { 2 };
    let mut rejected = 0;
    for index in 0..attempts {
        let (stream, _) = listener.accept().await?;
        let result = acceptor.accept(stream).await;
        if !valid_name || index >= 2 {
            if result.is_ok() {
                bail!("Untrusted/name-mismatched TLS accepted");
            }
            rejected += 1;
            continue;
        }
        let mut tls = result.context("Trusted fixture TLS handshake")?;
        if index == 0 {
            let mut bytes = Vec::new();
            while !bytes.ends_with(b"\r\n\r\n") {
                if bytes.len() >= 16 * 1024 {
                    bail!("TLS fixture header limit");
                }
                bytes.push(tls.read_u8().await?);
            }
            let header = String::from_utf8(bytes)?;
            if !header.starts_with("POST /auth HTTP/1.1\r\n")
                || !header
                    .to_ascii_lowercase()
                    .contains("authorization: bearer fixture-only\r\n")
            {
                bail!("Synthetic HTTPS request mismatch");
            }
            let length = header
                .lines()
                .find_map(|line| {
                    line.split_once(':')
                        .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                        .map(|(_, value)| value.trim().parse::<usize>())
                })
                .transpose()?
                .context("Fixture body length missing")?;
            if length > 1024 {
                bail!("TLS fixture body limit");
            }
            let mut body = vec![0; length];
            tls.read_exact(&mut body).await?;
            if body != b"fixture-login" {
                bail!("Synthetic HTTPS body mismatch");
            }
            let response = b"{\"token\":\"fixture-session\"}";
            tls.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    response.len()
                )
                .as_bytes(),
            )
            .await?;
            tls.write_all(response).await?;
            tls.shutdown().await?;
        } else {
            let mut ws = tokio_tungstenite::accept_async(tls).await?;
            ws.send(Message::Text("fixture-ready".into())).await?;
            if !matches!(ws.next().await,Some(Ok(Message::Text(text))) if text == "fixture-session")
            {
                bail!("Synthetic WSS credential exchange mismatch");
            }
            ws.send(Message::Text("fixture-accepted".into())).await?;
            if !matches!(ws.next().await, Some(Ok(Message::Close(_)))) {
                bail!("TLS fixture expected clean close");
            }
            ws.flush().await?;
        }
    }
    Ok(rejected)
}

pub fn probe() -> Result<serde_json::Value> {
    let mut good = Server::start(true)?;
    let mut wrong_name = Server::start(false)?;
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
        (import "env" "MTY_HttpRequest" (func $http (param i32 i32 i32 i32 i32 i32 i32 i32 i32 i32) (result i32)))
        (import "env" "MTY_WebSocketConnect" (func $connect (param i32 i32 i32 i32 i32) (result i32)))
        (import "env" "MTY_WebSocketRead" (func $read (param i32 i32 i32 i32) (result i32)))
        (import "env" "MTY_WebSocketWrite" (func $write (param i32 i32) (result i32)))
        (import "env" "MTY_WebSocketDestroy" (func $destroy (param i32)))
        (func (export "mty_system_alloc") (param i32 i32) (result i32) i32.const 8192)
        (func (export "mty_system_free") (param i32))
        (func (export "http") (result i32)
            i32.const 16 i32.const 1024 i32.const 1100 i32.const 2048 i32.const 13 i32.const 0 i32.const 2000 i32.const 400 i32.const 404 i32.const 408 call $http)
        (func (export "connect") (result i32)
            i32.const 16 i32.const 0 i32.const 0 i32.const 2000 i32.const 408 call $connect)
        (func (export "read") (param i32) (result i32)
            local.get 0 i32.const 2000 i32.const 4096 i32.const 1024 call $read)
        (func (export "write") (param i32) (result i32)
            local.get 0 i32.const 2048 call $write)
        (func (export "destroy") i32.const 400 call $destroy))"#,
    )?;
    let (mut store, instance) = crate::instantiate(&engine, &module)?;
    let memory = store.data().memory.clone();
    let http = instance.get_typed_func::<(), i32>(&mut store, "http")?;
    let connect = instance.get_typed_func::<(), i32>(&mut store, "connect")?;
    let read = instance.get_typed_func::<i32, i32>(&mut store, "read")?;
    let write = instance.get_typed_func::<i32, i32>(&mut store, "write")?;
    let destroy = instance.get_typed_func::<(), ()>(&mut store, "destroy")?;
    memory.c_string(1024, 64, "POST")?;
    memory.c_string(1100, 128, "Authorization: Bearer fixture-only\r\n")?;
    memory.c_string(2048, 64, "fixture-login")?;
    memory.c_string(16, 1000, &format!("https://127.0.0.1:{}/auth", good.port))?;
    if http.call(&mut store, ())? != 0 {
        bail!("Offline HTTPS unexpectedly enabled");
    }
    store.data_mut().http = Arc::new(crate::http::Network::diagnostic_tls(
        good.port,
        Some(good.root.clone()),
    )?);
    if http.call(&mut store, ())? != 1
        || memory.read(408, 2)? != 200u16.to_le_bytes()
        || memory.string(8192, 64)? != "{\"token\":\"fixture-session\"}"
    {
        bail!("Guest HTTPS exchange failed");
    }
    memory.c_string(16, 1000, &format!("wss://127.0.0.1:{}/signal", good.port))?;
    if connect.call(&mut store, ())? != 0 {
        bail!("Offline WSS unexpectedly enabled");
    }
    store.data_mut().websocket = Arc::new(crate::websocket::Network::diagnostic_tls(
        good.port,
        Some(good.root.clone()),
    )?);
    let id = connect.call(&mut store, ())?;
    if id <= 0
        || memory.read(408, 2)? != 101u16.to_le_bytes()
        || read.call(&mut store, id)? != 0
        || memory.string(4096, 1024)? != "fixture-ready"
    {
        bail!("Guest WSS upgrade/read failed");
    }
    memory.c_string(2048, 64, "fixture-session")?;
    if write.call(&mut store, id)? != 1
        || read.call(&mut store, id)? != 0
        || memory.string(4096, 1024)? != "fixture-accepted"
    {
        bail!("Guest WSS credential exchange failed");
    }
    memory.set_u32(400, id as u32)?;
    destroy.call(&mut store, ())?;
    if memory.u32(400)? != 0 || store.data().websocket.active_handles() != 0 {
        bail!("TLS socket cleanup failed");
    }
    // No CA at all in the first case; trusted certificate with the wrong SAN
    // in the second. Neither failure may be retried with insecure verification.
    for (server, root) in [(&good, None), (&wrong_name, Some(wrong_name.root.clone()))] {
        memory.c_string(16, 1000, &format!("https://127.0.0.1:{}/auth", server.port))?;
        store.data_mut().http = Arc::new(crate::http::Network::diagnostic_tls(
            server.port,
            root.clone(),
        )?);
        if http.call(&mut store, ())? != 0 || memory.read(400, 10)? != vec![0; 10] {
            bail!("Invalid HTTPS certificate accepted/leaked output");
        }
        memory.c_string(16, 1000, &format!("wss://127.0.0.1:{}/signal", server.port))?;
        store.data_mut().websocket = Arc::new(crate::websocket::Network::diagnostic_tls(
            server.port,
            root,
        )?);
        if connect.call(&mut store, ())? != 0
            || memory.read(408, 2)? != vec![0; 2]
            || store.data().websocket.active_handles() != 0
        {
            bail!("Invalid WSS certificate accepted/leaked handle");
        }
    }
    if good.finish()? != 2 || wrong_name.finish()? != 2 {
        bail!("TLS rejection proof incomplete");
    }
    Ok(serde_json::json!({
        "schema":1,"scope":"controlled-wasm-native-loopback-https-wss",
        "tls_connections_verified":6,"certificate_rejections_verified":4,
        "https_import_verified":true,"wss_import_verified":true,
        "synthetic_credential_exchange_verified":true,"untrusted_certificate_rejected_verified":true,
        "hostname_mismatch_rejected_verified":true,"offline_policy_verified":true,
        "guest_failure_outputs_verified":true,"destroy_cleanup_verified":true,"servers_closed":true,
        "certificate_verification_disabled":false,"external_requests_enabled":false,
        "authentication_integrated":false,"original_parsec_guest_auth_exercised":false,
        "parsec_host_connected":false,"video_decoded":false
    }))
}

#[cfg(test)]
mod tests {
    #[test]
    fn guest_tls_checks_trust_hostname_exchange_and_cleanup() {
        let report = super::probe().unwrap();
        assert_eq!(report["certificate_rejections_verified"], 4);
        assert_eq!(report["servers_closed"], true);
        assert_eq!(report["authentication_integrated"], false);
    }
}
