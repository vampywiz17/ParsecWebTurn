//! Opt-in, expiring LAN HTTPS setup. No public listener, external page or telemetry.
use crate::network::Network;
use anyhow::{ensure, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::{Ipv4Addr, Shutdown, TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

const LIMIT: usize = 32768;
const LIFETIME: Duration = Duration::from_secs(300);
struct View {
    url: String,
    fingerprint: String,
    qr: Vec<Vec<bool>>,
    pin: String,
    pending: Option<Network>,
    closed: bool,
}
struct State {
    stop: AtomicBool,
    expires: Instant,
    view: Mutex<View>,
    active: Mutex<Option<TcpStream>>,
}
pub struct Setup {
    state: Arc<State>,
    thread: Option<JoinHandle<()>>,
}
fn random_hex(bytes: usize) -> Result<String> {
    let mut raw = vec![0; bytes];
    rustls::crypto::ring::default_provider()
        .secure_random
        .fill(&mut raw)
        .map_err(|_| anyhow::anyhow!("Secure pairing unavailable"))?;
    Ok(raw.iter().map(|b| format!("{b:02x}")).collect())
}
impl Setup {
    pub fn start(ip: Ipv4Addr) -> Result<Self> {
        ensure!(ip.is_private(), "Connect the TV to a private local network");
        let listener = TcpListener::bind((ip, 0))?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let secret = random_hex(16)?;
        let path = format!("/{secret}");
        let url = format!("https://{address}{path}");
        let cert = rcgen::generate_simple_self_signed(vec![ip.to_string()])?;
        let fingerprint = Sha256::digest(cert.cert.der())
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(":");
        let key = rustls::pki_types::PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der());
        let config = Arc::new(
            rustls::ServerConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()?
            .with_no_client_auth()
            .with_single_cert(vec![cert.cert.der().clone()], key.into())?,
        );
        let code = qrcode::QrCode::new(url.as_bytes())?;
        let qr = code
            .to_colors()
            .chunks(code.width())
            .map(|row| row.iter().map(|c| *c == qrcode::Color::Dark).collect())
            .collect();
        let pin_raw = random_hex(4)?;
        let pin = format!("{:06}", u32::from_str_radix(&pin_raw, 16)? % 1_000_000);
        let state = Arc::new(State {
            stop: AtomicBool::new(false),
            expires: Instant::now() + LIFETIME,
            view: Mutex::new(View {
                url,
                fingerprint,
                qr,
                pin,
                pending: None,
                closed: false,
            }),
            active: Mutex::new(None),
        });
        let shared = state.clone();
        let handle = thread::Builder::new()
            .name("tv-local-setup".into())
            .spawn(move || {
                let started = Instant::now();
                let mut submissions = 0;
                while !shared.stop.load(Ordering::Acquire)
                    && started.elapsed() < LIFETIME
                    && submissions < 8
                {
                    match listener.accept() {
                        Ok((socket, peer)) => {
                            if !matches!(peer.ip(),std::net::IpAddr::V4(ip) if ip.is_private()) {
                                continue;
                            }
                            let _ = socket.set_read_timeout(Some(Duration::from_secs(2)));
                            let _ = socket.set_write_timeout(Some(Duration::from_secs(2)));
                            *shared.active.lock().unwrap_or_else(|e| e.into_inner()) =
                                socket.try_clone().ok();
                            let _ = serve(
                                socket,
                                config.clone(),
                                &shared,
                                &path,
                                &address.to_string(),
                                &mut submissions,
                            );
                            *shared.active.lock().unwrap_or_else(|e| e.into_inner()) = None;
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::park_timeout(Duration::from_millis(100))
                        }
                        Err(_) => break,
                    }
                }
                let mut view = shared.view.lock().unwrap_or_else(|e| e.into_inner());
                view.closed = true;
                view.pending = None;
            })?;
        Ok(Self {
            state,
            thread: Some(handle),
        })
    }
    pub fn snapshot(&self) -> Value {
        let v = self.state.view.lock().unwrap_or_else(|e| e.into_inner());
        json!({"ok":true,"url":v.url,"fingerprint":v.fingerprint,"qr":v.qr,"pin":v.pin,"closed":v.closed,"pending":v.pending.as_ref().map(Network::summary)})
    }
    pub fn pending(&self) -> Option<Network> {
        let v = self.state.view.lock().unwrap_or_else(|e| e.into_inner());
        if v.closed
            || self.state.stop.load(Ordering::Acquire)
            || Instant::now() >= self.state.expires
        {
            None
        } else {
            v.pending.clone()
        }
    }
    pub fn stop(&mut self) {
        self.state.stop.store(true, Ordering::Release);
        if let Some(socket) = self
            .state
            .active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            let _ = socket.shutdown(Shutdown::Both);
        }
        if let Some(handle) = self.thread.take() {
            handle.thread().unpark();
            let _ = handle.join();
        }
        let mut view = self.state.view.lock().unwrap_or_else(|e| e.into_inner());
        view.pending = None;
        view.closed = true;
    }
}
impl Drop for Setup {
    fn drop(&mut self) {
        self.stop();
    }
}
struct Request {
    method: String,
    path: String,
    origin: Option<String>,
    host: String,
    body: Zeroizing<Vec<u8>>,
    content_type: String,
}
fn request(input: &mut impl Read) -> Result<Request> {
    let started = Instant::now();
    let mut raw = Zeroizing::new(Vec::new());
    let (head, length, method, path, origin, host, content_type) = loop {
        ensure!(
            raw.len() < LIMIT && started.elapsed() < Duration::from_secs(5),
            "Request exceeds limits"
        );
        let mut buf = [0; 2048];
        let count = input.read(&mut buf)?;
        ensure!(count > 0, "Incomplete request");
        raw.extend_from_slice(&buf[..count]);
        let mut headers = [httparse::EMPTY_HEADER; 32];
        let mut req = httparse::Request::new(&mut headers);
        let httparse::Status::Complete(head) = req.parse(&raw)? else {
            continue;
        };
        ensure!(
            head <= 8192
                && req
                    .headers
                    .iter()
                    .all(|h| !h.name.eq_ignore_ascii_case("transfer-encoding")),
            "Unsupported request framing"
        );
        let field = |name: &str| -> Result<Option<String>> {
            let values: Vec<_> = req
                .headers
                .iter()
                .filter(|h| h.name.eq_ignore_ascii_case(name))
                .collect();
            ensure!(values.len() <= 1, "Duplicate request header");
            values
                .first()
                .map(|h| {
                    std::str::from_utf8(h.value)
                        .map(str::to_owned)
                        .map_err(Into::into)
                })
                .transpose()
        };
        let length = field("content-length")?.unwrap_or_else(|| "0".into());
        ensure!(
            !length.is_empty() && length.bytes().all(|b| b.is_ascii_digit()),
            "Invalid request length"
        );
        let length: usize = length.parse()?;
        ensure!(head + length <= LIMIT, "Request exceeds limit");
        break (
            head,
            length,
            req.method.unwrap_or("").to_owned(),
            req.path.unwrap_or("").to_owned(),
            field("origin")?,
            field("host")?.unwrap_or_default(),
            field("content-type")?.unwrap_or_default(),
        );
    };
    while raw.len() < head + length {
        ensure!(
            started.elapsed() < Duration::from_secs(5),
            "Request expired"
        );
        let mut buf = [0; 2048];
        let want = (head + length - raw.len()).min(buf.len());
        let n = input.read(&mut buf[..want])?;
        ensure!(n > 0, "Incomplete body");
        raw.extend_from_slice(&buf[..n]);
    }
    Ok(Request {
        method,
        path,
        origin,
        host,
        body: Zeroizing::new(raw[head..head + length].to_vec()),
        content_type,
    })
}
fn fields(body: &[u8]) -> Result<BTreeMap<String, String>> {
    ensure!(body.len() <= LIMIT, "Form exceeds limit");
    let mut values = BTreeMap::new();
    for (key, value) in url::form_urlencoded::parse(body) {
        ensure!(
            matches!(
                key.as_ref(),
                "pin"
                    | "provider"
                    | "stun"
                    | "turn"
                    | "username"
                    | "password"
                    | "key_id"
                    | "token"
                    | "force_relay"
            ),
            "Unknown field"
        );
        ensure!(
            values
                .insert(key.into_owned(), value.into_owned())
                .is_none(),
            "Duplicate field"
        );
    }
    Ok(values)
}
fn form(body: &[u8], pin: &str) -> Result<Network> {
    use subtle::ConstantTimeEq;
    let mut fields = fields(body)?;
    ensure!(
        fields
            .get("pin")
            .is_some_and(|p| bool::from(p.as_bytes().ct_eq(pin.as_bytes()))),
        "Incorrect pairing code"
    );
    let mut take = |name: &str| fields.remove(name).unwrap_or_default();
    let provider = take("provider");
    let urls = |s: String| s.split_whitespace().map(str::to_owned).collect();
    let mut n = Network {
        provider,
        stun_urls: urls(take("stun")),
        turn_urls: urls(take("turn")),
        username: take("username"),
        password: take("password"),
        key_id: take("key_id").trim().to_owned(),
        token: take("token").trim().to_owned(),
        force_relay: take("force_relay") == "on",
    };
    use zeroize::Zeroize;
    match n.provider.as_str() {
        "direct" => {
            n.turn_urls.clear();
            n.password.zeroize();
            n.username.clear();
            n.key_id.clear();
            n.token.zeroize();
        }
        "cloudflare" => {
            n.turn_urls.clear();
            n.username.clear();
            n.password.zeroize();
        }
        "custom" => {
            n.key_id.clear();
            n.token.zeroize();
        }
        _ => {}
    }
    n.validate()?;
    Ok(n)
}
fn respond(stream: &mut impl Write, status: &str, body: &str) -> Result<()> {
    write!(stream,"HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nX-Content-Type-Options: nosniff\r\nX-Frame-Options: DENY\r\nContent-Security-Policy: default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; frame-ancestors 'none'\r\n\r\n{body}",body.len())?;
    stream.flush()?;
    Ok(())
}
fn serve(
    socket: TcpStream,
    config: Arc<rustls::ServerConfig>,
    state: &State,
    path: &str,
    host: &str,
    submissions: &mut u8,
) -> Result<()> {
    let mut tls = rustls::StreamOwned::new(rustls::ServerConnection::new(config)?, socket);
    let result = serve_request(&mut tls, state, path, host, submissions);
    tls.conn.send_close_notify();
    let _ = tls.flush();
    result
}
trait ReadWrite: Read + Write {}
impl<T: Read + Write> ReadWrite for T {}
fn serve_request(
    tls: &mut impl ReadWrite,
    state: &State,
    path: &str,
    host: &str,
    submissions: &mut u8,
) -> Result<()> {
    let req = request(tls)?;
    if state.stop.load(Ordering::Acquire) || Instant::now() >= state.expires || req.host != host {
        return respond(tls, "403 Forbidden", "Setup unavailable");
    }
    if req.method == "GET" && req.path == path {
        return respond(
            tls,
            "200 OK",
            &include_str!("setup.html").replace("__ACTION__", &format!("{path}/apply")),
        );
    }
    if req.method == "POST" && req.path == format!("{path}/apply") {
        *submissions += 1;
        if req.origin.as_deref() != Some(&format!("https://{host}"))
            || req.content_type.split(';').next() != Some("application/x-www-form-urlencoded")
        {
            return respond(tls, "403 Forbidden", "Request origin refused");
        }
        let mut view = state.view.lock().unwrap_or_else(|e| e.into_inner());
        if view.pending.is_some() {
            return respond(tls, "409 Conflict", "Confirm or cancel on the TV first");
        }
        return match form(&req.body,&view.pin) {
            Ok(n)=>{view.pending=Some(n);respond(tls,"200 OK","Settings received. Confirm them on your TV. You can close this page.")}
            Err(_)=>respond(tls,"400 Bad Request","Check the pairing code, provider, URLs and required credentials. No settings were saved."),
        };
    }
    respond(tls, "404 Not Found", "Not found")
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn form_requires_pin_and_refuses_duplicate_or_unsafe_fields() {
        let good = b"pin=123456&provider=direct&stun=stun%3Astun.parsec.gg%3A3478";
        assert!(form(good, "123456").is_ok());
        assert!(form(good, "654321").is_err());
        assert!(form(b"pin=123456&pin=123456&provider=direct", "123456").is_err());
        assert!(form(
            b"pin=123456&provider=custom&turn=http%3A%2F%2Funsafe",
            "123456"
        )
        .is_err());
    }
    #[test]
    fn tls_pairing_requires_tv_approval_and_never_returns_secrets() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let mut roots = rustls::RootCertStore::empty();
        roots.add(cert.cert.der().clone()).unwrap();
        let config = Arc::new(
            rustls::ServerConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![cert.cert.der().clone()],
                rustls::pki_types::PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der())
                    .into(),
            )
            .unwrap(),
        );
        let state = Arc::new(State {
            stop: AtomicBool::new(false),
            expires: Instant::now() + LIFETIME,
            view: Mutex::new(View {
                url: String::new(),
                fingerprint: String::new(),
                qr: vec![],
                pin: "123456".into(),
                pending: None,
                closed: false,
            }),
            active: Mutex::new(None),
        });
        let shared = state.clone();
        let host = address.to_string();
        let handle = thread::spawn(move || {
            let (socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            serve(socket, config, &shared, "/pair", &host, &mut 0).unwrap();
        });
        let client = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
        let connection = rustls::ClientConnection::new(
            Arc::new(client),
            rustls::pki_types::ServerName::try_from("localhost").unwrap(),
        )
        .unwrap();
        let socket = TcpStream::connect(address).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut tls = rustls::StreamOwned::new(connection, socket);
        let body =
            "pin=123456&provider=cloudflare&key_id=fixture-id&token=secret-fixture&force_relay=on";
        write!(tls,"POST /pair/apply HTTP/1.1\r\nHost: {address}\r\nOrigin: https://{address}\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{body}",body.len()).unwrap();
        tls.flush().unwrap();
        let mut response = String::new();
        tls.read_to_string(&mut response).unwrap();
        handle.join().unwrap();
        assert!(response.contains("Confirm them on your TV"));
        assert!(!response.contains("secret-fixture"));
        assert!(
            state
                .view
                .lock()
                .unwrap()
                .pending
                .as_ref()
                .unwrap()
                .force_relay
        );
    }

    #[test]
    fn bounded_http_rejects_smuggling_and_preserves_post_body() {
        let raw =
            b"POST /pair/apply HTTP/1.1\r\nHost: 192.168.1.2:444\r\nContent-Length: 3\r\n\r\na=b";
        let r = request(&mut &raw[..]).unwrap();
        assert_eq!(&**r.body, b"a=b");
        assert!(request(
            &mut &b"POST / HTTP/1.1\r\nHost: a\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\nx"
                [..]
        )
        .is_err());
        assert!(request(
            &mut &b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\n\r\n"[..]
        )
        .is_err());
        assert!(Setup::start(Ipv4Addr::new(8, 8, 8, 8)).is_err());
    }
}
