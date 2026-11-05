//! `properties.cfg`: key names, value formats, writer and a tolerant parser.
//!
//! These are the values of the **model** (durability, physical material, lamp, texture filters, emission). They are
//! deliberately independent of how a model is *converted*; the converter's own settings live in `votv_convert::Options`.
//! [`crate::schema`] describes every property (label, group, range, documentation) so a UI can be generated from it.
//!
//! Field names are the cfg key names, character for character (including the trailing underscore of the
//! `filter_*_` keys), so serde output of [`PropertiesCfg`] uses the same names as the file.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Engine `TextureFilter` byte written per map (`filter_diffuse_` etc.).
/// Verified (votv-printer-internals, `printedObject`): 0 Nearest, 1 Bilinear, 2 Trilinear, 3 Default.
/// Multi-material cfgs may use `filter_<kind>_<material>`; the converter writes the single-material form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TextureFilter(pub u8);

impl TextureFilter {
    pub const NEAREST: Self = Self(0);
    pub const BILINEAR: Self = Self(1);
    pub const TRILINEAR: Self = Self(2);
    pub const DEFAULT: Self = Self(3);
    pub const ALL: [Self; 4] = [Self::NEAREST, Self::BILINEAR, Self::TRILINEAR, Self::DEFAULT];

    pub fn name(self) -> &'static str {
        match self.0 {
            0 => "Nearest",
            1 => "Bilinear",
            2 => "Trilinear",
            3 => "Default",
            _ => "?",
        }
    }
}

impl Default for TextureFilter {
    /// 0 (Nearest), what the converter has always written.
    fn default() -> Self {
        Self::NEAREST
    }
}

/// Names of the game's physical-material assets for `physical_material` 0-15 (read from `printedObject.setProps`).
/// Index 8 is `food`: the game sets `edible` exactly when the material is food, and only for models that have a
/// `UCX_` hull. Out-of-range values hit an undecoded switch default.
pub const PHYSICAL_MATERIALS: [&str; 16] = [
    "plastic", "wood", "metal", "concrete", "rubber", "bag", "metal_hollow", "flesh", "food", "device", "dirt",
    "glass", "paper", "vehicle", "wheel", "ball",
];

/// `physical_material` value that makes a print edible.
pub const PHYSICAL_MATERIAL_FOOD: u8 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rgb {
    pub r: f64,
    pub g: f64,
    pub b: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

/// Every key the converter writes. Defaults equal the converter's historical output.
#[allow(non_snake_case)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PropertiesCfg {
    /// `filter_diffuse_`: texture filter of the diffuse map.
    pub filter_diffuse_: TextureFilter,
    /// `filter_pbr_`: texture filter of the pbr map.
    pub filter_pbr_: TextureFilter,
    /// `filter_normal_`: texture filter of the normal map.
    pub filter_normal_: TextureFilter,
    /// `filter_emissive_`: texture filter of the emissive map.
    pub filter_emissive_: TextureFilter,
    /// `physical_material`: impact/footstep sound material, 0-15 (see [`PHYSICAL_MATERIALS`]; 8 = food = edible).
    pub physical_material: u8,
    /// `health`: hit points of the printed prop. 0 means "derive from size" in the game ((largest x smallest hull extent) / 5).
    pub health: f64,
    /// Leave the `health`, `impact_resistance` and `damage_resistance` lines out of the file. Observed in the game (reported
    /// from testing, not decoded from the Blueprints): a model whose cfg has **none of these keys is indestructible**
    /// (infinite health), while writing `impact_resistance=1` or `damage_resistance=1` makes it breakable again even without
    /// `health`. When true, those three values are ignored. **On by default.** [`PropertiesCfg::parse`] sets this when none
    /// of the three keys is present.
    pub indestructible: bool,
    /// `impact_resistance`: damage threshold for impacts.
    pub impact_resistance: f64,
    /// `damage_resistance`: general damage multiplier/resistance.
    pub damage_resistance: f64,
    /// `is_lamp`: 1 when the prop is a light source.
    pub is_lamp: bool,
    /// `lamp_color`: light colour, 0-1 per channel.
    pub lamp_color: Rgb,
    /// `lamp_offset`: light position relative to the model, in cm.
    pub lamp_offset: Vec3,
    /// `lamp_intensity`: light brightness.
    pub lamp_intensity: f64,
    /// `lamp_attenuation`: light radius.
    pub lamp_attenuation: f64,
    /// `lamp_shadows`: 1 when the light casts shadows.
    pub lamp_shadows: bool,
    /// `light_toggle`: 1 when the player can switch the light.
    pub light_toggle: bool,
    /// `emissive_strength`: brightness multiplier of the emissive map. Only written when the model has
    /// an emissive map (the converter passes `with_emissive`).
    pub emissive_strength: f64,
    /// `explotano`: a float that the game's `gatherConfigInfo` parses into the model struct; its purpose is unknown.
    /// **The key name is a guess** (the struct field name; the cfg literal was never read), so treat this as an experiment.
    /// `None` (default) leaves the line out.
    pub explotano: Option<f64>,
    /// Extra raw `key=value` lines appended after the known keys, for anything not modelled here (for example
    /// per-material filters `filter_diffuse_<material>` of multi-material models, or keys under investigation).
    /// [`PropertiesCfg::parse`] stores every key it does not know here, so nothing in an existing file is lost.
    /// Keys of known properties are rejected by [`PropertiesCfg::validate`].
    pub extra: BTreeMap<String, String>,
}

impl Default for PropertiesCfg {
    fn default() -> Self {
        Self {
            filter_diffuse_: TextureFilter::NEAREST,
            filter_pbr_: TextureFilter::NEAREST,
            filter_normal_: TextureFilter::NEAREST,
            filter_emissive_: TextureFilter::NEAREST,
            physical_material: 0,
            health: 200.0,
            indestructible: true,
            impact_resistance: 1.0,
            damage_resistance: 1.0,
            is_lamp: false,
            lamp_color: Rgb { r: 1.0, g: 1.0, b: 1.0 },
            lamp_offset: Vec3 { x: 0.0, y: 0.0, z: 0.0 },
            lamp_intensity: 5001.0,
            lamp_attenuation: 2500.0,
            lamp_shadows: false,
            light_toggle: true,
            emissive_strength: 10.0,
            explotano: None,
            extra: BTreeMap::new(),
        }
    }
}

fn num(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 { format!("{}", v as i64) } else { format!("{v:?}") }
}
fn b(v: bool) -> u8 {
    v as u8
}

impl PropertiesCfg {
    /// The file text. `with_emissive` adds the `emissive_strength` line (only meaningful when the model has
    /// an emissive map).
    pub fn to_cfg_string(&self, with_emissive: bool) -> String {
        let mut s = String::new();
        s.push_str(&format!("filter_diffuse_={}\nfilter_pbr_={}\nfilter_normal_={}\nfilter_emissive_={}\n",
            self.filter_diffuse_.0, self.filter_pbr_.0, self.filter_normal_.0, self.filter_emissive_.0));
        s.push_str(&format!("physical_material={}\n", self.physical_material));
        if !self.indestructible {
            s.push_str(&format!("health={}\nimpact_resistance={}\ndamage_resistance={}\n",
                num(self.health), num(self.impact_resistance), num(self.damage_resistance)));
        }
        s.push_str(&format!("is_lamp={}\n", b(self.is_lamp)));
        s.push_str(&format!("lamp_color=(R={:?},G={:?},B={:?})\nlamp_offset=(X={:?},Y={:?},Z={:?})\n",
            self.lamp_color.r, self.lamp_color.g, self.lamp_color.b, self.lamp_offset.x, self.lamp_offset.y, self.lamp_offset.z));
        s.push_str(&format!("lamp_intensity={}\nlamp_attenuation={}\nlamp_shadows={}\nlight_toggle={}\n",
            num(self.lamp_intensity), num(self.lamp_attenuation), b(self.lamp_shadows), b(self.light_toggle)));
        if with_emissive {
            s.push_str(&format!("emissive_strength={:?}\n", self.emissive_strength));
        }
        if let Some(x) = self.explotano {
            s.push_str(&format!("explotano={x:?}\n"));
        }
        for (k, v) in &self.extra {
            s.push_str(&format!("{k}={v}\n"));
        }
        s
    }

    /// Tolerant parser: malformed values are ignored (defaults stay); keys it does not know are kept in `extra`.
    pub fn parse(text: &str) -> Self {
        let mut c = Self::default();
        let mut seen_durability = false;
        let group = |v: &str| -> Vec<f64> {
            v.trim_matches(|ch| ch == '(' || ch == ')')
                .split(',')
                .filter_map(|p| p.split('=').nth(1).and_then(|x| x.trim().parse().ok()))
                .collect()
        };
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            let f = || v.parse::<f64>().ok();
            let flag = || v.parse::<f64>().ok().map(|x| x != 0.0);
            match k {
                "filter_diffuse_" => if let Ok(x) = v.parse() { c.filter_diffuse_ = TextureFilter(x) },
                "filter_pbr_" => if let Ok(x) = v.parse() { c.filter_pbr_ = TextureFilter(x) },
                "filter_normal_" => if let Ok(x) = v.parse() { c.filter_normal_ = TextureFilter(x) },
                "filter_emissive_" => if let Ok(x) = v.parse() { c.filter_emissive_ = TextureFilter(x) },
                "physical_material" => if let Ok(x) = v.parse::<u8>() { c.physical_material = x.min(15) },
                "health" => if let Some(x) = f() { c.health = x; seen_durability = true },
                "impact_resistance" => if let Some(x) = f() { c.impact_resistance = x; seen_durability = true },
                "damage_resistance" => if let Some(x) = f() { c.damage_resistance = x; seen_durability = true },
                "is_lamp" => if let Some(x) = flag() { c.is_lamp = x },
                "lamp_color" => if let [r, g, bl] = group(v)[..] { c.lamp_color = Rgb { r, g, b: bl } },
                "lamp_offset" => if let [x, y, z] = group(v)[..] { c.lamp_offset = Vec3 { x, y, z } },
                "lamp_intensity" => if let Some(x) = f() { c.lamp_intensity = x },
                "lamp_attenuation" => if let Some(x) = f() { c.lamp_attenuation = x },
                "lamp_shadows" => if let Some(x) = flag() { c.lamp_shadows = x },
                "light_toggle" => if let Some(x) = flag() { c.light_toggle = x },
                "emissive_strength" => if let Some(x) = f() { c.emissive_strength = x },
                "explotano" => if let Some(x) = f() { c.explotano = Some(x) },
                _ => {
                    if !k.is_empty() {
                        c.extra.insert(k.to_string(), v.to_string());
                    }
                }
            }
        }
        c.indestructible = !seen_durability;
        c
    }
}

impl PropertiesCfg {
    /// Check that the values can be written and make sense. Called by the converter before it writes anything.
    pub fn validate(&self) -> Result<(), String> {
        if self.physical_material > 15 {
            return Err("physical_material must be 0-15".into());
        }
        for (k, f) in [
            ("filter_diffuse_", self.filter_diffuse_),
            ("filter_pbr_", self.filter_pbr_),
            ("filter_normal_", self.filter_normal_),
            ("filter_emissive_", self.filter_emissive_),
        ] {
            if f.0 > 3 {
                return Err(format!("{k} must be 0-3 (Nearest, Bilinear, Trilinear, Default)"));
            }
        }
        let numbers = [
            ("health", self.health),
            ("impact_resistance", self.impact_resistance),
            ("damage_resistance", self.damage_resistance),
            ("lamp_color.r", self.lamp_color.r),
            ("lamp_color.g", self.lamp_color.g),
            ("lamp_color.b", self.lamp_color.b),
            ("lamp_offset.x", self.lamp_offset.x),
            ("lamp_offset.y", self.lamp_offset.y),
            ("lamp_offset.z", self.lamp_offset.z),
            ("lamp_intensity", self.lamp_intensity),
            ("lamp_attenuation", self.lamp_attenuation),
            ("emissive_strength", self.emissive_strength),
            ("explotano", self.explotano.unwrap_or(0.0)),
        ];
        for (k, v) in numbers {
            if !v.is_finite() {
                return Err(format!("{k} must be a finite number"));
            }
        }
        for (k, v) in [("lamp_intensity", self.lamp_intensity), ("lamp_attenuation", self.lamp_attenuation), ("emissive_strength", self.emissive_strength)] {
            if v < 0.0 {
                return Err(format!("{k} cannot be negative"));
            }
        }
        for (k, v) in &self.extra {
            if k.is_empty() || k.contains(['=', '\n', '\r']) || v.contains(['\n', '\r']) {
                return Err(format!("extra line {k:?} is not a valid key=value pair"));
            }
            if crate::schema::PROPERTY_SPECS.iter().any(|s| s.key == k.as_str()) {
                return Err(format!("{k} is a known property; set it through its own field, not `extra`"));
            }
        }
        Ok(())
    }

    /// The game marks a print edible exactly when its physical material is food (8).
    pub fn is_edible(&self) -> bool {
        self.physical_material == PHYSICAL_MATERIAL_FOOD
    }

    /// Health the game ends up with for hulls of the given bounding-box size (cm), or `None` when indestructible.
    /// Verified in `printedObject.setProps`: negative becomes 0, and 0 becomes (largest extent x smallest extent) / 5.
    pub fn health_in_game(&self, hull_size: [f64; 3]) -> Option<f64> {
        if self.indestructible {
            return None;
        }
        let h = self.health.max(0.0);
        Some(if h == 0.0 { auto_health(hull_size) } else { h })
    }
}

/// `bites` the game computes from the hull bounding box (cm): `max(round(X*Y*Z / 1000), 1)`.
pub fn bites_for(size: [f64; 3]) -> u32 {
    (size[0] * size[1] * size[2] / 1000.0).round().max(1.0) as u32
}

/// Health the game derives when `health` is 0: (largest extent x smallest extent) / 5.
pub fn auto_health(size: [f64; 3]) -> f64 {
    let max = size.iter().cloned().fold(f64::MIN, f64::max);
    let min = size.iter().cloned().fold(f64::MAX, f64::min);
    max * min / 5.0
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breakable_text_is_the_historical_file() {
        let want = "filter_diffuse_=0\nfilter_pbr_=0\nfilter_normal_=0\nfilter_emissive_=0\nphysical_material=0\n\
health=200\nimpact_resistance=1\ndamage_resistance=1\nis_lamp=0\nlamp_color=(R=1.0,G=1.0,B=1.0)\n\
lamp_offset=(X=0.0,Y=0.0,Z=0.0)\nlamp_intensity=5001\nlamp_attenuation=2500\nlamp_shadows=0\nlight_toggle=1\nemissive_strength=10.0\n";
        let mut c = PropertiesCfg::default();
        c.indestructible = false;
        assert_eq!(c.to_cfg_string(true), want);
        assert!(!c.to_cfg_string(false).contains("emissive_strength"));
    }

    #[test]
    fn default_is_indestructible_so_no_health_line() {
        assert!(PropertiesCfg::default().indestructible);
        let text = PropertiesCfg::default().to_cfg_string(true);
        for key in ["health=", "impact_resistance=", "damage_resistance="] {
            assert!(!text.contains(key), "{key} must be absent when indestructible");
        }
    }

    #[test]
    fn any_durability_key_makes_it_breakable_when_parsed() {
        assert!(!PropertiesCfg::parse("impact_resistance=1\n").indestructible);
        assert!(!PropertiesCfg::parse("damage_resistance=1\n").indestructible);
        assert!(!PropertiesCfg::parse("health=50\n").indestructible);
        assert!(PropertiesCfg::parse("physical_material=3\nis_lamp=0\n").indestructible);
    }

    #[test]
    fn missing_health_means_indestructible() {
        let mut c = PropertiesCfg::default();
        c.indestructible = true;
        let text = c.to_cfg_string(true);
        assert!(!text.contains("health=") && !text.contains("impact_resistance=") && !text.contains("damage_resistance="));
        assert!(PropertiesCfg::parse(&text).indestructible);
        let mut breakable = PropertiesCfg::default();
        breakable.indestructible = false;
        assert!(!PropertiesCfg::parse(&breakable.to_cfg_string(true)).indestructible);
    }

    #[test]
    fn roundtrip() {
        let mut c = PropertiesCfg::default();
        c.physical_material = 8;
        c.is_lamp = true;
        c.lamp_color = Rgb { r: 0.5, g: 0.25, b: 1.0 };
        c.emissive_strength = 2.5;
        assert_eq!(PropertiesCfg::parse(&c.to_cfg_string(true)), c);
    }
}
