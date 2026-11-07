# Integrating the converter into the GUI (hand-off for the GUI agent)

Scope: the converter agent maintains `crates/votv-convert` and `crates/votv-model-core` only. The GUI owns everything else.
Both crates are self-contained (no workspace inheritance), so copy or path-reference them from the GUI workspace:

```toml
votv-convert    = { path = "../votv-converter/crates/votv-convert" }     # library only; the `cli` feature is off by default
votv-model-core = { path = "../votv-converter/crates/votv-model-core" }
```

## First thing to do: compile
v0.2 was written without being compiled. Run `cargo test -p votv-model-core -p votv-convert --release` once. Expect a few small
compile errors; send the compiler output to the converter agent instead of patching around them, so both copies stay in sync.
A cold build of `parry3d`, `gltf` and `image` takes a few minutes. `image` 0.25 with `png`+`jpeg` is shared with the manager's `image`
(same version, no duplicate build). No Bevy dependency.

## What the GUI calls
```rust
let opts: votv_convert::Options = ...;        // Default + serde; bind a settings panel to it (all fields documented)
opts.validate()?;                              // cheap, per edit
let info = votv_convert::inspect(path)?;       // optional pre-flight (size in cm, materials, skins, hints)
let hooks = Hooks::default()
    .with_progress(|p| ...)                    // Progress { stage, fraction 0..1, message }
    .with_diagnostics(|d| ...)                 // Diagnostic { severity, code: DiagCode, message }
    .with_cancel(CancelToken::from_flag(flag)); // your Arc<AtomicBool>
let report = votv_convert::convert_with(&opts, path, &hooks)?;   // blocking: worker thread
```
Never prints, prompts or exits. `ConvertError::kind()` is a stable tag; `Display` is a user-ready message.
`Report`: `model_dir`, `name`, `files`, `extras`, `properties_kept`, `diagnostics`, `stats`.

## Contract the GUI can rely on
- Folder `<output_dir>/<name>/` with `<name>.obj`, `<name>.mtl`, `diffuse_<name>.png` (+ `normal_`/`pbr_`/`emissive_`), `properties.cfg`.
  ASCII `[a-z0-9_]` names; one material named `<name>`; every face under a named `usemtl`.
- At least one `UCX_` hull always (upper-case prefix).
- Built in a hidden `.<name>.tmp<pid>` folder and renamed; the `.obj` is written last. The folder scan may skip dot-folders.
- `overwrite`: Replace (own folders only) / Fail / Rename (`name_2`). `keep_existing_properties` defaults to true; a GUI that owns the
  cfg values should set it to false.
- Optional `icon.png` (`write_icon`) and `votv_convert.json` (`write_sidecar`), both off by default.
- `votv-model-core` offers `PHYSICAL_MATERIALS`, `TextureFilter`, `TextureKind`, `PropertiesCfg` (parse + write) to replace duplicates in the GUI.

- `properties.indestructible` (default **true**) writes a cfg without `health`, `impact_resistance` and `damage_resistance`, which makes the model indestructible in game (observed, not decoded; any of the three present makes it breakable).
  Suggested UI: a checkbox, on by default, that disables the health slider; unticking it writes `properties.health`.

## Emission, alpha and outlines (conversion options)
`emissive_mode` (Auto / Literal) and `drop_outline_parts` are ordinary conversion options (checkbox / dropdown). Auto is right for almost every model; Literal is the escape hatch
if a real glow map is wrongly reinterpreted. Their effects appear as `Info` diagnostics (`emissive_used_as_colour`, `emissive_duplicate_ignored`, `emissive_ignored`,
`outline_dropped`, `alpha_cutout`), which the GUI can show in its log. `inspect()` lists the emission ones in `hints` so the panel can explain them before converting.

## Metallic
`metallic_scale` (0-1, default 1 = as authored) is a conversion option (slider). Reported problem: a car converted with metallic textures of 255 (car paint, chrome) came out black *in the game*.
Two causes were found and are separate: flat colours were not converted from linear to sRGB (fixed, no option), and fully metallic surfaces have no diffuse light (UE: diffuse = base x (1 - metallic)),
so they depend entirely on reflections. Whether metallic 255 is dark in the game's *world* print (not only in the printer thumbnail, where it is known to be dark) is still unconfirmed, so the
default stays at 1 until a test says otherwise. Note for the manager's own preview: Bevy's `StandardMaterial` without an environment map also renders metallic 1 as black, which is a separate,
preview-only effect (an environment light fixes it); do not use the preview to judge metallic.

## Properties panel (generated from the schema)
Conversion settings (`Options`) and model properties (`Options.properties`, type `votv_model_core::PropertiesCfg`) are separate. The GUI should show them in
separate sections. Build the properties section from the schema instead of one hand-written row per field:
```rust
use votv_convert::model_core::{Group, Kind, PropValue, PROPERTY_SPECS};
for group in Group::ALL {
    for spec in PROPERTY_SPECS.iter().filter(|s| s.group == group) {
        let value = opts.properties.get(spec.key).unwrap();      // PropValue
        // spec.kind: Bool | Int{min,max} | Float{min,max,step} | Color | Vec3{..} | Choice(labels)  -> pick the widget
        // spec.label, spec.doc (tooltip), spec.confidence (Verified | Observed | Inferred | Unverified: e.g. badge the last two)
        // on change: opts.properties.set(spec.key, new_value)?   (Err(String) explains a bad value)
        // spec.optional (explotano): PropValue::Unset means "not written"
    }
}
```
`spec.virtual_key` marks `indestructible`, which is not a cfg key but decides whether health / impact_resistance / damage_resistance are written; the
GUI should grey those three out while it is on. Edible is no longer an option: show it as derived (`opts.properties.is_edible()`, true for physical material 8).
After a conversion `Report.stats` has what the game will derive from the hulls: `hull_size_cm`, `bites`, `edible`, `health_in_game` (None = indestructible);
good for a read-only line under the properties. `opts.properties.extra` is an advanced free-form `key=value` list.
Saved options: read them with `Options::from_json(&str)` (migrates the retired `edible` field), write with `to_json()`.

## Cache (re-generation speed)
Keep one `ConvertCache` for the converter session and give every job a clone:
```rust
let cache = votv_convert::ConvertCache::new();                    // once, e.g. in the session state
let hooks = Hooks::default().with_cancel(...).with_progress(...).with_cache(cache.clone());   // per job
```
Same file + same options = everything reused. Typical effect per edit: collision offsets, hull face cap, fallback size, show-collision offset and
every `properties` value reuse all stages (only writing the output folder remains); `scale` redoes only the geometry text; atlas/texture options
redo the atlas and geometry but not VHACD; `vhacd_resolution` / `max_decomp_hulls` / `collision_quality` redo only collision; `humanoid` or a changed
file reloads everything. `Report::cache_hits` shows what was reused (handy for a debug line). Call `cache.clear()` when the user closes the model
(it holds the decoded textures). Output is byte-identical to an uncached run, so no behaviour needs to change in the GUI.

## Threading
`convert_with` is blocking and CPU heavy (VHACD, Lanczos). Run it on a worker thread; progress and diagnostics callbacks are called
on that thread (`Send + Sync`), forward them over a channel. See `crates/votv-convert/examples/gui_worker.rs`.

## Known unverified / limits
Nothing has been run inside VotV. WebP/KTX2 textures fail with `unreadable`; only UV set 0; wrap modes treated as repeat.
The bites/health the game derives come from the hull bounding box, so `fallback_collision_size` matters for large models.

## Requesting changes
Send the converter agent: the option/field you need, the `DiagCode` or error behaviour you want, or the compiler/test output.
Changes are listed in CHANGELOG.md; breaking ones bump the minor version.
