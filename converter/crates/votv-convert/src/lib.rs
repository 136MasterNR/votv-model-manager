//! Converts `.glb` models into VotV (Voices of the Void) 3D-printer folders.
//!
//! ```no_run
//! use votv_convert::{convert, ConvertOptions};
//! let opts = ConvertOptions { output_dir: "out".into(), ..Default::default() };
//! let report = convert(&opts, "model.glb".as_ref())?;
//! println!("{}", report.model_dir.display());
//! # Ok::<(), votv_convert::ConvertError>(())
//! ```
//! For a GUI see [`convert_with`] (progress, live diagnostics, cancellation) and [`inspect`].
//! File naming, `UCX_` rules and `properties.cfg` live in the shared `votv-model-core` crate.

mod atlas;
mod cache;
mod collision;
mod convert;
mod fastfmt;
mod geom;
mod icon;
mod inspect;
mod scene;
mod tiling;

pub mod error;
pub mod options;
pub mod report;

pub use cache::ConvertCache;
pub use convert::{convert, convert_with, sanitize_name};
pub use error::{ConvertError, Result};
pub use inspect::{inspect, Inspection, MaterialInfo};
pub use options::{CollisionQuality, ConvertOptions, EmissiveMode, Options, OverwritePolicy, PngLevel};
pub use report::{CancelToken, DiagCode, Diagnostic, Hooks, OutputFile, Progress, Report, Severity, Stage, StageTiming, Stats};
pub use votv_model_core as model_core;
