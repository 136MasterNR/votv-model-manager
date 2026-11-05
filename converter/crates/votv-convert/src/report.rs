//! What `convert` tells the caller besides the files: structured diagnostics, statistics,
//! progress events and a cancel token.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::cache::ConvertCache;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Narration of what happened ("Packed 5 textures into a 512x256 atlas").
    Info,
    /// Something was compromised or probably needs the user's attention.
    Warning,
    /// Louder warning: the output is likely wrong unless the user acts
    /// (e.g. rigged model without `humanoid`).
    Attention,
}

/// Machine-readable id of a diagnostic. Stable: new codes are added, existing ones are not renamed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagCode {
    CustomCollisionFound,
    CustomCollisionIgnored,
    CustomCollisionUsed,
    NoCollCustomKept,
    SkeletonWithoutHumanoid,
    HumanoidApplied,
    HumanoidNoSkin,
    TransparentDropped,
    TransparentSummary,
    DuplicatePart,
    AtlasPacked,
    AtlasShrunk,
    EmissiveAtlas,
    NormalAtlas,
    PbrAtlas,
    CollisionDecomposed,
    CollisionAdded,
    HullsTrimmed,
    DecompositionFailed,
    CollisionFailed,
    CollisionBoxAdded,
    ShowColl,
    ShowCollNoCollision,
    TiledSplit,
    TiledUnsplit,
    PropertiesKept,
    PropertiesOptionsIgnored,
    UvSetIgnored,
    TextureTransformIgnored,
    OutputRenamed,
    IconWritten,
    SidecarWritten,
    Summary,
    Rigged,
    Transparent,
    TiledUvs,
    OddSize,
    SpecGlossConverted,
    AlphaCutout,
    EmissiveUsedAsColour,
    EmissiveDuplicateIgnored,
    EmissiveIgnored,
    OutlineDropped,
    UnsupportedExtensionIgnored,
}

impl DiagCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CustomCollisionFound => "custom_collision_found",
            Self::CustomCollisionIgnored => "custom_collision_ignored",
            Self::CustomCollisionUsed => "custom_collision_used",
            Self::NoCollCustomKept => "no_coll_custom_kept",
            Self::SkeletonWithoutHumanoid => "skeleton_without_humanoid",
            Self::HumanoidApplied => "humanoid_applied",
            Self::HumanoidNoSkin => "humanoid_no_skin",
            Self::TransparentDropped => "transparent_dropped",
            Self::TransparentSummary => "transparent_summary",
            Self::DuplicatePart => "duplicate_part",
            Self::AtlasPacked => "atlas_packed",
            Self::AtlasShrunk => "atlas_shrunk",
            Self::EmissiveAtlas => "emissive_atlas",
            Self::NormalAtlas => "normal_atlas",
            Self::PbrAtlas => "pbr_atlas",
            Self::CollisionDecomposed => "collision_decomposed",
            Self::CollisionAdded => "collision_added",
            Self::HullsTrimmed => "hulls_trimmed",
            Self::DecompositionFailed => "decomposition_failed",
            Self::CollisionFailed => "collision_failed",
            Self::CollisionBoxAdded => "collision_box_added",
            Self::ShowColl => "show_coll",
            Self::ShowCollNoCollision => "show_coll_no_collision",
            Self::TiledSplit => "tiled_split",
            Self::TiledUnsplit => "tiled_unsplit",
            Self::PropertiesKept => "properties_kept",
            Self::PropertiesOptionsIgnored => "properties_options_ignored",
            Self::UvSetIgnored => "uv_set_ignored",
            Self::TextureTransformIgnored => "texture_transform_ignored",
            Self::OutputRenamed => "output_renamed",
            Self::IconWritten => "icon_written",
            Self::SidecarWritten => "sidecar_written",
            Self::Summary => "summary",
            Self::Rigged => "rigged",
            Self::Transparent => "transparent",
            Self::TiledUvs => "tiled_uvs",
            Self::OddSize => "odd_size",
            Self::SpecGlossConverted => "spec_gloss_converted",
            Self::AlphaCutout => "alpha_cutout",
            Self::EmissiveUsedAsColour => "emissive_used_as_colour",
            Self::EmissiveDuplicateIgnored => "emissive_duplicate_ignored",
            Self::EmissiveIgnored => "emissive_ignored",
            Self::OutlineDropped => "outline_dropped",
            Self::UnsupportedExtensionIgnored => "unsupported_extension_ignored",
        }
    }
}

/// One message. `code` is stable and meant for GUIs (icons, filtering, links to help);
/// `message` is a ready-to-display sentence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: DiagCode,
    pub message: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Stats {
    pub parts: usize,
    pub unique_materials: usize,
    pub vertices: usize,
    pub faces: usize,
    /// Same as `faces` (every face is a triangle).
    pub triangles: usize,
    /// Material sections of the model (the converter writes exactly one: a single atlas material).
    pub sections: usize,
    /// Number of `UCX_` collision hulls written (same as `collision_objects`).
    pub hulls: usize,
    pub atlas_width: u32,
    pub atlas_height: u32,
    /// Number of `UCX_*` objects written.
    pub collision_objects: usize,
    pub collision_vertices: usize,
    pub collision_faces: usize,
    /// True when the model's own `UCX_*` meshes were used.
    pub collision_from_source: bool,
    /// Faces that were cut along texture-repeat lines.
    pub tiled_faces_split: usize,
    /// Faces left squeezed into a single tile (may look stretched).
    pub tiled_faces_unsplit: usize,
    pub has_emissive: bool,
    pub has_normal: bool,
    pub has_pbr: bool,
    /// Bounding-box size of all collision hulls in cm (the game derives bites and default health from it).
    pub hull_size_cm: Option<[f64; 3]>,
    /// Bites the game will give the print: `max(round(X*Y*Z / 1000), 1)` of `hull_size_cm`.
    pub bites: u32,
    /// The print is edible (physical material 8, food). From the written cfg, including a kept existing one.
    pub edible: bool,
    /// Health the game ends up with, or `None` when indestructible.
    pub health_in_game: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputFile {
    pub name: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    /// The finished folder (drop it into `%LocalAppData%\VotV\Assets\meshes\printer`).
    pub model_dir: PathBuf,
    /// Folder / file stem actually used (sanitised ASCII; may carry a `_2` suffix with `OverwritePolicy::Rename`).
    pub name: String,
    pub files: Vec<OutputFile>,
    pub stats: Stats,
    pub diagnostics: Vec<Diagnostic>,
    /// An existing `properties.cfg` was kept, so `edible` / `physical_material` /
    /// `emissive_strength` were not applied.
    pub properties_kept: bool,
    /// Stages that were reused from the [`ConvertCache`] this run (`scene`, `atlas`, `geometry`, `collision`); empty without a cache.
    pub cache_hits: Vec<String>,
    /// Extra files written next to the model (`icon.png`, `votv_convert.json`), if requested.
    pub extras: Vec<String>,
}

impl Report {
    pub fn warnings(&self) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics.iter().filter(|d| d.severity != Severity::Info)
    }
    pub fn has_attention(&self) -> bool {
        self.diagnostics.iter().any(|d| d.severity == Severity::Attention)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Loading,
    Preparing,
    Atlas,
    Geometry,
    Collision,
    Writing,
    Done,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Progress {
    pub stage: Stage,
    /// 0.0 - 1.0 across the whole conversion (monotonic).
    pub fraction: f32,
    pub message: String,
}

/// Cheap to clone; call `cancel()` from any thread. The converter checks it between steps and
/// returns `ConvertError::Cancelled`. An unfinished output folder is left as it is, not cleaned up,
/// except that the previous contents were already replaced (same as the Python version on error).
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }
    /// Share an existing flag (e.g. the manager's own `Arc<AtomicBool>`); setting it to true cancels.
    pub fn from_flag(flag: Arc<AtomicBool>) -> Self {
        Self(flag)
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

type ProgressFn = dyn Fn(&Progress) + Send + Sync;
type DiagFn = dyn Fn(&Diagnostic) + Send + Sync;

/// Optional callbacks and the cancel token. `Hooks::default()` = no callbacks.
#[derive(Default)]
pub struct Hooks {
    pub cancel: CancelToken,
    pub on_progress: Option<Box<ProgressFn>>,
    /// Called as each diagnostic is produced (they are also all in `Report::diagnostics`),
    /// so a GUI can show a live log.
    pub on_diagnostic: Option<Box<DiagFn>>,
    /// Reuse expensive results between runs; see [`ConvertCache`].
    pub cache: Option<ConvertCache>,
}

impl Hooks {
    pub fn with_progress(mut self, f: impl Fn(&Progress) + Send + Sync + 'static) -> Self {
        self.on_progress = Some(Box::new(f));
        self
    }
    pub fn with_diagnostics(mut self, f: impl Fn(&Diagnostic) + Send + Sync + 'static) -> Self {
        self.on_diagnostic = Some(Box::new(f));
        self
    }
    pub fn with_cache(mut self, c: ConvertCache) -> Self {
        self.cache = Some(c);
        self
    }
    pub fn with_cancel(mut self, t: CancelToken) -> Self {
        self.cancel = t;
        self
    }
}

/// Internal collector used by the pipeline.
pub(crate) struct Ctx<'a> {
    pub hooks: &'a Hooks,
    pub diags: Vec<Diagnostic>,
    pub cache_hits: Vec<&'static str>,
}

impl<'a> Ctx<'a> {
    pub fn new(hooks: &'a Hooks) -> Self {
        Self { hooks, diags: Vec::new(), cache_hits: Vec::new() }
    }
    pub fn emit(&mut self, severity: Severity, code: DiagCode, message: impl Into<String>) {
        let d = Diagnostic { severity, code, message: message.into() };
        if let Some(f) = &self.hooks.on_diagnostic {
            f(&d);
        }
        self.diags.push(d);
    }
    /// Re-emit messages that were recorded during an earlier run (cached stages).
    pub fn replay(&mut self, diags: &[Diagnostic]) {
        for d in diags {
            self.emit(d.severity, d.code, d.message.clone());
        }
    }
    pub fn info(&mut self, code: DiagCode, m: impl Into<String>) {
        self.emit(Severity::Info, code, m)
    }
    pub fn warn(&mut self, code: DiagCode, m: impl Into<String>) {
        self.emit(Severity::Warning, code, m)
    }
    pub fn attention(&mut self, code: DiagCode, m: impl Into<String>) {
        self.emit(Severity::Attention, code, m)
    }
    pub fn progress(&self, stage: Stage, fraction: f32, message: impl Into<String>) {
        if let Some(f) = &self.hooks.on_progress {
            f(&Progress { stage, fraction: fraction.clamp(0.0, 1.0), message: message.into() });
        }
    }
    pub fn check_cancel(&self) -> crate::error::Result<()> {
        if self.hooks.cancel.is_cancelled() {
            Err(crate::error::ConvertError::Cancelled)
        } else {
            Ok(())
        }
    }
}
