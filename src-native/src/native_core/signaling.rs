//! lowlat examples/client/signaling.cc account URL, using our existing Rust TLS stack.
//! The BUD adapter still owns offers, candidates, retries and media credentials.
use super::account::Session;
use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use std::{sync::mpsc, time::Duration};
use tokio_tungstenite::tungstenite::{protocol::WebSocketConfig, Message};

fn connect_url(token: &str) -> Result<String> {
    let mut url = reqwest::Url::parse("wss://kessel-ws.parsec.app/")?;
    // The WebSocket HTTP request requires the root path. Removing it
    // produces `GET ?session_id=...`, an invalid target.
    // See lowlat crates/kessel/src/url.rs at e548d48.
    url.query_pairs_mut().extend_pairs([
        ("session_id", token),
        ("role", "client"),
        ("version", "1"),
        ("build", "EvoParseq"),
        ("sdk_version", "0"),
    ]);
    Ok(url.into())
}

#[derive(Clone, Copy, Debug)]
struct UpgradeFailure {
    stage: &'static str,
    status: Option<u16>,
}
impl UpgradeFailure {
    fn at(stage: &'static str) -> Self {
        Self {
            stage,
            status: None,
        }
    }
    fn from_error(error: &tokio_tungstenite::tungstenite::Error) -> Self {
        use tokio_tungstenite::tungstenite::Error;
        // Never retain/format raw errors, response bodies, headers or URLs:
        // each can include the credential-bearing request.
        match error {
            Error::Http(response) => Self {
                stage: "http_upgrade",
                status: Some(response.status().as_u16()),
            },
            Error::Io(_) => Self::at("network_or_tls"),
            Error::Tls(_) => Self::at("tls"),
            Error::Url(_) | Error::HttpFormat(_) => Self::at("request_format"),
            _ => Self::at("websocket_handshake"),
        }
    }
}
impl std::fmt::Display for UpgradeFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.stage)?;
        if let Some(status) = self.status {
            write!(f, "; HTTP {status}")?;
        }
        Ok(())
    }
}

pub struct Socket {
    pub writer: crate::bud_attempt::SignalingWrite,
    pub incoming: mpsc::Receiver<String>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Socket {
    pub fn open(session: &Session) -> Result<Self> {
        let url = connect_url(session.token())?;
        let (out, outgoing) = tokio::sync::mpsc::channel::<String>(64);
        let (incoming_tx, incoming) = mpsc::sync_channel(64);
        let (stop, stopping) = tokio::sync::oneshot::channel();
        let (ready_tx, ready) = mpsc::sync_channel(1);
        let consumer = std::thread::current();
        let worker = std::thread::Builder::new()
            .name("native-account-signaling".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(_) => {
                        let _ = ready_tx.send(Err(UpgradeFailure::at("runtime")));
                        return;
                    }
                };
                runtime.block_on(relay(
                    url,
                    outgoing,
                    stopping,
                    ready_tx,
                    incoming_tx,
                    consumer,
                ));
            })?;
        // Handshake has its own deadline; never expose an error containing the URL/token.
        let opened = ready
            .recv_timeout(Duration::from_secs(15))
            .unwrap_or_else(|error| {
                Err(UpgradeFailure::at(match error {
                    mpsc::RecvTimeoutError::Timeout => "worker_timeout",
                    mpsc::RecvTimeoutError::Disconnected => "worker_stopped",
                }))
            });
        if let Err(failure) = opened {
            let _ = stop.send(());
            let _ = worker.join();
            crate::event_log::record(
                "ERROR",
                "native_signaling_upgrade_failed",
                serde_json::json!({"stage":failure.stage,"http_status":failure.status}),
            );
            anyhow::bail!("Could not open authenticated Parsec signaling ({failure}).");
        }
        Ok(Self {
            writer: std::sync::Arc::new(move |text| out.try_send(text).is_ok()),
            incoming,
            stop: Some(stop),
            worker: Some(worker),
        })
    }
    pub fn next(&self) -> Result<Option<String>> {
        match self.incoming.try_recv() {
            Ok(text) => Ok(Some(text)),
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => {
                Err(anyhow::anyhow!("Account signaling closed"))
            }
        }
    }
}

// This worker only carries account signaling, never video/audio packets.
type AccountSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn send(socket: &mut AccountSocket, message: Message) -> bool {
    matches!(
        tokio::time::timeout(Duration::from_secs(5), socket.send(message)).await,
        Ok(Ok(()))
    )
}

async fn relay(
    url: String,
    mut outgoing: tokio::sync::mpsc::Receiver<String>,
    mut stopping: tokio::sync::oneshot::Receiver<()>,
    ready: mpsc::SyncSender<Result<(), UpgradeFailure>>,
    incoming: mpsc::SyncSender<String>,
    consumer: std::thread::Thread,
) {
    let config = WebSocketConfig::default()
        .max_message_size(Some(16384))
        .max_frame_size(Some(16384));
    let connected = tokio::time::timeout(
        Duration::from_secs(12),
        tokio_tungstenite::connect_async_with_config(url.as_str(), Some(config), false),
    )
    .await;
    let mut socket = match connected {
        Ok(Ok((socket, _))) => socket,
        Ok(Err(error)) => {
            let _ = ready.send(Err(UpgradeFailure::from_error(&error)));
            return;
        }
        Err(_) => {
            let _ = ready.send(Err(UpgradeFailure::at("handshake_timeout")));
            return;
        }
    };
    if ready.send(Ok(())).is_err() {
        return;
    }
    loop {
        tokio::select! {
            _ = &mut stopping => {
                let _ = tokio::time::timeout(Duration::from_secs(1), socket.close(None)).await;
                break;
            }
            next = outgoing.recv() => {
                let Some(text) = next else { break; };
                if !send(&mut socket, Message::Text(text.into())).await {
                    break;
                }
            }
            next = socket.next() => match next {
                Some(Ok(Message::Text(text))) => {
                    if incoming.try_send(text.to_string()).is_err() {
                        break;
                    }
                    consumer.unpark();
                }
                Some(Ok(Message::Ping(bytes))) => {
                    if !send(&mut socket, Message::Pong(bytes)).await {
                        break;
                    }
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                _ => {}
            }
        }
    }
    consumer.unpark();
}

impl Drop for Socket {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_tungstenite::tungstenite::{
        client::IntoClientRequest, handshake::client::generate_request,
    };

    #[test]
    fn account_relay_preserves_text_ping_and_remote_close() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("ws://{}/", listener.local_addr().unwrap());
            let (out, outgoing) = tokio::sync::mpsc::channel(64);
            let (_stop, stopping) = tokio::sync::oneshot::channel();
            let (ready, opened) = mpsc::sync_channel(1);
            let (incoming, received) = mpsc::sync_channel(64);
            out.try_send("client-message".to_owned()).unwrap();
            let server = async {
                let (tcp, _) = listener.accept().await.unwrap();
                let mut socket = tokio_tungstenite::accept_async(tcp).await.unwrap();
                assert_eq!(
                    socket.next().await.unwrap().unwrap(),
                    Message::Text("client-message".into())
                );
                socket
                    .send(Message::Ping(b"ping".as_slice().into()))
                    .await
                    .unwrap();
                assert_eq!(
                    socket.next().await.unwrap().unwrap(),
                    Message::Pong(b"ping".as_slice().into())
                );
                socket
                    .send(Message::Text("host-message".into()))
                    .await
                    .unwrap();
                socket.close(None).await.unwrap();
            };
            tokio::time::timeout(Duration::from_secs(3), async {
                tokio::join!(
                    server,
                    relay(
                        url,
                        outgoing,
                        stopping,
                        ready,
                        incoming,
                        std::thread::current()
                    )
                );
            })
            .await
            .unwrap();
            assert!(opened.try_recv().unwrap().is_ok());
            assert_eq!(received.try_recv().unwrap(), "host-message");
            assert!(matches!(
                received.try_recv(),
                Err(mpsc::TryRecvError::Disconnected)
            ));
        });
    }

    #[test]
    fn upgrade_request_has_root_target_and_an_unmodified_host_header() {
        let url = connect_url("a b&c=d+/").unwrap();
        let parsed = reqwest::Url::parse(&url).unwrap();
        let fields: std::collections::HashMap<_, _> = parsed.query_pairs().into_owned().collect();
        assert_eq!(fields["session_id"], "a b&c=d+/");
        assert_eq!(fields["role"], "client");
        assert_eq!(fields["version"], "1");
        let request = url.into_client_request().unwrap();
        assert_eq!(request.headers()["host"], "kessel-ws.parsec.app");
        let (wire, _) = generate_request(request).unwrap();
        let wire = String::from_utf8(wire).unwrap();
        assert!(wire.starts_with("GET /?session_id="));
        assert!(!wire.starts_with("GET ?"));
    }

    #[test]
    fn handshake_diagnostics_preserve_status_without_server_secrets() {
        use tokio_tungstenite::tungstenite::{http::Response, Error};
        let error = Error::Http(Box::new(
            Response::builder()
                .status(400)
                .header("location", "https://example.invalid/?token=secret-sentinel")
                .body(Some(b"secret-sentinel".to_vec()))
                .unwrap(),
        ));
        let failure = UpgradeFailure::from_error(&error);
        assert_eq!(failure.status, Some(400));
        assert_eq!(failure.to_string(), "http_upgrade; HTTP 400");
        assert!(!format!("{failure:?}").contains("secret-sentinel"));
        let io = Error::Io(std::io::Error::other("secret-sentinel"));
        assert_eq!(
            UpgradeFailure::from_error(&io).to_string(),
            "network_or_tls"
        );
    }
}
