//! Loopback HTTP acceptance fixture, with no account or external requests.
use anyhow::{bail, Context, Result};
use std::{
    io::{Read, Write},
    net::{Shutdown, TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use wasmtime::{Config, Engine, Module};

struct Server {
    port: u16,
    stop: Arc<AtomicBool>,
    checks: Arc<Mutex<Vec<bool>>>,
    thread: Option<std::thread::JoinHandle<Result<()>>>,
}
impl Server {
    fn start() -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        listener.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let checks = Arc::new(Mutex::new(Vec::new()));
        let (flag, results) = (stop.clone(), checks.clone());
        let thread = std::thread::spawn(move || {
            while !flag.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => results.lock().unwrap().push(serve(stream)?),
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2))
                    }
                    Err(e) => return Err(e.into()),
                }
            }
            Ok(())
        });
        Ok(Self {
            port,
            stop,
            checks,
            thread: Some(thread),
        })
    }
    fn finish(&mut self) -> Result<()> {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            thread
                .join()
                .map_err(|_| anyhow::anyhow!("HTTP fixture server panicked"))??;
        }
        Ok(())
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

fn serve(mut stream: TcpStream) -> Result<bool> {
    // Accepted sockets can inherit the listener's nonblocking mode on Windows.
    // This fixture uses bounded blocking reads/writes, unlike its accept loop.
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    let mut header = Vec::new();
    while !header.ends_with(b"\r\n\r\n") {
        if header.len() >= 20 * 1024 {
            bail!("Fixture request header oversized");
        }
        let mut byte = [0];
        stream.read_exact(&mut byte)?;
        header.push(byte[0]);
    }
    let header = String::from_utf8(header)?;
    let start = header.lines().next().context("Fixture request missing")?;
    let path = start
        .split_whitespace()
        .nth(1)
        .context("Fixture path missing")?;
    let length = header
        .lines()
        .find_map(|l| {
            l.split_once(':')
                .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                .map(|(_, v)| v.trim().parse::<usize>())
        })
        .transpose()?
        .unwrap_or(0);
    if length > crate::http::MAX_BODY {
        bail!("Fixture request body oversized");
    }
    let mut body = vec![0; length];
    stream.read_exact(&mut body)?;
    let lower = header.to_ascii_lowercase();
    let checked = if path == "/binary" {
        start.starts_with("POST ")
            && body == "árvíz ✓".as_bytes()
            && lower.contains("authorization: bearer fixture:a:b\r\n")
    } else {
        true
    };
    match path {
        "/empty" => stream.write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")?,
        "/error" => stream.write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 3\r\nConnection: close\r\n\r\nerr")?,
        "/redirect" => stream.write_all(b"HTTP/1.1 302 Found\r\nLocation: https://example.com/never-request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")?,
        "/oversize" => stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", crate::http::MAX_BODY + 1).as_bytes())?,
        "/stream-oversize" => {
            stream.write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n")?;
            // The bounded client may close while the server is finishing.
            let _ = stream.write_all(&vec![42; crate::http::MAX_BODY + 1]);
        }
        "/slow" => {
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\nConnection: close\r\n\r\n")?;
            std::thread::sleep(Duration::from_millis(200));
            let _ = stream.write_all(b"x");
        }
        "/binary" | "/alloc" => {
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\n")?;
            stream.write_all(&[0, 255, 16, 128, 0])?;
        }
        _ => bail!("Unexpected fixture HTTP path"),
    }
    let _ = stream.shutdown(Shutdown::Write);
    Ok(checked)
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
        (import "env" "MTY_HttpRequest" (func $http (param i32 i32 i32 i32 i32 i32 i32 i32 i32 i32) (result i32)))
        (func (export "mty_system_alloc") (param $size i32) (param i32) (result i32)
            i32.const 516 local.get $size i32.store i32.const 512 i32.load)
        (func $free (export "mty_system_free") (param i32)
            i32.const 524 i32.const 524 i32.load i32.const 1 i32.add i32.store)
        (func (export "release")
            i32.const 400 i32.load if i32.const 400 i32.load call $free end
            i32.const 400 i32.const 0 i32.store)
        (func (export "request") (param $timeout i32) (param $output i32) (result i32)
            i32.const 16 i32.const 1024 i32.const 1100
            i32.const 520 i32.load if (result i32) i32.const 2048 else i32.const 0 end
            i32.const 520 i32.load i32.const 0 local.get $timeout
            local.get $output i32.const 404 i32.const 408 call $http))"#,
    )?;
    let (mut store, instance) = crate::instantiate(&engine, &module)?;
    let memory = store.data().memory.clone();
    let request = instance.get_typed_func::<(i32, i32), i32>(&mut store, "request")?;
    let release = instance.get_typed_func::<(), ()>(&mut store, "release")?;
    let url = |path: &str| format!("http://127.0.0.1:{}{path}", server.port);
    memory.set_u32(512, 8192)?;
    memory.c_string(16, 1000, &url("/binary"))?;
    memory.c_string(1024, 64, "POST")?;
    memory.c_string(
        1100,
        900,
        "Authorization: Bearer fixture:a:b\r\nX-Fixture: yes\n",
    )?;
    let text = "árvíz ✓";
    memory.write(2048, text.as_bytes())?;
    memory.set_u32(520, text.len() as u32)?;
    if request.call(&mut store, (1000, 400))? != 0 || memory.u32(400)? != 0 {
        bail!("Offline policy failed");
    }
    store.data_mut().http = Arc::new(crate::http::Network::diagnostic(server.port));
    if request.call(&mut store, (1000, 400))? != 1 {
        server.finish()?;
        bail!(
            "HTTP binary response failed: category={}, completed_fixture_requests={}",
            store.data().http_failure.unwrap_or("guest-allocation"),
            server.checks.lock().unwrap().len()
        );
    }
    if memory.u32(400)? != 8192
        || memory.u32(404)? != 5
        || memory.read(408, 2)? != 200u16.to_le_bytes()
        || memory.read(8192, 6)? != [0, 255, 16, 128, 0, 0]
        || memory.u32(516)? != 6
    {
        bail!("HTTP binary buffer/terminator mismatch");
    }
    release.call(&mut store, ())?;
    memory.c_string(1024, 64, "GET")?;
    memory.set_u32(520, 0)?;
    for (path, status, size) in [
        ("/empty", 204u16, 0u32),
        ("/error", 401, 3),
        ("/redirect", 302, 0),
    ] {
        memory.c_string(16, 1000, &url(path))?;
        if request.call(&mut store, (1000, 400))? != 1
            || memory.u32(404)? != size
            || memory.read(408, 2)? != status.to_le_bytes()
        {
            bail!("HTTP status semantics failed");
        }
        if size == 0 && memory.u32(400)? != 0 {
            bail!("Empty response allocation");
        }
        release.call(&mut store, ())?;
    }
    for (path, timeout) in [
        ("/oversize", 1000),
        ("/stream-oversize", 1000),
        ("/slow", 50),
    ] {
        memory.c_string(16, 1000, &url(path))?;
        if request.call(&mut store, (timeout, 400))? != 0 || memory.read(400, 10)? != vec![0; 10] {
            bail!("HTTP limit/timeout failed");
        }
    }
    memory.c_string(16, 1000, &url("/alloc"))?;
    for pointer in [0, 131071] {
        memory.set_u32(512, pointer)?;
        if request.call(&mut store, (1000, 400))? != 0 || memory.read(400, 10)? != vec![0; 10] {
            bail!("Allocation failure leaked output");
        }
    }
    if memory.u32(524)? != 3 {
        bail!("Response allocation ownership/cleanup failed");
    }
    // All output pointers are checked before any network operation/write.
    if request.call(&mut store, (1000, -1)).is_ok() || request.call(&mut store, (1000, 404)).is_ok()
    {
        bail!("HTTP output bounds/overlap not checked");
    }
    // Framing injection and oversized guest bodies fail before transport.
    memory.c_string(1100, 900, "Content-Length: 3\n")?;
    if request.call(&mut store, (1000, 400))? != 0 {
        bail!("Framing header accepted");
    }
    memory.c_string(1100, 900, "")?;
    memory.set_u32(520, (crate::http::MAX_BODY + 1) as u32)?;
    if request.call(&mut store, (1000, 400))? != 0 {
        bail!("Oversized guest body accepted");
    }
    server.finish()?;
    let checks = server.checks.lock().unwrap();
    if checks.len() != 9 || checks.iter().any(|x| !x) {
        bail!("Unexpected HTTP fixture requests");
    }
    Ok(serde_json::json!({
        "schema":1, "scope":"controlled-wasm-native-loopback-http",
        "requests_verified":9, "server_closed":true,
        "binary_response_verified":true, "utf8_request_verified":true,
        "header_colons_verified":true, "nul_termination_verified":true,
        "empty_response_verified":true, "http_error_status_verified":true,
        "redirect_not_followed_verified":true, "response_limits_verified":true,
        "body_read_timeout_verified":true, "output_bounds_verified":true,
        "allocation_failure_cleanup_verified":true, "offline_policy_verified":true,
        "request_validation_verified":true,
        "original_parsec_guest_http_exercised":false, "authentication_integrated":false,
        "external_requests_enabled":false, "websocket_signaling_integrated":false,
        "parsec_host_connected":false, "video_decoded":false
    }))
}

#[cfg(test)]
mod tests {
    #[test]
    fn actual_http_import_preserves_binary_status_and_failure_contracts() {
        let report = super::probe().unwrap();
        assert_eq!(report["requests_verified"], 9);
        assert_eq!(report["body_read_timeout_verified"], true);
        assert_eq!(report["allocation_failure_cleanup_verified"], true);
        assert_eq!(report["server_closed"], true);
        assert_eq!(report["authentication_integrated"], false);
    }
}
