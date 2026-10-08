//! Pinned MTY_HttpRequest ABI. Offline by default; exact origins in account mode.
use anyhow::{bail, Context, Result};
use reqwest::{
    blocking::Client,
    header::{HeaderMap, HeaderName, HeaderValue},
    Method, Url,
};
use std::{
    io::Read,
    ops::Range,
    sync::{Mutex, OnceLock},
    time::Duration,
};
use wasmtime::{Caller, Val};

pub const MAX_BODY: usize = 1024 * 1024;
const MAX_HEADERS: usize = 16 * 1024;
const MAX_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Default)]
pub struct Network {
    closed: std::sync::atomic::AtomicBool,
    pub audit: std::sync::Arc<crate::network_audit::Audit>,
    policy: crate::network_policy::Policy,
    fixture_root: Option<Vec<u8>>,
    client: OnceLock<Client>,
    active: Mutex<usize>,
}

struct Request {
    url: Url,
    method: Method,
    headers: HeaderMap,
    body: Option<Vec<u8>>,
    timeout: Duration,
}

struct Active<'a>(&'a Mutex<usize>);
impl Drop for Active<'_> {
    fn drop(&mut self) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) -= 1;
    }
}

impl Network {
    pub fn account(audit: std::sync::Arc<crate::network_audit::Audit>) -> Self {
        Self {
            audit,
            policy: crate::network_policy::Policy::account(),
            ..Default::default()
        }
    }
    pub fn shutdown(&self) {
        self.closed
            .store(true, std::sync::atomic::Ordering::Release);
    }
    pub fn offline(audit: std::sync::Arc<crate::network_audit::Audit>) -> Self {
        Self {
            audit,
            ..Default::default()
        }
    }
    pub fn diagnostic(port: u16) -> Self {
        Self {
            policy: crate::network_policy::Policy::loopback(port),
            ..Default::default()
        }
    }

    fn allowed(&self, url: &Url) -> bool {
        !self.closed.load(std::sync::atomic::Ordering::Acquire)
            && self.policy.allows(url)
            && matches!(url.scheme(), "http" | "https")
    }

    pub fn diagnostic_tls(port: u16, root: Option<Vec<u8>>) -> Result<Self> {
        Ok(Self {
            policy: crate::network_policy::Policy::secure(&[&format!("https://127.0.0.1:{port}")])?,
            fixture_root: root,
            ..Default::default()
        })
    }

    fn execute(&self, request: Request) -> Result<(u16, Vec<u8>)> {
        self.audit.record(
            &request.url,
            request.method.as_str(),
            request.headers.contains_key(reqwest::header::AUTHORIZATION),
            request.body.as_ref().map_or(0, Vec::len),
            self.allowed(&request.url),
        );
        if !self.allowed(&request.url) {
            bail!("HTTP destination not enabled");
        }
        {
            let mut active = self.active.lock().unwrap_or_else(|e| e.into_inner());
            if self.closed.load(std::sync::atomic::Ordering::Acquire) {
                bail!("HTTP service stopped");
            }
            if *active >= 8 {
                bail!("HTTP concurrency limit");
            }
            *active += 1;
        }
        let _active = Active(&self.active);
        // No URL/header/body/error is logged. No proxy, cookie jar or redirect:
        // even a loopback redirect must never reach an unintended destination.
        if self.client.get().is_none() {
            let mut builder = Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(MAX_TIMEOUT)
                .timeout(MAX_TIMEOUT)
                .pool_max_idle_per_host(2);
            if let Some(root) = &self.fixture_root {
                // Private trust is scoped to this loopback diagnostic client.
                builder = builder
                    .tls_built_in_root_certs(false)
                    .add_root_certificate(reqwest::Certificate::from_der(root)?);
            }
            let client = builder.build()?;
            let _ = self.client.set(client);
        }
        let client = self.client.get().context("HTTP client unavailable")?;
        let mut builder = client
            .request(request.method, request.url)
            .headers(request.headers)
            .timeout(request.timeout);
        if let Some(body) = request.body {
            builder = builder.body(body);
        }
        let response = builder.send()?;
        let status = response.status().as_u16();
        if response
            .content_length()
            .is_some_and(|n| n > MAX_BODY as u64)
        {
            bail!("HTTP response too large");
        }
        let mut body = Vec::new();
        response
            .take((MAX_BODY + 1) as u64)
            .read_to_end(&mut body)?;
        if body.len() > MAX_BODY {
            bail!("HTTP response too large");
        }
        Ok((status, body)) // HTTP 4xx/5xx are responses, not transport failures.
    }
}

fn headers(text: &str) -> Result<HeaderMap> {
    if text.len() > MAX_HEADERS {
        bail!("HTTP headers too large");
    }
    let mut headers = HeaderMap::new();
    for (index, line) in text.lines().filter(|line| !line.is_empty()).enumerate() {
        if index >= 64 {
            bail!("Too many HTTP headers");
        }
        let (name, value) = line.split_once(':').context("Malformed HTTP header")?;
        let name = HeaderName::from_bytes(name.trim().as_bytes())?;
        if matches!(
            name.as_str(),
            "host" | "content-length" | "transfer-encoding" | "connection"
        ) {
            bail!("HTTP framing headers are owned by the transport");
        }
        let mut value = HeaderValue::from_str(value.trim())?;
        value.set_sensitive(true);
        headers.append(name, value);
    }
    Ok(headers)
}

fn arg(args: &[Val], index: usize) -> Result<u32> {
    Ok(args
        .get(index)
        .and_then(Val::i32)
        .context("HTTP ABI requires i32")? as u32)
}
fn overlaps(a: &Range<usize>, b: &Range<usize>) -> bool {
    a.start < b.end && b.start < a.end
}

pub fn dispatch(
    caller: &mut Caller<'_, crate::host::HostState>,
    args: &[Val],
    out: &mut [Val],
) -> Result<()> {
    let memory = caller.data().memory.clone();
    let outputs = [(arg(args, 7)?, 4), (arg(args, 8)?, 4), (arg(args, 9)?, 2)];
    let ranges = outputs
        .iter()
        .map(|(p, n)| memory.range(*p, *n))
        .collect::<Result<Vec<_>>>()?;
    if ranges
        .iter()
        .enumerate()
        .any(|(i, a)| ranges.iter().skip(i + 1).any(|b| overlaps(a, b)))
    {
        bail!("Overlapping HTTP output pointers");
    }
    for (p, n) in outputs {
        memory.write(p, &vec![0; n])?;
    }
    out[0] = Val::I32(0);
    caller.data_mut().http_failure = None;
    // Match the pinned worker's boolean failure result, with deterministic zero
    // outputs. Invalid output pointers trap before any request or output write.
    let request: Result<Request> = (|| {
        let url = Url::parse(&memory.string(arg(args, 0)?, 8192)?)?;
        let method = Method::from_bytes(memory.string(arg(args, 1)?, 64)?.as_bytes())?;
        let header_ptr = arg(args, 2)?;
        let headers = headers(&if header_ptr == 0 {
            String::new()
        } else {
            memory.string(header_ptr, MAX_HEADERS + 1)?
        })?;
        let body_len = arg(args, 4)? as usize;
        if body_len > MAX_BODY {
            bail!("HTTP request too large");
        }
        let body_ptr = arg(args, 3)?;
        if body_ptr == 0 && body_len != 0 {
            bail!("Null HTTP body");
        }
        let body = if body_ptr == 0 {
            None
        } else {
            Some(memory.read(body_ptr, body_len)?)
        };
        let proxy = arg(args, 5)?;
        if proxy != 0 && !memory.string(proxy, 8192)?.is_empty() {
            bail!("Explicit proxy unsupported");
        }
        let timeout = arg(args, 6)?;
        let timeout = if timeout == 0 {
            MAX_TIMEOUT
        } else {
            Duration::from_millis(u64::from(timeout)).min(MAX_TIMEOUT)
        };
        Ok(Request {
            url,
            method,
            headers,
            body,
            timeout,
        })
    })();
    let request = match request {
        Ok(request) => request,
        Err(_) => {
            caller.data_mut().http_failure = Some("request-validation");
            return Ok(());
        }
    };
    let network = caller.data().http.clone();
    let (status, body) = match network.execute(request) {
        Ok(response) => response,
        Err(error) => {
            caller.data_mut().http_failure = Some(failure_kind(&error));
            return Ok(());
        }
    };
    let mut pointer = 0;
    if !body.is_empty() {
        let alloc = caller
            .get_export("mty_system_alloc")
            .and_then(|e| e.into_func())
            .context("HTTP guest allocator missing")?
            .typed::<(i32, i32), i32>(&*caller)?;
        let free = caller
            .get_export("mty_system_free")
            .and_then(|e| e.into_func())
            .context("HTTP guest deallocator missing")?
            .typed::<i32, ()>(&*caller)?;
        pointer = alloc.call(&mut *caller, ((body.len() + 1) as i32, 1))? as u32;
        if pointer == 0 {
            return Ok(());
        }
        let copy: Result<()> = (|| {
            let range = memory.range(pointer, body.len() + 1)?;
            if ranges.iter().any(|output| overlaps(output, &range)) {
                bail!("HTTP allocation overlaps outputs");
            }
            memory.write(pointer, &body)?;
            memory.write(pointer + body.len() as u32, &[0])
        })();
        if copy.is_err() {
            free.call(&mut *caller, pointer as i32)?;
            return Ok(());
        }
    }
    memory.set_u32(outputs[0].0, pointer)?;
    memory.set_u32(outputs[1].0, body.len() as u32)?;
    memory.write(outputs[2].0, &status.to_le_bytes())?;
    out[0] = Val::I32(1);
    Ok(()) // Allocated response ownership belongs to the guest.
}

fn failure_kind(error: &anyhow::Error) -> &'static str {
    if let Some(error) = error.downcast_ref::<reqwest::Error>() {
        if error.is_timeout() {
            "timeout"
        } else if error.is_connect() {
            "connect"
        } else if error.is_body() {
            "body"
        } else if error.is_request() {
            "request-transport"
        } else {
            "http-transport"
        }
    } else if error.downcast_ref::<std::io::Error>().is_some() {
        "body-io"
    } else {
        "policy-or-limit"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn account_shutdown_rejects_requests_before_creating_a_client() {
        let network = Network::account(Default::default());
        let url = Url::parse("https://kessel-api.parsec.app/v2/auth").unwrap();
        assert!(network.allowed(&url));
        assert!(
            !network.allowed(&Url::parse("https://kessel-api.parsec.app.evil.invalid").unwrap())
        );
        network.shutdown();
        assert!(network
            .execute(Request {
                url,
                method: Method::POST,
                headers: Default::default(),
                body: Some(b"synthetic-only".to_vec()),
                timeout: MAX_TIMEOUT,
            })
            .is_err());
        assert!(network.client.get().is_none());
        let audit = serde_json::to_value(network.audit.snapshot()).unwrap();
        assert_eq!(audit["intents"][0]["policy_allowed"], false);
    }
    #[test]
    fn header_values_preserve_colons_and_reject_injection_and_framing() {
        let h = headers("Authorization: Bearer fixture:a:b\r\nX-Test: yes\n").unwrap();
        assert_eq!(h["authorization"], "Bearer fixture:a:b");
        assert!(headers("X-Test: yes\rInjected: bad").is_err());
        assert!(headers("Content-Length: 7").is_err());
        assert!(headers("missing separator").is_err());
        assert!(headers(&"x".repeat(MAX_HEADERS + 1)).is_err());
    }
    #[test]
    fn default_policy_is_offline_and_diagnostic_origin_is_exact() {
        let url = Url::parse("http://127.0.0.1:1234/path").unwrap();
        assert!(!Network::default().allowed(&url));
        let n = Network::diagnostic(1234);
        assert!(n.allowed(&url));
        for url in [
            "http://localhost:1234",
            "http://127.0.0.1:1235",
            "https://127.0.0.1:1234",
            "http://user@127.0.0.1:1234",
            "http://example.com",
            "http://127.0.0.1:1234/#secret",
        ] {
            assert!(!n.allowed(&Url::parse(url).unwrap()));
        }
    }
}
