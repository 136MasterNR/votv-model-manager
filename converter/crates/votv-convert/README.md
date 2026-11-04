# votv-convert

Rust port of `printer_convert.py`: turns a `.glb` into a VotV (Voices of the Void) 3D-printer folder
(`<name>.obj`, `<name>.mtl`, `diffuse_/emissive_/normal_/pbr_<name>.png`, `properties.cfg`).

One crate, two faces:

* **library** (`votv_convert`) - what your GUI links against; no CLI dependencies.
* **binary** (`votv-convert`) - same flags as the Python script.

```toml
# GUI project's Cargo.toml
votv-convert = { path = "../votv-convert", default-features = false }
```

Pure Rust (VHACD and convex hulls come from `parry3d`): no C++ compiler, no Python, no DLLs.
Build: `cargo build --release` (Rust 1.80+).

## API

```rust
use votv_convert::*;

// 1. optional pre-flight when the user picks a file (fast, no output written)
let info: Inspection = inspect(Path::new("model.glb"))?;   // size, materials, skins, hints...

// 2. options: plain struct, Default = the Python defaults, serde (de)serializable for presets
let mut opts = ConvertOptions::default();
opts.humanoid = info.skins > 0;
opts.validate()?;

// 3. convert (blocking: call from a worker thread)
let hooks = Hooks::default()
    .with_progress(|p| { /* p.stage, p.fraction 0..1, p.message */ })
    .with_diagnostics(|d| { /* d.severity, d.code, d.message */ })
    .with_cancel(cancel_token.clone());                     // CancelToken::cancel() from the UI
let report: Report = convert_with(input, Some("my_name"), out_dir, &opts, &hooks)?;
// report.model_dir, report.files, report.stats, report.diagnostics, report.properties_kept
```

| Item | Notes |
|---|---|
| `convert(input, name, out_dir, &opts)` | no callbacks |
| `convert_with(..., &Hooks)` | progress, live diagnostics, cancel |
| `inspect(path)` | `Inspection` with materials, size in cm, skins, `hints` |
| `sanitize_name(s)` | the exact folder-name rule (lower case, `[a-z0-9_]`) |
| `ConvertError::kind()` | stable tag: `input_not_found`, `not_glb`, `unreadable`, `no_geometry`, `everything_transparent`, `output_folder_not_ours`, `permission_denied`, `invalid_option`, `cancelled`, `io`, `internal`. `Display` is a user-ready message |
| `Diagnostic { severity, code, message }` | `Info` = narration, `Warning`, `Attention` (output probably wrong unless the user acts, e.g. `skeleton_without_humanoid`). `code` is stable |

Diagnostic codes: `custom_collision_found`, `custom_collision_ignored`, `custom_collision_used`,
`no_coll_custom_kept`, `skeleton_without_humanoid`, `humanoid_applied`, `humanoid_no_skin`,
`transparent_dropped`, `transparent_summary`, `duplicate_part`, `atlas_packed`, `atlas_shrunk`,
`emissive_atlas`, `normal_atlas`, `pbr_atlas`, `collision_decomposed`, `collision_added`,
`hulls_trimmed`, `decomposition_failed`, `collision_failed`, `fallback_box`, `show_coll`,
`show_coll_no_collision`, `tiled_split`, `tiled_unsplit`, `properties_kept`,
`properties_options_ignored`, `uv_set_ignored`, `texture_transform_ignored`, `summary`.

See `examples/gui_worker.rs` for the thread + channel pattern.

## Verified against the Python original

`tools/` holds the differential harness (needs Python with trimesh, vhacdx, scipy, fast_simplification).
On the fixtures in `tests/data` the diffuse/emissive/normal/pbr atlases are pixel-identical (<=1 level
where Lanczos rounding differs), and `.mtl`, `properties.cfg`, visible OBJ geometry, UVs and normals
match face for face, including `--show-coll`, offsets, fallback box, `--no-transparent`,
`--humanoid` positions, tiled-UV cutting and duplicate removal.

### Deliberate differences

| Area | Python | Rust | Why |
|---|---|---|---|
| Generated collision | `vhacdx` (C++ VHACD 2), often hits the hull cap | `parry3d` VHACD, voxel `vhacd_resolution` (default 64) | pure Rust. Measured on a torus: hole kept, 100% surface within 0.5 cm, 12 hulls (Python: 64 hulls, 82% covered) |
| Hull face trimming | quadric decimation (`fast_simplification`) | drop least-important vertices and re-hull | result is guaranteed convex |
| Mirrored nodes (negative scale) | faces come out inside-out | winding is flipped | correct output |
| Skinned mesh without stored normals | stale bind-pose normals | normals recomputed on the posed mesh | correct output |
| Non-uniform node scale | normals use the plain rotation part | inverse-transpose | correct output |
| Material identity | name + base texture + factor | also emissive/normal/pbr maps and factors | two materials differing only there are no longer merged |
| Output folder | deleted before conversion starts | replaced only after everything succeeded | a failed run no longer destroys the old output |
| `.gltf` / other formats | rejected | rejected | unchanged |
| Option `apply_base_color_factor` | not done | off by default (parity); `--apply-base-color-factor` follows the glTF spec | opt-in |

### Known limits

* Only `.glb` with textures the `gltf`/`image` crates decode (PNG, JPEG). WebP / KTX2 / Basis textures
  (`EXT_texture_webp`, `KHR_texture_basisu`) make loading fail with `unreadable`.
* Only UV set 0; `KHR_texture_transform` is reported (`texture_transform_ignored`) but not applied.
* Texture wrap modes are treated as REPEAT.
* Nothing here has been verified inside VotV itself, only against the Python version's output.
