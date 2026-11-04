# votv-convert workspace (v0.2)

> **Status: written but NOT yet compiled or tested.** v0.1 (single crate, old API) was compiled and passed 14 tests plus
> a differential comparison against the Python converter; v0.2 adds the Model Manager contract below on top of it by hand.
> First step on any machine: `cargo test --release`, then fix whatever the compiler finds.

Crates:

* `crates/votv-model-core` - format knowledge shared with the Model Manager: `sanitize_name`, `is_ucx_name`, `ucx_object_name`
  (forces the upper-case `UCX_` prefix, which the game and the manager compare case-sensitively), `texture_file_name`,
  `PropertiesCfg` (cfg key names, writer, tolerant parser), `PHYSICAL_MATERIALS` (verified, 8 = food = edible) and
  `TextureFilter` (verified: 0 Nearest, 1 Bilinear, 2 Trilinear, 3 Default). Only `serde` as dependency. Sources: the converter's
  output and the `votv-printer-internals` skill. Not modelled: the cfg key for `explotano` (unknown). The manager's own
  `PHYS_MATERIALS` and `Kind` can be replaced by `votv_model_core::PHYSICAL_MATERIALS` and `TextureKind`.
* `crates/votv-convert` - the converter library. The CLI is a developer extra behind the `cli` feature (off by default).

The converter is integrated into the GUI; see `INTEGRATION.md` (hand-off) and `CHANGELOG.md`.

## API

```rust
use votv_convert::*;

let mut opts = Options::default();          // Default + serde, every field documented
opts.output_dir = "C:/.../printer".into();
opts.properties.physical_material = 3;      // mirrors properties.cfg key names (votv_model_core::PropertiesCfg)
opts.validate()?;

let info = inspect(path)?;                  // optional pre-flight
let hooks = Hooks::default()
    .with_progress(|p| { /* p.stage, p.fraction 0..1, p.message */ })
    .with_diagnostics(|d| { /* d.severity, d.code: DiagCode, d.message */ })
    .with_cancel(CancelToken::from_flag(my_atomic_bool_arc));
let report = convert_with(&opts, path, &hooks)?;   // or convert(&opts, path)
```

The library never prints, prompts or exits. Report: `model_dir`, `name` (final, ASCII), `files`, `extras`, `properties_kept`,
`diagnostics` (`DiagCode` enum + severity + message), `stats` (`sections`, `triangles`, `faces`, `hulls`, atlas size, ...).
Errors are `ConvertError` (enum; `kind()` gives a stable tag, `Display` a user-ready message).

## Properties
`Options.properties` is a `votv_model_core::PropertiesCfg`: the model's `properties.cfg` values, independent of conversion settings. `PROPERTY_SPECS` +
`get`/`set` describe every property for UI generation; `extra` carries unknown keys. See `INTEGRATION.md`.

## Cache
`Hooks::with_cache(ConvertCache)` reuses file loading, atlas baking + PNG compression, UV cutting and VHACD between runs when their inputs are unchanged (byte-identical output). See `INTEGRATION.md` and `cache.rs`.

## Output contract

* `<name>/<name>.obj`, `<name>.mtl`, `diffuse_<name>.png` (+ `normal_`, `pbr_`, `emissive_` when present), `properties.cfg`.
  Names are ASCII `[a-z0-9_]`. The single atlas material is named `<name>` so `usemtl`, `newmtl` and the texture names agree.
  Every face follows a named `usemtl`; a built-in self-check refuses to publish an OBJ that violates this.
* **At least one `UCX_` object is always written** (verified in the game: without a hull the print gets no health, damage data, `edible` or `bites`): source `UCX_*` meshes, else generated hulls, else (also with `generate_collision = false`
  or when generation fails) a box at the model's base.
* **Atomic**: built in hidden `.<name>.tmp<pid>`, then renamed; the `.obj` is written last, so even a scan that does not skip hidden folders (the manager's `scan_models` lists any folder with an `.obj`) never lists a half-written model. Scans should still ignore names starting with `.`.
  An existing folder is moved aside and deleted only after the new one is in place (restored on failure).
* `overwrite`: `Replace` (only folders this converter wrote), `Fail` (`OutputExists`), `Rename` (`<name>_2`...).
* `keep_existing_properties` (default true, as before): with `Replace`, an existing `properties.cfg` is kept; set false to always write from `properties`.
* Optional: `write_icon` -> `icon.png` (software render, `icon_size`), `write_sidecar` -> `votv_convert.json`
  (source path, options, converter version). Both off by default; whether the game ignores extra files is unverified.

Dependencies: no Bevy; `image` 0.25 (png + jpeg features; jpeg is only for decoding textures inside GLBs), `parry3d` for VHACD/hulls.
Dev-dependency `tobj` 4 loads our OBJs in a test, as the manager does.

## Differences from the Python converter

| Area | Python | Rust |
|---|---|---|
| Generated collision | `vhacdx` | `parry3d` VHACD (`vhacd_resolution`); torus test: hole kept, 12 hulls vs 64 |
| Hull face trimming | quadric decimation | vertex dropping + re-hull (stays convex) |
| Mirrored nodes / skinned normals / non-uniform scale | wrong winding / stale normals / plain rotation | corrected |
| Material identity | name + base texture + factor | also emissive/normal/pbr data |
| Collision when none | none (`--no-coll`) | box collider always |
| Material name | `atlas_material` | model name |
| Output folder | deleted first | swapped in at the end |

## Known limits

WebP/KTX2 textures fail to load; only UV set 0; `KHR_texture_transform` reported not applied; wrap modes treated as repeat.
The game derives `bites` (hull volume / 1000) and the default health from the hull bounding box, so the fallback box size matters for large models.
Nothing here has been verified inside VotV by running it; the format facts come from `votv-printer-internals`.
