//! `--benchmark`: records how long every workload of the app takes and writes the data to a folder.
//!
//! Usage:
//!   votv-model-manager --benchmark                 writes to %LocalAppData%\VotV Model Manager\benchmark\run-<UTC time>\
//!   votv-model-manager --benchmark=<folder>        writes to <folder> (created if needed)
//!
//! Use the app normally (open models, click tiles, drop .glb files, change converter options, regenerate icons,
//! export, import, ...), close it, and send the whole run folder back. The folder explains itself (`README.txt`).
//!
//! Without the flag nothing is recorded: every hook below starts with one relaxed atomic load and returns.
//!
//! How it is wired (so this file stays the only place with logic):
//! - Other files call `benchmark::sys("..")` (a system / handler that runs every frame or per event: aggregated, and
//!   written to `events.csv` only when it takes 1 ms or more) and `benchmark::work("..")` (a discrete workload:
//!   every run is written to `events.csv`). Both return a guard that records the time when it is dropped.
//! - `BenchmarkPlugin` adds frame timing (`frames.csv`), entity / asset counts (`ecs.csv`) and the icon batch probe.
//! - A sampler thread writes process CPU and memory once a second (`samples.csv`) and flushes the files.
//! - `summary.txt` / `summary.csv` hold count, total, mean, min, median, p95, p99 and max of every label.
//!
//! The converter crate is not touched: its stages are timed from the progress hook the app already passes to it.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use bevy::prelude::*;
use bevy::render::renderer::RenderAdapterInfo;

use crate::icons::IconPipe;
use crate::{Layout, Library};

/// Spans created with `sys` are written to `events.csv` only from this duration on (they are always aggregated).
const SYS_EVENT_MIN_MS: f64 = 1.0;
/// Samples kept per label for the percentiles (older ones are replaced at random once the cap is reached).
const SAMPLE_CAP: usize = 10_000;
/// How often the sampler thread wakes up (CPU / memory row), and how many wake-ups between file flushes.
const SAMPLE_EVERY: Duration = Duration::from_millis(1000);
const FLUSH_EVERY_TICKS: u64 = 2;
/// The summary is recomputed every this many flushes (it sorts every label's samples), and at exit.
const SUMMARY_EVERY_FLUSHES: u64 = 5;
/// A frame whose real delta is longer than this is an idle gap (the app redraws on demand), not a slow frame.
const IDLE_GAP_MS: f64 = 250.0;

static ENABLED: AtomicBool = AtomicBool::new(false);
static FRAMES: AtomicU64 = AtomicU64::new(0);
static T0: OnceLock<Instant> = OnceLock::new();
static BENCH: OnceLock<Mutex<Bench>> = OnceLock::new();

/// True when the app was started with `--benchmark`.
#[inline]
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

// ---- statistics ------------------------------------------------------------------------------------------

#[derive(Clone, Default)]
struct Stat {
    count: u64,
    total: f64,
    min: f64,
    max: f64,
    samples: Vec<f32>,
    rng: u64,
}

impl Stat {
    fn add(&mut self, v: f64) {
        if self.count == 0 {
            self.min = v;
            self.max = v;
            self.rng = 0x9E37_79B9_7F4A_7C15;
        }
        self.count += 1;
        self.total += v;
        self.min = self.min.min(v);
        self.max = self.max.max(v);
        if self.samples.len() < SAMPLE_CAP {
            self.samples.push(v as f32);
        } else {
            // xorshift64: a cheap random slot, so a long session keeps a spread of samples, not only the first ones
            self.rng ^= self.rng << 13;
            self.rng ^= self.rng >> 7;
            self.rng ^= self.rng << 17;
            let slot = (self.rng % self.count) as usize;
            if slot < SAMPLE_CAP {
                self.samples[slot] = v as f32;
            }
        }
    }
}

/// Percentile (0..=100) of an ascending slice, nearest rank.
fn pct(sorted: &[f32], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1] as f64
}

struct Row {
    label: String,
    count: u64,
    total: f64,
    mean: f64,
    min: f64,
    p50: f64,
    p95: f64,
    p99: f64,
    max: f64,
}

fn row_of(label: &str, s: &Stat) -> Row {
    let mut v = s.samples.clone();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    Row {
        label: label.to_string(),
        count: s.count,
        total: s.total,
        mean: if s.count > 0 { s.total / s.count as f64 } else { 0.0 },
        min: s.min,
        p50: pct(&v, 50.0),
        p95: pct(&v, 95.0),
        p99: pct(&v, 99.0),
        max: s.max,
    }
}

// ---- global state ----------------------------------------------------------------------------------------

struct Bench {
    dir: PathBuf,
    times: BTreeMap<String, Stat>,  // milliseconds
    values: BTreeMap<String, Stat>, // plain numbers (counts, sizes, ...)
    counters: BTreeMap<String, u64>,
    marks: HashMap<(&'static str, u64), Instant>,
    stages: HashMap<(&'static str, u64), (Instant, String)>,
    events: String,
    frames: String,
    samples: String,
    ecs: String,
    sysinfo: String,
    idle_cpu_sum: f64,
    idle_ticks: u64,
    busy_cpu_sum: f64,
    busy_ticks: u64,
    peak_rss_mb: f64,
    flushes: u64,
}

fn lock() -> Option<std::sync::MutexGuard<'static, Bench>> {
    BENCH
        .get()
        .map(|m| m.lock().unwrap_or_else(|e| e.into_inner()))
}

fn ms_since_start(t: Instant) -> f64 {
    let t0 = T0.get().copied().unwrap_or(t);
    t.saturating_duration_since(t0).as_secs_f64() * 1000.0
}

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\"").replace(['\n', '\r'], " "))
    } else {
        s.to_string()
    }
}

fn thread_label() -> String {
    let t = std::thread::current();
    match t.name() {
        Some(n) => n.to_string(),
        None => format!("{:?}", t.id()).replace("ThreadId", "t"),
    }
}

/// Digits become `#` so that "Baking atlas 3/10" and "Baking atlas 4/10" are one stage.
fn normalize_stage(name: &str) -> String {
    let s: String = name
        .trim()
        .chars()
        .map(|c| if c.is_ascii_digit() { '#' } else { c })
        .take(60)
        .collect();
    s.replace(',', ";")
}

// ---- recording API ---------------------------------------------------------------------------------------

struct Active {
    label: &'static str,
    t0: Instant,
    always: bool,
    detail: String,
}

/// Records the elapsed time when dropped. Create it with `sys` or `work`, keep it in a `let _b = ...;` binding.
#[must_use = "bind the guard to a variable; it measures until it is dropped"]
pub struct BenchSpan(Option<Active>);

impl BenchSpan {
    /// Attach a short description (model name, sizes, ...). The closure only runs when benchmarking is on.
    pub fn detail_with(&mut self, f: impl FnOnce() -> String) -> &mut Self {
        if let Some(a) = self.0.as_mut() {
            a.detail = f();
        }
        self
    }
}

impl Drop for BenchSpan {
    fn drop(&mut self) {
        if let Some(a) = self.0.take() {
            record_span(a.label, a.t0, a.always, &a.detail);
        }
    }
}

/// A system or event handler that runs often: aggregated always, one `events.csv` row only when it took 1 ms or more.
#[inline]
pub fn sys(label: &'static str) -> BenchSpan {
    span(label, false)
}

/// A discrete workload (a load, a scan, a conversion, a save): every run gets an `events.csv` row.
#[inline]
pub fn work(label: &'static str) -> BenchSpan {
    span(label, true)
}

#[inline]
fn span(label: &'static str, always: bool) -> BenchSpan {
    if !enabled() {
        return BenchSpan(None);
    }
    BenchSpan(Some(Active {
        label,
        t0: Instant::now(),
        always,
        detail: String::new(),
    }))
}

/// `Some(now)` while benchmarking, else `None`: carry it across a callback and finish with `since`.
#[inline]
pub fn now() -> Option<Instant> {
    enabled().then(Instant::now)
}

/// A named point of the launch (`startup.phase` rows in `events.csv`, time in `t_ms` since `main` began).
pub fn phase(name: &'static str) {
    event("startup.phase", || name.to_string());
}

/// Add it to the plugin list to mark the moment every plugin before it has been built.
pub struct PhaseMark(pub &'static str);

impl Plugin for PhaseMark {
    fn build(&self, _app: &mut App) {
        phase(self.0);
    }

    fn is_unique(&self) -> bool {
        false
    }
}

/// Record the time since `t0` (from `now()`) as a workload.
pub fn since(label: &'static str, t0: Option<Instant>, detail: impl FnOnce() -> String) {
    if let Some(t0) = t0 {
        record_span(label, t0, true, &detail());
    }
}

fn record_span(label: &str, t0: Instant, always: bool, detail: &str) {
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    let Some(mut b) = lock() else { return };
    match b.times.get_mut(label) {
        Some(s) => s.add(ms),
        None => {
            let mut s = Stat::default();
            s.add(ms);
            b.times.insert(label.to_string(), s);
        }
    }
    if always || ms >= SYS_EVENT_MIN_MS {
        let line = format!(
            "{:.3},{},{},{:.3},{}\n",
            ms_since_start(t0),
            csv_field(&thread_label()),
            csv_field(label),
            ms,
            csv_field(detail)
        );
        b.events.push_str(&line);
    }
}

/// A number worth keeping statistics of (triangle count, texture bytes, frames per icon, ...). Also one `events.csv` row
/// (`dur_ms` is empty, the value is in `detail` as `value=<n>`).
pub fn note(label: &'static str, value: f64, detail: impl FnOnce() -> String) {
    if !enabled() {
        return;
    }
    let detail = detail();
    let Some(mut b) = lock() else { return };
    match b.values.get_mut(label) {
        Some(s) => s.add(value),
        None => {
            let mut s = Stat::default();
            s.add(value);
            b.values.insert(label.to_string(), s);
        }
    }
    let line = format!(
        "{:.3},{},{},,{}\n",
        ms_since_start(Instant::now()),
        csv_field(&thread_label()),
        csv_field(label),
        csv_field(&format!("value={value} {detail}"))
    );
    b.events.push_str(&line);
}

/// Add to a plain counter (shown in `summary.txt`).
pub fn count(label: &'static str, n: u64) {
    if !enabled() {
        return;
    }
    if let Some(mut b) = lock() {
        *b.counters.entry(label.to_string()).or_insert(0) += n;
    }
}

/// A point in time (a click, a drop, a request): one `events.csv` row with an empty duration.
pub fn event(label: &'static str, detail: impl FnOnce() -> String) {
    if !enabled() {
        return;
    }
    let detail = detail();
    let Some(mut b) = lock() else { return };
    let line = format!(
        "{:.3},{},{},,{}\n",
        ms_since_start(Instant::now()),
        csv_field(&thread_label()),
        csv_field(label),
        csv_field(&detail)
    );
    b.events.push_str(&line);
}

/// Start a stopwatch that ends elsewhere (`mark_end` with the same label and key), e.g. click -> model on screen.
/// A second start for the same label and key is ignored: the first one counts (the user waited since then).
pub fn mark_start(label: &'static str, key: u64) {
    if !enabled() {
        return;
    }
    if let Some(mut b) = lock() {
        b.marks.entry((label, key)).or_insert_with(Instant::now);
    }
}

/// Finish a `mark_start`; records the time as a workload. Does nothing when there is no matching start.
pub fn mark_end(label: &'static str, key: u64, detail: impl FnOnce() -> String) {
    if !enabled() {
        return;
    }
    let t0 = lock().and_then(|mut b| b.marks.remove(&(label, key)));
    if let Some(t0) = t0 {
        record_span(label, t0, true, &detail());
    }
}

/// Forget a `mark_start` that will never end (a cancelled job).
pub fn mark_cancel(label: &'static str, key: u64) {
    if !enabled() {
        return;
    }
    if let Some(mut b) = lock() {
        b.marks.remove(&(label, key));
    }
}

/// The converter reports its progress as a changing message. Each call closes the previous stage of `(group, key)` and
/// opens the one called `name` (digits in the name are folded, see `normalize_stage`). Label: `<group>.stage.<name>`.
pub fn stage(group: &'static str, key: u64, name: &str) {
    if !enabled() {
        return;
    }
    let norm = normalize_stage(name);
    let prev = lock().and_then(|mut b| {
        if b.stages.get(&(group, key)).map(|(_, n)| n.as_str()) == Some(norm.as_str()) {
            return None; // the same stage again (a fraction update): keep it running
        }
        b.stages.insert((group, key), (Instant::now(), norm.clone()))
    });
    if let Some((t0, n)) = prev {
        record_span(&format!("{group}.stage.{n}"), t0, true, "");
    }
}

/// Close the last open stage of `(group, key)`.
pub fn stage_end(group: &'static str, key: u64) {
    if !enabled() {
        return;
    }
    let prev = lock().and_then(|mut b| b.stages.remove(&(group, key)));
    if let Some((t0, n)) = prev {
        record_span(&format!("{group}.stage.{n}"), t0, true, "");
    }
}

// ---- process CPU and memory ------------------------------------------------------------------------------

struct Snap {
    cpu_secs: f64,
    rss_mb: f64,
    peak_rss_mb: f64,
    pagefile_mb: f64,
    page_faults: u64,
}

#[cfg(windows)]
mod os {
    use super::Snap;

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct FileTime {
        lo: u32,
        hi: u32,
    }

    /// PROCESS_MEMORY_COUNTERS
    #[repr(C)]
    #[derive(Default)]
    struct MemCounters {
        cb: u32,
        page_faults: u32,
        peak_working_set: usize,
        working_set: usize,
        quota_peak_paged_pool: usize,
        quota_paged_pool: usize,
        quota_peak_non_paged_pool: usize,
        quota_non_paged_pool: usize,
        pagefile: usize,
        peak_pagefile: usize,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> isize;
        fn GetSystemTimeAsFileTime(file_time: *mut FileTime);
        fn GetProcessTimes(
            process: isize,
            creation: *mut FileTime,
            exit: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> i32;
        fn K32GetProcessMemoryInfo(process: isize, counters: *mut MemCounters, cb: u32) -> i32;
    }

    fn secs(t: FileTime) -> f64 {
        (((t.hi as u64) << 32) | t.lo as u64) as f64 / 1.0e7 // 100 ns units
    }

    /// Milliseconds since the OS created this process (includes loading the exe, before `main`).
    pub fn process_age_ms() -> Option<f64> {
        unsafe {
            let h = GetCurrentProcess();
            let (mut c, mut e, mut k, mut u) = (
                FileTime::default(),
                FileTime::default(),
                FileTime::default(),
                FileTime::default(),
            );
            if GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u) == 0 {
                return None;
            }
            let mut now = FileTime::default();
            GetSystemTimeAsFileTime(&mut now);
            Some(((secs(now) - secs(c)) * 1000.0).max(0.0))
        }
    }

    /// CPU seconds (kernel + user, all threads) this process has used so far; cheap enough to call every frame.
    pub fn cpu_secs() -> Option<f64> {
        unsafe {
            let h = GetCurrentProcess();
            let (mut c, mut e, mut k, mut u) = (
                FileTime::default(),
                FileTime::default(),
                FileTime::default(),
                FileTime::default(),
            );
            if GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u) == 0 {
                return None;
            }
            Some(secs(k) + secs(u))
        }
    }

    pub fn snap() -> Option<Snap> {
        unsafe {
            let h = GetCurrentProcess();
            let (mut c, mut e, mut k, mut u) = (
                FileTime::default(),
                FileTime::default(),
                FileTime::default(),
                FileTime::default(),
            );
            if GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u) == 0 {
                return None;
            }
            let mut m = MemCounters {
                cb: std::mem::size_of::<MemCounters>() as u32,
                ..Default::default()
            };
            if K32GetProcessMemoryInfo(h, &mut m, m.cb) == 0 {
                return None;
            }
            let mb = |b: usize| b as f64 / (1024.0 * 1024.0);
            Some(Snap {
                cpu_secs: secs(k) + secs(u),
                rss_mb: mb(m.working_set),
                peak_rss_mb: mb(m.peak_working_set),
                pagefile_mb: mb(m.pagefile),
                page_faults: m.page_faults as u64,
            })
        }
    }
}

#[cfg(not(windows))]
mod os {
    use super::Snap;

    pub fn process_age_ms() -> Option<f64> {
        None
    }

    pub fn cpu_secs() -> Option<f64> {
        snap().map(|s| s.cpu_secs)
    }

    pub fn snap() -> Option<Snap> {
        let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
        let rest = &stat[stat.rfind(')')? + 1..];
        let f: Vec<&str> = rest.split_whitespace().collect();
        // after the ")": state is field 3, so utime (14) is index 11 and stime (15) index 12
        let ticks: f64 = f.get(11)?.parse::<f64>().ok()? + f.get(12)?.parse::<f64>().ok()?;
        let faults: u64 = f.get(7).and_then(|s| s.parse().ok()).unwrap_or(0);
        let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
        let pages: f64 = statm.split_whitespace().nth(1)?.parse().ok()?;
        let virt: f64 = statm.split_whitespace().next()?.parse().ok()?;
        let rss = pages * 4096.0 / (1024.0 * 1024.0);
        Some(Snap {
            cpu_secs: ticks / 100.0,
            rss_mb: rss,
            peak_rss_mb: rss,
            pagefile_mb: virt * 4096.0 / (1024.0 * 1024.0),
            page_faults: faults,
        })
    }
}

// ---- files -----------------------------------------------------------------------------------------------

fn append(path: &Path, text: &str) {
    if text.is_empty() {
        return;
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = f.write_all(text.as_bytes());
    }
}

/// UTC date and time from Unix seconds (civil-from-days, no date crate in the project).
fn utc(secs: u64) -> (i64, u32, u32, u32, u32, u32) {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    (
        y,
        m as u32,
        d as u32,
        (rem / 3600) as u32,
        ((rem % 3600) / 60) as u32,
        (rem % 60) as u32,
    )
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn stamp(secs: u64) -> String {
    let (y, mo, d, h, mi, s) = utc(secs);
    format!("{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}")
}

const README: &str = "\
VotV Model Manager benchmark run
================================
Send this whole folder back for analysis. Nothing in it contains your models, only names, sizes and timings.

summary.txt    Start here. Every measured label: count, total, mean, min, median (p50), p95, p99, max, in milliseconds
               (labels under 'values' are plain numbers: triangles, bytes, frames, ...), plus frame statistics and idle CPU.
summary.csv    The same table for spreadsheets.
events.csv     One row per workload run, in time order: t_ms (start, ms since the app started), thread, label, dur_ms,
               detail. Rows with an empty dur_ms are points in time (clicks, requests) or numbers (value=...).
               Per-frame systems (labels starting with 'sys.') only appear here when a run took 1 ms or more.
frames.csv     One row per rendered frame: t_ms, frame, delta_ms (real time since the previous frame; above 250 ms it is
               an idle gap because the app only redraws on demand), main_ms (time the main thread spent in the frame's
               update, First to Last schedule), mode (list or converter), cpu_ms (CPU time all threads of the app used since the
               previous frame; a long delta_ms with a high cpu_ms is a freeze, with a low cpu_ms it is idle waiting).
               Freezes are also listed in events.csv as frame.stall rows and counted as frame.STALLS in summary.txt.
samples.csv    Once a second: process CPU in percent of one core (100 = one core busy), memory (working set, peak, page
               file in MB), page faults, and how many frames were drawn in that second (0 = idle).
ecs.csv        Every 2 s of frames: entity count, number of images / meshes / materials in memory, models in the library,
               icons done / total.
system.txt     Machine, build and library facts.

Label families: startup.* (startup.phase rows mark the launch: main_begin, default_plugins_built, all_plugins_built,
first_update_begin, frame_3_done, window_shown; startup.process_age_at_main_ms is the time before main() began)  lib.* (folder scan, tiles)  icon.*  model.* (OBJ / texture loading)  tex.*  scene.* (mesh and
material building)  preview.*  convert.* (the converter job, its stages from the progress hook, copying, installing)
export.* import.* menu.*  sys.* (per-frame systems and event handlers)  frame.*

Tips for a useful run: open a few small and a few big models, click through many tiles quickly, drop a .glb and change a
few options (sliders and toggles), regenerate some icons, export and import, then close the app normally (closing writes
the final summary). Note which GPU you have and roughly how many models are in the library.
";

fn write_system_info(b: &mut Bench, root: Option<&Path>, models: Option<usize>) {
    let mut s = String::new();
    let _ = writeln!(s, "app: votv-model-manager {}", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(s, "build number: {:?}", crate::build_number());
    let _ = writeln!(
        s,
        "profile: {}",
        if cfg!(debug_assertions) { "debug (NOT representative)" } else { "release" }
    );
    let _ = writeln!(s, "bevy: 0.16.1 (pinned)");
    let _ = writeln!(s, "os: {} {}", std::env::consts::OS, std::env::consts::ARCH);
    let _ = writeln!(
        s,
        "logical cpus: {}",
        std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0)
    );
    if let Ok(exe) = std::env::current_exe() {
        let size = std::fs::metadata(&exe).map(|m| m.len()).unwrap_or(0);
        let _ = writeln!(s, "exe: {} ({size} bytes)", exe.display());
    }
    let _ = writeln!(s, "args: {:?}", std::env::args().skip(1).collect::<Vec<_>>());
    let _ = writeln!(s, "started (UTC): {}", stamp(unix_now()));
    if let Some(r) = root {
        let _ = writeln!(s, "library: {}", r.display());
    }
    if let Some(n) = models {
        let _ = writeln!(s, "models at start: {n}");
    }
    let _ = writeln!(s, "(model count over time: see ecs.csv)");
    b.sysinfo = s.clone();
    let _ = std::fs::write(b.dir.join("system.txt"), s);
}

/// Add a line to `system.txt` (and keep it for the next rewrite).
fn append_system_info(line: &str) {
    if let Some(mut b) = lock() {
        b.sysinfo.push_str(line);
        b.sysinfo.push('\n');
        let _ = std::fs::write(b.dir.join("system.txt"), &b.sysinfo);
    }
}

/// Called once the folder to open is known.
pub fn set_library(root: &Path) {
    if !enabled() {
        return;
    }
    if let Some(mut b) = lock() {
        write_system_info(&mut b, Some(root), None);
    }
}

/// Parse `--benchmark[=<folder>]`; when present, create the run folder and start recording. Returns whether it is on.
pub fn init(args: &[String]) -> bool {
    let Some(arg) = args
        .iter()
        .find(|a| a.as_str() == "--benchmark" || a.starts_with("--benchmark="))
    else {
        return false;
    };
    let _ = T0.set(Instant::now());
    let custom = arg.strip_prefix("--benchmark=").filter(|s| !s.is_empty());
    let base = crate::settings_dir().join("benchmark");
    let dir = match custom {
        Some(p) => PathBuf::from(p),
        None => base.join(format!("run-{}", stamp(unix_now()))),
    };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("[benchmark] cannot create {}: {e}; benchmark is off", dir.display());
        return false;
    }
    let _ = std::fs::write(dir.join("README.txt"), README);
    for (file, header) in [
        ("events.csv", "t_ms,thread,label,dur_ms,detail\n"),
        ("frames.csv", "t_ms,frame,delta_ms,main_ms,mode,cpu_ms\n"),
        (
            "samples.csv",
            "t_ms,cpu_pct,rss_mb,peak_rss_mb,pagefile_mb,page_faults,frames_in_interval\n",
        ),
        (
            "ecs.csv",
            "t_ms,mode,entities,images,meshes,materials,models,icons_done,icons_total\n",
        ),
    ] {
        let _ = std::fs::write(dir.join(file), header);
    }
    if custom.is_none() {
        let _ = std::fs::write(
            base.join("LAST_RUN.txt"),
            format!("{}\n", dir.display()),
        );
    }
    let mut b = Bench {
        dir: dir.clone(),
        times: BTreeMap::new(),
        values: BTreeMap::new(),
        counters: BTreeMap::new(),
        marks: HashMap::new(),
        stages: HashMap::new(),
        events: String::new(),
        frames: String::new(),
        samples: String::new(),
        ecs: String::new(),
        sysinfo: String::new(),
        idle_cpu_sum: 0.0,
        idle_ticks: 0,
        busy_cpu_sum: 0.0,
        busy_ticks: 0,
        peak_rss_mb: 0.0,
        flushes: 0,
    };
    write_system_info(&mut b, None, None);
    if BENCH.set(Mutex::new(b)).is_err() {
        return false;
    }
    ENABLED.store(true, Ordering::Relaxed);
    if let Some(ms) = os::process_age_ms() {
        // How long the process had existed when main() began: loading the exe and its DLLs, antivirus, ...
        note("startup.process_age_at_main_ms", ms, String::new);
    }
    println!("[benchmark] recording to {}", dir.display());
    println!("[benchmark] use the app, close it normally, then send that folder back");
    let _ = std::thread::Builder::new()
        .name("benchmark-sampler".into())
        .spawn(sampler);
    true
}

fn sampler() {
    let mut last_cpu: Option<(f64, Instant)> = None;
    let mut last_frames = 0u64;
    let mut tick = 0u64;
    loop {
        std::thread::sleep(SAMPLE_EVERY);
        tick += 1;
        let now = Instant::now();
        let frames = FRAMES.load(Ordering::Relaxed);
        let in_interval = frames - last_frames;
        last_frames = frames;
        if let Some(s) = os::snap() {
            let cpu_pct = match last_cpu {
                Some((c0, t0)) => {
                    let wall = now.saturating_duration_since(t0).as_secs_f64().max(1e-3);
                    ((s.cpu_secs - c0) / wall * 100.0).max(0.0)
                }
                None => 0.0,
            };
            last_cpu = Some((s.cpu_secs, now));
            if let Some(mut b) = lock() {
                let line = format!(
                    "{:.0},{:.1},{:.1},{:.1},{:.1},{},{}\n",
                    ms_since_start(now),
                    cpu_pct,
                    s.rss_mb,
                    s.peak_rss_mb,
                    s.pagefile_mb,
                    s.page_faults,
                    in_interval
                );
                b.samples.push_str(&line);
                b.peak_rss_mb = b.peak_rss_mb.max(s.peak_rss_mb);
                if in_interval == 0 {
                    b.idle_cpu_sum += cpu_pct;
                    b.idle_ticks += 1;
                } else {
                    b.busy_cpu_sum += cpu_pct;
                    b.busy_ticks += 1;
                }
            }
        }
        if tick % FLUSH_EVERY_TICKS == 0 {
            flush(false);
        }
    }
}

/// Write the buffered rows, and now and then (or at exit) the summary.
pub fn flush(final_: bool) {
    if !enabled() {
        return;
    }
    let (dir, events, frames, samples, ecs, summary_input) = {
        let Some(mut b) = lock() else { return };
        b.flushes += 1;
        let want_summary = final_ || b.flushes % SUMMARY_EVERY_FLUSHES == 0;
        let input = want_summary.then(|| {
            (
                b.times.clone(),
                b.values.clone(),
                b.counters.clone(),
                b.sysinfo.clone(),
                (b.idle_cpu_sum, b.idle_ticks, b.busy_cpu_sum, b.busy_ticks, b.peak_rss_mb),
            )
        });
        (
            b.dir.clone(),
            std::mem::take(&mut b.events),
            std::mem::take(&mut b.frames),
            std::mem::take(&mut b.samples),
            std::mem::take(&mut b.ecs),
            input,
        )
    };
    // The lock is free again: file writes and the sorting for the summary happen outside it.
    append(&dir.join("events.csv"), &events);
    append(&dir.join("frames.csv"), &frames);
    append(&dir.join("samples.csv"), &samples);
    append(&dir.join("ecs.csv"), &ecs);
    if let Some((times, values, counters, sysinfo, cpu)) = summary_input {
        write_summary(&dir, &times, &values, &counters, &sysinfo, cpu);
    }
}

fn write_summary(
    dir: &Path,
    times: &BTreeMap<String, Stat>,
    values: &BTreeMap<String, Stat>,
    counters: &BTreeMap<String, u64>,
    sysinfo: &str,
    cpu: (f64, u64, f64, u64, f64),
) {
    let mut trows: Vec<Row> = times.iter().map(|(k, s)| row_of(k, s)).collect();
    let mut vrows: Vec<Row> = values.iter().map(|(k, s)| row_of(k, s)).collect();

    let mut csv = String::from("kind,label,count,total,mean,min,p50,p95,p99,max\n");
    for (kind, rows) in [("ms", &trows), ("value", &vrows)] {
        for r in rows.iter() {
            let _ = writeln!(
                csv,
                "{kind},{},{},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3}",
                csv_field(&r.label),
                r.count,
                r.total,
                r.mean,
                r.min,
                r.p50,
                r.p95,
                r.p99,
                r.max
            );
        }
    }
    let _ = std::fs::write(dir.join("summary.csv"), csv);

    let mut txt = String::new();
    txt.push_str("VotV Model Manager benchmark summary (times in milliseconds)\n\n");
    txt.push_str(sysinfo);
    let (idle_sum, idle_n, busy_sum, busy_n, peak) = cpu;
    let _ = writeln!(txt, "\nprocess CPU (percent of one core): idle seconds {} avg {:.2}, busy seconds {} avg {:.2}",
        idle_n,
        if idle_n > 0 { idle_sum / idle_n as f64 } else { 0.0 },
        busy_n,
        if busy_n > 0 { busy_sum / busy_n as f64 } else { 0.0 });
    let _ = writeln!(txt, "peak working set: {peak:.1} MB");
    if !counters.is_empty() {
        txt.push_str("\ncounters:\n");
        for (k, v) in counters {
            let _ = writeln!(txt, "  {k:<44} {v}");
        }
    }
    let header = format!(
        "{:<46} {:>7} {:>11} {:>9} {:>9} {:>9} {:>9} {:>9} {:>9}\n",
        "label", "count", "total", "mean", "min", "p50", "p95", "p99", "max"
    );
    let line = |r: &Row| {
        format!(
            "{:<46} {:>7} {:>11.2} {:>9.3} {:>9.3} {:>9.3} {:>9.3} {:>9.3} {:>9.3}\n",
            r.label, r.count, r.total, r.mean, r.min, r.p50, r.p95, r.p99, r.max
        )
    };
    trows.sort_by(|a, b| b.total.partial_cmp(&a.total).unwrap_or(std::cmp::Ordering::Equal));
    txt.push_str("\ntimed labels, largest total first:\n");
    txt.push_str(&header);
    for r in &trows {
        txt.push_str(&line(r));
    }
    vrows.sort_by(|a, b| a.label.cmp(&b.label));
    txt.push_str("\nvalues (plain numbers):\n");
    txt.push_str(&header);
    for r in &vrows {
        txt.push_str(&line(r));
    }
    let _ = std::fs::write(dir.join("summary.txt"), txt);
}

// ---- Bevy side: frames, entity counts, icon batch, exit --------------------------------------------------

pub struct BenchmarkPlugin;

impl Plugin for BenchmarkPlugin {
    fn build(&self, app: &mut App) {
        if !enabled() {
            return;
        }
        phase("all_plugins_built");
        app.init_resource::<BenchFrame>()
            .add_systems(First, (frame_begin, gpu_info))
            .add_systems(
                Last,
                (frame_end, ecs_sample, icon_probe, exit_flush).chain(),
            );
    }
}

#[derive(Resource, Default)]
struct BenchFrame {
    begin: Option<Instant>,
    n: u64,
    frames_since_ecs: u64,
    last_cpu: Option<f64>,
}

fn frame_begin(mut f: ResMut<BenchFrame>) {
    f.begin = Some(Instant::now());
    if f.n == 0 {
        phase("first_update_begin"); // Startup systems have run by now
    }
}

/// The GPU and graphics backend in use, into `system.txt`. Tries for the first few frames (the resource may be late).
fn gpu_info(info: Option<Res<RenderAdapterInfo>>, mut tries: Local<u32>) {
    if *tries >= 5 {
        return;
    }
    *tries += 1;
    if let Some(info) = info {
        let line = format!(
            "gpu: {} | backend: {:?} | type: {:?} | driver: {} {}",
            info.0.name, info.0.backend, info.0.device_type, info.0.driver, info.0.driver_info
        );
        println!("[benchmark] {line}");
        append_system_info(&line);
        *tries = 5;
    } else if *tries == 5 {
        append_system_info("gpu: not available to the benchmark (RenderAdapterInfo is not a main-world resource here)");
    }
}

fn mode(layout: &Layout) -> &'static str {
    if layout.converter {
        "converter"
    } else {
        "list"
    }
}

fn frame_end(mut f: ResMut<BenchFrame>, time: Res<Time<Real>>, layout: Res<Layout>) {
    let Some(begin) = f.begin.take() else { return };
    let main_ms = begin.elapsed().as_secs_f64() * 1000.0;
    let delta_ms = time.delta_secs_f64() * 1000.0;
    f.n += 1;
    f.frames_since_ecs += 1;
    FRAMES.fetch_add(1, Ordering::Relaxed);
    // CPU time the whole process (every thread) used since the previous frame: tells a stall from an idle wait.
    let cpu = os::cpu_secs();
    let cpu_ms = match (cpu, f.last_cpu) {
        (Some(now), Some(before)) => Some(((now - before) * 1000.0).max(0.0)),
        _ => None,
    };
    f.last_cpu = cpu;
    let Some(mut b) = lock() else { return };
    let line = format!(
        "{:.3},{},{:.3},{:.3},{},{}\n",
        ms_since_start(Instant::now()),
        f.n,
        delta_ms,
        main_ms,
        mode(&layout),
        cpu_ms.map(|c| format!("{c:.1}")).unwrap_or_default()
    );
    b.frames.push_str(&line);
    let add = |m: &mut BTreeMap<String, Stat>, k: &str, v: f64| match m.get_mut(k) {
        Some(s) => s.add(v),
        None => {
            let mut s = Stat::default();
            s.add(v);
            m.insert(k.to_string(), s);
        }
    };
    add(&mut b.times, "frame.main_ms", main_ms);
    if delta_ms < IDLE_GAP_MS {
        add(&mut b.times, "frame.delta_ms (active frames, no idle gaps)", delta_ms);
    } else if cpu_ms.is_some_and(|c| c > delta_ms * 0.3) {
        // No frame for a long time while the process was busy: the app was frozen, not waiting for input.
        *b.counters.entry("frame.STALLS (gap over 250 ms, process busy)".into()).or_insert(0) += 1;
        add(&mut b.times, "frame.stall_ms", delta_ms);
        let start = (ms_since_start(Instant::now()) - delta_ms).max(0.0);
        let line = format!(
            "{start:.3},{},frame.stall,{delta_ms:.3},cpu_ms={:.0}\n",
            csv_field(&thread_label()),
            cpu_ms.unwrap_or(0.0)
        );
        b.events.push_str(&line);
    } else {
        *b.counters.entry("frame.idle_gaps (gap over 250 ms, process idle)".into()).or_insert(0) += 1;
    }
    for (limit, name) in [
        (8.3, "frame.main_over_8.3ms"),
        (16.7, "frame.main_over_16.7ms"),
        (33.3, "frame.main_over_33.3ms"),
        (100.0, "frame.main_over_100ms"),
    ] {
        if main_ms > limit {
            *b.counters.entry(name.into()).or_insert(0) += 1;
        }
    }
    *b.counters.entry("frame.total".into()).or_insert(0) += 1;
    if f.n == 3 {
        let line = format!(
            "{:.3},{},startup.phase,,frame_3_done\n",
            ms_since_start(Instant::now()),
            csv_field(&thread_label())
        );
        b.events.push_str(&line);
    }
    if f.n == 1 {
        let t = ms_since_start(Instant::now());
        let line = format!(
            "{t:.3},{},startup.first_frame_done,,since process start\n",
            csv_field(&thread_label())
        );
        b.events.push_str(&line);
        b.values.entry("startup.first_frame_done_ms".into()).or_default().add(t);
    }
}

fn ecs_sample(
    mut f: ResMut<BenchFrame>,
    time: Res<Time<Real>>,
    mut next: Local<f32>,
    all: Query<Entity>,
    images: Res<Assets<Image>>,
    meshes: Res<Assets<Mesh>>,
    mats: Res<Assets<StandardMaterial>>,
    lib: Res<Library>,
    layout: Res<Layout>,
    pipe: Res<IconPipe>,
) {
    let t = time.elapsed_secs();
    if t < *next {
        return;
    }
    *next = t + 2.0;
    f.frames_since_ecs = 0;
    let entities = all.iter().count();
    let line = format!(
        "{:.0},{},{},{},{},{},{},{},{}\n",
        ms_since_start(Instant::now()),
        mode(&layout),
        entities,
        images.len(),
        meshes.len(),
        mats.len(),
        lib.entries.len(),
        pipe.done,
        pipe.total
    );
    if let Some(mut b) = lock() {
        b.ecs.push_str(&line);
    }
}

#[derive(Default)]
struct IconWatch {
    total: usize,
    started: Option<Instant>,
    reported: bool,
}

/// Time from "the list's icon jobs were queued" to "every tile has its icon" (the title's `icons x/x` counter).
fn icon_probe(pipe: Res<IconPipe>, mut w: Local<IconWatch>) {
    if pipe.total != w.total || (pipe.done == 0 && w.reported) {
        w.total = pipe.total;
        w.started = Some(Instant::now());
        w.reported = false;
    }
    if pipe.total > 0 && pipe.done >= pipe.total && !w.reported {
        w.reported = true;
        if let Some(t0) = w.started {
            record_span("icon.batch_all_tiles", t0, true, &format!("{} icons", pipe.total));
        }
    }
}

fn exit_flush(mut exits: EventReader<AppExit>) {
    if exits.read().next().is_some() {
        flush(true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_use_the_nearest_rank() {
        let v: Vec<f32> = (1..=100).map(|n| n as f32).collect();
        assert_eq!(pct(&v, 50.0), 50.0);
        assert_eq!(pct(&v, 95.0), 95.0);
        assert_eq!(pct(&v, 100.0), 100.0);
        assert_eq!(pct(&[], 50.0), 0.0);
        assert_eq!(pct(&[7.0], 99.0), 7.0);
    }

    #[test]
    fn a_stat_keeps_count_total_and_extremes_beyond_the_sample_cap() {
        let mut s = Stat::default();
        for i in 0..(SAMPLE_CAP as u64 * 3) {
            s.add(i as f64);
        }
        assert_eq!(s.count, SAMPLE_CAP as u64 * 3);
        assert_eq!(s.min, 0.0);
        assert_eq!(s.max, (SAMPLE_CAP as u64 * 3 - 1) as f64);
        assert_eq!(s.samples.len(), SAMPLE_CAP);
    }

    #[test]
    fn csv_fields_are_quoted_only_when_needed() {
        assert_eq!(csv_field("plain"), "plain");
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_field("two\nlines"), "\"two lines\"");
    }

    #[test]
    fn stage_names_fold_digits() {
        assert_eq!(normalize_stage("Baking atlas 3/10"), "Baking atlas #/##");
        assert_eq!(normalize_stage("a,b"), "a;b");
    }

    #[test]
    fn utc_matches_known_dates() {
        assert_eq!(utc(0), (1970, 1, 1, 0, 0, 0));
        assert_eq!(utc(951_782_400), (2000, 2, 29, 0, 0, 0));
        assert_eq!(utc(1_794_268_800 + 3_661), (2026, 11, 10, 1, 1, 1));
        assert_eq!(stamp(1_794_268_800), "20261110-000000");
    }

    #[test]
    fn everything_is_a_no_op_while_off() {
        assert!(!enabled());
        let mut s = work("x");
        s.detail_with(|| unreachable!("the detail must not be built while off"));
        drop(s);
        assert!(now().is_none());
        phase("x");
        event("x", || unreachable!());
        note("x", 1.0, || unreachable!());
        mark_start("x", 1);
        mark_end("x", 1, || unreachable!());
        stage("x", 1, "y");
    }
}
