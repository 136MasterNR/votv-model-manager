//! A description of every `properties.cfg` property, for generating settings panels and for completeness checks.
//!
//! [`PROPERTY_SPECS`] lists each property once, in the order a panel should show it, with a label, a group, the kind of
//! control it needs (and its range), a documentation string and how sure we are about it ([`Confidence`]). The generic
//! [`PropertiesCfg::get`] / [`PropertiesCfg::set`] read and write a property by key, so a UI needs no per-field code:
//!
//! ```
//! use votv_model_core::{PropertiesCfg, PropValue, PROPERTY_SPECS};
//! let mut cfg = PropertiesCfg::default();
//! for spec in PROPERTY_SPECS {
//!     let _current = cfg.get(spec.key).unwrap();        // render a control of `spec.kind`
//! }
//! cfg.set("physical_material", PropValue::Int(8)).unwrap();
//! assert!(cfg.is_edible());
//! ```
//!
//! What the game supports is read from `printedObject.setProps` and `ui_printer.gatherConfigInfo` (the
//! `votv-printer-internals` skill). The game's `struct_multisectionCustomMesh` has exactly these model-wide values; the
//! per-section texture filters are the four `filter_*_` keys here (a multi-material model would repeat them per material,
//! see `PropertiesCfg::extra`). `edible` and `bites` are not properties: the game derives them (see `is_edible`, `bites_for`).

use serde::{Deserialize, Serialize};

use crate::cfg::{PropertiesCfg, Rgb, Vec3, PHYSICAL_MATERIALS};

/// Section of a settings panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Group {
    Durability,
    Material,
    Light,
    Appearance,
    Experimental,
}

impl Group {
    pub const ALL: [Group; 5] = [Self::Durability, Self::Material, Self::Light, Self::Appearance, Self::Experimental];

    pub fn label(self) -> &'static str {
        match self {
            Self::Durability => "Durability",
            Self::Material => "Material",
            Self::Light => "Light",
            Self::Appearance => "Appearance",
            Self::Experimental => "Experimental",
        }
    }
}

/// How sure we are about a property.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    /// Read from the game's decoded Blueprints.
    Verified,
    /// Seen in the game by testing, not decoded.
    Observed,
    /// Follows from names and call patterns.
    Inferred,
    /// Guessed; may do nothing.
    Unverified,
}

/// The control a property needs. Ranges are what a UI should offer (soft limits), except where the doc says the game limits it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Bool,
    Int { min: i64, max: i64 },
    Float { min: f64, max: f64, step: f64 },
    /// RGB, each channel 0..1.
    Color,
    /// X/Y/Z in cm.
    Vec3 { min: f64, max: f64, step: f64 },
    /// An integer whose value is the index into the labels.
    Choice(&'static [&'static str]),
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct PropertySpec {
    /// The `properties.cfg` key (or, for `virtual_key`, the name used by `get` / `set`).
    pub key: &'static str,
    pub label: &'static str,
    pub group: Group,
    pub kind: Kind,
    pub doc: &'static str,
    pub confidence: Confidence,
    /// Not a cfg key: a switch of the converter's `PropertiesCfg` that decides which keys are written.
    pub virtual_key: bool,
    /// Optional keys can be unset (`PropValue::Unset`) and are then not written.
    pub optional: bool,
}

/// A value read from or written to a property.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum PropValue {
    Bool(bool),
    Int(i64),
    Float(f64),
    Color(Rgb),
    Vec3(Vec3),
    /// An optional property that is not written.
    Unset,
}

impl PropValue {
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            Self::Int(i) => Some(*i != 0),
            _ => None,
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Float(f) => Some(*f),
            Self::Int(i) => Some(*i as f64),
            _ => None,
        }
    }
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Self::Int(i) => Some(*i),
            Self::Float(f) if f.fract() == 0.0 => Some(*f as i64),
            _ => None,
        }
    }
}

const FILTERS: &[&str] = &["Nearest", "Bilinear", "Trilinear", "Default"];

const fn spec(key: &'static str, label: &'static str, group: Group, kind: Kind, confidence: Confidence, doc: &'static str) -> PropertySpec {
    PropertySpec { key, label, group, kind, doc, confidence, virtual_key: false, optional: false }
}

/// Every property, in panel order.
pub const PROPERTY_SPECS: &[PropertySpec] = &[
    // ---- durability
    PropertySpec {
        key: "indestructible",
        label: "Indestructible",
        group: Group::Durability,
        kind: Kind::Bool,
        doc: "Leaves health, impact_resistance and damage_resistance out of the file. Observed in the game: without them the print cannot be destroyed; writing either resistance makes it breakable again.",
        confidence: Confidence::Observed,
        virtual_key: true,
        optional: false,
    },
    spec("health", "Health", Group::Durability, Kind::Float { min: 0.0, max: 2000.0, step: 10.0 }, Confidence::Verified,
        "Hit points. Negative becomes 0, and 0 means derived from the hull size ((largest extent x smallest extent) / 5). Applied only when the model has a UCX_ hull. Ignored while indestructible."),
    spec("impact_resistance", "Impact resistance", Group::Durability, Kind::Float { min: 0.0, max: 100.0, step: 0.1 }, Confidence::Verified,
        "Goes into the damage data of the print (impactResistance). Units and effect are not decoded. Ignored while indestructible."),
    spec("damage_resistance", "Damage resistance", Group::Durability, Kind::Float { min: 0.0, max: 100.0, step: 0.1 }, Confidence::Verified,
        "Goes into the damage data of the print (damageResistance). Units and effect are not decoded. Ignored while indestructible."),
    // ---- material
    spec("physical_material", "Physical material", Group::Material, Kind::Choice(&PHYSICAL_MATERIALS), Confidence::Verified,
        "Impact and footstep material, 0-15 (the game's physmat asset). 8 (food) also makes the print edible, and the game derives its bites from the hull volume. Out-of-range values are undefined."),
    // ---- light
    spec("is_lamp", "Lamp", Group::Light, Kind::Bool, Confidence::Verified, "Adds a point light to the print. The other lamp values only matter when this is on."),
    spec("lamp_color", "Lamp colour", Group::Light, Kind::Color, Confidence::Verified, "Colour of the light, each channel 0-1."),
    spec("lamp_offset", "Lamp offset", Group::Light, Kind::Vec3 { min: -500.0, max: 500.0, step: 1.0 }, Confidence::Verified, "Position of the light relative to the print."),
    spec("lamp_intensity", "Lamp intensity", Group::Light, Kind::Float { min: 0.0, max: 50000.0, step: 100.0 }, Confidence::Verified, "Brightness of the light."),
    spec("lamp_attenuation", "Lamp radius", Group::Light, Kind::Float { min: 0.0, max: 10000.0, step: 50.0 }, Confidence::Verified, "Attenuation radius of the light."),
    spec("lamp_shadows", "Lamp casts shadows", Group::Light, Kind::Bool, Confidence::Verified, "Whether the light casts shadows."),
    spec("light_toggle", "Switchable light", Group::Light, Kind::Bool, Confidence::Verified, "The player can switch the light on and off (it starts off, with a click sound)."),
    // ---- appearance
    spec("emissive_strength", "Emissive strength", Group::Appearance, Kind::Float { min: 0.0, max: 50.0, step: 0.5 }, Confidence::Verified,
        "Multiplier of the emissive map. 0 means no glow. The converter only writes it when the model has an emissive map."),
    spec("filter_diffuse_", "Diffuse filter", Group::Appearance, Kind::Choice(FILTERS), Confidence::Verified, "Texture filter of the diffuse map: Nearest, Bilinear, Trilinear or Default."),
    spec("filter_pbr_", "PBR filter", Group::Appearance, Kind::Choice(FILTERS), Confidence::Verified, "Texture filter of the pbr map."),
    spec("filter_normal_", "Normal filter", Group::Appearance, Kind::Choice(FILTERS), Confidence::Verified, "Texture filter of the normal map."),
    spec("filter_emissive_", "Emissive filter", Group::Appearance, Kind::Choice(FILTERS), Confidence::Verified, "Texture filter of the emissive map."),
    // ---- experimental
    PropertySpec {
        key: "explotano",
        label: "explotano (unknown)",
        group: Group::Experimental,
        kind: Kind::Float { min: -1000.0, max: 1000.0, step: 1.0 },
        doc: "The game parses a float with this name into the model struct, but what it does and the exact cfg key are unknown. Unset by default (the line is not written). Use it to experiment.",
        confidence: Confidence::Unverified,
        virtual_key: false,
        optional: true,
    },
];

impl PropertiesCfg {
    /// Current value of a property by key (see [`PROPERTY_SPECS`]); `None` for an unknown key.
    pub fn get(&self, key: &str) -> Option<PropValue> {
        use PropValue::*;
        Some(match key {
            "indestructible" => Bool(self.indestructible),
            "health" => Float(self.health),
            "impact_resistance" => Float(self.impact_resistance),
            "damage_resistance" => Float(self.damage_resistance),
            "physical_material" => Int(self.physical_material as i64),
            "is_lamp" => Bool(self.is_lamp),
            "lamp_color" => Color(self.lamp_color),
            "lamp_offset" => Vec3(self.lamp_offset),
            "lamp_intensity" => Float(self.lamp_intensity),
            "lamp_attenuation" => Float(self.lamp_attenuation),
            "lamp_shadows" => Bool(self.lamp_shadows),
            "light_toggle" => Bool(self.light_toggle),
            "emissive_strength" => Float(self.emissive_strength),
            "filter_diffuse_" => Int(self.filter_diffuse_.0 as i64),
            "filter_pbr_" => Int(self.filter_pbr_.0 as i64),
            "filter_normal_" => Int(self.filter_normal_.0 as i64),
            "filter_emissive_" => Int(self.filter_emissive_.0 as i64),
            "explotano" => match self.explotano {
                Some(x) => Float(x),
                None => Unset,
            },
            _ => return None,
        })
    }

    /// Set a property by key. Fails for an unknown key, a value of the wrong type or an out-of-range choice.
    pub fn set(&mut self, key: &str, v: PropValue) -> Result<(), String> {
        let bad = || format!("wrong kind of value for {key}");
        let byte = |v: &PropValue, max: i64| -> Result<u8, String> {
            let i = v.as_int().ok_or_else(|| format!("wrong kind of value for {key}"))?;
            if (0..=max).contains(&i) { Ok(i as u8) } else { Err(format!("{key} must be 0-{max}")) }
        };
        match key {
            "indestructible" => self.indestructible = v.as_bool().ok_or_else(bad)?,
            "health" => self.health = v.as_f64().ok_or_else(bad)?,
            "impact_resistance" => self.impact_resistance = v.as_f64().ok_or_else(bad)?,
            "damage_resistance" => self.damage_resistance = v.as_f64().ok_or_else(bad)?,
            "physical_material" => self.physical_material = byte(&v, 15)?,
            "is_lamp" => self.is_lamp = v.as_bool().ok_or_else(bad)?,
            "lamp_color" => match v {
                PropValue::Color(c) => self.lamp_color = c,
                _ => return Err(bad()),
            },
            "lamp_offset" => match v {
                PropValue::Vec3(o) => self.lamp_offset = o,
                _ => return Err(bad()),
            },
            "lamp_intensity" => self.lamp_intensity = v.as_f64().ok_or_else(bad)?,
            "lamp_attenuation" => self.lamp_attenuation = v.as_f64().ok_or_else(bad)?,
            "lamp_shadows" => self.lamp_shadows = v.as_bool().ok_or_else(bad)?,
            "light_toggle" => self.light_toggle = v.as_bool().ok_or_else(bad)?,
            "emissive_strength" => self.emissive_strength = v.as_f64().ok_or_else(bad)?,
            "filter_diffuse_" => self.filter_diffuse_.0 = byte(&v, 3)?,
            "filter_pbr_" => self.filter_pbr_.0 = byte(&v, 3)?,
            "filter_normal_" => self.filter_normal_.0 = byte(&v, 3)?,
            "filter_emissive_" => self.filter_emissive_.0 = byte(&v, 3)?,
            "explotano" => {
                self.explotano = match v {
                    PropValue::Unset => None,
                    other => Some(other.as_f64().ok_or_else(bad)?),
                }
            }
            _ => return Err(format!("unknown property {key}")),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cfg::{auto_health, bites_for};

    #[test]
    fn every_spec_can_be_read_and_written_back_unchanged() {
        let mut cfg = PropertiesCfg::default();
        for s in PROPERTY_SPECS {
            let v = cfg.get(s.key).unwrap_or_else(|| panic!("no getter for {}", s.key));
            let before = cfg.clone();
            cfg.set(s.key, v).unwrap_or_else(|e| panic!("{}: {e}", s.key));
            assert_eq!(cfg, before, "{} changed by get/set", s.key);
        }
        let keys: Vec<_> = PROPERTY_SPECS.iter().map(|s| s.key).collect();
        let mut sorted = keys.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), keys.len(), "duplicate spec keys");
    }

    #[test]
    fn the_schema_covers_exactly_what_the_writer_can_emit() {
        let mut cfg = PropertiesCfg::default();
        cfg.indestructible = false;
        cfg.explotano = Some(1.0);
        let text = cfg.to_cfg_string(true);
        let written: Vec<&str> = text.lines().map(|l| l.split('=').next().unwrap()).collect();
        for k in &written {
            assert!(PROPERTY_SPECS.iter().any(|s| s.key == *k && !s.virtual_key), "{k} is written but not described");
        }
        for s in PROPERTY_SPECS.iter().filter(|s| !s.virtual_key) {
            assert!(written.contains(&s.key), "{} is described but never written", s.key);
        }
        // the game's prepareCfg has 16 lines: our 17 keys are those plus the experimental explotano
        assert_eq!(written.len(), 17);
    }

    #[test]
    fn set_rejects_bad_input() {
        let mut cfg = PropertiesCfg::default();
        assert!(cfg.set("physical_material", PropValue::Int(16)).is_err());
        assert!(cfg.set("filter_pbr_", PropValue::Int(4)).is_err());
        assert!(cfg.set("lamp_color", PropValue::Float(1.0)).is_err());
        assert!(cfg.set("nope", PropValue::Bool(true)).is_err());
        assert!(cfg.set("health", PropValue::Bool(true)).is_err());
        cfg.set("explotano", PropValue::Float(2.5)).unwrap();
        assert_eq!(cfg.explotano, Some(2.5));
        cfg.set("explotano", PropValue::Unset).unwrap();
        assert_eq!(cfg.explotano, None);
    }

    #[test]
    fn extra_lines_survive_a_roundtrip_and_known_keys_are_refused() {
        let mut cfg = PropertiesCfg::default();
        cfg.extra.insert("filter_diffuse_body".into(), "1".into());
        cfg.extra.insert("some_future_key".into(), "abc".into());
        let back = PropertiesCfg::parse(&cfg.to_cfg_string(true));
        assert_eq!(back.extra, cfg.extra);
        cfg.extra.insert("health".into(), "5".into());
        assert!(cfg.validate().is_err());
        let mut bad = PropertiesCfg::default();
        bad.extra.insert("a=b".into(), "c".into());
        assert!(bad.validate().is_err());
        assert!(PropertiesCfg::default().validate().is_ok());
    }

    #[test]
    fn derived_values_follow_the_games_formulas() {
        assert_eq!(bites_for([10.0, 10.0, 10.0]), 1);
        assert_eq!(bites_for([100.0, 100.0, 100.0]), 1000);
        assert_eq!(bites_for([33.0, 33.0, 33.0]), 36);
        assert_eq!(auto_health([100.0, 50.0, 20.0]), 400.0);
        let mut cfg = PropertiesCfg::default();
        assert_eq!(cfg.health_in_game([100.0, 50.0, 20.0]), None); // indestructible
        cfg.indestructible = false;
        cfg.health = 0.0;
        assert_eq!(cfg.health_in_game([100.0, 50.0, 20.0]), Some(400.0));
        cfg.health = -5.0;
        assert_eq!(cfg.health_in_game([100.0, 50.0, 20.0]), Some(400.0));
        cfg.health = 75.0;
        assert_eq!(cfg.health_in_game([100.0, 50.0, 20.0]), Some(75.0));
        cfg.physical_material = 8;
        assert!(cfg.is_edible());
    }
}
