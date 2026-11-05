//! What both the converter and the Model Manager need to agree on about a VotV printer model folder:
//! file naming, the `UCX_` collision rules and the `properties.cfg` keys.
//!
//! No I/O, no heavy dependencies (only `serde`).
//!
//! Facts here come from the converter's output and from `votv-printer-internals` (verified by decoding the game's
//! Blueprints): the physical-material table, `TextureFilter` values, the `UCX_` rule. `explotano` (a cfg float the game
//! parses) is not modelled because its key name is still unknown.

pub mod cfg;
pub mod schema;
pub mod naming;

pub use cfg::{auto_health, bites_for, PropertiesCfg, Rgb, TextureFilter, Vec3, PHYSICAL_MATERIALS, PHYSICAL_MATERIAL_FOOD};
pub use schema::{Confidence, Group, Kind, PropValue, PropertySpec, PROPERTY_SPECS};
pub use naming::{is_ucx_name, sanitize_name, texture_file_name, ucx_object_name, TextureKind, UCX_PREFIX};
