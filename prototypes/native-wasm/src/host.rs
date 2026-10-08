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
    pub started: Instant,
    pub calls: BTreeMap<String, u64>,
    pub boundary: Option<String>,
    pub stdout: String,
    pub title: Option<String>,
    pub app_pointer: Option<u32>,
    #[serde(skip)]
    pub keys: BTreeMap<i32, String>,
}

impl HostState {
    pub fn new(memory: GuestMemory) -> Self {
        Self {
            memory,
            threads: None,
            started: Instant::now(),
            calls: BTreeMap::new(),
            boundary: None,
            stdout: String::new(),
            title: None,
            app_pointer: None,
            keys: BTreeMap::new(),
        }
    }
}

pub fn implemented(module: &str, name: &str) -> bool {
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
        ),
        "wasi_snapshot_preview1" => matches!(
            name,
            "args_get"
                | "args_sizes_get"
                | "environ_get"
                | "environ_sizes_get"
                | "clock_time_get"
                | "fd_prestat_get"
                | "fd_prestat_dir_name"
                | "fd_fdstat_get"
                | "fd_fdstat_set_flags"
                | "fd_write"
                | "fd_close"
                | "sched_yield"
                | "path_open"
                | "path_filestat_get"
                | "proc_exit"
        ),
        "wasi" => name == "thread-spawn",
        _ => false,
    }
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
    if !implemented(module, name) {
        caller.data_mut().boundary = Some(id.clone());
        bail!("native bridge not implemented: {id}");
    }
    let m = caller.data().memory.clone();
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
        match name {
            "flock" => result(results, 0),
            "web_get_hostname" => {
                // Reuse the guest allocator rather than inventing an address.
                let alloc = caller
                    .get_export("mty_system_alloc")
                    .and_then(|e| e.into_func())
                    .context("guest allocator missing")?
                    .typed::<(i32, i32), i32>(&caller)?;
                let p = alloc.call(&mut caller, (14, 1))? as u32;
                m.c_string(p, 14, "web.parsec.app")?;
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
    // Standard WASI preview1 ABI. No preopened directories, env or user files.
    // Unsupported operations fail rather than returning fabricated success.
    let errno = match name {
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
        "fd_prestat_get" | "fd_prestat_dir_name" => 8, // BADF: no filesystem capability
        "path_open" | "path_filestat_get" => 8,
        "fd_fdstat_get" => {
            if !(0..=2).contains(&int(args, 0)?) {
                8
            } else {
                let mut stat = [0u8; 24];
                stat[0] = 2; // character device
                let rights: u64 = if int(args, 0)? == 0 { 2 } else { 64 };
                stat[8..16].copy_from_slice(&rights.to_le_bytes());
                m.write(ptr(args, 1)?, &stat)?;
                0
            }
        }
        "fd_fdstat_set_flags" | "fd_close" => {
            if (0..=2).contains(&int(args, 0)?) {
                0
            } else {
                8
            }
        }
        "fd_write" => {
            if !(1..=2).contains(&int(args, 0)?) {
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
                    let text = String::from_utf8_lossy(&bytes);
                    if caller.data().stdout.len() + text.len() <= 65536 {
                        caller.data_mut().stdout.push_str(&text);
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
