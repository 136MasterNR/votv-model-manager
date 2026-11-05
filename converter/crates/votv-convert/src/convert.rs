//! The conversion pipeline: glb in, VotV printer folder out.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use image::{DynamicImage, RgbaImage};
use votv_model_core::{bites_for, texture_file_name, PropertiesCfg, TextureKind};

use crate::atlas::{self, MapKind, SwatchKey};
use crate::cache::{AtlasEntry, AtlasKey, GeoEntry, GeoKey, HullEntry, HullKey, SceneEntry, SceneKey};
use crate::collision::{self, Mesh};
use crate::error::{ConvertError, Result};
use crate::geom::*;
use crate::icon::{self, IconTri};
use crate::options::{ConvertOptions, OverwritePolicy};
use crate::report::*;
use crate::scene::{self, Material, Part, Scene};
use crate::tiling;

pub use votv_model_core::sanitize_name;

fn looks_like_previous_output(dir: &Path, name: &str) -> bool {
    let Ok(rd) = std::fs::read_dir(dir) else { return false };
    let existing: Vec<String> = rd.flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
    existing.iter().any(|f| f == "properties.cfg")
        && (existing.iter().any(|f| *f == format!("{name}.obj")) || existing.iter().any(|f| *f == format!("{name}.mtl")))
}

/// How a material's alpha behaves, following the glTF rules (VotV clips alpha at about 1/3 but cannot blend).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Transparency {
    /// `OPAQUE` (or unspecified): alpha is ignored. Renders as expected in VotV once the tile alpha is forced to 255.
    Opaque,
    /// `MASK`: alpha cutout (leaves, fences, hair cards). VotV clips alpha, so this works and is kept.
    Cutout,
    /// `BLEND` with real see-through alpha (glass, ghosts). VotV cannot blend; this is what "drop transparent" removes.
    Translucent,
}

pub(crate) fn material_transparency(mat: Option<&Material>, sc: &Scene) -> Transparency {
    let Some(m) = mat else { return Transparency::Opaque };
    match m.alpha_mode.as_deref() {
        Some("MASK") => Transparency::Cutout,
        Some("BLEND") => {
            // exporters (and toon shaders) often tag everything BLEND with an alpha like 0.99; only count real see-through
            if m.base_factor.map(|f| f[3] < 230).unwrap_or(false) {
                return Transparency::Translucent;
            }
            let Some(t) = m.base_tex.and_then(|i| sc.textures[i].as_ref()).filter(|t| t.had_alpha) else {
                return Transparency::Opaque;
            };
            // texture alpha: lots of semi-transparent texels = glass-like; fully transparent texels with a mostly solid
            // rest (hair cards, leaves, soft edges) = a cutout, which VotV's alpha clip handles
            let (mut covered, mut semi, mut clear) = (0u64, 0u64, 0u64);
            for p in t.img.pixels() {
                match p[3] {
                    0..=4 => clear += 1,
                    5..=249 => {
                        covered += 1;
                        semi += 1;
                    }
                    _ => covered += 1,
                }
            }
            if covered > 0 && semi as f64 / covered as f64 >= 0.15 {
                Transparency::Translucent
            } else if clear > 0 || semi > 0 {
                Transparency::Cutout
            } else {
                Transparency::Opaque
            }
        }
        _ => Transparency::Opaque,
    }
}

/// True when the material should be removed by "drop transparent materials".
pub(crate) fn material_is_transparent(mat: Option<&Material>, sc: &Scene) -> bool {
    material_transparency(mat, sc) == Transparency::Translucent
}

/// Toon outline shells (inverted hulls) are conventionally named like "..._Outline".
fn is_outline_name(name: &str) -> bool {
    name.to_ascii_lowercase().contains("outline")
}

fn allclose(a: &[V3], b: &[V3]) -> bool {
    a.iter().zip(b).all(|(p, q)| (0..3).all(|k| (p[k] - q[k]).abs() <= 1e-5 + 1e-5 * q[k].abs()))
}

fn find_duplicate_parts(parts: &[Part], sc: &Scene, ctx: &mut Ctx) -> std::collections::HashSet<usize> {
    let mut groups: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
    for (i, p) in parts.iter().enumerate() {
        groups.entry((p.pos.len(), p.tris.len())).or_default().push(i);
    }
    let mut keys: Vec<_> = groups.keys().copied().collect();
    keys.sort();
    let mut drop = std::collections::HashSet::new();
    let has_tex = |p: &Part| p.material.and_then(|m| sc.materials[m].base_tex).is_some();
    let mname = |p: &Part, i: usize| p.material.map(|m| sc.materials[m].name.clone()).unwrap_or_else(|| format!("part_{i}"));
    for k in keys {
        let idxs = &groups[&k];
        if idxs.len() < 2 {
            continue;
        }
        for a in 0..idxs.len() {
            let i = idxs[a];
            if drop.contains(&i) {
                continue;
            }
            for &j in &idxs[a + 1..] {
                if drop.contains(&i) || drop.contains(&j) {
                    continue;
                }
                let (pi, pj) = (&parts[i], &parts[j]);
                if !allclose(&pi.pos, &pj.pos) || pi.tris != pj.tris {
                    continue;
                }
                let (ti, tj) = (has_tex(pi), has_tex(pj));
                let (d, keep_n, drop_n) = if ti && !tj {
                    (j, mname(pi, i), mname(pj, j))
                } else if tj && !ti {
                    (i, mname(pj, j), mname(pi, i))
                } else {
                    (j, mname(pi, i), mname(pj, j))
                };
                drop.insert(d);
                ctx.info(DiagCode::DuplicatePart, format!(
                    "[dedup] parts {i} ({:?}) and {j} ({:?}) are geometrically identical, keeping {keep_n:?}, dropping {drop_n:?}.",
                    mname(pi, i), mname(pj, j)));
            }
        }
    }
    drop
}

/// 8-bit PNG with the best compression (slow for big atlases, which is why the bytes are cached).
fn encode_png(img: &RgbaImage, rgb: bool) -> Result<Vec<u8>> {
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    let d = DynamicImage::ImageRgba8(img.clone());
    let d = if rgb { DynamicImage::ImageRgb8(d.into_rgb8()) } else { d };
    let mut buf: Vec<u8> = Vec::new();
    let enc = PngEncoder::new_with_quality(&mut buf, CompressionType::Best, FilterType::Adaptive);
    d.write_with_encoder(enc).map_err(|e| ConvertError::Internal(format!("could not encode a PNG: {e}")))?;
    Ok(buf)
}

fn write_png_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    std::fs::write(path, bytes).map_err(|e| perm_or_io(e, path))
}

fn save_png(img: &RgbaImage, rgb: bool, path: &Path) -> Result<()> {
    write_png_bytes(path, &encode_png(img, rgb)?)
}

impl AtlasEntry {
    /// Compressed PNG of one map (0 diffuse, 1 emissive, 2 normal, 3 pbr), encoded on first use and kept.
    fn png_bytes(&self, slot: usize, rgb: bool) -> Result<Arc<Vec<u8>>> {
        let img = match slot {
            0 => Some(&self.diffuse),
            1 => self.emissive.as_ref(),
            2 => self.normal.as_ref(),
            _ => self.pbr.as_ref(),
        }
        .ok_or_else(|| ConvertError::Internal("requested a map that was not built".into()))?;
        let mut guard = self.png.lock().map_err(|_| ConvertError::Internal("png cache lock poisoned".into()))?;
        if let Some(b) = &guard[slot] {
            return Ok(b.clone());
        }
        let bytes = Arc::new(encode_png(img, rgb)?);
        guard[slot] = Some(bytes.clone());
        Ok(bytes)
    }
}

fn perm_or_io(e: std::io::Error, path: &Path) -> ConvertError {
    if e.kind() == std::io::ErrorKind::PermissionDenied {
        ConvertError::PermissionDenied(path.to_path_buf())
    } else {
        ConvertError::Io(e)
    }
}

/// Bake the swatches, pack the atlas and build the emissive / normal / pbr atlases (everything texture related).
fn build_atlas(opts: &ConvertOptions, sc: &Scene, parts: &[Part], ctx: &mut Ctx) -> Result<AtlasEntry> {
    ctx.progress(Stage::Atlas, 0.20, "Building the texture atlas");
    let mut key_index: HashMap<SwatchKey, usize> = HashMap::new();
    let mut swatches: Vec<RgbaImage> = Vec::new();
    let mut real_uv: Vec<bool> = Vec::new();
    let mut slot_mat: Vec<Option<&Material>> = Vec::new();
    let mut part_slot: Vec<usize> = Vec::new();
    for p in parts {
        let m = p.material.map(|i| &sc.materials[i]);
        let key = atlas::material_key(m, &sc.textures);
        let slot = *key_index.entry(key).or_insert_with(|| {
            let (mut img, real) = atlas::material_swatch(m, &sc.textures, opts.apply_base_color_factor);
            let (w, h) = img.dimensions();
            if w > opts.max_tile_size || h > opts.max_tile_size {
                let (nw, nh) = atlas::thumbnail_size(w, h, opts.max_tile_size);
                img = atlas::resize_rgba(&img, nw, nh);
            }
            swatches.push(img);
            real_uv.push(real);
            slot_mat.push(m);
            swatches.len() - 1
        });
        part_slot.push(slot);
    }
    let unique_materials = swatches.len();
    let red_slot = if opts.show_collision {
        swatches.push(RgbaImage::from_pixel(32, 32, image::Rgba([255, 0, 0, 255])));
        real_uv.push(false);
        slot_mat.push(None);
        Some(swatches.len() - 1)
    } else {
        None
    };

    let (atlas_img, placements) = atlas::pack(&swatches);
    ctx.info(DiagCode::AtlasPacked, format!(
        "Packed {} texture(s)/color(s) into a {}x{} atlas ({}px edge padding per tile).",
        swatches.len(), atlas_img.width(), atlas_img.height(), atlas::PAD));
    ctx.check_cancel()?;

    let count = |f: fn(&Material) -> bool| slot_mat.iter().flatten().filter(|m| f(m)).count();
    let emissive_img = if opts.emissive { atlas::build_map(MapKind::Emissive, &slot_mat, &placements, &sc.textures) } else { None };
    if emissive_img.is_some() {
        ctx.info(DiagCode::EmissiveAtlas, format!("Built matching emissive atlas ({}/{} materials have real emission).", count(atlas::has_emission), swatches.len()));
    }
    let normal_img = atlas::build_map(MapKind::Normal { flip_green: opts.flip_normal_green }, &slot_mat, &placements, &sc.textures);
    if normal_img.is_some() {
        ctx.info(DiagCode::NormalAtlas, format!("Built matching normal atlas ({}/{} materials have a normal map).", count(atlas::has_normal), swatches.len()));
    }
    let pbr_img = atlas::build_map(MapKind::Pbr, &slot_mat, &placements, &sc.textures);
    if pbr_img.is_some() {
        ctx.info(DiagCode::PbrAtlas, format!("Built matching pbr atlas ({}/{} materials have roughness data).", count(atlas::has_roughness), swatches.len()));
    }

    let (diffuse, shrunk) = atlas::shrink_to_fit(atlas_img, opts.max_atlas_size);
    if let Some((o, n)) = shrunk {
        ctx.warn(DiagCode::AtlasShrunk, format!("the diffuse atlas came out to {}x{}px, bigger than the {}px limit, so it was shrunk to {}x{}px. Raise the max atlas size if the texture looks too blurry.", o.0, o.1, opts.max_atlas_size, n.0, n.1));
    }
    let emissive_img = emissive_img.map(|i| atlas::shrink_to_fit(i, opts.max_atlas_size).0);
    let normal_img = normal_img.map(|i| atlas::shrink_to_fit(i, opts.max_atlas_size).0);
    let pbr_img = pbr_img.map(|i| atlas::shrink_to_fit(i, opts.max_atlas_size).0);

    Ok(AtlasEntry {
        diffuse,
        emissive: emissive_img,
        normal: normal_img,
        pbr: pbr_img,
        placements,
        real_uv,
        part_slot,
        unique_materials,
        red_slot,
        diags: Vec::new(),
        png: Mutex::new([None, None, None, None]),
    })
}

/// UV cutting plus the OBJ text of the visible mesh (without the name-dependent header).
fn build_geometry(opts: &ConvertOptions, parts: &[Part], a: &AtlasEntry, ctx: &mut Ctx) -> Result<GeoEntry> {
    ctx.progress(Stage::Geometry, 0.35, "Writing geometry");
    let (placements, real_uv, part_slot) = (&a.placements, &a.real_uv, &a.part_slot);
    let mut obj = String::new();
    let mut vt_cache: HashMap<String, usize> = HashMap::new();
    let (mut vt_count, mut vert_offset) = (0usize, 0usize);
    let (mut total_verts, mut total_faces, mut tiled_split, mut tiled_unsplit) = (0usize, 0usize, 0usize, 0usize);
    let mut icon_tris: Vec<IconTri> = Vec::new();

    for (i, p) in parts.iter().enumerate() {
        ctx.check_cancel()?;
        ctx.progress(Stage::Geometry, 0.35 + 0.25 * (i as f32 / parts.len() as f32), format!("Part {} of {}", i + 1, parts.len()));
        let slot = part_slot[i];
        let pl = &placements[slot];
        let verts: Vec<V3> = p.pos.iter().map(|v| mul(*v, opts.scale)).collect();
        let uvs = if real_uv[slot] { p.uv.as_deref() } else { None };
        let g = tiling::build_part(&verts, &p.nrm, &p.tris, uvs);
        tiled_split += g.split;
        tiled_unsplit += g.unsplit;

        let _ = writeln!(obj, "# part_{i}");
        for v in &g.verts {
            let _ = writeln!(obj, "v {:.6} {:.6} {:.6}", v[0], v[1], v[2]);
        }
        for n in &g.nrms {
            let _ = writeln!(obj, "vn {:.6} {:.6} {:.6}", n[0], n[1], n[2]);
        }
        let y_bottom = pl.atlas_h - pl.y_top - pl.h;
        let (eu, ev) = (0.5 / pl.w.max(1) as f64, 0.5 / pl.h.max(1) as f64);
        let (cu, cv) = ((pl.x as f64 + 0.5 * pl.w as f64) / pl.atlas_w as f64, (y_bottom as f64 + 0.5 * pl.h as f64) / pl.atlas_h as f64);
        for f in &g.faces {
            let mut vt = [0usize; 3];
            let mut uv_atlas = [[0.0f64; 2]; 3];
            for k in 0..3 {
                let (u, v) = match &f.uv {
                    None => (cu, cv),
                    Some(c) => {
                        let lu = eu + c[k][0] * (1.0 - 2.0 * eu);
                        let lv = ev + c[k][1] * (1.0 - 2.0 * ev);
                        ((pl.x as f64 + lu * pl.w as f64) / pl.atlas_w as f64, (y_bottom as f64 + lv * pl.h as f64) / pl.atlas_h as f64)
                    }
                };
                uv_atlas[k] = [u, v];
                let key = format!("{u:.6} {v:.6}");
                vt[k] = *vt_cache.entry(key.clone()).or_insert_with(|| {
                    let _ = writeln!(obj, "vt {key}");
                    vt_count += 1;
                    vt_count
                });
            }
            let ix = |k: usize| f.idx[k] + 1 + vert_offset;
            let _ = writeln!(obj, "f {0}/{1}/{0} {2}/{3}/{2} {4}/{5}/{4}", ix(0), vt[0], ix(1), vt[1], ix(2), vt[2]);
            if opts.write_icon {
                icon_tris.push(IconTri { p: [g.verts[f.idx[0]], g.verts[f.idx[1]], g.verts[f.idx[2]]], uv: uv_atlas });
            }
        }
        vert_offset += g.verts.len();
        total_verts += g.verts.len();
        total_faces += g.faces.len();
    }

    Ok(GeoEntry { text: obj, vt_count, vert_offset, total_verts, total_faces, tiled_split, tiled_unsplit, icon_tris })
}

/// Removes a half-written temp folder unless told to keep it.
struct TempDir {
    path: PathBuf,
    keep: bool,
}
impl Drop for TempDir {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

/// Decide the final folder name before anything is written (the name is part of every file name).
fn resolve_target(out_dir: &Path, base: &str, policy: OverwritePolicy) -> Result<String> {
    let target = out_dir.join(base);
    if !target.exists() {
        return Ok(base.to_string());
    }
    match policy {
        OverwritePolicy::Fail => Err(ConvertError::OutputExists { name: base.to_string(), path: target }),
        OverwritePolicy::Replace => {
            if looks_like_previous_output(&target, base) {
                Ok(base.to_string())
            } else {
                Err(ConvertError::OutputFolderNotOurs { name: base.to_string(), path: target })
            }
        }
        OverwritePolicy::Rename => {
            let mut i = 2u32;
            loop {
                let cand = format!("{base}_{i}");
                if !out_dir.join(&cand).exists() {
                    return Ok(cand);
                }
                i += 1;
            }
        }
    }
}

/// Swap the finished temp folder in. The old folder (if any) is moved aside first and removed only after the
/// new one is in place; if anything fails the old one is put back.
fn swap_into_place(tmp: &Path, target: &Path, out_dir: &Path, name: &str) -> Result<()> {
    let pid = std::process::id();
    if target.exists() {
        let old = out_dir.join(format!(".{name}.old{pid}"));
        let _ = std::fs::remove_dir_all(&old);
        std::fs::rename(target, &old).map_err(|e| perm_or_io(e, target))?;
        if let Err(e) = std::fs::rename(tmp, target) {
            let _ = std::fs::rename(&old, target);
            return Err(perm_or_io(e, target));
        }
        let _ = std::fs::remove_dir_all(&old);
    } else {
        std::fs::rename(tmp, target).map_err(|e| perm_or_io(e, target))?;
    }
    Ok(())
}

/// Self-check of what was written: the OBJ's material usage and the files the MTL points at. A failure here is a
/// converter bug, so it is reported as `Internal` instead of handing the game a broken model.
fn verify_output(dir: &Path, name: &str) -> Result<()> {
    let bad = |m: String| Err(ConvertError::Internal(format!("output self-check failed: {m}")));
    let obj = std::fs::read_to_string(dir.join(format!("{name}.obj")))?;
    let mut mat: Option<&str> = None;
    let mut has_ucx = false;
    for l in obj.lines() {
        let t: Vec<&str> = l.split_whitespace().collect();
        match t.first().copied() {
            Some("usemtl") => match t.get(1) {
                Some(m) => mat = Some(m),
                None => return bad("bare usemtl".into()),
            },
            Some("f") if mat.is_none() => return bad("face before any usemtl".into()),
            Some("o") if t.get(1).map(|n| n.starts_with(votv_model_core::UCX_PREFIX)).unwrap_or(false) => has_ucx = true,
            _ => {}
        }
    }
    if !has_ucx {
        return bad("no UCX_ object".into());
    }
    let mtl = std::fs::read_to_string(dir.join(format!("{name}.mtl")))?;
    for l in mtl.lines() {
        let t: Vec<&str> = l.split_whitespace().collect();
        if matches!(t.first().copied(), Some("map_Kd" | "map_Ke" | "map_Bump")) {
            let f = t.get(1).copied().unwrap_or("");
            if !dir.join(f).is_file() {
                return bad(format!("{f} is referenced by the mtl but missing"));
            }
        }
    }
    Ok(())
}

/// Convert with no callbacks. See [`convert_with`].
pub fn convert(opts: &ConvertOptions, input: &Path) -> Result<Report> {
    convert_with(opts, input, &Hooks::default())
}

/// Convert a `.glb` into a VotV printer folder `opts.output_dir/<name>/`.
///
/// Never prints, prompts or exits the process. Blocking: run it on a worker thread and use `hooks` for progress,
/// live messages and cancellation. The finished folder appears atomically (it is built in a hidden
/// `.<name>.tmp<pid>` folder next to it and renamed), so a directory scan never sees a half-written model;
/// scans should ignore names starting with a dot.
pub fn convert_with(opts: &ConvertOptions, input: &Path, hooks: &Hooks) -> Result<Report> {
    opts.validate()?;
    let mut ctx = Ctx::new(hooks);

    let mut input = input.to_path_buf();
    if input.extension().is_none() {
        input.set_extension("glb");
    }
    if !input.exists() {
        return Err(ConvertError::InputNotFound(input));
    }
    let ext = input.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if ext != "glb" {
        return Err(ConvertError::NotGlb(format!(".{ext}")));
    }
    let stem = input.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let base = sanitize_name(opts.output_name.as_deref().filter(|s| !s.is_empty()).unwrap_or(&stem));
    let out_dir = opts.output_dir.clone();
    let name = resolve_target(&out_dir, &base, opts.overwrite)?;
    if name != base {
        ctx.info(DiagCode::OutputRenamed, format!("A folder named {base:?} already exists, so this model was written as {name:?}."));
    }
    let model_dir = out_dir.join(&name);
    // the single material is named after the model so `usemtl <name>` matches `diffuse_<name>.png`
    let mtl_name = name.clone();

    // ---- load (cached per file + humanoid) ----
    ctx.progress(Stage::Loading, 0.0, "Reading the model");
    let cache = hooks.cache.clone();
    let meta = std::fs::metadata(&input);
    let scene_key = SceneKey {
        path: input.clone(),
        mtime: meta.as_ref().ok().and_then(|m| m.modified().ok()),
        len: meta.as_ref().map(|m| m.len()).unwrap_or(0),
        humanoid: opts.humanoid,
        emissive_mode: opts.emissive_mode,
    };
    let scene_entry: Arc<SceneEntry> = match cache.as_ref().and_then(|c| c.scene(&scene_key)) {
        Some(e) => {
            ctx.cache_hits.push("scene");
            let d = e.diags.clone();
            ctx.replay(&d);
            e
        }
        None => {
            let start = ctx.diags.len();
            let loaded = scene::load(&input, opts.humanoid, opts.emissive_mode, &mut ctx)?;
            let diags = ctx.diags[start..].to_vec();
            match &cache {
                Some(c) => c.put_scene(scene_key, loaded, diags),
                None => Arc::new(SceneEntry { id: 0, scene: loaded, diags }),
            }
        }
    };
    let sc: &Scene = &scene_entry.scene;
    ctx.check_cancel()?;
    if opts.humanoid {
        if sc.skinned_primitives == 0 {
            ctx.warn(DiagCode::HumanoidNoSkin, "humanoid mode was requested, but this file doesn't seem to contain any skinned mesh/skeleton data, continuing as a normal model.");
        } else {
            ctx.info(DiagCode::HumanoidApplied, format!("[humanoid] repositioned {} skinned part(s) using skeleton bind data instead of node transforms.", sc.skinned_repositioned));
        }
    } else if sc.skin_count > 0 {
        ctx.attention(DiagCode::SkeletonWithoutHumanoid, "this model contains skeleton/bone data, but humanoid mode wasn't used, if this is a humanoid model you might notice T-posing.");
    }

    ctx.progress(Stage::Preparing, 0.10, "Preparing parts");
    let mut parts: Vec<Part> = sc.parts.clone();
    let mut custom: Vec<Part> = sc.collision_parts.clone();
    if !custom.is_empty() {
        let mut names: Vec<String> = custom.iter().map(|p| p.name.clone()).collect();
        names.sort();
        names.dedup();
        ctx.info(DiagCode::CustomCollisionFound, format!(
            "Found {} custom collision mesh part(s) in the source file (named UCX_*): {}. These will be used as-is instead of auto-generating collision.",
            custom.len(), names.join(", ")));
        if opts.force_generate_collision {
            ctx.warn(DiagCode::CustomCollisionIgnored, "force-generate collision was requested, so the custom UCX_* mesh(es) found above will be ignored, and new collision will be generated from scratch instead.");
            custom.clear();
        }
    }
    if parts.is_empty() {
        return Err(ConvertError::NoGeometry);
    }

    if opts.drop_outline_parts {
        let before = parts.len();
        let mut kept = Vec::new();
        for p in parts {
            let mat_name = p.material.map(|i| sc.materials[i].name.clone()).unwrap_or_default();
            if is_outline_name(&mat_name) || is_outline_name(&p.name) {
                ctx.info(DiagCode::OutlineDropped, format!("[outline] dropping {:?} (material {mat_name:?}): toon outline shells show up as solid dark surfaces in VotV.", p.name));
            } else {
                kept.push(p);
            }
        }
        parts = kept;
        if before != parts.len() && parts.is_empty() {
            return Err(ConvertError::NoGeometry);
        }
    }

    if opts.drop_transparent {
        let before = parts.len();
        let mut kept = Vec::new();
        for p in parts {
            let m = p.material.map(|i| &sc.materials[i]);
            if material_is_transparent(m, sc) {
                let n = m.map(|m| m.name.clone()).unwrap_or_else(|| "material".into());
                ctx.info(DiagCode::TransparentDropped, format!("[transparent] dropping part using material {n:?} (translucent: BLEND with see-through alpha), VotV cannot blend."));
            } else {
                kept.push(p);
            }
        }
        parts = kept;
        if before != parts.len() {
            ctx.info(DiagCode::TransparentSummary, format!("Removed {} transparent part(s).", before - parts.len()));
        }
        if parts.is_empty() {
            return Err(ConvertError::EverythingTransparent);
        }
    }

    {
        let mut seen: Vec<usize> = Vec::new();
        for p in &parts {
            if let Some(mi) = p.material {
                if !seen.contains(&mi) && material_transparency(Some(&sc.materials[mi]), sc) == Transparency::Cutout {
                    seen.push(mi);
                    ctx.info(DiagCode::AlphaCutout, format!("material {:?} is an alpha cutout (MASK): kept. VotV clips alpha below about one third, so the cut-out areas should disappear in game.", sc.materials[mi].name));
                }
            }
        }
    }

    let dups = find_duplicate_parts(&parts, sc, &mut ctx);
    if !dups.is_empty() {
        parts = parts.into_iter().enumerate().filter(|(i, _)| !dups.contains(i)).map(|(_, p)| p).collect();
    }
    ctx.check_cancel()?;

    // ---- atlas (cached) ----
    let akey = AtlasKey {
        scene_id: scene_entry.id,
        drop_transparent: opts.drop_transparent,
        max_tile_size: opts.max_tile_size,
        max_atlas_size: opts.max_atlas_size,
        apply_factor: opts.apply_base_color_factor,
        flip_green: opts.flip_normal_green,
        emissive: opts.emissive,
        show_collision: opts.show_collision,
    };
    let atlas_entry: Arc<AtlasEntry> = match cache.as_ref().and_then(|c| c.atlas(&akey)) {
        Some(a) => {
            ctx.progress(Stage::Atlas, 0.20, "Reusing the texture atlas");
            ctx.cache_hits.push("atlas");
            let d = a.diags.clone();
            ctx.replay(&d);
            a
        }
        None => {
            let start = ctx.diags.len();
            let mut built = build_atlas(opts, sc, &parts, &mut ctx)?;
            built.diags = ctx.diags[start..].to_vec();
            let a = Arc::new(built);
            if let Some(c) = &cache {
                c.put_atlas(akey.clone(), a.clone());
            }
            a
        }
    };
    ctx.check_cancel()?;
    let placements = &atlas_entry.placements;
    let diffuse = &atlas_entry.diffuse;
    let emissive_img = &atlas_entry.emissive;
    let normal_img = &atlas_entry.normal;
    let pbr_img = &atlas_entry.pbr;
    let unique_materials = atlas_entry.unique_materials;
    let red_uv = atlas_entry.red_slot.map(|s| {
        let p = &placements[s];
        ((p.x as f64 + 0.5 * p.w as f64) / p.atlas_w as f64, (((p.atlas_h - p.y_top - p.h) as f64) + 0.5 * p.h as f64) / p.atlas_h as f64)
    });

    // ---- model geometry (cached) ----
    let gkey = GeoKey { atlas: akey.clone(), scale_bits: opts.scale.to_bits(), write_icon: opts.write_icon };
    let geo: Arc<GeoEntry> = match cache.as_ref().and_then(|c| c.geometry(&gkey)) {
        Some(g) => {
            ctx.progress(Stage::Geometry, 0.35, "Reusing the model geometry");
            ctx.cache_hits.push("geometry");
            g
        }
        None => {
            let g = Arc::new(build_geometry(opts, &parts, &atlas_entry, &mut ctx)?);
            if let Some(c) = &cache {
                c.put_geometry(gkey, g.clone());
            }
            g
        }
    };
    let mut obj = String::with_capacity(geo.text.len() + 4096);
    let _ = writeln!(obj, "mtllib {name}.mtl");
    let _ = writeln!(obj, "usemtl {mtl_name}");
    obj.push_str(&geo.text);
    let mut vt_count = geo.vt_count;
    let (vert_offset, total_verts, total_faces) = (geo.vert_offset, geo.total_verts, geo.total_faces);
    let (tiled_split, tiled_unsplit) = (geo.tiled_split, geo.tiled_unsplit);
    let icon_tris = &geo.icon_tris;

    // ---- collision (the game needs at least one UCX_ object, so there is always one) ----
    ctx.progress(Stage::Collision, 0.60, "Collision");
    let mut blocks: Vec<(String, Mesh)> = Vec::new();
    let mut stats = Stats::default();
    let off = |m: &Mesh| Mesh { verts: m.verts.iter().map(|v| [v[0] + opts.collision_offset_x, v[1] + opts.collision_offset_y, v[2]]).collect(), faces: m.faces.clone() };
    let box_block = |parts: &[Part]| collision::fallback_box(parts, opts.fallback_collision_size, opts.scale, opts.collision_offset_x, opts.collision_offset_y);

    if !custom.is_empty() {
        let names = collision::unique_names(custom.iter().map(|c| collision::sanitize_obj_object_name(&c.name)));
        for (c, n) in custom.iter().zip(names) {
            let m = Mesh { verts: c.pos.iter().map(|v| mul(*v, opts.scale)).collect(), faces: c.tris.clone() };
            blocks.push((n, off(&m)));
        }
        stats.collision_from_source = true;
        let (v, f): (usize, usize) = blocks.iter().fold((0, 0), |a, b| (a.0 + b.1.verts.len(), a.1 + b.1.faces.len()));
        ctx.info(DiagCode::CustomCollisionUsed, format!("Used {} custom collision mesh(es) from the source file instead of auto-generating collision: {v} verts, {f} faces total.", blocks.len()));
        if !opts.generate_collision {
            ctx.info(DiagCode::NoCollCustomKept, "(auto-generated collision was turned off, but it only suppresses generated collision, the custom UCX_ mesh(es) found in the source file are still used.)");
        }
    } else if opts.generate_collision {
        let high = opts.collision_quality == crate::options::CollisionQuality::High;
        let hkey = HullKey {
            scene_id: scene_entry.id,
            drop_transparent: opts.drop_transparent,
            quality: high as u8,
            resolution: if high { opts.vhacd_resolution } else { 0 },
            max_hulls: if high { opts.max_decomp_hulls } else { 0 },
        };
        let hull_entry: Arc<HullEntry> = match cache.as_ref().and_then(|c| c.hulls(&hkey)) {
            Some(h) => {
                ctx.cache_hits.push("collision");
                h
            }
            None => {
                // stored right away, so a run that is cancelled just after this step still leaves its result behind
                let h = Arc::new(HullEntry { raw: collision::generate_raw(&parts, opts) });
                if let Some(c) = &cache {
                    c.put_hulls(hkey, h.clone());
                }
                h
            }
        };
        match &hull_entry.raw {
            Ok(raw) => {
                let g = collision::trim(raw, opts.hull_face_cap());
                if let Some(e) = &g.decomposition_error {
                    ctx.warn(DiagCode::DecompositionFailed, format!("convex decomposition failed on this model's geometry ({e}). Falling back to a single convex hull ('low' quality) for this print instead."));
                } else if opts.collision_quality == crate::options::CollisionQuality::High {
                    ctx.info(DiagCode::CollisionDecomposed, format!("High-quality collision: decomposed into {} convex hull(s).", g.hulls.len()));
                }
                if g.hulls_over_cap > 0 {
                    ctx.info(DiagCode::HullsTrimmed, format!("{} collision hull(s) had more than {} faces (high-facet convex colliders are a common cause of physics jitter) and were simplified.", g.hulls_over_cap, opts.hull_face_cap()));
                }
                let n = g.hulls.len();
                for (i, h) in g.hulls.iter().enumerate() {
                    let nm = if n == 1 { format!("UCX_{name}") } else { format!("UCX_{name}_{i:02}") };
                    let scaled = Mesh { verts: h.verts.iter().map(|v| mul(*v, opts.scale)).collect(), faces: h.faces.clone() };
                    blocks.push((nm, off(&scaled)));
                }
                let (v, f): (usize, usize) = blocks.iter().fold((0, 0), |a, b| (a.0 + b.1.verts.len(), a.1 + b.1.faces.len()));
                ctx.info(DiagCode::CollisionAdded, format!("Added {} UCX_ collision hull(s): {v} verts, {f} faces total.", blocks.len()));
            }
            Err(e) => {
                ctx.warn(DiagCode::CollisionFailed, format!("could not generate a collision shape for this model ({e}). Adding a fallback box collision at the base instead so it doesn't fall through the floor in-game."));
            }
        }
    }
    if blocks.is_empty() {
        if let Some(b) = box_block(&parts) {
            blocks.push((format!("UCX_{name}"), b));
            ctx.info(DiagCode::CollisionBoxAdded, format!("Added a {:.1}cm box collision at the base of the model. VotV needs at least one UCX_ object, otherwise the print skips health, edible and bites.", opts.fallback_collision_size));
        }
    }
    ctx.check_cancel()?;

    // show-collision overlay: a red, double-sided, pushed-out copy of the collision in the visible mesh
    let mut coll_base = vert_offset;
    if opts.show_collision {
        let (mut mn, mut mx) = ([f64::MAX; 3], [f64::MIN; 3]);
        for p in &parts {
            for v in &p.pos {
                for k in 0..3 {
                    mn[k] = mn[k].min(v[k] * opts.scale);
                    mx[k] = mx[k].max(v[k] * opts.scale);
                }
            }
        }
        // just enough to clear the surface (z-fighting), not a visible shell; see `show_collision_offset`
        let push = opts.show_collision_offset.unwrap_or_else(|| (0.002 * len(sub(mx, mn))).clamp(0.05, 0.5));
        let (ru, rv) = red_uv.unwrap();
        vt_count += 1;
        let red_vt = vt_count;
        let _ = writeln!(obj, "# --show-coll: red copy of the collision shapes, pushed outward, next to the model (for checking only)");
        let _ = writeln!(obj, "vt {ru:.6} {rv:.6}");
        let mut red_v: Vec<V3> = Vec::new();
        let mut red_f: Vec<[usize; 3]> = Vec::new();
        for (_, m) in &blocks {
            let inf = collision::inflate(m, push);
            let b = red_v.len();
            red_v.extend(inf);
            red_f.extend(m.faces.iter().map(|f| [f[0] as usize + b, f[1] as usize + b, f[2] as usize + b]));
        }
        for v in &red_v {
            let _ = writeln!(obj, "v {:.6} {:.6} {:.6}", v[0], v[1], v[2]);
        }
        let vn_base = total_verts + 1;
        for f in &red_f {
            let n = cross(sub(red_v[f[1]], red_v[f[0]]), sub(red_v[f[2]], red_v[f[0]]));
            let l = len(n);
            let n = if l > 1e-12 { mul(n, 1.0 / l) } else { [0.0, 0.0, 1.0] };
            let _ = writeln!(obj, "vn {:.6} {:.6} {:.6}", n[0], n[1], n[2]);
            let _ = writeln!(obj, "vn {:.6} {:.6} {:.6}", -n[0], -n[1], -n[2]);
        }
        for (i, f) in red_f.iter().enumerate() {
            let (a, b, c) = (f[0] + vert_offset + 1, f[1] + vert_offset + 1, f[2] + vert_offset + 1);
            let (nf, nb) = (vn_base + 2 * i, vn_base + 2 * i + 1);
            let _ = writeln!(obj, "f {a}/{red_vt}/{nf} {b}/{red_vt}/{nf} {c}/{red_vt}/{nf}");
            let _ = writeln!(obj, "f {a}/{red_vt}/{nb} {c}/{red_vt}/{nb} {b}/{red_vt}/{nb}");
        }
        coll_base = vert_offset + red_v.len();
        ctx.info(DiagCode::ShowColl, format!("show-collision: added a red copy of the collision ({} faces), pushed {push:.2}cm outward. Red only shows where the collision reaches the model's surface; collision hidden inside the model stays hidden. For checking only, leave it off for the final model.", 2 * red_f.len()));
    }

    for (n, m) in &blocks {
        let _ = writeln!(obj, "o {n}");
        for v in &m.verts {
            let _ = writeln!(obj, "v {:.6} {:.6} {:.6}", v[0], v[1], v[2]);
        }
        for f in &m.faces {
            let _ = writeln!(obj, "f {} {} {}", f[0] as usize + 1 + coll_base, f[1] as usize + 1 + coll_base, f[2] as usize + 1 + coll_base);
        }
        coll_base += m.verts.len();
        stats.collision_vertices += m.verts.len();
        stats.collision_faces += m.faces.len();
    }
    stats.collision_objects = blocks.len();
    stats.hulls = blocks.len();
    ctx.check_cancel()?;

    // ---- write into a hidden temp folder, then swap it in ----
    ctx.progress(Stage::Writing, 0.90, "Writing files");
    std::fs::create_dir_all(&out_dir).map_err(|e| perm_or_io(e, &out_dir))?;
    let tmp_path = out_dir.join(format!(".{name}.tmp{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp_path);
    std::fs::create_dir_all(&tmp_path).map_err(|e| perm_or_io(e, &tmp_path))?;
    let mut tmp = TempDir { path: tmp_path.clone(), keep: false };

    let tex = |k: TextureKind| texture_file_name(k, &mtl_name);
    let diffuse_name = tex(TextureKind::Diffuse);
    let emissive_name = emissive_img.as_ref().map(|_| tex(TextureKind::Emissive));
    let normal_name = normal_img.as_ref().map(|_| tex(TextureKind::Normal));
    let pbr_name = pbr_img.as_ref().map(|_| tex(TextureKind::Pbr));
    write_png_bytes(&tmp_path.join(&diffuse_name), &atlas_entry.png_bytes(0, false)?)?;
    if let Some(n) = &emissive_name { write_png_bytes(&tmp_path.join(n), &atlas_entry.png_bytes(1, false)?)?; }
    if let Some(n) = &normal_name { write_png_bytes(&tmp_path.join(n), &atlas_entry.png_bytes(2, true)?)?; }
    if let Some(n) = &pbr_name { write_png_bytes(&tmp_path.join(n), &atlas_entry.png_bytes(3, true)?)?; }

    let write = |file: String, text: &str| -> Result<()> {
        std::fs::write(tmp_path.join(&file), text).map_err(|e| perm_or_io(e, &tmp_path))
    };
    let mut mtl = format!("newmtl {mtl_name}\nKd 1.0000 1.0000 1.0000\nKa 0.2 0.2 0.2\nKs 0.1 0.1 0.1\nNs 10.0\nd 1.0000\nmap_Kd {diffuse_name}\n");
    if let Some(n) = &emissive_name { mtl.push_str(&format!("map_Ke {n}\n")); }
    if let Some(n) = &normal_name { mtl.push_str(&format!("map_Bump {n}\n")); }
    mtl.push('\n'); // the Python version ends the file with a blank line; kept for byte parity
    write(format!("{name}.mtl"), &mtl)?;

    // properties.cfg: keep an existing one when asked to, else write from the options
    let preserved = if opts.keep_existing_properties && model_dir.join("properties.cfg").is_file() && opts.overwrite == OverwritePolicy::Replace && name == base {
        std::fs::read(model_dir.join("properties.cfg")).ok()
    } else {
        None
    };
    let properties_kept = preserved.is_some();
    let mut effective_props = opts.properties.clone();
    if let Some(bytes) = preserved {
        std::fs::write(tmp_path.join("properties.cfg"), &bytes).map_err(|e| perm_or_io(e, &tmp_path))?;
        ctx.info(DiagCode::PropertiesKept, "Kept the existing properties.cfg (delete the whole output folder, or turn off keep-existing-properties, to write a fresh one).");
        if opts.properties != Default::default() {
            ctx.warn(DiagCode::PropertiesOptionsIgnored, "the properties settings (physical material, durability, lamp, filters, emissive strength, ...) were not applied, because the existing properties.cfg was kept.");
        }
        effective_props = PropertiesCfg::parse(&String::from_utf8_lossy(&bytes));
    } else {
        write("properties.cfg".into(), &opts.properties.to_cfg_string(emissive_img.is_some()))?;
    }

    let mut extras: Vec<String> = Vec::new();
    if opts.write_icon {
        let img = icon::render(&icon_tris, &diffuse, opts.icon_size);
        save_png(&img, false, &tmp_path.join("icon.png"))?;
        extras.push("icon.png".into());
        ctx.info(DiagCode::IconWritten, format!("Wrote a {0}x{0} icon.png preview.", opts.icon_size));
    }
    if opts.write_sidecar {
        let side = serde_json::json!({
            "converter": "votv-convert",
            "converter_version": env!("CARGO_PKG_VERSION"),
            "source": input.to_string_lossy(),
            "name": name,
            "unix_time": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
            "options": opts,
        });
        write("votv_convert.json".into(), &serde_json::to_string_pretty(&side).unwrap_or_default())?;
        extras.push("votv_convert.json".into());
        ctx.info(DiagCode::SidecarWritten, "Wrote votv_convert.json (source path, options, converter version).");
    }

    // the .obj goes in last: a folder scan that does not skip hidden folders only lists folders with an .obj
    write(format!("{name}.obj"), &obj)?;
    verify_output(&tmp_path, &name)?;
    ctx.check_cancel()?;
    swap_into_place(&tmp_path, &model_dir, &out_dir, &name)?;
    tmp.keep = true;

    if tiled_split > 0 {
        ctx.info(DiagCode::TiledSplit, format!("Tiled textures: {tiled_split} faces were cut along texture-repeat lines to keep tiling intact."));
    }
    if tiled_unsplit > 0 {
        ctx.warn(DiagCode::TiledUnsplit, format!("{tiled_unsplit} faces repeat their texture more than {} times and were left squeezed into a single tile (they may look stretched).", tiling::MAX_UV_CELLS_PER_FACE));
    }
    ctx.info(DiagCode::Summary, format!("Parts: {}  Unique materials baked: {}  Verts: {}  Faces: {}", parts.len(), unique_materials, total_verts, total_faces));

    stats.parts = parts.len();
    stats.unique_materials = unique_materials;
    stats.vertices = total_verts;
    stats.faces = total_faces;
    stats.triangles = total_faces;
    stats.sections = 1;
    stats.atlas_width = diffuse.width();
    stats.atlas_height = diffuse.height();
    stats.tiled_faces_split = tiled_split;
    stats.tiled_faces_unsplit = tiled_unsplit;
    stats.has_emissive = emissive_img.is_some();
    stats.has_normal = normal_img.is_some();
    stats.has_pbr = pbr_img.is_some();
    {
        // what the game will derive from the collision hulls (printedObject.setProps)
        let (mut mn, mut mx) = ([f64::MAX; 3], [f64::MIN; 3]);
        for (_, m) in &blocks {
            for v in &m.verts {
                for k in 0..3 {
                    mn[k] = mn[k].min(v[k]);
                    mx[k] = mx[k].max(v[k]);
                }
            }
        }
        if mn[0] <= mx[0] {
            let size = [mx[0] - mn[0], mx[1] - mn[1], mx[2] - mn[2]];
            stats.hull_size_cm = Some(size);
            stats.bites = bites_for(size);
            stats.health_in_game = effective_props.health_in_game(size);
        }
        stats.edible = effective_props.is_edible();
    }

    let mut files: Vec<OutputFile> = std::fs::read_dir(&model_dir)?
        .flatten()
        .map(|e| OutputFile { name: e.file_name().to_string_lossy().to_string(), bytes: e.metadata().map(|m| m.len()).unwrap_or(0) })
        .collect();
    files.sort_by(|a, b| a.name.cmp(&b.name));

    ctx.progress(Stage::Done, 1.0, "Done");
    let cache_hits: Vec<String> = ctx.cache_hits.iter().map(|h| h.to_string()).collect();
    Ok(Report { model_dir, name, files, stats, diagnostics: ctx.diags, properties_kept, extras, cache_hits })
}
