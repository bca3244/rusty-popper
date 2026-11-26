//! Real-time performance for the phone remote: how busy each CPU core, each
//! disk and the GPU are, read from Windows performance counters (PDH), the
//! same numbers Task Manager shows.
//!
//! Nothing runs in the background. The counters are opened when the page's
//! Real-time Performance view first asks, read at most about once a second
//! while it keeps asking, and closed again once it stops (see `Load::idle`).

use serde::Serialize;
use std::time::{Duration, Instant};

/// Answers within this long of the last reading reuse it (two phones, or a
/// fast poll, don't read the counters more often).
const FRESH: Duration = Duration::from_millis(900);
/// The counters are closed after this long without a request.
const IDLE: Duration = Duration::from_secs(15);
/// The wait between the very first two readings (rates need two).
const FIRST_GAP: Duration = Duration::from_millis(250);
/// After the counters couldn't be opened, wait this long before trying again
/// (the counter services may still be starting after boot).
const RETRY: Duration = Duration::from_secs(60);

/// One reading, as the page shows it. Percentages are 0 to 100; `None` where
/// Windows had no number (e.g. no GPU counters on this machine).
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Stats {
    /// Whether the counters could be opened at all.
    pub available: bool,
    /// All cores together.
    pub cpu: Option<f64>,
    /// Each logical core, in Windows' order.
    pub cores: Vec<f64>,
    /// The busiest GPU: its busiest engine, as Task Manager's "GPU" column.
    pub gpu: Option<f64>,
    /// That GPU's main engines ("3D", "Video Decode", ...).
    pub gpu_engines: Vec<Named>,
    pub disks: Vec<Disk>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Named {
    pub name: String,
    pub pct: f64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Disk {
    /// As Task Manager names it: "Disk 0 (C:)".
    pub name: String,
    /// Active time.
    pub busy: f64,
    /// Bytes read and written per second.
    pub bytes: f64,
}

/// The counters while the page is reading them.
#[derive(Default)]
pub struct Load {
    sampler: Option<pdh::Sampler>,
    /// When the counters last couldn't be opened (not retried every second).
    failed_at: Option<Instant>,
    last: Stats,
    read_at: Option<Instant>,
    asked_at: Option<Instant>,
}

impl Load {
    /// The current reading (opening the counters if needed).
    pub fn read(&mut self) -> Stats {
        let now = Instant::now();
        self.asked_at = Some(now);
        if self.read_at.is_some_and(|t| now - t < FRESH) {
            return self.last.clone();
        }
        if self.sampler.is_none() && self.failed_at.is_none_or(|t| now - t >= RETRY) {
            match pdh::Sampler::open() {
                Some(mut s) => {
                    s.collect();
                    std::thread::sleep(FIRST_GAP);
                    self.sampler = Some(s);
                    self.failed_at = None;
                }
                None => {
                    if self.failed_at.is_none() {
                        log::warn!("Phone remote: Windows performance counters unavailable");
                    }
                    self.failed_at = Some(now);
                }
            }
        }
        self.last = match &mut self.sampler {
            Some(s) => s.sample(),
            None => Stats::default(),
        };
        self.read_at = Some(Instant::now());
        self.last.clone()
    }

    /// Closes the counters once the page has stopped asking.
    pub fn idle(&mut self) {
        if self.sampler.is_some() && self.asked_at.is_none_or(|t| t.elapsed() >= IDLE) {
            self.sampler = None;
            self.read_at = None;
        }
    }
}

/// Clamps a counter value into 0..=100 (Windows can report a little over).
fn pct(v: f64) -> f64 {
    if v.is_finite() { v.clamp(0.0, 100.0) } else { 0.0 }
}

/// Rounds to one decimal, so the JSON stays short.
fn tidy(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

/// "_Total" and per-core values from "\Processor Information(*)" ("0,3")
/// or "\Processor(*)" ("3") instances.
fn cpu_from(items: &[(String, f64)]) -> (Option<f64>, Vec<f64>) {
    let total = items.iter().find(|(n, _)| n == "_Total").map(|(_, v)| tidy(pct(*v)));
    let mut cores: Vec<((u32, u32), f64)> = items
        .iter()
        .filter_map(|(n, v)| {
            let key = match n.split_once(',') {
                Some((g, c)) => (g.parse().ok()?, c.parse().ok()?),
                None => (0, n.parse().ok()?),
            };
            Some((key, tidy(pct(*v))))
        })
        .collect();
    cores.sort_by_key(|(k, _)| *k);
    (total, cores.into_iter().map(|(_, v)| v).collect())
}

/// Disks from "\PhysicalDisk(*)\% Idle Time" and "Disk Bytes/sec", whose
/// instances are like "0 C: D:" (skipping "_Total").
fn disks_from(idle: &[(String, f64)], bytes: &[(String, f64)]) -> Vec<Disk> {
    let mut out: Vec<(u32, Disk)> = idle
        .iter()
        .filter_map(|(n, v)| {
            let (num, letters) = n.split_once(' ').unwrap_or((n, ""));
            let num: u32 = num.parse().ok()?;
            let letters = letters.split_whitespace().collect::<Vec<_>>().join(" ");
            let name = if letters.is_empty() { format!("Disk {num}") } else { format!("Disk {num} ({letters})") };
            let b = bytes.iter().find(|(bn, _)| bn == n).map_or(0.0, |(_, b)| b.max(0.0).round());
            Some((num, Disk { name, busy: tidy(pct(100.0 - v)), bytes: b }))
        })
        .collect();
    out.sort_by_key(|(n, _)| *n);
    out.into_iter().map(|(_, d)| d).collect()
}

/// The engine types shown by name, in this order (others still count
/// towards the GPU total).
const ENGINES: &[(&str, &str)] = &[("3D", "3D"), ("VideoDecode", "Video Decode"), ("VideoProcessing", "Video Processing"), ("Copy", "Copy"), ("VideoEncode", "Video Encode")];

/// The GPU from "\GPU Engine(*)\Utilization Percentage", whose instances are
/// one per process and engine, like
/// "pid_1234_luid_0x00000000_0x0000D1B5_phys_0_eng_0_engtype_3D". As in Task
/// Manager: each engine adds up over processes, a GPU is as busy as its
/// busiest engine, and an engine type (a GPU can have several Copy or Video
/// Decode engines) as busy as its busiest engine of that type. Shown is the
/// GPU whose 3D engine is busiest (the one drawing the tables), then the
/// busiest overall.
fn gpu_from(items: &[(String, f64)]) -> (Option<f64>, Vec<Named>) {
    use std::collections::BTreeMap;
    // adapter (luid and phys) -> engine (eng_N) -> (type, sum over processes)
    let mut gpus: BTreeMap<&str, BTreeMap<&str, (&str, f64)>> = BTreeMap::new();
    for (n, v) in items {
        let Some((_, rest)) = n.split_once("_luid_") else { continue };
        let Some((adapter, rest)) = rest.split_once("_eng_") else { continue };
        let Some((eng, engtype)) = rest.split_once("_engtype_") else { continue };
        let e = gpus.entry(adapter).or_default().entry(eng).or_insert((engtype, 0.0));
        if v.is_finite() && *v > 0.0 {
            e.1 += v;
        }
    }
    let of_type = |engines: &BTreeMap<&str, (&str, f64)>, t: &str| engines.values().filter(|(et, _)| *et == t).fold(None, |m: Option<f64>, (_, v)| Some(m.unwrap_or(0.0).max(pct(*v))));
    let busiest = |engines: &BTreeMap<&str, (&str, f64)>| engines.values().fold(0.0f64, |m, (_, v)| m.max(pct(*v)));
    let rank = |e: &BTreeMap<&str, (&str, f64)>| (of_type(e, "3D").unwrap_or(0.0), busiest(e));
    let Some(engines) = gpus.values().max_by(|a, b| {
        let (a, b) = (rank(a), rank(b));
        a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1))
    }) else {
        return (None, Vec::new());
    };
    let named = ENGINES.iter().filter_map(|(key, label)| of_type(engines, key).map(|v| Named { name: (*label).into(), pct: tidy(v) })).collect();
    (Some(tidy(busiest(engines))), named)
}

#[cfg(windows)]
mod pdh {
    use super::{Stats, cpu_from, disks_from, gpu_from};
    use windows_sys::Win32::System::Performance::*;

    /// One open PDH query with its counters.
    pub struct Sampler {
        query: PDH_HQUERY,
        cpu: Option<PDH_HCOUNTER>,
        disk_idle: Option<PDH_HCOUNTER>,
        disk_bytes: Option<PDH_HCOUNTER>,
        gpu: Option<PDH_HCOUNTER>,
        /// The last good reading, kept for a counter that fails one read
        /// (instances come and go between readings).
        last: Stats,
    }

    // The handles are only used under the remote's mutex, one thread at a time.
    unsafe impl Send for Sampler {}

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    impl Sampler {
        pub fn open() -> Option<Sampler> {
            let mut query: PDH_HQUERY = std::ptr::null_mut();
            if unsafe { PdhOpenQueryW(std::ptr::null(), 0, &mut query) } != 0 {
                return None;
            }
            let add = |path: &str| {
                let mut c: PDH_HCOUNTER = std::ptr::null_mut();
                let w = wide(path);
                (unsafe { PdhAddEnglishCounterW(query, w.as_ptr(), 0, &mut c) } == 0).then_some(c)
            };
            // "% Processor Utility" is what Task Manager shows (it allows for
            // turbo); older Windows only has "% Processor Time".
            let cpu = add(r"\Processor Information(*)\% Processor Utility").or_else(|| add(r"\Processor(*)\% Processor Time"));
            let sampler = Sampler {
                cpu,
                disk_idle: add(r"\PhysicalDisk(*)\% Idle Time"),
                disk_bytes: add(r"\PhysicalDisk(*)\Disk Bytes/sec"),
                gpu: add(r"\GPU Engine(*)\Utilization Percentage"),
                query,
                last: Stats { available: true, ..Stats::default() },
            };
            if sampler.cpu.is_none() && sampler.disk_idle.is_none() && sampler.gpu.is_none() {
                return None; // dropping closes the query
            }
            Some(sampler)
        }

        pub fn collect(&mut self) {
            unsafe { PdhCollectQueryData(self.query) };
        }

        pub fn sample(&mut self) -> Stats {
            self.collect();
            if let Some(v) = values(self.cpu) {
                (self.last.cpu, self.last.cores) = cpu_from(&v);
            }
            if let Some(v) = values(self.gpu) {
                (self.last.gpu, self.last.gpu_engines) = gpu_from(&v);
            }
            if let Some(idle) = values(self.disk_idle) {
                self.last.disks = disks_from(&idle, &values(self.disk_bytes).unwrap_or_default());
            }
            self.last.clone()
        }
    }

    impl Drop for Sampler {
        fn drop(&mut self) {
            unsafe { PdhCloseQuery(self.query) };
        }
    }

    /// Every instance's value of a wildcard counter; `None` if this read
    /// failed, empty if the counter isn't there.
    fn values(counter: Option<PDH_HCOUNTER>) -> Option<Vec<(String, f64)>> {
        let Some(counter) = counter else { return Some(Vec::new()) };
        let item = std::mem::size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>();
        // The instance list can grow between asking for the size and reading.
        for _ in 0..3 {
            let (mut bytes, mut count) = (0u32, 0u32);
            let status = unsafe { PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut bytes, &mut count, std::ptr::null_mut()) };
            if status != PDH_MORE_DATA || bytes == 0 {
                return None;
            }
            // The names follow the items in the same buffer; items need
            // their own alignment, so the buffer is made of items.
            let mut buf: Vec<PDH_FMT_COUNTERVALUE_ITEM_W> = Vec::with_capacity((bytes as usize).div_ceil(item));
            let status = unsafe { PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut bytes, &mut count, buf.as_mut_ptr()) };
            if status == PDH_MORE_DATA {
                continue;
            }
            if status != 0 {
                return None;
            }
            unsafe { buf.set_len((count as usize).min(buf.capacity())) };
            return Some(
                buf.iter()
                    .filter(|i| i.FmtValue.CStatus == PDH_CSTATUS_VALID_DATA || i.FmtValue.CStatus == PDH_CSTATUS_NEW_DATA)
                    .map(|i| (name_of(i.szName), unsafe { i.FmtValue.Anonymous.doubleValue }))
                    .collect(),
            );
        }
        None
    }

    fn name_of(p: windows_sys::core::PWSTR) -> String {
        if p.is_null() {
            return String::new();
        }
        let mut len = 0;
        while unsafe { *p.add(len) } != 0 {
            len += 1;
        }
        String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, len) })
    }
}

#[cfg(not(windows))]
mod pdh {
    use super::Stats;

    pub struct Sampler;

    impl Sampler {
        pub fn open() -> Option<Sampler> {
            None
        }
        pub fn collect(&mut self) {}
        pub fn sample(&mut self) -> Stats {
            Stats::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(list: &[(&str, f64)]) -> Vec<(String, f64)> {
        list.iter().map(|(n, v)| (n.to_string(), *v)).collect()
    }

    #[test]
    fn cores_in_order() {
        let (total, cores) = cpu_from(&items(&[("_Total", 12.34), ("0,_Total", 12.0), ("0,10", 5.0), ("0,2", 104.0), ("0,0", 1.0)]));
        assert_eq!(total, Some(12.3));
        assert_eq!(cores, vec![1.0, 100.0, 5.0]);
        let (total, cores) = cpu_from(&items(&[("1", 20.0), ("0", 10.0), ("_Total", 15.0)]));
        assert_eq!((total, cores), (Some(15.0), vec![10.0, 20.0]));
    }

    #[test]
    fn disks_named_like_task_manager() {
        let d = disks_from(&items(&[("_Total", 50.0), ("1 D:", 90.0), ("0 C: E:", 30.0), ("2", 100.0)]), &items(&[("0 C: E:", 2_000_000.4)]));
        let names: Vec<_> = d.iter().map(|d| (d.name.as_str(), d.busy, d.bytes)).collect();
        assert_eq!(names, vec![("Disk 0 (C: E:)", 70.0, 2_000_000.0), ("Disk 1 (D:)", 10.0, 0.0), ("Disk 2", 0.0, 0.0)]);
    }

    #[test]
    fn gpu_is_its_busiest_engine() {
        let a = "luid_0x00000000_0x0000D1B5_phys_0";
        let b = "luid_0x00000000_0x0000AAAA_phys_0";
        let list = items(&[
            (&format!("pid_10_{a}_eng_0_engtype_3D"), 20.0),
            (&format!("pid_11_{a}_eng_0_engtype_3D"), 15.0),
            // Two Video Decode engines: the busier one counts, not the sum.
            (&format!("pid_11_{a}_eng_4_engtype_VideoDecode"), 12.0),
            (&format!("pid_11_{a}_eng_5_engtype_VideoDecode"), 9.0),
            (&format!("pid_11_{a}_eng_7_engtype_Compute_0"), 0.0),
            // Another GPU, busier overall but with less 3D work.
            (&format!("pid_12_{b}_eng_0_engtype_3D"), 3.0),
            (&format!("pid_12_{b}_eng_2_engtype_VideoDecode"), 60.0),
            ("junk", 99.0),
        ]);
        let (gpu, engines) = gpu_from(&list);
        assert_eq!(gpu, Some(35.0));
        assert_eq!(engines, vec![Named { name: "3D".into(), pct: 35.0 }, Named { name: "Video Decode".into(), pct: 12.0 }]);
        assert_eq!(gpu_from(&[]), (None, vec![]));
    }

    #[test]
    fn reads_the_counters() {
        let mut load = Load::default();
        let s = load.read();
        // Windows always has CPU counters; elsewhere there are none.
        assert_eq!(s.available, cfg!(windows));
        assert_eq!(!s.cores.is_empty(), cfg!(windows));
        assert_eq!(load.read(), s, "a second read straight away reuses the first");
        load.asked_at = Instant::now().checked_sub(IDLE);
        load.idle();
        assert!(load.sampler.is_none());
    }
}
