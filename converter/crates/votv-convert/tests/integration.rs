use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use votv_convert::*;

fn data(n: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data").join(format!("{n}.glb"))
}

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("votv_convert_test_{}_{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn no_coll() -> ConvertOptions {
    ConvertOptions { generate_collision: false, ..Default::default() }
}

fn has(r: &Report, code: &str) -> bool {
    r.diagnostics.iter().any(|d| d.code.as_str() == code)
}

/// Options + input + target name in one call.
fn conv(input: &Path, name: Option<&str>, out: &Path, o: &ConvertOptions) -> Result<Report> {
    conv_with(input, name, out, o, &Hooks::default())
}

fn conv_with(input: &Path, name: Option<&str>, out: &Path, o: &ConvertOptions, h: &Hooks) -> Result<Report> {
    let mut o = o.clone();
    o.output_dir = out.to_path_buf();
    o.output_name = name.map(String::from);
    convert_with(&o, input, h)
}

/// Every face index in the OBJ must point at an existing vertex / uv / normal.
fn check_obj(path: &Path) {
    let text = std::fs::read_to_string(path).unwrap();
    let (mut v, mut vt, mut vn) = (0usize, 0usize, 0usize);
    for l in text.lines() {
        let t: Vec<&str> = l.split_whitespace().collect();
        match t.first().copied() {
            Some("v") => v += 1,
            Some("vt") => vt += 1,
            Some("vn") => vn += 1,
            Some("f") => {
                assert_eq!(t.len(), 4, "triangles only: {l}");
                for c in &t[1..] {
                    let p: Vec<&str> = c.split('/').collect();
                    assert!((1..=v).contains(&p[0].parse::<usize>().unwrap()), "bad v index: {l}");
                    if p.len() > 1 && !p[1].is_empty() { assert!((1..=vt).contains(&p[1].parse::<usize>().unwrap()), "bad vt: {l}"); }
                    if p.len() > 2 { assert!((1..=vn).contains(&p[2].parse::<usize>().unwrap()), "bad vn: {l}"); }
                }
            }
            _ => {}
        }
    }
}

#[test]
fn basic_model_writes_all_maps() {
    let out = tmp("basic");
    let r = conv(&data("basic"), None, &out, &ConvertOptions { collision_quality: CollisionQuality::Low, ..Default::default() }).unwrap();
    assert_eq!(r.name, "basic");
    for f in ["basic.obj", "basic.mtl", "diffuse_basic.png", "emissive_basic.png", "normal_basic.png", "pbr_basic.png", "properties.cfg"] {
        assert!(out.join("basic").join(f).is_file(), "missing {f}");
    }
    assert_eq!((r.stats.atlas_width, r.stats.atlas_height), (136, 144));
    assert_eq!(r.stats.unique_materials, 2);
    assert!(r.stats.has_emissive && r.stats.has_normal && r.stats.has_pbr);
    assert_eq!(r.stats.collision_objects, 1);
    assert_eq!((r.stats.sections, r.stats.hulls, r.stats.triangles), (1, 1, r.stats.faces));
    check_obj(&out.join("basic/basic.obj"));
}

#[test]
fn custom_ucx_collision_is_used_and_can_be_overridden() {
    let out = tmp("ucx");
    let r = conv(&data("ucx"), None, &out, &no_coll()).unwrap();
    assert!(r.stats.collision_from_source);
    assert_eq!(r.stats.collision_objects, 2);
    assert_eq!(r.stats.parts, 1);
    check_obj(&out.join("ucx/ucx.obj"));
    let r = conv(&data("ucx"), Some("forced"), &out, &ConvertOptions { force_generate_collision: true, collision_quality: CollisionQuality::Low, ..Default::default() }).unwrap();
    assert!(!r.stats.collision_from_source);
    assert!(has(&r, "custom_collision_ignored"));
}

#[test]
fn tiled_uvs_are_cut() {
    let out = tmp("tiled");
    let r = conv(&data("tiled"), None, &out, &no_coll()).unwrap();
    assert!(r.stats.tiled_faces_split > 0);
    assert!(r.stats.faces > 12);
    check_obj(&out.join("tiled/tiled.obj"));
}

#[test]
fn transparent_parts_can_be_dropped() {
    let out = tmp("glass");
    let keep = conv(&data("glass"), None, &out, &no_coll()).unwrap();
    assert_eq!(keep.stats.parts, 2);
    let drop = conv(&data("glass"), Some("glass2"), &out, &ConvertOptions { drop_transparent: true, ..no_coll() }).unwrap();
    assert_eq!(drop.stats.parts, 1);
    assert!(has(&drop, "transparent_dropped"));
}

#[test]
fn rigged_models_warn_unless_humanoid() {
    let out = tmp("rig");
    let plain = conv(&data("rigged"), None, &out, &no_coll()).unwrap();
    assert!(plain.has_attention() && has(&plain, "skeleton_without_humanoid"));
    let h = conv(&data("rigged"), Some("posed"), &out, &ConvertOptions { humanoid: true, ..no_coll() }).unwrap();
    assert!(has(&h, "humanoid_applied") && !h.has_attention());
    let width = |n: &str| {
        let t = std::fs::read_to_string(out.join(n).join(format!("{n}.obj"))).unwrap();
        let xs: Vec<f64> = t.lines().filter(|l| l.starts_with("v ")).map(|l| l.split_whitespace().nth(1).unwrap().parse().unwrap()).collect();
        xs.iter().cloned().fold(f64::MIN, f64::max) - xs.iter().cloned().fold(f64::MAX, f64::min)
    };
    assert!(width("posed") > width("rigged") * 2.0);
}

#[test]
fn duplicate_parts_are_removed() {
    let out = tmp("misc");
    let r = conv(&data("misc"), None, &out, &no_coll()).unwrap();
    assert!(has(&r, "duplicate_part"));
    assert_eq!(r.stats.parts, 3);
    check_obj(&out.join("misc/misc.obj"));
}

#[test]
fn generated_collision_keeps_the_hole_of_a_torus() {
    let out = tmp("torus");
    let r = conv(&data("torus"), None, &out, &ConvertOptions::default()).unwrap();
    assert!(r.stats.collision_objects > 1, "a torus must be decomposed");
    let text = std::fs::read_to_string(out.join("torus/torus.obj")).unwrap();
    let mut verts: Vec<[f64; 3]> = vec![];
    let mut cur: Option<Vec<[usize; 3]>> = None;
    let mut blocks: Vec<Vec<[usize; 3]>> = vec![];
    for l in text.lines() {
        let t: Vec<&str> = l.split_whitespace().collect();
        match t.first().copied() {
            Some("v") => verts.push([t[1].parse().unwrap(), t[2].parse().unwrap(), t[3].parse().unwrap()]),
            Some("o") => { if let Some(b) = cur.take() { blocks.push(b); } cur = Some(vec![]); }
            Some("f") => if let Some(b) = cur.as_mut() { b.push([t[1].parse::<usize>().unwrap() - 1, t[2].parse::<usize>().unwrap() - 1, t[3].parse::<usize>().unwrap() - 1]); },
            _ => {}
        }
    }
    blocks.extend(cur);
    for b in &blocks {
        // origin = centre of the hole; it is "inside" a convex hull if it is behind every outward face
        let inside = b.iter().all(|f| {
            let (a, bb, c) = (verts[f[0]], verts[f[1]], verts[f[2]]);
            let u = [bb[0] - a[0], bb[1] - a[1], bb[2] - a[2]];
            let w = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let n = [u[1] * w[2] - u[2] * w[1], u[2] * w[0] - u[0] * w[2], u[0] * w[1] - u[1] * w[0]];
            (-a[0]) * n[0] + (-a[1]) * n[1] + (-a[2]) * n[2] < 0.0
        });
        assert!(!inside, "a hull fills the torus hole");
    }
}

#[test]
fn show_collision_adds_an_overlay() {
    let out = tmp("show");
    let r = conv(&data("ucx"), None, &out, &ConvertOptions { show_collision: true, ..Default::default() }).unwrap();
    assert!(has(&r, "show_coll"));
    check_obj(&out.join("ucx/ucx.obj"));
}

#[test]
fn existing_properties_are_kept_and_foreign_folders_refused() {
    let out = tmp("props");
    conv(&data("ucx"), None, &out, &no_coll()).unwrap();
    std::fs::write(out.join("ucx/properties.cfg"), "custom=1\n").unwrap();
    let mut edible = no_coll();
    edible.properties.physical_material = 8;
    let r = conv(&data("ucx"), None, &out, &edible).unwrap();
    assert!(r.properties_kept && has(&r, "properties_options_ignored"));
    assert_eq!(std::fs::read_to_string(out.join("ucx/properties.cfg")).unwrap(), "custom=1\n");
    std::fs::create_dir_all(out.join("mine")).unwrap();
    std::fs::write(out.join("mine/notes.txt"), "keep me").unwrap();
    let e = conv(&data("ucx"), Some("mine"), &out, &no_coll()).unwrap_err();
    assert_eq!(e.kind(), "output_folder_not_ours");
    assert!(out.join("mine/notes.txt").exists());
}

#[test]
fn progress_is_monotonic_and_cancel_works() {
    let out = tmp("prog");
    let seen: Arc<Mutex<Vec<f32>>> = Default::default();
    let s2 = seen.clone();
    let hooks = Hooks::default().with_progress(move |p| s2.lock().unwrap().push(p.fraction));
    conv_with(&data("basic"), None, &out, &no_coll(), &hooks).unwrap();
    let v = seen.lock().unwrap();
    assert!(v.len() > 3 && v.windows(2).all(|w| w[0] <= w[1]) && *v.last().unwrap() == 1.0);
    let token = CancelToken::new();
    token.cancel();
    let e = conv_with(&data("basic"), Some("never"), &out, &no_coll(), &Hooks::default().with_cancel(token)).unwrap_err();
    assert_eq!(e.kind(), "cancelled");
    assert!(!out.join("never").exists());
}

#[test]
fn bad_inputs_give_clear_errors() {
    let out = tmp("bad");
    assert_eq!(conv(Path::new("nope.glb"), None, &out, &no_coll()).unwrap_err().kind(), "input_not_found");
    let txt = out.join("a.txt");
    std::fs::write(&txt, "x").unwrap();
    assert_eq!(conv(&txt, None, &out, &no_coll()).unwrap_err().kind(), "not_glb");
    let fake = out.join("fake.glb");
    std::fs::write(&fake, "not a glb").unwrap();
    assert_eq!(conv(&fake, None, &out, &no_coll()).unwrap_err().kind(), "unreadable");
    let bad = ConvertOptions { scale: -1.0, ..no_coll() };
    assert_eq!(conv(&data("basic"), None, &out, &bad).unwrap_err().kind(), "invalid_option");
}

#[test]
fn inspect_reports_contents() {
    let i = inspect(&data("rigged")).unwrap();
    assert_eq!(i.skins, 1);
    assert!(i.hints.iter().any(|h| h.code.as_str() == "rigged"));
    assert!(inspect(&data("glass")).unwrap().any_transparent);
    let i = inspect(&data("basic")).unwrap();
    assert_eq!(i.materials.len(), 2);
}

#[test]
fn options_roundtrip_through_json() {
    let o = ConvertOptions { scale: 1.0, humanoid: true, collision_quality: CollisionQuality::Low, max_hull_faces: Some(50), ..Default::default() };
    let j = serde_json::to_string(&o).unwrap();
    assert_eq!(serde_json::from_str::<ConvertOptions>(&j).unwrap(), o);
    assert_eq!(serde_json::from_str::<ConvertOptions>("{}").unwrap(), ConvertOptions::default());
}

#[test]
fn sanitize_matches_python_rules() {
    assert_eq!(sanitize_name("  My Cool-Model (v2)! "), "my_cool_model_v2");
    assert_eq!(sanitize_name("***"), "model");
}

#[test]
fn material_is_named_after_the_model_and_matches_texture_files() {
    let out = tmp("matname");
    let r = conv(&data("basic"), Some("My Prop"), &out, &ConvertOptions { collision_quality: CollisionQuality::Low, ..Default::default() }).unwrap();
    assert_eq!(r.name, "my_prop");
    let dir = out.join("my_prop");
    let obj = std::fs::read_to_string(dir.join("my_prop.obj")).unwrap();
    let mtl = std::fs::read_to_string(dir.join("my_prop.mtl")).unwrap();
    assert!(obj.lines().any(|l| l == "usemtl my_prop"));
    assert!(!obj.lines().any(|l| l.trim() == "usemtl"));
    assert!(mtl.contains("newmtl my_prop") && mtl.contains("map_Kd diffuse_my_prop.png"));
    assert!(dir.join("normal_my_prop.png").is_file() && dir.join("pbr_my_prop.png").is_file());
}

#[test]
fn a_ucx_hull_is_always_written_and_tobj_can_load_it() {
    let out = tmp("alwaysucx");
    let r = conv(&data("tiled"), None, &out, &no_coll()).unwrap();
    assert!(r.stats.hulls >= 1 && has(&r, "collision_box_added"));
    let (models, mats) = tobj::load_obj(out.join("tiled/tiled.obj"), &tobj::LoadOptions { triangulate: true, single_index: true, ..Default::default() }).unwrap();
    assert!(models.len() >= 2);
    assert!(models[0].mesh.material_id.is_some());
    let mats = mats.unwrap();
    assert_eq!(mats[0].name, "tiled");
    assert_eq!(mats[0].diffuse_texture.as_deref(), Some("diffuse_tiled.png"));
}

#[test]
fn overwrite_policies_and_atomic_output() {
    let out = tmp("overwrite");
    conv(&data("ucx"), None, &out, &no_coll()).unwrap();
    let fail = ConvertOptions { overwrite: OverwritePolicy::Fail, ..no_coll() };
    assert_eq!(conv(&data("ucx"), None, &out, &fail).unwrap_err().kind(), "output_exists");
    let ren = ConvertOptions { overwrite: OverwritePolicy::Rename, ..no_coll() };
    let r = conv(&data("ucx"), None, &out, &ren).unwrap();
    assert_eq!(r.name, "ucx_2");
    assert!(out.join("ucx_2/ucx_2.obj").is_file() && has(&r, "output_renamed"));
    let left: Vec<String> = std::fs::read_dir(&out).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
    assert!(left.iter().all(|n| !n.starts_with('.')), "{left:?}");
}

#[test]
fn fresh_properties_come_from_the_options() {
    let out = tmp("freshprops");
    conv(&data("ucx"), None, &out, &no_coll()).unwrap();
    std::fs::write(out.join("ucx/properties.cfg"), "custom=1\n").unwrap();
    let mut o = ConvertOptions { keep_existing_properties: false, ..no_coll() };
    o.properties.physical_material = 3;
    let r = conv(&data("ucx"), None, &out, &o).unwrap();
    assert!(!r.properties_kept);
    let cfg = votv_convert::model_core::PropertiesCfg::parse(&std::fs::read_to_string(out.join("ucx/properties.cfg")).unwrap());
    assert_eq!(cfg.physical_material, 3);
}

#[test]
fn icon_and_sidecar_are_optional_extras() {
    let out = tmp("extras");
    let o = ConvertOptions { write_icon: true, icon_size: 64, write_sidecar: true, ..no_coll() };
    let r = conv(&data("basic"), None, &out, &o).unwrap();
    assert_eq!(r.extras, vec!["icon.png".to_string(), "votv_convert.json".to_string()]);
    let icon = image::open(out.join("basic/icon.png")).unwrap();
    assert_eq!((icon.width(), icon.height()), (64, 64));
    let side: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(out.join("basic/votv_convert.json")).unwrap()).unwrap();
    assert_eq!(side["name"], "basic");
    let plain = conv(&data("basic"), Some("plain"), &out, &no_coll()).unwrap();
    assert!(plain.extras.is_empty());
}

#[test]
fn specular_glossiness_models_load_with_their_diffuse_texture() {
    let out = tmp("specgloss");
    let r = conv(&data("specgloss"), None, &out, &no_coll()).unwrap();
    assert!(has(&r, "spec_gloss_converted"));
    assert_eq!((r.stats.atlas_width, r.stats.atlas_height), (136, 72)); // the 128x64 diffuse texture is used, not a flat colour
    assert_eq!(inspect(&data("specgloss")).unwrap().materials[0].base_texture, Some((128, 64)));
}

#[test]
fn unknown_required_extensions_are_ignored_with_a_warning() {
    let out = tmp("reqext");
    let r = conv(&data("required_unknown"), None, &out, &no_coll()).unwrap();
    assert!(has(&r, "unsupported_extension_ignored"));
    assert!(r.stats.faces > 0);
}

// ---------------------------------------------------------------- cache

fn hits(r: &Report) -> Vec<&str> {
    r.cache_hits.iter().map(|s| s.as_str()).collect()
}

fn dir_bytes(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut v: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| (e.file_name().to_string_lossy().to_string(), std::fs::read(e.path()).unwrap()))
        .collect();
    v.sort();
    v
}

#[test]
fn cached_output_is_byte_identical_to_uncached() {
    let (o1, o2, o3) = (tmp("cache_a"), tmp("cache_b"), tmp("cache_c"));
    let cache = ConvertCache::new();
    // single-hull collision: byte comparison must not depend on VHACD being deterministic
    let opts = ConvertOptions { write_icon: true, collision_quality: CollisionQuality::Low, ..Default::default() };
    let cached = |out: &Path| conv_with(&data("basic"), Some("same"), out, &opts, &Hooks::default().with_cache(cache.clone())).unwrap();
    let first = cached(&o1);
    let second = cached(&o2);
    let plain = conv(&data("basic"), Some("same"), &o3, &opts).unwrap();
    assert!(first.cache_hits.is_empty() && plain.cache_hits.is_empty());
    assert_eq!(hits(&second), ["scene", "atlas", "geometry", "collision"]);
    assert_eq!(dir_bytes(&o1.join("same")), dir_bytes(&o3.join("same")));
    assert_eq!(dir_bytes(&o2.join("same")), dir_bytes(&o3.join("same")));
    // the reused run still reports the same messages
    let codes = |r: &Report| r.diagnostics.iter().map(|d| d.code.as_str()).collect::<Vec<_>>();
    assert_eq!(codes(&first), codes(&second));
}

#[test]
fn each_option_only_invalidates_the_stages_that_depend_on_it() {
    let out = tmp("cache_inval");
    let cache = ConvertCache::new();
    let run = |o: &ConvertOptions| conv_with(&data("basic"), Some("m"), &out, o, &Hooks::default().with_cache(cache.clone())).unwrap();
    let base = ConvertOptions::default();
    run(&base);

    let mut o = base.clone();
    o.collision_offset_x = 3.0;
    o.fallback_collision_size = 10.0;
    o.show_collision_offset = Some(1.0);
    o.properties.physical_material = 3;
    o.properties.emissive_strength = 2.0;
    assert_eq!(hits(&run(&o)), ["scene", "atlas", "geometry", "collision"], "offsets and cfg values reuse everything");

    let mut o = base.clone();
    o.max_hull_faces = Some(20);
    assert_eq!(hits(&run(&o)), ["scene", "atlas", "geometry", "collision"], "the face cap is applied after the cached hulls");

    let mut o = base.clone();
    o.scale = 50.0;
    assert_eq!(hits(&run(&o)), ["scene", "atlas", "collision"], "scale changes the geometry only");

    let mut o = base.clone();
    o.max_atlas_size = 64;
    assert_eq!(hits(&run(&o)), ["scene", "collision"], "atlas settings redo atlas and geometry");

    let mut o = base.clone();
    o.vhacd_resolution = 32;
    assert_eq!(hits(&run(&o)), ["scene", "atlas", "geometry"], "VHACD settings redo only collision");

    let mut o = base.clone();
    o.humanoid = true;
    assert!(hits(&run(&o)).is_empty(), "humanoid reloads the scene, so nothing downstream can be reused");

    cache.clear();
    assert!(hits(&run(&base)).is_empty());
}

#[test]
fn a_changed_file_is_not_served_from_the_cache() {
    let out = tmp("cache_file");
    let cache = ConvertCache::new();
    let copy = out.join("src.glb");
    std::fs::copy(data("tiled"), &copy).unwrap();
    let h = || Hooks::default().with_cache(cache.clone());
    let o = no_coll();
    conv_with(&copy, Some("x"), &out, &o, &h()).unwrap();
    std::fs::copy(data("basic"), &copy).unwrap(); // different size
    let r = conv_with(&copy, Some("x"), &out, &o, &h()).unwrap();
    assert!(hits(&r).is_empty());
    assert_eq!(r.stats.unique_materials, 2);
}

// ---------------------------------------------------------------- alpha handling

fn diffuse_alpha(out: &Path, name: &str) -> Vec<u8> {
    image::open(out.join(name).join(format!("diffuse_{name}.png"))).unwrap().to_rgba8().pixels().map(|p| p[3]).collect()
}

#[test]
fn alpha_cutouts_are_kept_even_when_dropping_transparent_materials() {
    let out = tmp("cutout");
    let info = inspect(&data("cutout")).unwrap();
    assert!(info.materials[0].cutout && !info.materials[0].transparent && !info.any_transparent);
    let o = ConvertOptions { drop_transparent: true, ..no_coll() };
    let r = conv(&data("cutout"), None, &out, &o).unwrap();
    assert_eq!(r.stats.parts, 1);
    assert!(has(&r, "alpha_cutout") && !has(&r, "transparent_dropped"));
    assert!(diffuse_alpha(&out, "cutout").contains(&0), "the cut-out area must stay transparent for VotV's alpha clip");
}

#[test]
fn opaque_materials_ignore_texture_alpha() {
    let out = tmp("opaquealpha");
    let info = inspect(&data("opaque_alpha")).unwrap();
    assert!(!info.materials[0].transparent && !info.materials[0].cutout);
    let o = ConvertOptions { drop_transparent: true, ..no_coll() };
    let r = conv(&data("opaque_alpha"), None, &out, &o).unwrap();
    assert_eq!(r.stats.parts, 1);
    assert!(diffuse_alpha(&out, "opaque_alpha").iter().all(|a| *a == 255), "stray alpha would become holes in VotV");
}

#[test]
fn blend_without_real_transparency_counts_as_opaque() {
    let out = tmp("blendopaque");
    assert!(!inspect(&data("blend_opaque")).unwrap().any_transparent);
    let r = conv(&data("blend_opaque"), None, &out, &ConvertOptions { drop_transparent: true, ..no_coll() }).unwrap();
    assert_eq!(r.stats.parts, 1);
    // while real glass is still dropped
    assert!(inspect(&data("glass")).unwrap().any_transparent);
}

// ---------------------------------------------------------------- properties

#[test]
fn derived_stats_follow_the_hull_size_and_the_written_cfg() {
    let out = tmp("stats");
    // no collision in the source and none generated: the 33 cm fallback box defines bites and size
    let r = conv(&data("tiled"), None, &out, &no_coll()).unwrap();
    let size = r.stats.hull_size_cm.unwrap();
    assert!(size.iter().all(|s| (s - 33.0).abs() < 0.01), "{size:?}");
    assert_eq!(r.stats.bites, 36);
    assert!(!r.stats.edible && r.stats.health_in_game.is_none(), "indestructible by default");

    let mut o = no_coll();
    o.properties.physical_material = 8;
    o.properties.indestructible = false;
    o.properties.health = 0.0; // derived from size: 33 * 33 / 5
    let r = conv(&data("tiled"), Some("food"), &out, &o).unwrap();
    assert!(r.stats.edible);
    assert!((r.stats.health_in_game.unwrap() - 217.8).abs() < 0.01);
}

#[test]
fn every_property_reaches_the_cfg_file() {
    use votv_convert::model_core::{PropValue, PropertiesCfg, PROPERTY_SPECS};
    let out = tmp("allprops");
    let mut o = no_coll();
    o.properties.indestructible = false;
    o.properties.explotano = Some(2.0);
    o.properties.extra.insert("filter_diffuse_body".into(), "1".into());
    for (k, v) in [("physical_material", 3), ("filter_diffuse_", 1), ("filter_pbr_", 2), ("filter_normal_", 3), ("filter_emissive_", 1)] {
        o.properties.set(k, PropValue::Int(v)).unwrap();
    }
    o.properties.is_lamp = true;
    o.properties.lamp_intensity = 1234.0;
    let r = conv(&data("basic"), None, &out, &o).unwrap();
    let text = std::fs::read_to_string(out.join("basic/properties.cfg")).unwrap();
    for s in PROPERTY_SPECS.iter().filter(|s| !s.virtual_key) {
        assert!(text.lines().any(|l| l.starts_with(&format!("{}=", s.key))), "{} missing from the cfg", s.key);
    }
    assert!(text.contains("filter_diffuse_body=1"));
    assert_eq!(PropertiesCfg::parse(&text).lamp_intensity, 1234.0);
    assert!(r.stats.has_emissive);
}

#[test]
fn invalid_properties_are_rejected_before_converting() {
    let out = tmp("badprops");
    let mut o = no_coll();
    o.properties.filter_pbr_.0 = 9;
    assert_eq!(conv(&data("basic"), None, &out, &o).unwrap_err().kind(), "invalid_option");
    let mut o = no_coll();
    o.properties.extra.insert("health".into(), "1".into());
    assert_eq!(conv(&data("basic"), None, &out, &o).unwrap_err().kind(), "invalid_option");
}

#[test]
fn old_saved_options_still_load_and_edible_is_migrated() {
    let o = ConvertOptions::from_json(r#"{"scale": 50, "edible": true, "some_removed_option": 1}"#).unwrap();
    assert_eq!(o.scale, 50.0);
    assert_eq!(o.properties.physical_material, 8);
    let o = ConvertOptions::from_json(r#"{"edible": true, "properties": {"health": 10.0}}"#).unwrap();
    assert!(o.properties.is_edible() && o.properties.health == 10.0);
    assert!(!ConvertOptions::from_json("{}").unwrap().properties.is_edible());
    let back = ConvertOptions::from_json(&ConvertOptions::default().to_json()).unwrap();
    assert_eq!(back, ConvertOptions::default());
    assert!(ConvertOptions::from_json("nonsense").is_err());
}

// ---------------------------------------------------------------- toon-shader exports (emission / alpha / outlines)

fn mean_diffuse(out: &Path, name: &str) -> f64 {
    let img = image::open(out.join(name).join(format!("diffuse_{name}.png"))).unwrap().to_rgba8();
    let (mut sum, mut n) = (0.0, 0.0);
    for p in img.pixels().filter(|p| p[3] == 255) {
        sum += (p[0] as f64 + p[1] as f64 + p[2] as f64) / 3.0;
        n += 1.0;
    }
    sum / n
}

#[test]
fn colour_stored_in_the_emissive_slot_becomes_the_diffuse() {
    let out = tmp("selflit");
    let r = conv(&data("selflit"), None, &out, &no_coll()).unwrap();
    assert!(has(&r, "emissive_used_as_colour") && !r.stats.has_emissive);
    assert!(mean_diffuse(&out, "selflit") > 120.0, "the face must not stay black");
    // literal mode keeps the file's own reading (near-black diffuse, glowing)
    let lit = ConvertOptions { emissive_mode: EmissiveMode::Literal, ..no_coll() };
    let r = conv(&data("selflit"), Some("literal"), &out, &lit).unwrap();
    assert!(r.stats.has_emissive && mean_diffuse(&out, "literal") < 80.0, "literal: the dark flat factor, not the emissive texture");
}

#[test]
fn emission_that_copies_the_base_texture_is_not_glow() {
    let out = tmp("emdup");
    let r = conv(&data("emissive_dup"), None, &out, &no_coll()).unwrap();
    assert!(has(&r, "emissive_duplicate_ignored") && !r.stats.has_emissive);
    assert!(!inspect(&data("emissive_dup")).unwrap().materials[0].has_emission);
}

#[test]
fn real_glow_masks_are_kept_and_black_emission_is_ignored() {
    let out = tmp("glow");
    let r = conv(&data("glow"), None, &out, &no_coll()).unwrap();
    assert!(r.stats.has_emissive && !has(&r, "emissive_used_as_colour") && !has(&r, "emissive_duplicate_ignored"));
    let r = conv(&data("black_emissive"), None, &out, &no_coll()).unwrap();
    assert!(!r.stats.has_emissive && has(&r, "emissive_ignored"));
}

#[test]
fn blend_hair_with_a_high_alpha_factor_is_kept_but_real_glass_is_dropped() {
    let out = tmp("blendhair");
    let info = inspect(&data("blend_hair")).unwrap();
    assert!(!info.any_transparent && info.materials[0].cutout);
    let drop = ConvertOptions { drop_transparent: true, ..no_coll() };
    assert_eq!(conv(&data("blend_hair"), None, &out, &drop).unwrap().stats.parts, 1);
    // glass: BLEND with a texture whose alpha is see-through everywhere
    assert!(inspect(&data("blend_glass_tex")).unwrap().any_transparent);
    assert_eq!(conv(&data("blend_glass_tex"), None, &out, &drop).unwrap_err().kind(), "everything_transparent");
}

#[test]
fn toon_outline_shells_are_dropped_unless_asked_to_keep_them() {
    let out = tmp("outline");
    let r = conv(&data("outline"), None, &out, &no_coll()).unwrap();
    assert_eq!(r.stats.parts, 1);
    assert!(has(&r, "outline_dropped"));
    let keep = ConvertOptions { drop_outline_parts: false, ..no_coll() };
    assert_eq!(conv(&data("outline"), Some("kept"), &out, &keep).unwrap().stats.parts, 2);
}

// ---------------------------------------------------------------- colour spaces and metallic

#[test]
fn flat_colours_are_converted_from_linear_to_srgb() {
    let out = tmp("flatpaint");
    conv(&data("flat_paint"), None, &out, &no_coll()).unwrap();
    // factor ~0.09 (linear) is about 86/255 in sRGB; multiplying by 255 (the old behaviour) gave 24
    let m = mean_diffuse(&out, "flat_paint");
    assert!((80.0..=95.0).contains(&m), "flat paint came out as {m}");
}

#[test]
fn metallic_values_can_be_scaled_down() {
    let center_r = |out: &Path, name: &str| {
        let img = image::open(out.join(name).join(format!("pbr_{name}.png"))).unwrap().to_rgb8();
        img.get_pixel(img.width() / 2, img.height() / 2)[0]
    };
    let out = tmp("metal");
    conv(&data("metal_paint"), None, &out, &no_coll()).unwrap();
    assert_eq!(center_r(&out, "metal_paint"), 191, "capped at 0.75: fully metallic smooth surfaces render dark in the game");
    let uncapped = ConvertOptions { metallic_max: 1.0, ..no_coll() };
    conv(&data("metal_paint"), Some("uncapped"), &out, &uncapped).unwrap();
    assert_eq!(center_r(&out, "uncapped"), 255, "authored value with the cap lifted");
    let o = ConvertOptions { metallic_scale: 0.3, ..no_coll() };
    conv(&data("metal_paint"), Some("scaled"), &out, &o).unwrap();
    assert!((center_r(&out, "scaled") as i32 - 77).abs() <= 1);
    assert_eq!(conv(&data("metal_paint"), Some("bad"), &out, &ConvertOptions { metallic_scale: 1.5, ..no_coll() }).unwrap_err().kind(), "invalid_option");
    assert_eq!(conv(&data("metal_paint"), Some("bad2"), &out, &ConvertOptions { metallic_max: -0.1, ..no_coll() }).unwrap_err().kind(), "invalid_option");
}

#[test]
fn metallic_scale_is_part_of_the_atlas_cache_key() {
    let out = tmp("metalcache");
    let cache = ConvertCache::new();
    let run = |o: &ConvertOptions| conv_with(&data("metal_paint"), Some("m"), &out, o, &Hooks::default().with_cache(cache.clone())).unwrap();
    run(&no_coll());
    let r = run(&ConvertOptions { metallic_scale: 0.5, ..no_coll() });
    assert_eq!(r.cache_hits.iter().map(|s| s.as_str()).collect::<Vec<_>>(), ["scene"]);
}

// ---------------------------------------------------------------- layered shells and colour factors

#[test]
fn translucent_shells_are_clipped_instead_of_becoming_a_solid_skin() {
    let out = tmp("coat");
    let r = conv(&data("coat_shell"), None, &out, &no_coll()).unwrap();
    assert_eq!(r.stats.parts, 2);
    let img = image::open(out.join("coat_shell/diffuse_coat_shell.png")).unwrap().to_rgba8();
    // factor alpha 0.24 is baked into the coat's tile (about 61/255, below VotV's alpha clip), the paint stays opaque
    assert!(img.pixels().any(|p| (58..=64).contains(&p[3])), "coat tile alpha must be about 0.24 * 255");
    assert!(img.pixels().any(|p| p[3] == 255));
}

#[test]
fn base_colour_factors_are_applied_by_default() {
    assert!(ConvertOptions::default().apply_base_color_factor);
    let out = tmp("shadow");
    conv(&data("ground_shadow"), None, &out, &no_coll()).unwrap();
    assert!(mean_diffuse(&out, "ground_shadow") < 30.0, "white texture x near-black factor is a dark shadow, not a white patch");
    let raw = ConvertOptions { apply_base_color_factor: false, ..no_coll() };
    conv(&data("ground_shadow"), Some("raw"), &out, &raw).unwrap();
    assert!(mean_diffuse(&out, "raw") > 200.0);
}

// ---------------------------------------------------------------- PNG levels

#[test]
fn png_levels_change_only_the_file_size_never_the_pixels() {
    let out = tmp("pnglevels");
    let load = |name: &str| image::open(out.join(name).join(format!("diffuse_{name}.png"))).unwrap().to_rgba8();
    let norm = |name: &str| image::open(out.join(name).join(format!("normal_{name}.png"))).unwrap().to_rgb8();
    let mut results = Vec::new();
    for (name, level) in [("best", PngLevel::Best), ("balanced", PngLevel::Balanced), ("fast", PngLevel::Fast)] {
        let o = ConvertOptions { png_level: level, ..no_coll() };
        let r = conv(&data("basic"), Some(name), &out, &o).unwrap();
        assert!(r.stats.has_normal);
        results.push((load(name), norm(name)));
    }
    assert!(results.iter().all(|r| r.0 == results[0].0 && r.1 == results[0].1), "decoded pixels must be identical at every level");
}

#[test]
fn png_bytes_are_cached_per_level_and_reencoded_when_the_level_changes() {
    let out = tmp("pngcache");
    let cache = ConvertCache::new();
    let run = |lvl: PngLevel| conv_with(&data("basic"), Some("p"), &out, &ConvertOptions { png_level: lvl, collision_quality: CollisionQuality::Low, ..Default::default() }, &Hooks::default().with_cache(cache.clone())).unwrap();
    run(PngLevel::Best);
    let size_best = std::fs::metadata(out.join("p/diffuse_p.png")).unwrap().len();
    let r = run(PngLevel::Fast);
    assert_eq!(r.cache_hits.iter().map(|s| s.as_str()).collect::<Vec<_>>(), ["scene", "atlas", "geometry", "collision"]);
    let size_fast = std::fs::metadata(out.join("p/diffuse_p.png")).unwrap().len();
    assert_ne!(size_best, size_fast, "a different level must produce different bytes (re-encoded, not served from the cache)");
}

// ---------------------------------------------------------------- fast OBJ writer

#[test]
fn obj_text_has_six_decimals_and_no_duplicate_vt_lines() {
    for fixture in ["basic", "tiled", "misc"] {
        let out = tmp(&format!("objfmt_{fixture}"));
        let r = conv(&data(fixture), Some("m"), &out, &no_coll()).unwrap();
        let text = std::fs::read_to_string(r.model_dir.join("m.obj")).unwrap();
        let mut seen = std::collections::HashSet::new();
        let (mut vts, mut faces) = (0usize, 0usize);
        for l in text.lines() {
            let mut t = l.split_whitespace();
            let tag = t.next().unwrap_or("");
            if matches!(tag, "v" | "vn" | "vt") {
                for tok in t {
                    let (_, frac) = tok.split_once('.').unwrap_or_else(|| panic!("{fixture}: no decimal point in {l:?}"));
                    assert_eq!(frac.len(), 6, "{fixture}: {l:?}");
                    tok.parse::<f64>().unwrap_or_else(|_| panic!("{fixture}: not a number in {l:?}"));
                }
            }
            if tag == "vt" {
                vts += 1;
                assert!(seen.insert(l.to_string()), "{fixture}: duplicate vt line {l:?}");
            }
            if tag == "f" {
                faces += 1;
                for corner in l.split_whitespace().skip(1) {
                    assert_eq!(corner.split('/').count(), 3, "{fixture}: {l:?}");
                }
            }
        }
        assert!(faces > 0 && vts > 0, "{fixture}");
        check_obj(&r.model_dir.join("m.obj"));
    }
}
