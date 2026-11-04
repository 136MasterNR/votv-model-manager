//! Model files on disk: OBJ + MTL, PNG maps, properties.cfg. No rendering types here, so all of this
//! can run on a worker thread. How the game reads these is documented in `votv-printer-internals`.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use bevy::color::Color;
use bevy::math::Vec3;

pub struct RawSection {
    pub name: String,
    pub mat_name: String,
    pub kd: Option<String>,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    /// `indices` without the triangles dropped by the duplicate rule (None when nothing was dropped).
    pub dedup: Option<Vec<u32>>,
}

/// One `UCX_` object: the game turns each into a convex hull.
pub struct Hull {
    pub name: String,
    pub points: Vec<[f32; 3]>,
    pub edges: Vec<([f32; 3], [f32; 3])>,
}

pub struct ModelData {
    pub name: String,
    pub sections: Vec<RawSection>,
    pub hulls: Vec<Hull>,
    pub cfg: HashMap<String, String>,
    pub files: HashMap<String, PathBuf>, // lowercase file name -> path
    pub center: [f32; 3],
    pub radius: f32,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Diffuse,
    Pbr,
    Normal,
    Emissive,
}
pub const KINDS: [Kind; 4] = [Kind::Diffuse, Kind::Pbr, Kind::Normal, Kind::Emissive];

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Diffuse => "diffuse",
            Kind::Pbr => "pbr",
            Kind::Normal => "normal",
            Kind::Emissive => "emissive",
        }
    }
    pub fn idx(self) -> usize {
        match self {
            Kind::Diffuse => 0,
            Kind::Pbr => 1,
            Kind::Normal => 2,
            Kind::Emissive => 3,
        }
    }
}

pub const PHYS_MATERIALS: [&str; 16] = [
    "plastic",
    "wood",
    "metal",
    "concrete",
    "rubber",
    "bag",
    "metal_hollow",
    "flesh",
    "food",
    "device",
    "dirt",
    "glass",
    "paper",
    "vehicle",
    "wheel",
    "ball",
];

// ---------------------------------------------------------------------------------------------
// Folder scanning
// ---------------------------------------------------------------------------------------------

pub struct Entry {
    pub name: String,
    pub dir: PathBuf,
    /// When the model folder was created (what "last added" sorts by).
    pub added: std::time::SystemTime,
    /// The newest change of any file in the folder except the icon (what "last modified" sorts by).
    pub modified: std::time::SystemTime,
    /// The `.obj` was renamed to `.obj.disabled` (the game skips it). Such models are listed in their own section.
    pub disabled: bool,
    /// The folder holds a saved `.glb`, so the converter can reopen it (Edit).
    pub editable: bool,
}

impl Entry {
    /// List section: 0 = EDITABLE, 1 = MODELS. (A disabled model stays in its section and is greyed out.)
    pub fn section(&self) -> u8 {
        if self.editable {
            0
        } else {
            1
        }
    }
}

/// How a section of the model list is ordered. A click on the sort button in the section header
/// (`ui::spawn_section`) switches between the section's date order and the alphabetic one: EDITABLE has Last Modified and
/// Alphabetic, MODELS has Last Added and Alphabetic.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SortMode {
    LastAdded,
    LastModified,
    NameAz,
}

impl SortMode {
    /// The mode a click switches to in section `rank` (0 = EDITABLE, 1 = MODELS).
    pub fn next(self, rank: u8) -> SortMode {
        match self {
            SortMode::NameAz if rank == 0 => SortMode::LastModified,
            SortMode::NameAz => SortMode::LastAdded,
            _ => SortMode::NameAz,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SortMode::LastAdded => "Last Added",
            SortMode::LastModified => "Last Modified",
            SortMode::NameAz => "Alphabetic",
        }
    }
}

/// Orders `entries` by list section, then inside each section by that section's mode (newest first for the dates).
pub fn sort_entries(entries: &mut [Entry], modes: [SortMode; 2]) {
    entries.sort_by(|a, b| {
        let by_name = || a.name.to_lowercase().cmp(&b.name.to_lowercase());
        a.section()
            .cmp(&b.section())
            .then_with(|| match modes[a.section() as usize] {
                SortMode::LastAdded => b.added.cmp(&a.added).then_with(by_name),
                SortMode::LastModified => b.modified.cmp(&a.modified).then_with(by_name),
                SortMode::NameAz => by_name(),
            })
    });
}

/// (added, modified) of a model folder: the folder's creation time, and the newest file change in it (not counting
/// `icon.png`, which the app rewrites on its own).
fn entry_times(dir: &Path) -> (std::time::SystemTime, std::time::SystemTime) {
    use std::time::UNIX_EPOCH;
    let meta = fs::metadata(dir).ok();
    let added = meta
        .as_ref()
        .and_then(|m| m.created().ok().or_else(|| m.modified().ok()))
        .unwrap_or(UNIX_EPOCH);
    let mut modified = UNIX_EPOCH;
    if let Ok(rd) = fs::read_dir(dir) {
        for f in rd.flatten() {
            if f.file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case("icon.png")
            {
                continue;
            }
            if let Some(t) = f.metadata().ok().and_then(|m| m.modified().ok()) {
                modified = modified.max(t);
            }
        }
    }
    if modified == UNIX_EPOCH {
        modified = added;
    }
    (added, modified)
}

/// Name of the file a model is disabled with: `<x>.obj` -> `<x>.obj.disabled`.
pub const DISABLED_SUFFIX: &str = ".disabled";

fn has_file_ending(dir: &Path, ends: &str) -> bool {
    fs::read_dir(dir)
        .map(|d| {
            d.flatten().any(|f| {
                f.file_name()
                    .to_string_lossy()
                    .to_lowercase()
                    .ends_with(ends)
            })
        })
        .unwrap_or(false)
}

/// Every sub-folder of `root` that contains an .obj file. Fast: no model is parsed.
pub fn scan_models(root: &Path) -> Vec<Entry> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(root) else {
        return out;
    };
    for e in rd.flatten() {
        let p = e.path();
        if !p.is_dir() {
            continue;
        }
        let has_obj = fs::read_dir(&p)
            .map(|d| {
                d.flatten().any(|f| {
                    f.path()
                        .extension()
                        .and_then(|x| x.to_str())
                        .map(|x| x.eq_ignore_ascii_case("obj"))
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false);
        let name = p
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        if name.starts_with('.') {
            continue; // hidden folders: the converter's temp folders
        }
        if has_obj {
            let editable = has_file_ending(&p, ".glb");
            let (added, modified) = entry_times(&p);
            out.push(Entry {
                name,
                dir: p,
                added,
                modified,
                disabled: false,
                editable,
            });
        } else if has_file_ending(&p, ".obj.disabled") {
            let editable = has_file_ending(&p, ".glb");
            let (added, modified) = entry_times(&p);
            out.push(Entry {
                name,
                dir: p,
                added,
                modified,
                disabled: true,
                editable,
            });
        }
    }
    // Sorted by list section (editable, models), then by name.
    out.sort_by_key(|e| (e.section(), e.name.to_lowercase()));
    out
}

// ---------------------------------------------------------------------------------------------
// Loading one model
// ---------------------------------------------------------------------------------------------

pub fn load_model(dir: &Path) -> Result<ModelData, String> {
    let dir = dir
        .canonicalize()
        .map_err(|e| format!("cannot open {}: {e}", dir.display()))?;
    let name = dir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("model")
        .to_string();

    let mut files = HashMap::new();
    for entry in fs::read_dir(&dir).map_err(|e| format!("cannot read folder: {e}"))? {
        let p = entry.map_err(|e| e.to_string())?.path();
        if let Some(n) = p.file_name().and_then(|s| s.to_str()) {
            files.insert(n.to_lowercase(), p.clone());
        }
    }
    let obj = files
        .get(&format!("{}.obj", name.to_lowercase()))
        .or_else(|| files.get(&format!("{}.obj.disabled", name.to_lowercase())))
        .cloned()
        .or_else(|| {
            files
                .iter()
                .filter(|(k, _)| k.ends_with(".obj"))
                .map(|(_, v)| v.clone())
                .min()
        })
        .or_else(|| {
            files
                .iter()
                .filter(|(k, _)| k.ends_with(".obj.disabled"))
                .map(|(_, v)| v.clone())
                .min()
        })
        .ok_or("no .obj file in the folder")?;

    let (models, mats) = load_obj_robust(&obj, &files)?;

    let mut sections = Vec::new();
    let mut hulls = Vec::new();
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);

    for m in models {
        let mesh = m.mesh;
        let pos: Vec<[f32; 3]> = mesh
            .positions
            .chunks_exact(3)
            .map(|c| [c[0], c[1], c[2]])
            .collect();
        // The game removes `UCX_` objects from the render list and keeps their points as hulls.
        if m.name.starts_with("UCX_") {
            let mut edges = Vec::new();
            for t in mesh.indices.chunks_exact(3) {
                for k in 0..3 {
                    edges.push((pos[t[k] as usize], pos[t[(k + 1) % 3] as usize]));
                }
            }
            hulls.push(Hull {
                name: m.name,
                points: pos,
                edges,
            });
            continue;
        }
        for p in &pos {
            for a in 0..3 {
                lo[a] = lo[a].min(p[a]);
                hi[a] = hi[a].max(p[a]);
            }
        }
        let uvs: Vec<[f32; 2]> = if mesh.texcoords.len() == pos.len() * 2 {
            mesh.texcoords
                .chunks_exact(2)
                .map(|c| [c[0], c[1]])
                .collect()
        } else {
            vec![[0.0, 0.0]; pos.len()]
        };
        let (positions, normals, uvs, indices) = if mesh.normals.len() == mesh.positions.len() {
            let n = mesh
                .normals
                .chunks_exact(3)
                .map(|c| [c[0], c[1], c[2]])
                .collect();
            (pos, n, uvs, mesh.indices)
        } else {
            flatten(&pos, &uvs, &mesh.indices) // no normals in the file: flat shading
        };
        let mat = mesh.material_id.and_then(|i| mats.get(i));
        sections.push(RawSection {
            name: m.name,
            mat_name: mat.map(|x| x.name.clone()).unwrap_or_default(),
            kd: mat.and_then(|x| x.diffuse_texture.clone()),
            positions,
            normals,
            uvs,
            indices,
            dedup: None,
        });
    }
    if sections.is_empty() {
        return Err("the OBJ has no renderable (non-UCX_) geometry".into());
    }

    let center = [
        (lo[0] + hi[0]) / 2.0,
        (lo[1] + hi[1]) / 2.0,
        (lo[2] + hi[2]) / 2.0,
    ];
    let radius = Vec3::from(hi).distance(Vec3::from(lo)) / 2.0;

    let mut cfg = HashMap::new();
    if let Some(p) = files.get("properties.cfg") {
        if let Ok(text) = fs::read_to_string(p) {
            for line in text.lines() {
                if let Some((k, v)) = line.split_once('=') {
                    cfg.insert(k.trim().to_string(), v.trim().to_string());
                }
            }
        }
    }
    Ok(ModelData {
        name,
        sections,
        hulls,
        cfg,
        files,
        center,
        radius: radius.max(0.001),
    })
}

// ---------------------------------------------------------------------------------------------
// Tolerant OBJ/MTL reading
// ---------------------------------------------------------------------------------------------

fn read_text(p: &Path) -> Result<String, String> {
    let bytes = fs::read(p).map_err(|e| format!("cannot read {}: {e}", p.display()))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Rebuild an MTL that tobj cannot reject: keep only `newmtl` and `map_Kd` (all the manager uses),
/// and make sure every material the OBJ asks for with `usemtl` exists. Odd values in Ka/Kd/Ks/Ns/d/illum
/// lines, or a `usemtl` with no matching `newmtl`, are what usually cause "material parse error".
pub fn clean_mtl(obj_text: &str, mtl_texts: &[String]) -> String {
    let mut order: Vec<String> = Vec::new();
    let mut kd: HashMap<String, String> = HashMap::new();
    let mut cur: Option<String> = None;
    for text in mtl_texts {
        for line in text.lines() {
            let l = line.trim();
            let (key, rest) = l.split_once(char::is_whitespace).unwrap_or((l, ""));
            if key.eq_ignore_ascii_case("newmtl") {
                let name = rest.trim().to_string();
                if !name.is_empty() {
                    if !order.contains(&name) {
                        order.push(name.clone());
                    }
                    cur = Some(name);
                }
            } else if key.eq_ignore_ascii_case("map_Kd") {
                if let (Some(c), Some(p)) = (&cur, rest.split_whitespace().last()) {
                    kd.insert(c.clone(), p.to_string());
                }
            }
        }
    }
    for line in obj_text.lines() {
        if let Some(rest) = line.trim().strip_prefix("usemtl") {
            let name = rest.trim().to_string();
            if !name.is_empty() && !order.contains(&name) {
                order.push(name);
            }
        }
    }
    let mut out = String::new();
    for n in &order {
        out.push_str(&format!("newmtl {n}\n"));
        if let Some(p) = kd.get(n) {
            out.push_str(&format!("map_Kd {p}\n"));
        }
    }
    out
}

fn load_obj_robust(
    obj: &Path,
    files: &HashMap<String, PathBuf>,
) -> Result<(Vec<tobj::Model>, Vec<tobj::Material>), String> {
    let raw = read_text(obj)?;
    // A `usemtl` line with no name makes tobj fail with "material parse error". Some exporters write one for
    // faces without a material. Give those faces a named placeholder material instead.
    let mut text = String::with_capacity(raw.len() + 64);
    for line in raw.lines() {
        if line.trim() == "usemtl" {
            text.push_str("usemtl __none__\n");
        } else {
            text.push_str(line);
            text.push('\n');
        }
    }

    let mut mtl_texts = Vec::new();
    let mut has_lib = false;
    for line in text.lines() {
        if let Some(rest) = line.trim().strip_prefix("mtllib") {
            has_lib = true;
            if let Some(p) = files.get(&rest.trim().to_lowercase()) {
                if let Ok(t) = read_text(p) {
                    mtl_texts.push(t);
                }
            }
        }
    }
    if mtl_texts.is_empty() {
        // No (readable) mtllib: fall back to a .mtl with the same name as the OBJ.
        let stem = obj
            .file_stem()
            .and_then(|x| x.to_str())
            .unwrap_or("")
            .to_lowercase();
        let stem = stem.strip_suffix(".obj").unwrap_or(&stem).to_string(); // `x.obj.disabled` -> `x`
        if let Some(t) = files
            .get(&format!("{stem}.mtl"))
            .and_then(|p| read_text(p).ok())
        {
            mtl_texts.push(t);
        }
    }
    let mtl = clean_mtl(&text, &mtl_texts);
    if !has_lib {
        text.insert_str(0, "mtllib __auto.mtl\n");
    }

    // tobj calls the loader once per `mtllib` line; give it the cleaned MTL once and nothing after.
    let first = std::cell::Cell::new(true);
    let opts = tobj::LoadOptions {
        triangulate: true,
        single_index: true,
        ..Default::default()
    };
    let (models, mats) =
        tobj::load_obj_buf(&mut std::io::Cursor::new(text.into_bytes()), &opts, |_| {
            let body = if first.replace(false) {
                mtl.clone()
            } else {
                String::new()
            };
            tobj::load_mtl_buf(&mut std::io::Cursor::new(body.into_bytes()))
        })
        .map_err(|e| format!("OBJ parse error ({e:?}): {e}"))?;
    Ok((models, mats.unwrap_or_default()))
}

fn flatten(
    pos: &[[f32; 3]],
    uv: &[[f32; 2]],
    idx: &[u32],
) -> (Vec<[f32; 3]>, Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<u32>) {
    let (mut p, mut n, mut u, mut i) = (vec![], vec![], vec![], vec![]);
    for t in idx.chunks_exact(3) {
        let (a, b, c) = (
            Vec3::from(pos[t[0] as usize]),
            Vec3::from(pos[t[1] as usize]),
            Vec3::from(pos[t[2] as usize]),
        );
        let nn = (b - a).cross(c - a).normalize_or_zero().to_array();
        for k in 0..3 {
            p.push(pos[t[k] as usize]);
            n.push(nn);
            u.push(uv[t[k] as usize]);
            i.push(i.len() as u32);
        }
    }
    (p, n, u, i)
}

fn parse_struct(s: &str) -> HashMap<String, f32> {
    s.trim()
        .trim_matches(|c| c == '(' || c == ')')
        .split(',')
        .filter_map(|kv| {
            let (k, v) = kv.split_once('=')?;
            Some((k.trim().to_uppercase(), v.trim().parse().ok()?))
        })
        .collect()
}

impl ModelData {
    pub fn num(&self, key: &str, default: f32) -> f32 {
        self.cfg
            .get(key)
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    }
    pub fn vec3(&self, key: &str) -> Vec3 {
        let m = self
            .cfg
            .get(key)
            .map(|s| parse_struct(s))
            .unwrap_or_default();
        Vec3::new(
            *m.get("X").unwrap_or(&0.0),
            *m.get("Y").unwrap_or(&0.0),
            *m.get("Z").unwrap_or(&0.0),
        )
    }
    pub fn color(&self, key: &str) -> Color {
        let m = self
            .cfg
            .get(key)
            .map(|s| parse_struct(s))
            .unwrap_or_default();
        Color::linear_rgb(
            *m.get("R").unwrap_or(&1.0),
            *m.get("G").unwrap_or(&1.0),
            *m.get("B").unwrap_or(&1.0),
        )
    }
    /// `filter_<kind>_` is an engine TextureFilter value: 0 Nearest, 1 Bilinear, 2 Trilinear, 3 Default.
    pub fn filter(&self, kind: Kind) -> u8 {
        self.num(&format!("filter_{}_", kind.name()), 0.0) as u8
    }
    /// Filter for one section's texture. Multi-material cfgs use `filter_<kind>_<material>` (for example
    /// `filter_diffuse_body`); single-material ones use `filter_<kind>_`. Per-material wins, default is 0 (nearest).
    pub fn filter_for(&self, kind: Kind, s: &RawSection) -> u8 {
        self.cfg
            .get(&format!("filter_{}_{}", kind.name(), s.mat_name))
            .and_then(|v| v.parse::<f32>().ok())
            .map(|v| v as u8)
            .unwrap_or_else(|| self.filter(kind))
    }
    /// Texture lookup: `<kind>_<material>.png`, then `<kind>_<folder>.png`, then the name implied by map_Kd.
    pub fn find_tex(&self, kind: Kind, s: &RawSection) -> Option<PathBuf> {
        let mut cands = vec![];
        if !s.mat_name.is_empty() {
            cands.push(format!("{}_{}.png", kind.name(), s.mat_name));
        }
        cands.push(format!("{}_{}.png", kind.name(), self.name));
        if let Some(kd) = &s.kd {
            let stem = Path::new(kd)
                .file_stem()
                .and_then(|x| x.to_str())
                .unwrap_or("");
            if stem.to_lowercase().starts_with("diffuse_") {
                cands.push(format!("{}_{}.png", kind.name(), &stem[8..]));
            }
        }
        cands
            .iter()
            .find_map(|c| self.files.get(&c.to_lowercase()).cloned())
    }
}

// ---------------------------------------------------------------------------------------------
// Textures: decoded off the main thread, optionally downscaled (icons only need ~128 px)
// ---------------------------------------------------------------------------------------------

pub struct RawImage {
    pub w: u32,
    pub h: u32,
    pub data: Vec<u8>,
}

pub struct PoolImg {
    pub img: RawImage,
    pub kind: Kind,
    pub filter: u8,
}

/// A model plus its decoded textures. `tex[section][kind.idx()]` indexes into `pool`.
pub struct Loaded {
    pub data: ModelData,
    pub tex: Vec<[Option<usize>; 4]>,
    pub pool: Vec<PoolImg>,
    pub dup_removed: usize, // triangles dropped by the converter's duplicate rule
}

fn decode(path: &Path, kind: Kind, max_side: Option<u32>) -> Option<RawImage> {
    let mut img = image::open(path).ok()?.to_rgba8();
    if let Some(m) = max_side {
        let (w, h) = img.dimensions();
        let longest = w.max(h);
        if longest > m {
            let s = m as f32 / longest as f32;
            let (nw, nh) = (
                ((w as f32 * s).round() as u32).max(1),
                ((h as f32 * s).round() as u32).max(1),
            );
            img = image::imageops::thumbnail(&img, nw, nh);
        }
    }
    let (w, h) = img.dimensions();
    let mut data = img.into_raw();
    if kind == Kind::Pbr {
        // VotV: R metallic, G specular, B roughness. Bevy wants G roughness, B metallic. Bevy ignores R, so the game's
        // specular goes there (scene.rs averages it into the material's `reflectance`).
        for px in data.chunks_exact_mut(4) {
            let (metal, spec, rough) = (px[0], px[1], px[2]);
            px[0] = spec;
            px[1] = rough;
            px[2] = metal;
            px[3] = 255;
        }
    }
    Some(RawImage { w, h, data })
}

/// Parse the model and decode only the requested texture kinds. Textures shared between sections
/// (the usual single-atlas layout) are decoded once.
pub fn load_with_textures(
    dir: &Path,
    kinds: &[Kind],
    max_side: Option<u32>,
) -> Result<Loaded, String> {
    let mut data = load_model(dir)?;
    let mut pool: Vec<PoolImg> = Vec::new();
    let mut seen: HashMap<(PathBuf, Kind), usize> = HashMap::new();
    let mut tex = Vec::new();
    for s in &data.sections {
        let mut slots: [Option<usize>; 4] = [None; 4];
        for &k in kinds {
            let Some(p) = data.find_tex(k, s) else {
                continue;
            };
            let key = (p.clone(), k);
            let idx = if let Some(&i) = seen.get(&key) {
                Some(i)
            } else if let Some(img) = decode(&p, k, max_side) {
                pool.push(PoolImg {
                    img,
                    kind: k,
                    filter: data.filter_for(k, s),
                });
                seen.insert(key, pool.len() - 1);
                Some(pool.len() - 1)
            } else {
                None
            };
            slots[k.idx()] = idx;
        }
        tex.push(slots);
    }
    // Same policy as printer_convert.py: where geometry is duplicated, keep the copy that has a texture.
    let has_tex: Vec<bool> = tex
        .iter()
        .map(|t| t[Kind::Diffuse.idx()].is_some())
        .collect();
    let dup_removed = mark_duplicate_faces(&mut data, &has_tex);
    Ok(Loaded {
        data,
        tex,
        pool,
        dup_removed,
    })
}

/// The converter's duplicate rule (`_find_exact_duplicate_parts` in printer_convert.py), applied per triangle.
///
/// The converter drops a whole part when another part has identical vertices and faces, keeping the one with a
/// base-colour texture (or the earlier one when both or neither have one). Here a triangle counts as a duplicate when
/// another section has a triangle with the same three corners in the same winding order; the copy in a section
/// without a texture is dropped. Copies inside one section, and reversed-winding copies, are left alone.
/// Fills `RawSection::dedup` and returns how many triangles were dropped.
pub fn mark_duplicate_faces(data: &mut ModelData, has_tex: &[bool]) -> usize {
    let q = |p: [f32; 3]| {
        [
            (p[0] * 1.0e4).round() as i64,
            (p[1] * 1.0e4).round() as i64,
            (p[2] * 1.0e4).round() as i64,
        ]
    };
    let mut groups: HashMap<[[i64; 3]; 3], Vec<(usize, usize)>> = HashMap::new(); // corners -> (section, triangle)
    for (si, s) in data.sections.iter().enumerate() {
        for (ti, t) in s.indices.chunks_exact(3).enumerate() {
            let c = [
                q(s.positions[t[0] as usize]),
                q(s.positions[t[1] as usize]),
                q(s.positions[t[2] as usize]),
            ];
            let r = (0..3).min_by_key(|&i| c[i]).unwrap_or(0); // rotate so the smallest corner is first: keeps winding
            groups
                .entry([c[r], c[(r + 1) % 3], c[(r + 2) % 3]])
                .or_default()
                .push((si, ti));
        }
    }
    let mut drop: Vec<HashSet<usize>> = vec![HashSet::new(); data.sections.len()];
    let mut n = 0;
    for members in groups.values() {
        if members.len() < 2 {
            continue;
        }
        let keeper = members
            .iter()
            .find(|(si, _)| has_tex.get(*si).copied().unwrap_or(false))
            .unwrap_or(&members[0])
            .0;
        for &(si, ti) in members {
            if si != keeper {
                drop[si].insert(ti);
                n += 1;
            }
        }
    }
    for (si, set) in drop.iter().enumerate() {
        if set.is_empty() {
            continue;
        }
        let s = &mut data.sections[si];
        let mut out = Vec::with_capacity(s.indices.len());
        for (ti, t) in s.indices.chunks_exact(3).enumerate() {
            if !set.contains(&ti) {
                out.extend_from_slice(t);
            }
        }
        s.dedup = Some(out);
    }
    n
}

/// Triangles that sit exactly on top of an earlier triangle (same three corner positions, 0.001 unit grid).
/// Returns ((section that has it first, section holding the copy), how many). They fight for the same pixels,
/// so a copy with a different material shows as flickering stripes.
pub fn duplicate_faces(m: &ModelData) -> Vec<((usize, usize), usize)> {
    let q = |p: [f32; 3]| {
        [
            (p[0] * 1000.0).round() as i64,
            (p[1] * 1000.0).round() as i64,
            (p[2] * 1000.0).round() as i64,
        ]
    };
    let mut seen: HashMap<[[i64; 3]; 3], usize> = HashMap::new();
    let mut dup: HashMap<(usize, usize), usize> = HashMap::new();
    for (si, s) in m.sections.iter().enumerate() {
        for t in s.indices.chunks_exact(3) {
            let mut v = [
                q(s.positions[t[0] as usize]),
                q(s.positions[t[1] as usize]),
                q(s.positions[t[2] as usize]),
            ];
            v.sort();
            match seen.get(&v) {
                Some(&first) => *dup.entry((first, si)).or_default() += 1,
                None => {
                    seen.insert(v, si);
                }
            }
        }
    }
    let mut out: Vec<_> = dup.into_iter().collect();
    out.sort_by_key(|a| std::cmp::Reverse(a.1));
    out
}

/// Print what the loader found, without opening a window (`--check <folder>`).
pub fn check(m: &ModelData) {
    println!("model  : {}", m.name);
    println!(
        "sections: {}   hulls (UCX_): {}",
        m.sections.len(),
        m.hulls.len()
    );
    for (i, s) in m.sections.iter().enumerate() {
        println!(
            "  [{i}] {:<24} verts {:<6} tris {:<6} material '{}'",
            s.name,
            s.positions.len(),
            s.indices.len() / 3,
            s.mat_name
        );
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for p in &s.positions {
            for a in 0..3 {
                lo[a] = lo[a].min(p[a]);
                hi[a] = hi[a].max(p[a]);
            }
        }
        println!(
            "        bbox   x {:.1}..{:.1}   y {:.1}..{:.1}   z {:.1}..{:.1}",
            lo[0], hi[0], lo[1], hi[1], lo[2], hi[2]
        );
        if s.uvs.iter().all(|u| u[0] == 0.0 && u[1] == 0.0) {
            println!("        uv     NONE (every vertex has UV 0,0: no texture coordinates for this part)");
        } else {
            let (mut ulo, mut uhi) = ([f32::MAX; 2], [f32::MIN; 2]);
            let mut outside = 0usize;
            for u in &s.uvs {
                for a in 0..2 {
                    ulo[a] = ulo[a].min(u[a]);
                    uhi[a] = uhi[a].max(u[a]);
                }
                if u[0] < 0.0 || u[0] > 1.0 || u[1] < 0.0 || u[1] > 1.0 {
                    outside += 1;
                }
            }
            let mut degenerate = 0usize;
            for t in s.indices.chunks_exact(3) {
                let (a, b, c) = (
                    s.uvs[t[0] as usize],
                    s.uvs[t[1] as usize],
                    s.uvs[t[2] as usize],
                );
                let area =
                    ((b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1])).abs() * 0.5;
                if area < 1e-9 {
                    degenerate += 1;
                }
            }
            println!(
                "        uv     u {:.3}..{:.3}   v {:.3}..{:.3}   outside 0..1: {:.0}%   zero-area UV triangles: {} of {}",
                ulo[0], uhi[0], ulo[1], uhi[1],
                100.0 * outside as f32 / s.uvs.len().max(1) as f32,
                degenerate, s.indices.len() / 3
            );
        }
        for k in KINDS {
            match m.find_tex(k, s) {
                Some(p) => {
                    let d = image::image_dimensions(&p)
                        .map(|(w, h)| format!("{w}x{h}"))
                        .unwrap_or("unreadable".into());
                    println!(
                        "        {:<9} {} ({d}) filter={}",
                        k.name(),
                        p.file_name().unwrap().to_string_lossy(),
                        m.filter_for(k, s)
                    );
                }
                None => println!("        {:<9} (not found)", k.name()),
            }
        }
    }
    let dups = duplicate_faces(m);
    if !dups.is_empty() {
        println!("WARNING: duplicate faces (triangles drawn exactly on top of each other; they z-fight and flicker).");
        println!("         The manager preview drops the copy without a texture, like printer_convert.py does (D toggles).");
        for ((a, b), n) in &dups {
            println!(
                "  {n} triangles of section [{b}] ('{}') duplicate section [{a}] ('{}')",
                m.sections[*b].mat_name, m.sections[*a].mat_name
            );
        }
    }
    for h in &m.hulls {
        let uniq: HashSet<[u32; 3]> = h.points.iter().map(|p| p.map(f32::to_bits)).collect();
        println!("  hull {:<14} unique points {}", h.name, uniq.len());
    }
    let pm = m.num("physical_material", 0.0) as usize;
    println!(
        "physical_material {} ({})",
        pm,
        PHYS_MATERIALS.get(pm).unwrap_or(&"?")
    );
    println!("center {:?}  radius {:.1} cm", m.center, m.radius);
    if m.hulls.is_empty() {
        println!("note: no UCX_ hulls, so in game this model gets no health, damage data, edible or bites");
    }
}
