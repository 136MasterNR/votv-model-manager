//! `--diagnose <model-folder>`: a plain-text report you can paste when a model fails to load.
//! It looks for the usual causes with line numbers: unreadable numbers, faces that point past the
//! vertex list, `usemtl` names with no `newmtl`, and MTL values that strict parsers reject.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

fn clip(s: &str) -> String {
    let s = s.trim_end();
    if s.chars().count() > 110 {
        format!("{}...", s.chars().take(107).collect::<String>())
    } else {
        s.to_string()
    }
}

fn has_ext(p: &Path, e: &str) -> bool {
    p.extension()
        .and_then(|x| x.to_str())
        .map(|x| x.eq_ignore_ascii_case(e))
        .unwrap_or(false)
}

pub fn run(dir: &Path) {
    println!("=== VotV Model Manager diagnostics ===");
    println!("folder: {}", dir.display());

    let Ok(rd) = fs::read_dir(dir) else {
        println!("cannot read the folder");
        return;
    };
    let mut files: Vec<_> = rd.flatten().map(|e| e.path()).collect();
    files.sort();
    println!("\n--- files ---");
    for p in &files {
        let size = fs::metadata(p).map(|m| m.len()).unwrap_or(0);
        println!(
            "  {:>10} bytes  {}",
            size,
            p.file_name().unwrap_or_default().to_string_lossy()
        );
    }

    let mut obj_texts = Vec::new();
    for p in files.iter().filter(|p| has_ext(p, "obj")) {
        if let Some(t) = obj_report(p) {
            obj_texts.push(t);
        }
    }
    let mut defined = BTreeSet::new();
    for p in files.iter().filter(|p| has_ext(p, "mtl")) {
        mtl_report(p, &mut defined);
    }

    // usemtl names that no MTL defines
    let mut used = BTreeSet::new();
    for t in &obj_texts {
        for l in t.lines() {
            if let Some(r) = l.trim().strip_prefix("usemtl") {
                if !r.trim().is_empty() {
                    used.insert(r.trim().to_string());
                }
            }
        }
    }
    let missing: Vec<_> = used.difference(&defined).collect();
    println!("\n--- materials ---");
    println!("  used by the OBJ : {:?}", used);
    println!("  defined in MTLs : {:?}", defined);
    if missing.is_empty() {
        println!("  every used material is defined");
    } else {
        println!(
            "  USED BUT NOT DEFINED: {:?}  (the manager adds these automatically)",
            missing
        );
    }

    println!("\n--- full load attempt ---");
    match crate::model::load_model(dir) {
        Ok(m) => {
            println!("loaded OK");
            crate::model::check(&m);
        }
        Err(e) => println!("FAILED: {e}"),
    }
    println!("\n=== end of report: paste everything above ===");
}

fn obj_report(p: &Path) -> Option<String> {
    let bytes = fs::read(p).ok()?;
    let utf8 = std::str::from_utf8(&bytes).is_ok();
    let text = String::from_utf8_lossy(&bytes).into_owned();
    println!(
        "\n--- OBJ: {} ---",
        p.file_name().unwrap_or_default().to_string_lossy()
    );
    println!(
        "  size {} bytes, {} lines, {}, line endings {}",
        bytes.len(),
        text.lines().count(),
        if utf8 {
            "valid UTF-8"
        } else {
            "NOT valid UTF-8"
        },
        if text.contains("\r\n") { "CRLF" } else { "LF" }
    );

    // pass 1: counts
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for l in text.lines() {
        let k = l.split_whitespace().next().unwrap_or("");
        *counts.entry(k.to_string()).or_default() += 1;
    }
    let get = |k: &str| counts.get(k).copied().unwrap_or(0);
    let (nv, nvt, nvn) = (get("v"), get("vt"), get("vn"));
    println!(
        "  counts: v={} vt={} vn={} f={} o={} g={} usemtl={} mtllib={}",
        nv,
        nvt,
        nvn,
        get("f"),
        get("o"),
        get("g"),
        get("usemtl"),
        get("mtllib")
    );

    let known = [
        "v", "vt", "vn", "vp", "f", "l", "p", "g", "o", "s", "usemtl", "mtllib", "mg", "#", "",
    ];
    let (mut unknown, mut badnum, mut badface) = (vec![], vec![], vec![]);
    let (mut names, mut libs) = (vec![], vec![]);
    let mut empty_usemtl: Vec<usize> = vec![];
    // (line, material, faces, faces with no UV index) for every run of faces between `usemtl` lines
    let mut blocks: Vec<(usize, String, usize, usize)> =
        vec![(0, "(before first usemtl)".to_string(), 0, 0)];
    for (i, l) in text.lines().enumerate() {
        let n = i + 1;
        let mut it = l.split_whitespace();
        let k = it.next().unwrap_or("");
        if k.starts_with('#') {
            continue;
        }
        if !known.contains(&k) && unknown.len() < 5 {
            unknown.push(format!("line {n}: {}", clip(l)));
        }
        match k {
            "v" | "vn" | "vt" => {
                let want = if k == "vt" { 2 } else { 3 };
                let vals: Vec<_> = it.take(want.max(3)).collect();
                if (vals.len() < want || vals.iter().any(|t| t.parse::<f32>().is_err()))
                    && badnum.len() < 5
                {
                    badnum.push(format!("line {n}: {}", clip(l)));
                }
            }
            "f" => {
                let mut no_vt = false;
                for tok in it {
                    let mut parts = tok.split('/');
                    let (a, b, c) = (parts.next(), parts.next(), parts.next());
                    if b.is_none_or(|x| x.is_empty()) {
                        no_vt = true;
                    }
                    let ok = |s: Option<&str>, total: usize, allow_empty: bool| match s {
                        None => allow_empty,
                        Some("") => allow_empty,
                        Some(x) => x
                            .parse::<i64>()
                            .map(|i| i != 0 && i.unsigned_abs() as usize <= total)
                            .unwrap_or(false),
                    };
                    if !(ok(a, nv, false) && ok(b, nvt, true) && ok(c, nvn, true))
                        && badface.len() < 5
                    {
                        badface.push(format!("line {n}: bad vertex '{tok}' in: {}", clip(l)));
                        break;
                    }
                }
                if let Some(blk) = blocks.last_mut() {
                    blk.2 += 1;
                    if no_vt {
                        blk.3 += 1;
                    }
                }
            }
            "usemtl" => {
                let name = it.collect::<Vec<_>>().join(" ");
                if name.is_empty() && empty_usemtl.len() < 10 {
                    empty_usemtl.push(n);
                }
                blocks.push((
                    n,
                    if name.is_empty() {
                        "(bare)".to_string()
                    } else {
                        name
                    },
                    0,
                    0,
                ));
            }
            "o" | "g" if names.len() < 30 => names.push(l.trim().to_string()),
            "mtllib" => libs.push(l.trim().to_string()),
            _ => {}
        }
    }
    if !empty_usemtl.is_empty() {
        println!("  'usemtl' lines with NO name (lines {:?}): strict parsers fail on these; the manager renames them to __none__", empty_usemtl);
    }
    println!("  material blocks (a 'usemtl' line starts each; faces below it use that material):");
    for (l, name, f, nov) in blocks.iter().take(60) {
        if *f > 0 {
            println!(
                "      line {:>6}  {:<18} faces {:>6}   faces with no UV index {:>6}",
                l, name, f, nov
            );
        }
    }
    println!("  mtllib lines: {:?}", libs);
    println!("  objects/groups (first 30): {:?}", names);
    for (title, v) in [
        ("unknown directives", &unknown),
        ("unreadable numbers", &badnum),
        ("faces with bad indices", &badface),
    ] {
        if v.is_empty() {
            println!("  {title}: none");
        } else {
            println!("  {title} (first {}):", v.len());
            for x in v {
                println!("      {x}");
            }
        }
    }
    println!("  first 12 lines:");
    for l in text.lines().take(12) {
        println!("      {}", clip(l));
    }
    Some(text)
}

fn mtl_report(p: &Path, defined: &mut BTreeSet<String>) {
    let Ok(bytes) = fs::read(p) else { return };
    let text = String::from_utf8_lossy(&bytes).into_owned();
    println!(
        "\n--- MTL: {} ---",
        p.file_name().unwrap_or_default().to_string_lossy()
    );
    println!(
        "  size {} bytes, {} lines",
        bytes.len(),
        text.lines().count()
    );

    // Keys a strict parser reads as numbers, and how many values each needs.
    let floats3 = ["ka", "kd", "ks", "ke", "tf"];
    let float1 = ["ns", "ni", "d", "tr"];
    let mut bad = vec![];
    let mut kd_maps = vec![];
    for (i, l) in text.lines().enumerate() {
        let n = i + 1;
        let t = l.trim();
        let mut it = t.split_whitespace();
        let k = it.next().unwrap_or("").to_lowercase();
        let vals: Vec<&str> = it.collect();
        if k == "newmtl" {
            defined.insert(t["newmtl".len()..].trim().to_string());
        } else if k == "map_kd" {
            kd_maps.push(format!("line {n}: {}", clip(t)));
        } else if floats3.contains(&k.as_str()) {
            if vals.is_empty() || vals.iter().any(|v| v.parse::<f32>().is_err()) {
                bad.push(format!("line {n}: {}", clip(t)));
            }
        } else if float1.contains(&k.as_str()) {
            if vals.len() != 1 || vals[0].parse::<f32>().is_err() {
                bad.push(format!("line {n}: {}", clip(t)));
            }
        } else if k == "illum" && (vals.len() != 1 || vals[0].parse::<i32>().is_err()) {
            bad.push(format!(
                "line {n}: {}  (illum must be a whole number)",
                clip(t)
            ));
        }
    }
    if bad.is_empty() {
        println!("  lines a strict parser rejects: none");
    } else {
        println!(
            "  lines a strict parser rejects (first 10 of {}):",
            bad.len()
        );
        for x in bad.iter().take(10) {
            println!("      {x}");
        }
    }
    println!(
        "  map_Kd lines: {}",
        if kd_maps.is_empty() {
            "none".into()
        } else {
            kd_maps.join(" | ")
        }
    );
    println!("  first 25 lines:");
    for l in text.lines().take(25) {
        println!("      {}", clip(l));
    }
}
