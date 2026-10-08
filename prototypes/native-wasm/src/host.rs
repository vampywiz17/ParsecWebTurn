use crate::memory::GuestMemory;
use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use wasmtime::{Caller, Val};

#[derive(Serialize)]
pub struct HostState {
    #[serde(skip)]
    pub memory: GuestMemory,
    #[serde(skip)]
    pub threads: Option<std::sync::Arc<crate::threads::ThreadRuntime>>,
    #[serde(skip)]
    pub filesystem: std::sync::Arc<std::sync::Mutex<crate::filesystem::VirtualFs>>,
    #[serde(skip)]
    pub backend: std::sync::Arc<std::sync::Mutex<crate::backend::Backend>>,
    #[serde(skip)]
    pub started: Instant,
    pub calls: BTreeMap<String, u64>,
    pub boundary: Option<String>,
    pub stdout: String,
    pub title: Option<String>,
    pub app_pointer: Option<u32>,
    pub filesystem_requests: Vec<(String, String, i32)>,
    #[serde(skip)]
    pub keys: BTreeMap<i32, String>,
    #[cfg(windows)]
    #[serde(skip)]
    pub window: Option<std::sync::Arc<crate::window::Window>>,
    #[cfg(windows)]
    #[serde(skip)]
    pub graphics: Option<crate::graphics::Graphics>,
    pub event_loop: Option<(u32, u32)>,
}

impl HostState {
    pub fn new(memory: GuestMemory) -> Self {
        Self {
            memory,
            threads: None,
            filesystem: Default::default(),
            backend: Default::default(),
            started: Instant::now(),
            calls: BTreeMap::new(),
            boundary: None,
            stdout: String::new(),
            title: None,
            app_pointer: None,
            filesystem_requests: Vec::new(),
            keys: BTreeMap::new(),
            #[cfg(windows)]
            window: None,
            #[cfg(windows)]
            graphics: None,
            event_loop: None,
        }
    }
}

pub fn implemented(module: &str, name: &str) -> bool {
    if disabled_web_stub(module, name) {
        return true;
    }
    match module {
        "env" => matches!(
            name,
            "flock"
                | "web_get_hostname"
                | "web_platform"
                | "web_set_key"
                | "web_get_key"
                | "web_set_title"
                | "web_set_app"
                | "MTY_GetRandomBytes"
                | "parsec_web_init"
                | "parsec_web_new_attempt"
                | "parsec_web_begin_p2p"
                | "parsec_web_add_candidate"
                | "parsec_web_destroy"
                | "parsec_web_disconnect"
                | "parsec_web_get_status"
                | "parsec_web_send_message"
                | "parsec_web_get_guests"
                | "parsec_web_poll_events"
                | "parsec_web_get_buffer_size"
                | "parsec_web_get_buffer"
                | "parsec_web_get_metrics"
                | "parsec_web_get_network_failure"
                | "parsec_web_get_self"
                | "parsec_web_get_host_mode"
                | "parsec_web_poll_audio"
                | "parsec_client_set_config"
        ),
        "wasi_snapshot_preview1" => matches!(
            name,
            "args_get"
                | "args_sizes_get"
                | "environ_get"
                | "environ_sizes_get"
                | "clock_time_get"
                | "poll_oneoff"
                | "fd_prestat_get"
                | "fd_prestat_dir_name"
                | "fd_fdstat_get"
                | "fd_fdstat_set_flags"
                | "fd_write"
                | "fd_close"
                | "sched_yield"
                | "path_open"
                | "path_filestat_get"
                | "path_create_directory"
                | "path_unlink_file"
                | "path_remove_directory"
                | "fd_read"
                | "fd_seek"
                | "proc_exit"
        ),
        "wasi" => name == "thread-spawn",
        _ => false,
    }
}

/// The audited weblib.js leaves these optional services empty. Its undefined
/// result becomes a null/zero WASM handle. Preserve unavailability, not a
/// fabricated maintenance service, USB forwarding or native feature.
pub fn disabled_web_stub(module: &str, name: &str) -> bool {
    module == "env"
        && matches!(
            name,
            "maintenance_create"
                | "maintenance_destroy"
                | "maintenance_force_poll"
                | "maintenance_get_state"
                | "usb_devices_create"
                | "usb_devices_destroy"
                | "usb_device_submit"
                | "usb_devices_set_config"
                | "usb_devices_get"
        )
}

fn int(args: &[Val], index: usize) -> Result<i32> {
    args.get(index)
        .and_then(Val::i32)
        .context("expected i32 argument")
}
fn ptr(args: &[Val], index: usize) -> Result<u32> {
    Ok(int(args, index)? as u32)
}
fn result(results: &mut [Val], value: i32) {
    if let Some(slot) = results.first_mut() {
        *slot = Val::I32(value);
    }
}

pub fn dispatch(
    mut caller: Caller<'_, HostState>,
    module: &str,
    name: &str,
    args: &[Val],
    results: &mut [Val],
) -> Result<()> {
    let id = format!("{module}::{name}");
    *caller.data_mut().calls.entry(id.clone()).or_default() += 1;
    #[cfg(windows)]
    if module == "env" && caller.data().window.is_some() && crate::desktop::handles(name) {
        let outcome = crate::desktop::dispatch(&mut caller, name, args, results);
        if outcome.is_err() && name != "web_run_and_yield" {
            caller.data_mut().boundary = Some(id);
        }
        return outcome;
    }
    if !implemented(module, name) {
        caller.data_mut().boundary = Some(id.clone());
        bail!("native bridge not implemented: {id}");
    }
    let m = caller.data().memory.clone();
    if disabled_web_stub(module, name) {
        result(results, 0);
        return Ok(());
    }
    if module == "wasi" {
        let runtime = caller
            .data()
            .threads
            .clone()
            .context("thread runtime missing")?;
        result(results, runtime.spawn(ptr(args, 0)?));
        return Ok(());
    }
    if module == "env" {
        if name.starts_with("parsec_") {
            return backend_call(&caller, name, args, results);
        }
        match name {
            "flock" => result(results, 0),
            "web_get_hostname" => {
                // Reuse the guest allocator rather than inventing an address.
                let alloc = caller
                    .get_export("mty_system_alloc")
                    .and_then(|e| e.into_func())
                    .context("guest allocator missing")?
                    .typed::<(i32, i32), i32>(&caller)?;
                let hostname = "web.parsec.app";
                let capacity = hostname.len() + 1;
                let p = alloc.call(&mut caller, (capacity as i32, 1))? as u32;
                m.c_string(p, capacity, hostname)?;
                result(results, p as i32);
            }
            "web_platform" => m.c_string(ptr(args, 0)?, ptr(args, 1)? as usize, "Win32")?,
            "web_set_key" => {
                if int(args, 0)? != 0 {
                    let code = m.string(ptr(args, 1)?, 128)?;
                    caller.data_mut().keys.insert(int(args, 2)?, code);
                }
            }
            "web_get_key" => {
                if let Some(code) = caller.data().keys.get(&int(args, 0)?) {
                    m.c_string(ptr(args, 1)?, ptr(args, 2)? as usize, code)?;
                    result(results, 1);
                } else {
                    result(results, 0);
                }
            }
            "web_set_title" => caller.data_mut().title = Some(m.string(ptr(args, 0)?, 1024)?),
            "web_set_app" => caller.data_mut().app_pointer = Some(ptr(args, 0)?),
            "MTY_GetRandomBytes" => {
                let len = ptr(args, 1)? as usize;
                if len > 1024 * 1024 {
                    bail!("random request too large");
                }
                let mut bytes = vec![0; len];
                getrandom::fill(&mut bytes)
                    .map_err(|e| anyhow::anyhow!("OS random failed: {e}"))?;
                m.write(ptr(args, 0)?, &bytes)?;
            }
            _ => unreachable!(),
        }
        return Ok(());
    }
    // Standard WASI preview1 ABI. One empty synthetic root, no host directories,
    // environment or user files. The libc bootstrap expects a root preopen.
    // Unsupported operations fail rather than returning fabricated success.
    let errno = match name {
        "poll_oneoff" => crate::poll::clock_poll(
            &m,
            caller.data().started,
            ptr(args, 0)?,
            ptr(args, 1)?,
            ptr(args, 2)?,
            ptr(args, 3)?,
        )?,
        "args_sizes_get" => {
            m.set_u32(ptr(args, 0)?, 1)?;
            m.set_u32(ptr(args, 1)?, 8)?;
            0
        }
        "args_get" => {
            m.set_u32(ptr(args, 0)?, ptr(args, 1)?)?;
            m.c_string(ptr(args, 1)?, 8, "parsecd")?;
            0
        }
        "environ_sizes_get" => {
            m.set_u32(ptr(args, 0)?, 0)?;
            m.set_u32(ptr(args, 1)?, 0)?;
            0
        }
        "environ_get" => 0,
        "clock_time_get" => {
            let ns = match int(args, 0)? {
                0 => SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos(),
                1 => caller.data().started.elapsed().as_nanos(),
                _ => {
                    result(results, 28);
                    return Ok(());
                }
            };
            m.write(ptr(args, 2)?, &(u64::try_from(ns)?).to_le_bytes())?;
            0
        }
        "fd_prestat_get" => {
            if int(args, 0)? != 3 {
                8
            } else {
                let mut prestat = [0u8; 8];
                prestat[4..8].copy_from_slice(&1u32.to_le_bytes());
                m.write(ptr(args, 1)?, &prestat)?;
                0
            }
        }
        "fd_prestat_dir_name" => {
            if int(args, 0)? != 3 {
                8
            } else if ptr(args, 2)? < 1 {
                37
            } else {
                m.write(ptr(args, 1)?, b"/")?;
                0
            }
        }
        "path_open"
        | "path_filestat_get"
        | "path_create_directory"
        | "path_unlink_file"
        | "path_remove_directory"
        | "fd_read"
        | "fd_seek" => {
            let fs = caller.data().filesystem.clone();
            let mut fs = fs
                .lock()
                .map_err(|_| anyhow::anyhow!("virtual filesystem lock poisoned"))?;
            let errno = filesystem_call(&m, &mut fs, name, args)?;
            drop(fs);
            if name.starts_with("path_") && caller.data().filesystem_requests.len() < 32 {
                let index = if matches!(
                    name,
                    "path_create_directory" | "path_unlink_file" | "path_remove_directory"
                ) {
                    1
                } else {
                    2
                };
                let len = ptr(args, index + 1)? as usize;
                if len <= 4096 {
                    let path =
                        String::from_utf8_lossy(&m.read(ptr(args, index)?, len)?).into_owned();
                    caller
                        .data_mut()
                        .filesystem_requests
                        .push((name.into(), path, errno));
                }
            }
            errno
        }
        "fd_fdstat_get" => {
            let fd = ptr(args, 0)?;
            if fd > 3 {
                let fs = caller
                    .data()
                    .filesystem
                    .lock()
                    .map_err(|_| anyhow::anyhow!("virtual filesystem lock poisoned"))?;
                if let Some(h) = fs.handles.get(&fd) {
                    let mut stat = [0u8; 24];
                    stat[0] = 4;
                    stat[2..4].copy_from_slice(&h.flags.to_le_bytes());
                    stat[8..16].copy_from_slice(&h.rights.to_le_bytes());
                    m.write(ptr(args, 1)?, &stat)?;
                    0
                } else {
                    8
                }
            } else {
                let mut stat = [0u8; 24];
                stat[0] = if int(args, 0)? == 3 { 3 } else { 2 };
                let rights: u64 = match int(args, 0)? {
                    0 => 2,
                    3 => 512 | 1024 | 8192 | 262144 | 524288,
                    _ => 64,
                };
                stat[8..16].copy_from_slice(&rights.to_le_bytes());
                m.write(ptr(args, 1)?, &stat)?;
                0
            }
        }
        "fd_close" => {
            let fd = ptr(args, 0)?;
            if fd > 3 {
                let fs = caller.data().filesystem.clone();
                let mut fs = fs
                    .lock()
                    .map_err(|_| anyhow::anyhow!("virtual filesystem lock poisoned"))?;
                if fs.close(fd) {
                    0
                } else {
                    8
                }
            } else {
                0
            }
        }
        "fd_fdstat_set_flags" => {
            let fd = ptr(args, 0)?;
            let flags = ptr(args, 1)?;
            if flags & !1 != 0 {
                28
            } else if fd <= 2 {
                0
            } else {
                let fs = caller.data().filesystem.clone();
                let mut fs = fs
                    .lock()
                    .map_err(|_| anyhow::anyhow!("virtual filesystem lock poisoned"))?;
                if let Some(h) = fs.handles.get_mut(&fd) {
                    if h.rights & 8 == 0 {
                        76
                    } else {
                        h.flags = flags as u16;
                        0
                    }
                } else {
                    8
                }
            }
        }
        "fd_write" => {
            let fd = ptr(args, 0)?;
            if fd == 0 || fd == 3 {
                8
            } else {
                let count = ptr(args, 2)?;
                if count > 1024 {
                    bail!("too many stdout iovecs");
                }
                let mut total = 0u32;
                for i in 0..count {
                    let address = ptr(args, 1)?.checked_add(i * 8).context("iovec overflow")?;
                    let p = m.u32(address)?;
                    let len = m.u32(address.checked_add(4).context("iovec overflow")?)?;
                    if len > 65536 {
                        bail!("stdout iovec exceeds limit");
                    }
                    let bytes = m.read(p, len as usize)?;
                    if fd <= 2 {
                        let text = String::from_utf8_lossy(&bytes);
                        if caller.data().stdout.len() + text.len() <= 65536 {
                            caller.data_mut().stdout.push_str(&text);
                        }
                    } else {
                        let fs = caller.data().filesystem.clone();
                        let mut fs = fs
                            .lock()
                            .map_err(|_| anyhow::anyhow!("virtual filesystem lock poisoned"))?;
                        if let Err(errno) = fs.write(fd, &bytes) {
                            m.set_u32(ptr(args, 3)?, total)?;
                            result(results, errno);
                            return Ok(());
                        }
                    }
                    total = total.checked_add(len).context("write count overflow")?;
                }
                m.set_u32(ptr(args, 3)?, total)?;
                0
            }
        }
        "sched_yield" => {
            std::thread::yield_now();
            0
        }
        "proc_exit" => bail!("guest requested exit code {}", int(args, 0)?),
        _ => unreachable!(),
    };
    result(results, errno);
    Ok(())
}

fn backend_call(
    caller: &Caller<'_, HostState>,
    name: &str,
    args: &[Val],
    results: &mut [Val],
) -> Result<()> {
    let m = &caller.data().memory;
    let mut b = caller
        .data()
        .backend
        .lock()
        .map_err(|_| anyhow::anyhow!("backend lock poisoned"))?;
    match name {
        "parsec_web_init" => b.init(),
        "parsec_web_destroy" => b.destroy(),
        _ => {
            b.require_initialized()?;
            match name {
                "parsec_web_new_attempt" => {
                    let id = m.string(ptr(args, 0)?, 257)?;
                    crate::signaling::CandidateGate::new(&id)?;
                    let output = crate::attempt::Output::new(
                        m.clone(),
                        [ptr(args, 1)?, ptr(args, 2)?, ptr(args, 3)?],
                        ptr(args, 4)? as usize,
                        ptr(args, 5)?,
                        ptr(args, 6)?,
                    )?;
                    if b.status != Some(-3) || b.native_attempt.is_some() {
                        output.finish(None)?;
                    } else {
                        // Spawn returns immediately. Native async work never
                        // needs this backend lock or a Wasmtime Store/Caller.
                        match crate::attempt::Attempt::spawn_named(&id, output.clone()) {
                            Ok(attempt) => {
                                b.status = Some(20);
                                b.native_attempt = Some(attempt);
                            }
                            Err(_) => {
                                output.finish(None)?;
                            }
                        }
                    }
                }
                "parsec_web_begin_p2p" => {
                    let id = m.string(ptr(args, 0)?, 257)?;
                    // The pinned JS ignores port: ICE selects the endpoint.
                    let remote = crate::signaling::Credentials {
                        ufrag: m.string(ptr(args, 2)?, 257)?,
                        password: m.string(ptr(args, 3)?, 257)?,
                        fingerprint: m.string(ptr(args, 4)?, 257)?,
                    };
                    b.native_attempt
                        .as_ref()
                        .context("no native attempt")?
                        .begin(&id, remote)?;
                }
                "parsec_web_add_candidate" => {
                    let id = m.string(ptr(args, 0)?, 257)?;
                    let attempt = b.native_attempt.as_ref().context("no native attempt")?;
                    if int(args, 3)? != 0 {
                        // Sync is a protocol marker, never an IP endpoint.
                        attempt.sync(&id)?;
                    } else {
                        let candidate = crate::signaling::Candidate::new(
                            &m.string(ptr(args, 1)?, 128)?,
                            u16::try_from(int(args, 2)?).context("invalid candidate port")?,
                            int(args, 4)? != 0,
                        )?;
                        attempt.candidate(&id, candidate)?;
                    }
                }
                "parsec_web_disconnect" => b.disconnect(int(args, 0)?, int(args, 1)?)?,
                "parsec_web_send_message" => {
                    // weblib.js parses JSON even when no transport is connected.
                    let _: serde_json::Value =
                        serde_json::from_str(&m.string(ptr(args, 0)?, 65536)?)?;
                    b.discard_idle_message()?;
                }
                "parsec_web_get_status" => {
                    if b.native_attempt
                        .as_ref()
                        .is_some_and(|attempt| attempt.failed())
                    {
                        b.status = Some(-3);
                        b.native_attempt.take();
                    }
                    result(results, b.status.context("backend status missing")?)
                }
                "parsec_client_set_config" => {
                    b.set_video_protocol(crate::backend::VideoProtocol {
                        version: ptr(args, 0)?,
                        message_size: ptr(args, 1)?,
                        version_offset: ptr(args, 2)?,
                        flag_offset: ptr(args, 3)?,
                    })?
                }
                "parsec_web_get_guests" => {
                    m.c_string(ptr(args, 0)?, ptr(args, 1)? as usize, "[]")?
                }
                "parsec_web_poll_events" => {
                    b.pump_native_events();
                    if let Some(event) = b.peek_event() {
                        let json = serde_json::to_string(event)?;
                        // Consume only after a successful copy, so a bad guest
                        // buffer cannot silently discard an event.
                        m.c_string(ptr(args, 0)?, ptr(args, 1)? as usize, &json)?;
                        b.poll_event();
                        result(results, 1);
                    } else {
                        result(results, 0);
                    }
                }
                "parsec_web_get_self" => {
                    m.write(ptr(args, 0)?, &[0])?;
                    m.set_u32(ptr(args, 1)?, 0)?;
                    // The pinned import has an i32 result, although weblib.js
                    // returns undefined. WebAssembly coerces that to zero.
                    result(results, 0);
                }
                "parsec_web_get_metrics" => {
                    // Idle values from X's constructor, NOT measured telemetry.
                    for i in [0, 1, 4, 5, 6] {
                        m.set_u32(ptr(args, i)?, 0)?;
                    }
                    for i in [2, 3] {
                        m.write(ptr(args, i)?, &[0])?;
                    }
                }
                "parsec_web_get_buffer" => {} // No buffers exist before a live attempt.
                "parsec_web_get_buffer_size"
                | "parsec_web_get_network_failure"
                | "parsec_web_get_host_mode"
                | "parsec_web_poll_audio" => result(results, 0),
                _ => unreachable!(),
            }
        }
    }
    Ok(())
}

fn filesystem_call(
    m: &GuestMemory,
    fs: &mut crate::filesystem::VirtualFs,
    name: &str,
    args: &[Val],
) -> Result<i32> {
    use crate::filesystem::*;
    match name {
        "path_open"
        | "path_filestat_get"
        | "path_create_directory"
        | "path_unlink_file"
        | "path_remove_directory" => {
            if ptr(args, 0)? != 3 {
                return Ok(BADF);
            }
            let index = if matches!(
                name,
                "path_create_directory" | "path_unlink_file" | "path_remove_directory"
            ) {
                1
            } else {
                2
            };
            let len = ptr(args, index + 1)? as usize;
            if len > 4096 {
                return Ok(INVAL);
            }
            let path = m.read(ptr(args, index)?, len)?;
            if name == "path_unlink_file" {
                return Ok(fs.unlink(&path).err().unwrap_or(0));
            }
            if name == "path_remove_directory" {
                return Ok(fs.remove_directory(&path).err().unwrap_or(0));
            }
            if name == "path_open" {
                let rights = args.get(5).and_then(Val::i64).context("expected rights")? as u64;
                match fs.open(&path, ptr(args, 4)?, rights, ptr(args, 7)? as u16) {
                    Ok(fd) => {
                        m.set_u32(ptr(args, 8)?, fd)?;
                        Ok(0)
                    }
                    Err(errno) => Ok(errno),
                }
            } else {
                let path = match VirtualFs::path(&path) {
                    Ok(p) => p,
                    Err(errno) => return Ok(errno),
                };
                if name == "path_create_directory" {
                    if fs.directories.contains(&path) {
                        return Ok(EXIST);
                    }
                    if fs.directories.len() >= 32 {
                        return Ok(INVAL);
                    }
                    fs.directories.insert(path);
                    Ok(0)
                } else {
                    let mut stat = [0u8; 64];
                    if fs.directories.contains(&path) {
                        stat[16] = 3;
                    } else if let Some(data) = fs.files.get(&path) {
                        stat[16] = 4;
                        stat[32..40].copy_from_slice(&(data.len() as u64).to_le_bytes());
                    } else {
                        return Ok(NOENT);
                    }
                    m.write(ptr(args, 4)?, &stat)?;
                    Ok(0)
                }
            }
        }
        "fd_read" => {
            let count = ptr(args, 2)?;
            if count > 1024 {
                return Ok(INVAL);
            }
            let mut total = 0u32;
            for i in 0..count {
                let address = ptr(args, 1)?.checked_add(i * 8).context("iovec overflow")?;
                let dest = m.u32(address)?;
                let len = m.u32(address.checked_add(4).context("iovec overflow")?)?;
                match fs.read(ptr(args, 0)?, len as usize) {
                    Ok(bytes) => {
                        m.write(dest, &bytes)?;
                        total = total
                            .checked_add(bytes.len() as u32)
                            .context("read count overflow")?;
                    }
                    Err(errno) => {
                        m.set_u32(ptr(args, 3)?, total)?;
                        return Ok(errno);
                    }
                }
            }
            m.set_u32(ptr(args, 3)?, total)?;
            Ok(0)
        }
        "fd_seek" => {
            let offset = args
                .get(1)
                .and_then(Val::i64)
                .context("expected file offset")?;
            match fs.seek(ptr(args, 0)?, offset, int(args, 2)?) {
                Ok(position) => {
                    m.write(ptr(args, 3)?, &position.to_le_bytes())?;
                    Ok(0)
                }
                Err(errno) => Ok(errno),
            }
        }
        _ => unreachable!(),
    }
}
