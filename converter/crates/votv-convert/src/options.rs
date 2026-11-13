//! All conversion settings. `Default`, serde-serializable (missing fields fall back to defaults, so saved
//! settings keep working when fields are added), every field documented so a settings panel can be built from them.
//!
//! The `properties` field mirrors `properties.cfg` key for key (see `votv_model_core::PropertiesCfg`).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use votv_model_core::PropertiesCfg;

use crate::error::{ConvertError, Result};

/// How generated collision is built when the source has no `UCX_*` meshes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CollisionQuality {
    /// A single convex hull of the whole model.
    Low,
    /// VHACD decomposition into several convex hulls (follows concave shapes).
    High,
}

/// How emissive data in the file is interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EmissiveMode {
    /// Recognise toon-shader exports that put the colour in the *emission* slot: an emissive texture that is just the albedo (same
    /// image as the base texture, or the only colour a material has) is not real glow. It is dropped, or promoted to the diffuse
    /// texture when the material has no base texture. Black or zero-factor emission is ignored too.
    Auto,
    /// Follow the file literally: every emissive texture or factor becomes glow.
    Literal,
}

/// PNG compression effort for the texture atlases. Pixels are identical at every level; only file size and encode time change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PngLevel {
    /// Very fast (fdeflate, simple filter), clearly larger files. For live previews.
    Fast,
    /// The default: adaptive filtering with the standard compression effort. Measured on the Porsche and Ninomae test models it was 4-6x faster than
    /// `Best` for files only 0.5-1.3% larger.
    Balanced,
    /// Smallest files, slowest (the converter's historical setting, before v0.2). Worth it only when file size matters more than waiting.
    Best,
}

/// What to do when `<output_dir>/<name>` already exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OverwritePolicy {
    /// Replace it, but only if it looks like a folder this converter wrote (otherwise `OutputFolderNotOurs`).
    /// The old folder is swapped out only after the new one is complete.
    Replace,
    /// Fail with `OutputExists` and touch nothing.
    Fail,
    /// Keep the existing folder and write to `<name>_2`, `<name>_3`, ... (the report says which name was used).
    Rename,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConvertOptions {
    /// Folder that will contain the finished `<name>/` model folder. Created if missing.
    pub output_dir: PathBuf,
    /// Name of the model folder and of every file in it. `None` = the input file's name.
    /// Always reduced to lower-case ASCII `[a-z0-9_]` (`votv_model_core::sanitize_name`).
    pub output_name: Option<String>,
    /// Behaviour when the target folder exists.
    pub overwrite: OverwritePolicy,
    /// With `Replace`: keep the existing `properties.cfg` (the game and the user edit it) instead of writing a
    /// fresh one from `properties`. A settings panel that owns the cfg values should set this to `false`.
    pub keep_existing_properties: bool,
    /// Also write `votv_convert.json` (source path, options, converter version) so a manager can offer
    /// "re-convert". Whether the game ignores extra files is unverified, so this is off by default.
    pub write_sidecar: bool,
    /// Also render `icon.png`, a small software-rendered preview of the textured model.
    pub write_icon: bool,
    /// Edge length in pixels of `icon.png` (16-1024).
    pub icon_size: u32,

    /// Multiplier from source units to VotV cm. glTF is metres, so 100 converts; use 1 if the model is already in cm.
    pub scale: f64,
    /// Place skinned meshes using the skeleton's bind data (fixes characters exported in a T-pose).
    pub humanoid: bool,
    /// Largest edge (px) of any single texture before it is packed into the atlas.
    pub max_tile_size: u32,
    /// Largest edge (px) of the final atlas; bigger atlases are shrunk to fit.
    pub max_atlas_size: u32,
    /// Remove parts whose material is transparent or masked. VotV has no alpha blending.
    pub drop_transparent: bool,
    /// Multiply base colour textures by `baseColorFactor` in linear space (the glTF rule; on by default). Many exports use a white or grey texture with a
    /// colour or a near-black factor (shadow blobs, tinted plastics), which looks wrong when the factor is ignored. The alpha factor of BLEND/MASK materials is
    /// always applied, independent of this option. The old Python converter ignored the factor.
    pub apply_base_color_factor: bool,
    /// Multiplier (0-1) for every metallic value written to the pbr map; 1 = as authored. A fully metallic surface (car paint, chrome) has no
    /// diffuse light and relies on reflections, so it can look dark or black in game; the printer thumbnail is known to show it dark, whether the
    /// world print does is unconfirmed. Lower it (for example 0.3) to keep such models readable; 0 makes everything non-metallic.
    pub metallic_scale: f64,
    /// Upper limit (0-1) for metallic values in the pbr map, applied after `metallic_scale`. Default 0.75. Measured in the game with a calibration print:
    /// a fully metallic (255) surface with low or medium roughness renders dark, like a mirror of the dark environment, while 192 (0.75) and below
    /// still look like the paint colour. Raise it to 1 for authored chrome at the cost of dark metal. Factor-only materials are never metallic.
    pub metallic_max: f64,
    /// See [`EmissiveMode`]. Fixes models whose colour is stored as emission (flat or black diffuse, whole model glowing).
    pub emissive_mode: EmissiveMode,
    /// Remove parts whose material or object name contains "outline": toon outline shells (inverted hulls) are only meant to be
    /// seen with special culling and show up as solid dark surfaces in VotV.
    pub drop_outline_parts: bool,
    /// PNG compression effort of the atlas files (see [`PngLevel`]). The default is `Balanced`; a GUI can use `Fast` while the user is tweaking, and `Best` is opt-in for the smallest files. The encoded bytes are
    /// cached per level, and the maps are compressed in parallel on separate threads while geometry and collision are computed.
    pub png_level: PngLevel,

    /// Generate collision from the visible geometry when the source has none. When false, only a small
    /// box collider is written (the game needs at least one `UCX_` object or it skips health, edible and bites).
    pub generate_collision: bool,
    /// Ignore `UCX_*` meshes found in the source and generate collision instead.
    pub force_generate_collision: bool,
    /// Single hull (`Low`) or VHACD decomposition (`High`).
    pub collision_quality: CollisionQuality,
    /// Face cap per hull. `None` = 200 for `High`, 80 for `Low`. Hulls above the cap are simplified (stay convex).
    pub max_hull_faces: Option<u32>,
    /// Upper bound of hulls produced by decomposition.
    pub max_decomp_hulls: u32,
    /// VHACD voxel resolution per axis: higher is tighter and slower.
    pub vhacd_resolution: u32,
    /// Edge length (cm) of the fallback box collider placed at the bottom centre of the model. Note: the game derives
    /// `bites` (volume / 1000) and the default health from the *hull* bounding box, so a small fallback box on a large
    /// model gives few bites.
    pub fallback_collision_size: f64,
    /// Horizontal collision offset in cm (applied to X of every collision object).
    pub collision_offset_x: f64,
    /// Vertical collision offset in cm (applied to Y of every collision object).
    pub collision_offset_y: f64,
    /// Debug aid: keep the model and add a red, slightly enlarged copy of the collision. Turn off for final models.
    pub show_collision: bool,
    /// How far (cm) the `show_collision` copy is pushed outward. `None` = automatic: just enough to render above the
    /// model's surface without z-fighting (0.2% of the model's size, between 0.05 and 0.5 cm). Raise it if the red copy
    /// flickers at a distance.
    pub show_collision_offset: Option<f64>,

    /// Bake an emissive atlas when any material emits light.
    pub emissive: bool,
    /// Invert the normal map's green channel (glTF is OpenGL style, VotV is DirectX style).
    pub flip_normal_green: bool,
    /// The model's `properties.cfg` values (durability, physical material, lamp, texture filters, ...), kept apart from the conversion
    /// settings above: their own type, documented and described field by field in `votv_model_core::PROPERTY_SPECS`. Ignored when an existing
    /// cfg is kept (`keep_existing_properties`). Edible is simply `properties.physical_material = 8` (food).
    pub properties: PropertiesCfg,
}

/// Shorter name for the manager.
pub type Options = ConvertOptions;

impl Default for ConvertOptions {
    fn default() -> Self {
        Self {
            output_dir: PathBuf::from("."),
            output_name: None,
            overwrite: OverwritePolicy::Replace,
            keep_existing_properties: true,
            write_sidecar: false,
            write_icon: false,
            icon_size: 256,
            scale: 100.0,
            humanoid: false,
            max_tile_size: 1024,
            max_atlas_size: 4096,
            drop_transparent: false,
            apply_base_color_factor: true,
            metallic_scale: 1.0,
            metallic_max: 0.75,
            emissive_mode: EmissiveMode::Auto,
            drop_outline_parts: true,
            png_level: PngLevel::Balanced,
            generate_collision: true,
            force_generate_collision: false,
            collision_quality: CollisionQuality::High,
            max_hull_faces: None,
            max_decomp_hulls: 64,
            vhacd_resolution: 64,
            fallback_collision_size: 33.0,
            collision_offset_x: 0.0,
            collision_offset_y: 0.0,
            show_collision: false,
            show_collision_offset: None,
            emissive: true,
            flip_normal_green: true,
            properties: PropertiesCfg::default(),
        }
    }
}

impl ConvertOptions {
    pub(crate) fn hull_face_cap(&self) -> usize {
        self.max_hull_faces.map(|v| v as usize).unwrap_or(match self.collision_quality {
            CollisionQuality::High => 200,
            CollisionQuality::Low => 80,
        })
    }

    /// Kept so older callers compile; the properties are used as they are.
    #[deprecated(note = "use `properties` directly; the separate `edible` option is gone (physical_material 8 is food)")]
    pub fn effective_properties(&self) -> PropertiesCfg {
        self.properties.clone()
    }

    /// Load options saved by an earlier version (for example a `votv_convert.json` sidecar or a preset). Unknown fields are ignored,
    /// missing ones take their defaults, and retired fields are migrated: `"edible": true` becomes `properties.physical_material = 8`.
    pub fn from_json(text: &str) -> Result<Self> {
        let mut v: serde_json::Value = serde_json::from_str(text).map_err(|e| ConvertError::InvalidOption(format!("not valid JSON: {e}")))?;
        if let Some(o) = v.as_object_mut() {
            let edible = o.remove("edible").and_then(|e| e.as_bool()).unwrap_or(false);
            if edible {
                let props = o.entry("properties").or_insert_with(|| serde_json::json!({}));
                if let Some(p) = props.as_object_mut() {
                    p.insert("physical_material".into(), serde_json::json!(8));
                }
            }
        }
        serde_json::from_value(v).map_err(|e| ConvertError::InvalidOption(format!("could not read the options: {e}")))
    }

    /// The options as JSON (what the sidecar stores).
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// Check values the converter cannot work with. `convert` calls this itself; a GUI can call it on every edit.
    pub fn validate(&self) -> Result<()> {
        let bad = |m: &str| Err(ConvertError::InvalidOption(m.to_string()));
        if !(self.scale.is_finite() && self.scale > 0.0) {
            return bad("scale must be a positive number");
        }
        if self.max_tile_size == 0 || self.max_atlas_size == 0 {
            return bad("max tile / atlas size must be at least 1");
        }
        self.properties.validate().map_err(ConvertError::InvalidOption)?;
        if self.max_decomp_hulls == 0 {
            return bad("max decomposition hulls must be at least 1");
        }
        if self.vhacd_resolution < 8 {
            return bad("VHACD resolution must be at least 8");
        }
        if !(self.fallback_collision_size.is_finite() && self.fallback_collision_size > 0.0) {
            return bad("fallback collision size must be positive");
        }
        if let Some(o) = self.show_collision_offset {
            if !(o.is_finite() && (0.0..=100.0).contains(&o)) {
                return bad("show-collision offset must be between 0 and 100 cm");
            }
        }
        if !(self.metallic_max.is_finite() && (0.0..=1.0).contains(&self.metallic_max)) {
            return bad("metallic max must be between 0 and 1");
        }
        if !(self.metallic_scale.is_finite() && (0.0..=1.0).contains(&self.metallic_scale)) {
            return bad("metallic scale must be between 0 and 1");
        }
        if self.write_icon && !(16..=1024).contains(&self.icon_size) {
            return bad("icon size must be 16-1024");
        }
        Ok(())
    }
}
