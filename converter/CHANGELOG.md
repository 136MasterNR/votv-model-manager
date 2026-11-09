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
- **Toon-shader exports** (found with Blue Archive Miyu and Honkai Higokumaru, which store the albedo in the *emission* slot):
  - New `EmissiveMode` option (`emissive_mode`, default `Auto`; `Literal` = follow the file): an emissive texture that is the same image as the
    base texture is not glow (`emissive_duplicate_ignored`); a material without a base texture whose emissive texture is a real colour map
    (mean luminance >= 48) uses it as the diffuse (`emissive_used_as_colour`, fixes flat/black diffuse); black emission or a factor below 0.02 is ignored
    (`emissive_ignored`). `inspect()` reports these as hints. The scene cache key includes the mode.
  - The emissive texture is now multiplied by `emissiveFactor` (glTF rule); before, the factor was ignored.
  - BLEND classification refined: a base alpha factor >= 0.9 no longer counts as translucent; a BLEND texture is translucent only when >= 15% of its covered
    texels are semi-transparent, otherwise it is a cutout (hair cards, soft edges) or opaque. (Miyu's hair, BLEND with alpha 0.99, was dropped.)
  - New option `drop_outline_parts` (default true): parts whose material or object name contains "outline" (toon inverted-hull shells) are removed (`outline_dropped`).
  - New `DiagCode`s: `EmissiveUsedAsColour`, `EmissiveDuplicateIgnored`, `EmissiveIgnored`, `OutlineDropped`. CLI: `--literal-emissive`, `--keep-outlines`.
- **Colour spaces (found with a Porsche 930 whose paint rendered black):** glTF colour factors are *linear*; the atlas is sRGB. Flat colours from `baseColorFactor`
  / spec-gloss `diffuseFactor` and flat emissive colours are now encoded properly (a 0.096 paint is 86/255, it was 24), and the factor-times-texture
  options (`apply_base_color_factor`, the emissive factor) multiply in linear space. `Material::base_linear` keeps the exact factor. Output differs from the Python converter here on purpose.
- **New option `metallic_scale`** (0-1, default 1 = as authored): multiplies every metallic value in the pbr map. The car's paint had a metallic texture of 255 with no factor (glTF default 1).
  Fully metallic surfaces have no diffuse light in UE, so they may look black in game; the printer thumbnail is known to show them dark, the world print is unconfirmed (open question in votv-printer-internals).
  The default stays 1 until an in-game test says otherwise. Part of the atlas cache key. CLI: `--metallic-scale`.
- **Layered translucent shells (Porsche 930, in-game black car):** the alpha *factor* of BLEND/MASK materials is now always baked into the atlas tile, independent of
  `apply_base_color_factor`. The car's clear-coat layer (`coat`: BLEND, factor alpha 0.24, a texture without alpha, a second shell of 43,713 triangles over the paint) used to be
  written fully opaque and hid the paint; now its tile alpha is 61/255, below VotV's alpha clip, so it disappears. (Fixing the paint colour alone changed nothing visible for this reason.)
- **`apply_base_color_factor` now defaults to true** (breaking default; CLI flag is now `--ignore-base-color-factor`): textures are multiplied by `baseColorFactor` in linear space like every
  glTF viewer does. The same car had a 3 x 6 m ground quad (white texture x near-black factor) that came out as a white patch.
- **New option `metallic_max`** (0-1, default **0.75**), applied after `metallic_scale`: measured in the game with the calibration grid, a fully metallic (255) surface with low or
  medium roughness renders dark (a mirror of the dark environment; the ground texture shows in it) while 192 (0.75) and below still look like the paint colour, and roughness 220 at 255 looks
  orange. The Porsche's paint (metallic 255, roughness 255 x 0.4248 = 108) was exactly the dark case. Metallic values above the cap are written as the cap. Set `metallic_max` to 1 for authored
  chrome. Nothing was misread and no shader is missing: it is ordinary PBR in an environment with little to reflect. CLI: `--metallic-max`.
- CLI is dev-only and no longer a default feature.
- `show_collision` pushes the red copy only as far as needed to avoid z-fighting (auto: 0.2% of model size, 0.05-0.5 cm; was 1.5%, min 0.5 cm,
  with corners up to 2.5x further) and has a new option `show_collision_offset: Option<f64>` (cm, `None` = auto). Output of `--show-coll`
  therefore no longer matches the Python converter (the `tools/` comparison will differ there by design).

## 0.1.0 (compiled, 14 tests, compared with the Python converter)
- Port of `printer_convert.py`; old API `convert(input, name, out_dir, &opts)`.
