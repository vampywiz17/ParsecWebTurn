use std::{collections::BTreeMap, time::Instant};
use windows::{
    core::w,
    Win32::{
        Foundation::{CloseHandle, FILETIME},
        System::{
            Performance::*,
            Threading::{
                GetActiveProcessorCount, GetProcessTimes, OpenProcess, ALL_PROCESSOR_GROUPS,
                PROCESS_QUERY_LIMITED_INFORMATION,
            },
        },
    },
};

#[derive(Default)]
pub struct Processes {
    pub all: Vec<u32>,
    pub gpu: Vec<u32>,
}

// PDH queries support successive samples across callers. Store handle addresses
// as integers so no COM interface/raw pointer is sent across threads.
#[derive(Default)]
pub struct Sampler {
    previous: BTreeMap<u32, (u64, u64)>,
    time: Option<Instant>,
    query: Option<(usize, usize)>,
    attempted: bool,
}

fn ticks(value: FILETIME) -> u64 {
    (u64::from(value.dwHighDateTime) << 32) | u64::from(value.dwLowDateTime)
}
fn process_time(pid: u32) -> Option<(u64, u64)> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let (mut creation, mut exit, mut kernel, mut user) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        let result = GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user);
        let _ = CloseHandle(handle);
        result.ok()?;
        Some((ticks(creation), ticks(kernel) + ticks(user)))
    }
}

impl Sampler {
    pub fn sample(&mut self, processes: &Processes) -> (Option<f64>, Option<f64>, Option<f64>) {
        let now = Instant::now();
        let current: BTreeMap<_, _> = processes
            .all
            .iter()
            .filter_map(|pid| process_time(*pid).map(|value| (*pid, value)))
            .collect();
        let mut delta = Some(0_u64);
        for (pid, (creation, total)) in &current {
            delta = delta.and_then(|sum| {
                self.previous
                    .get(pid)
                    .filter(|(old_creation, old_total)| {
                        old_creation == creation && old_total <= total
                    })
                    .map(|(_, old_total)| sum + total - old_total)
            });
        }
        let cores = unsafe { GetActiveProcessorCount(ALL_PROCESSOR_GROUPS) };
        let cpu = if current.len() == processes.all.len() && !current.is_empty() && cores > 0 {
            self.time.zip(delta).map(|(previous, delta)| {
                (delta as f64
                    / 10_000_000.0
                    / now.duration_since(previous).as_secs_f64()
                    / f64::from(cores)
                    * 100.0)
                    .clamp(0.0, 100.0)
            })
        } else {
            None
        };
        self.previous = current;
        self.time = Some(now);
        let (gpu, decode) = self.gpu(&processes.gpu).unwrap_or((None, None));
        (cpu, gpu, decode)
    }
    fn gpu(&mut self, pids: &[u32]) -> Option<(Option<f64>, Option<f64>)> {
        unsafe {
            if !self.attempted {
                self.attempted = true;
                let mut query = PDH_HQUERY::default();
                if PdhOpenQueryW(None, 0, &mut query) != 0 {
                    return None;
                }
                let mut counter = PDH_HCOUNTER::default();
                if PdhAddEnglishCounterW(
                    query,
                    w!("\\GPU Engine(*)\\Utilization Percentage"),
                    0,
                    &mut counter,
                ) != 0
                {
                    PdhCloseQuery(query);
                    return None;
                }
                self.query = Some((query.0 as usize, counter.0 as usize));
                PdhCollectQueryData(query);
                return None; // Rates require two samples.
            }
            let (query, counter) = self.query?;
            let query = PDH_HQUERY(query as *mut _);
            let counter = PDH_HCOUNTER(counter as *mut _);
            if PdhCollectQueryData(query) != 0 || pids.is_empty() {
                return None;
            }
            let mut size = 0;
            let mut count = 0;
            if PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut size, &mut count, None)
                != PDH_MORE_DATA
                || size > 4 * 1024 * 1024
            {
                return None;
            }
            // u64 storage supplies the alignment required by PDH's pointer/double structs.
            let mut buffer = vec![0_u64; (size as usize).div_ceil(8)];
            let items = buffer.as_mut_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>();
            if PdhGetFormattedCounterArrayW(
                counter,
                PDH_FMT_DOUBLE,
                &mut size,
                &mut count,
                Some(items),
            ) != 0
                || count as usize
                    > buffer.len() * 8 / std::mem::size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>()
            {
                return None;
            }
            let mut engines = BTreeMap::<String, f64>::new();
            for item in std::slice::from_raw_parts(items, count as usize) {
                if item.FmtValue.CStatus > 1 {
                    continue;
                }
                let address = item.szName.0 as usize;
                let start = buffer.as_ptr() as usize;
                let end = start + buffer.len() * 8;
                if address < start || address >= end || address % 2 != 0 {
                    continue;
                }
                let name = std::slice::from_raw_parts(item.szName.0, (end - address) / 2);
                let Some(length) = name.iter().position(|c| *c == 0) else {
                    continue;
                };
                let name = String::from_utf16_lossy(&name[..length]);
                let Some((pid, engine)) = name
                    .strip_prefix("pid_")
                    .and_then(|name| name.split_once('_'))
                else {
                    continue;
                };
                if !pid
                    .parse::<u32>()
                    .ok()
                    .is_some_and(|pid| pids.contains(&pid))
                {
                    continue;
                }
                let value = item.FmtValue.Anonymous.doubleValue;
                if value.is_finite() && value >= 0.0 {
                    *engines.entry(engine.into()).or_default() += value;
                }
            }
            let maximum = |decode: bool| {
                engines
                    .iter()
                    .filter(|(name, _)| !decode || name.contains("engtype_VideoDecode"))
                    .map(|(_, value)| value.clamp(0.0, 100.0))
                    .reduce(f64::max)
            };
            Some((maximum(false), maximum(true)))
        }
    }
}
impl Drop for Sampler {
    fn drop(&mut self) {
        if let Some((query, _)) = self.query {
            unsafe {
                PdhCloseQuery(PDH_HQUERY(query as *mut _));
            }
        }
    }
}

pub async fn processes(window: &tauri::WebviewWindow) -> Option<Processes> {
    let (send, receive) = tokio::sync::oneshot::channel();
    window
        .with_webview(move |view| {
            let result = (|| unsafe {
                use webview2_com::Microsoft::Web::WebView2::Win32::{
                    ICoreWebView2Environment8, COREWEBVIEW2_PROCESS_KIND,
                    COREWEBVIEW2_PROCESS_KIND_GPU,
                };
                use windows::core::Interface;
                let environment = view
                    .environment()
                    .cast::<ICoreWebView2Environment8>()
                    .ok()?;
                let infos = environment.GetProcessInfos().ok()?;
                let mut count = 0;
                infos.Count(&mut count).ok()?;
                if count > 1024 {
                    return None;
                }
                let mut result = Processes {
                    all: vec![std::process::id()],
                    gpu: vec![],
                };
                for index in 0..count {
                    let info = infos.GetValueAtIndex(index).ok()?;
                    let mut pid = 0;
                    let mut kind = COREWEBVIEW2_PROCESS_KIND::default();
                    info.ProcessId(&mut pid).ok()?;
                    info.Kind(&mut kind).ok()?;
                    if pid > 0 {
                        result.all.push(pid as u32);
                        if kind == COREWEBVIEW2_PROCESS_KIND_GPU {
                            result.gpu.push(pid as u32);
                        }
                    }
                }
                result.all.sort_unstable();
                result.all.dedup();
                Some(result)
            })();
            let _ = send.send(result);
        })
        .ok()?;
    tokio::time::timeout(std::time::Duration::from_secs(2), receive)
        .await
        .ok()?
        .ok()?
}
