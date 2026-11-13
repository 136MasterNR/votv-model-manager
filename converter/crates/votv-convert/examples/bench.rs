//! Benchmark harness for the converter (Phase 0 of OPTIMIZATION_PLAN.md).
//!
//!     cargo run --release -p votv-convert --example bench -- model1.glb model2.glb --runs 3 --json results.json
//!
//! For every model and run it measures: `inspect`, a cold conversion without cache, a cold conversion with a cache, an identical repeat with the cache
//! (everything reused), and (unless `--quick`) six "one option changed" runs that each start from a freshly populated cache. For every measurement it records
//! wall time, the per-stage timings from `Report::timings`, peak and retained heap (Rust allocations, counted by a wrapping allocator), cache hits and
//! output size. A markdown summary goes to the console, everything goes to the JSON file.
//!
//! Flags: --runs N (default 3)  --out DIR (default bench_out)  --json FILE (default bench_results.json)  --quick  --no-collision  --icon

use std::alloc::{GlobalAlloc, Layout, System};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::time::Instant;

use serde::Serialize;
use votv_convert::*;

// ---- allocation counter (Rust heap only; the OS-level peak can be a little higher) ----
struct Counting;
static CUR: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = System.alloc(l);
        if !p.is_null() {
            let c = CUR.fetch_add(l.size(), Relaxed) + l.size();
            PEAK.fetch_max(c, Relaxed);
        }
        p
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        let p = System.alloc_zeroed(l);
        if !p.is_null() {
            let c = CUR.fetch_add(l.size(), Relaxed) + l.size();
            PEAK.fetch_max(c, Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        System.dealloc(p, l);
        CUR.fetch_sub(l.size(), Relaxed);
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        let q = System.realloc(p, l, new);
        if !q.is_null() {
            if new >= l.size() {
                let c = CUR.fetch_add(new - l.size(), Relaxed) + (new - l.size());
                PEAK.fetch_max(c, Relaxed);
            } else {
                CUR.fetch_sub(l.size() - new, Relaxed);
            }
        }
        q
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

fn mb(b: usize) -> f64 {
    b as f64 / 1_048_576.0
}

/// Runs `f`, returning its result, wall time (ms), peak heap above the starting level (MB) and heap still held afterwards (MB).
fn measure<T>(f: impl FnOnce() -> T) -> (T, f64, f64, f64) {
    let base = CUR.load(Relaxed);
    PEAK.store(base, Relaxed);
    let t = Instant::now();
    let r = f();
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    let peak = PEAK.load(Relaxed).saturating_sub(base);
    let held = CUR.load(Relaxed).saturating_sub(base);
    (r, ms, mb(peak), mb(held))
}

#[derive(Serialize, Clone)]
struct Row {
    model: String,
    scenario: String,
    run: usize,
    total_ms: f64,
    stage_ms: Vec<(String, f64)>,
    peak_heap_mb: f64,
    retained_heap_mb: f64,
    cache_hits: Vec<String>,
    triangles: usize,
    hulls: usize,
    atlas: String,
    output_mb: f64,
}

fn row_from_report(model: &str, scenario: &str, run: usize, r: &Report, ms: f64, peak: f64, held: f64) -> Row {
    Row {
        model: model.to_string(),
        scenario: scenario.to_string(),
        run,
        total_ms: ms,
        stage_ms: r.timings.iter().map(|t| (t.stage.clone(), t.micros as f64 / 1000.0)).collect(),
        peak_heap_mb: peak,
        retained_heap_mb: held,
        cache_hits: r.cache_hits.clone(),
        triangles: r.stats.triangles,
        hulls: r.stats.hulls,
        atlas: format!("{}x{}", r.stats.atlas_width, r.stats.atlas_height),
        output_mb: r.files.iter().map(|f| f.bytes).sum::<u64>() as f64 / 1_048_576.0,
    }
}

fn variants(base: &ConvertOptions) -> Vec<(&'static str, ConvertOptions)> {
    let mut v = Vec::new();
    let mut o = base.clone();
    o.properties.physical_material = 3;
    o.properties.indestructible = false;
    o.properties.health = 123.0;
    v.push(("warm_properties_only", o));
    let mut o = base.clone();
    o.collision_offset_x = 2.5;
    v.push(("warm_collision_offset", o));
    let mut o = base.clone();
    o.scale = base.scale * 0.5;
    v.push(("warm_scale", o));
    let mut o = base.clone();
    o.max_atlas_size = 1024;
    v.push(("warm_atlas_size", o));
    let mut o = base.clone();
    o.vhacd_resolution = base.vhacd_resolution + 16;
    v.push(("warm_vhacd_resolution", o));
    let mut o = base.clone();
    o.humanoid = !base.humanoid;
    v.push(("warm_humanoid_reload", o));
    let mut o = base.clone();
    o.png_level = PngLevel::Fast;
    v.push(("warm_png_fast", o));
    let mut o = base.clone();
    o.png_level = PngLevel::Best;
    v.push(("warm_png_best", o));
    v
}

fn bench_model(path: &Path, runs: usize, base: &ConvertOptions, quick: bool, rows: &mut Vec<Row>) -> std::result::Result<(), String> {
    let model = path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let size_mb = std::fs::metadata(path).map(|m| m.len() as f64 / 1_048_576.0).unwrap_or(0.0);
    println!("\n== {model} ({size_mb:.1} MB)");
    for run in 0..runs {
        // inspect (cold)
        let (info, ms, peak, held) = measure(|| inspect(path));
        let info = info.map_err(|e| format!("inspect: {e}"))?;
        rows.push(Row {
            model: model.clone(), scenario: "inspect".into(), run, total_ms: ms, stage_ms: vec![], peak_heap_mb: peak, retained_heap_mb: held,
            cache_hits: vec![], triangles: info.triangles, hulls: 0, atlas: String::new(), output_mb: 0.0,
        });

        // cold, no cache
        let (r, ms, peak, held) = measure(|| convert_with(base, path, &Hooks::default()));
        let r = r.map_err(|e| format!("convert: {e}"))?;
        rows.push(row_from_report(&model, "cold_no_cache", run, &r, ms, peak, held));

        // cold with a cache, then the same again (everything reused)
        let cache = ConvertCache::new();
        let hooks = || Hooks::default().with_cache(cache.clone());
        let (r, ms, peak, held) = measure(|| convert_with(base, path, &hooks()));
        let r = r.map_err(|e| format!("convert: {e}"))?;
        rows.push(row_from_report(&model, "cold_with_cache", run, &r, ms, peak, held));
        let (r, ms, peak, held) = measure(|| convert_with(base, path, &hooks()));
        let r = r.map_err(|e| format!("convert: {e}"))?;
        rows.push(row_from_report(&model, "warm_identical", run, &r, ms, peak, held));

        if !quick {
            for (label, opts) in variants(base) {
                cache.clear();
                convert_with(base, path, &hooks()).map_err(|e| format!("convert: {e}"))?; // populate, not timed
                let (r, ms, peak, held) = measure(|| convert_with(&opts, path, &hooks()));
                match r {
                    Ok(r) => rows.push(row_from_report(&model, label, run, &r, ms, peak, held)),
                    Err(e) => eprintln!("{model} {label}: {e}"),
                }
            }
        }
        cache.clear();
        println!("  run {}/{} done", run + 1, runs);
    }
    Ok(())
}

fn median<'a>(v: &[&'a Row]) -> &'a Row {
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.total_ms.partial_cmp(&b.total_ms).unwrap_or(std::cmp::Ordering::Equal));
    s[s.len() / 2]
}

fn cpu_name() -> String {
    if let Ok(s) = std::env::var("PROCESSOR_IDENTIFIER") {
        return s;
    }
    if let Ok(t) = std::fs::read_to_string("/proc/cpuinfo") {
        if let Some(l) = t.lines().find(|l| l.starts_with("model name")) {
            return l.split(':').nth(1).unwrap_or("").trim().to_string();
        }
    }
    "unknown".to_string()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (mut runs, mut out, mut json, mut quick, mut no_coll, mut icon) = (3usize, PathBuf::from("bench_out"), PathBuf::from("bench_results.json"), false, false, false);
    let mut models: Vec<PathBuf> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--runs" => { i += 1; runs = args.get(i).and_then(|s| s.parse().ok()).unwrap_or(3).max(1); }
            "--out" => { i += 1; out = PathBuf::from(args.get(i).cloned().unwrap_or_default()); }
            "--json" => { i += 1; json = PathBuf::from(args.get(i).cloned().unwrap_or_default()); }
            "--quick" => quick = true,
            "--no-collision" => no_coll = true,
            "--icon" => icon = true,
            other => models.push(PathBuf::from(other)),
        }
        i += 1;
    }
    if models.is_empty() {
        eprintln!("usage: bench MODEL.glb [MODEL.glb ...] [--runs N] [--out DIR] [--json FILE] [--quick] [--no-collision] [--icon]");
        std::process::exit(2);
    }
    if cfg!(debug_assertions) {
        eprintln!("WARNING: this is a debug build, the numbers are meaningless. Run with --release.");
    }
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0);
    println!("votv-convert {} | {} {} | {} logical cores | {}", env!("CARGO_PKG_VERSION"), std::env::consts::OS, std::env::consts::ARCH, cores, cpu_name());

    let base = ConvertOptions { output_dir: out, write_sidecar: true, write_icon: icon, generate_collision: !no_coll, ..Default::default() };
    let mut rows: Vec<Row> = Vec::new();
    for m in &models {
        if let Err(e) = bench_model(m, runs, &base, quick, &mut rows) {
            eprintln!("{}: skipped ({e})", m.display());
        }
    }

    // ---- markdown summary (median run per scenario) ----
    println!("\n### Summary (median of {runs} runs; times in ms)\n");
    println!("| model | scenario | median ms | min ms | peak heap MB | retained MB | reused | stages (median run) |");
    println!("|---|---|---:|---:|---:|---:|---|---|");
    let mut seen: Vec<(String, String)> = Vec::new();
    for r in &rows {
        let key = (r.model.clone(), r.scenario.clone());
        if seen.contains(&key) {
            continue;
        }
        seen.push(key.clone());
        let group: Vec<&Row> = rows.iter().filter(|x| x.model == key.0 && x.scenario == key.1).collect();
        let med = median(&group);
        let min = group.iter().map(|x| x.total_ms).fold(f64::MAX, f64::min);
        let stages = med.stage_ms.iter().filter(|(s, _)| s != "total").map(|(s, ms)| format!("{s} {ms:.0}")).collect::<Vec<_>>().join(", ");
        println!("| {} | {} | {:.0} | {:.0} | {:.0} | {:.0} | {} | {} |", key.0, key.1, med.total_ms, min, med.peak_heap_mb, med.retained_heap_mb, med.cache_hits.join("+"), stages);
    }

    // which files were measured: name, size and modification time, so two result files can be compared knowing the input did not change
    let inputs: Vec<serde_json::Value> = models
        .iter()
        .map(|m| {
            let md = std::fs::metadata(m).ok();
            serde_json::json!({
                "path": m.to_string_lossy(),
                "bytes": md.as_ref().map(|x| x.len()).unwrap_or(0),
                "modified_unix": md.as_ref().and_then(|x| x.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0),
            })
        })
        .collect();
    let doc = serde_json::json!({
        "tool": "votv-convert bench",
        "inputs": inputs,
        "env_votv_convert_threads": std::env::var("VOTV_CONVERT_THREADS").ok(),
        "version": env!("CARGO_PKG_VERSION"),
        "debug_build": cfg!(debug_assertions),
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "logical_cores": cores,
        "cpu": cpu_name(),
        "runs": runs,
        "quick": quick,
        "rows": rows,
    });
    match std::fs::write(&json, serde_json::to_string_pretty(&doc).unwrap_or_default()) {
        Ok(()) => println!("\nwrote {}", json.display()),
        Err(e) => eprintln!("could not write {}: {e}", json.display()),
    }
}
