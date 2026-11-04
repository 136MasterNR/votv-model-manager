# Changelog

Versioning: 0.x. Breaking API changes bump the minor version and are listed here with the migration. The converter agent
only edits `crates/votv-convert` and `crates/votv-model-core`; the GUI is not part of this repository.

## 0.2.0 (not yet compiled, see INTEGRATION.md)
- New `convert(&Options, input)` / `convert_with(&Options, input, &Hooks)`; output folder and name are options now (breaking vs 0.1).
- `DiagCode` enum, `Stats::{sections, triangles, hulls}`, `CancelToken::from_flag`, `OverwritePolicy`, `Report::extras`.
- Options mirror `properties.cfg` via `votv_model_core::PropertiesCfg`; every field documented.
- Output: material named after the model, always a `UCX_` hull, atomic folder swap, `.obj` written last, self-check, optional
  `icon.png` and `votv_convert.json`.
- New crate `votv-model-core` (naming, `UCX_` rule, cfg, verified physical-material table and TextureFilter values).
- Models using `KHR_materials_pbrSpecularGlossiness` (old Sketchfab-style materials) now load: the diffuse texture/colour becomes the base
  colour (`spec_gloss_converted`). Any other *required* glTF extension the loader does not know is ignored with a warning
  (`unsupported_extension_ignored`) instead of failing with `unreadable`. New `DiagCode` variants: `SpecGlossConverted`, `UnsupportedExtensionIgnored`.
- `PropertiesCfg::indestructible` (additive, default false): omits the `health` line. Observed in game (user testing): no `health` key = infinite
  health. `PropertiesCfg::parse` sets it when the key is absent, so a manager reading an existing cfg shows the right state. `health` itself is unchanged
  (value 0 still means "derive from size").
- **`ConvertCache`** (additive): `Hooks::with_cache(cache.clone())` lets repeated conversions of the same file skip work whose inputs did not
  change: reading/decoding the .glb (`scene`), atlas baking and PNG compression (`atlas`), UV cutting and OBJ text (`geometry`), VHACD (`collision`).
  Output is byte-identical to a run without a cache; `Report::cache_hits` lists the reused stages. A finished VHACD run is kept even when the job
  was cancelled right after it. The invalidation table is in `cache.rs`. Compatibility rule for saved options (`votv_convert.json`): fields are only
  ever added, with defaults; never renamed or removed.
- **Transparency classification fixed** (a MASK-material chicken model was dropped entirely by "drop transparent"): materials are now Opaque, Cutout
  (`MASK`, kept, info `alpha_cutout`) or Translucent (`BLEND` with real see-through alpha; the only kind dropped). Before, any non-OPAQUE mode or any
  texture alpha below 254 counted, which also made `inspect().any_transparent` pre-tick the option wrongly. OPAQUE materials (and flat colours) now
  get fully opaque atlas tiles, since stray alpha would be clipped into holes by VotV. `MaterialInfo::cutout` added; new `DiagCode::AlphaCutout`.
  Behaviour differs from the Python converter here on purpose.
- **`PropertiesCfg::indestructible` now defaults to true** (no `health` line, infinite health in game). Set it to false (and `health` to the value you want) for a
  breakable print. Saved options from older `votv_convert.json` files have no such field, so they load as indestructible too.
- `indestructible` now omits `impact_resistance` and `damage_resistance` as well as `health` (a model with either of them was still breakable). `parse` counts
  any of the three keys as "breakable".
- **Properties are separate from options.** `votv_model_core` now describes every model property: `PROPERTY_SPECS` (key, label, group, control kind with range,
  documentation, confidence) plus generic `PropertiesCfg::get` / `set` (`PropValue`), so a panel can be generated. Covers all 16 keys the game's `prepareCfg`
  writes (4 texture filters, physical_material, health, impact_resistance, damage_resistance, is_lamp, lamp_color, lamp_offset, lamp_intensity,
  lamp_attenuation, lamp_shadows, light_toggle, emissive_strength) plus the virtual `indestructible` switch and the experimental `explotano` (unverified key name,
  `Option`, not written by default). `PropertiesCfg::extra` keeps/writes any other `key=value` line (per-material filters, keys under investigation); `parse` now
  stores unknown keys there. `PropertiesCfg::validate()` is called by `ConvertOptions::validate()`.
- **`ConvertOptions::edible` removed** (breaking): edible is `properties.physical_material = 8`. `ConvertOptions::effective_properties()` is deprecated and returns
  `properties`. `ConvertOptions::from_json(&str)` loads saved options (sidecar, presets), ignores retired fields and migrates `"edible": true` to physical material 8;
  `to_json()` writes them. Use `from_json` instead of plain serde when reading saved options.
- New derived values, because the game computes them from the collision hulls: `Stats::{hull_size_cm, bites, edible, health_in_game}` and the core helpers
  `bites_for`, `auto_health`, `PropertiesCfg::{is_edible, health_in_game}`.
- CLI: `--edible` replaced by `--physical-material 8`; new `--breakable` and `--health N`.
- CLI is dev-only and no longer a default feature.
- `show_collision` pushes the red copy only as far as needed to avoid z-fighting (auto: 0.2% of model size, 0.05-0.5 cm; was 1.5%, min 0.5 cm,
  with corners up to 2.5x further) and has a new option `show_collision_offset: Option<f64>` (cm, `None` = auto). Output of `--show-coll`
  therefore no longer matches the Python converter (the `tools/` comparison will differ there by design).

## 0.1.0 (compiled, 14 tests, compared with the Python converter)
- Port of `printer_convert.py`; old API `convert(input, name, out_dir, &opts)`.
