//! The converter session. Dropping a `.glb` starts it; from then on every parameter change cancels whatever is
//! running and regenerates the model with the new parameters, and the result is shown in the 3D preview.
//!
//! Flow (all heavy work is on worker threads; this file only polls channels):
//!
//!   drop .glb -> `begin`  -> inspect thread (size, skins, transparency) -> presets applied -> `changed(0)`
//!   parameter edited      -> `changed(delay)`: the running job is cancelled at once, a new one is queued
//!   queue ticks down      -> `start_job`: convert_with() on a worker thread, then the output folder is parsed for the
//!                            preview on the same thread, so the UI thread never touches disk
//!   worker sends Done     -> `converter_system` shows the model with `preview::show_loaded`
//!
//! Rules from the spec:
//!   * discrete controls (toggles, choices) regenerate immediately (delay 0);
//!   * sliders wait until the pointer is released AND 0.5 s have passed without another change (`SLIDER_DELAY`);
//!   * a change always cancels the running conversion / preview load first. The converter only checks its cancel
//!     flag between steps, so a cancelled worker can keep running for a moment (e.g. inside VHACD); the next job
//!     therefore waits until that worker has really exited (`workers`), so cancelled runs never pile up.
//!
//! Every job is built in its own temp folder `<temp>/votv-model-manager-convert/job<N>/`, so a cancelled job can never
//! disturb the one that follows. A finished model is then installed into the library folder (the custom folder the
//! user picked, or the default VotV printer folder) together with a copy of the source `.glb` and the
//! `votv_convert.json` sidecar (the options used; the Edit button reads it back).
//!
//! Name rule when installing: if `<name>` does not exist, or exists and carries a sidecar (this program made it), it
//! is used / replaced. Otherwise the model is saved as `<name>_gen`, and an existing `<name>_gen` is replaced entirely.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};

use bevy::{prelude::*, window::RequestRedraw};
use votv_convert::{
    convert_with, inspect, CancelToken, ConvertCache, Diagnostic, Hooks, Inspection, Options,
    OverwritePolicy, Report,
};

use crate::model::{load_with_textures, Loaded, KINDS};
use crate::preview::{show_loaded, Orbit, PreviewState};
use crate::scene::Opts;
use crate::{Layout, Library};

/// Seconds a slider must be left alone before the model is regenerated.
pub const SLIDER_DELAY: f32 = 0.5;

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    /// Nothing is happening (no file, or the last job finished).
    Idle,
    /// Reading the .glb to pre-fill the parameters.
    Inspecting,
    /// A change is waiting for the slider delay / for the previous worker to stop.
    Queued,
    Running {
        fraction: f32,
        message: String,
    },
    /// The last job finished and the model is saved in the library folder.
    Done,
    /// The last job failed; the full user-ready message.
    Failed(String),
}

// The Done message is sent once per job, so the size difference between the variants does not matter.
#[allow(clippy::large_enum_variant)]
enum Msg {
    Progress(f32, String),
    Diag(Diagnostic),
    Done(Result<(Report, Loaded), (String, String)>), // Err = (ConvertError::kind(), message)
}

struct Run {
    cancel: CancelToken,
    rx: Mutex<Receiver<Msg>>,
}

/// Decrements the live-worker count when a worker thread ends, however it ends.
struct WorkerGuard(Arc<AtomicUsize>);
impl Drop for WorkerGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[derive(Resource)]
pub struct ConvertState {
    /// The dropped file; `Some` while the converter screen is in use.
    pub input: Option<PathBuf>,
    /// The parameters the sidebar edits.
    pub opts: Options,
    pub info: Option<Inspection>,
    pub status: Status,
    /// Diagnostics of the current / last job (cleared when a job starts).
    pub diags: Vec<Diagnostic>,
    /// Report of the last successful job (the one on screen).
    pub report: Option<Report>,
    /// Seconds until the queued job may start. `None` = nothing queued.
    pub pending: Option<f32>,
    /// Sliders currently being dragged; the queue does not tick while this is above 0.
    pub hold: u32,
    pub(crate) session: u64,
    job: u64,
    shown: bool,
    restored: bool,
    run: Option<Run>,
    inspect_rx: Option<Mutex<Receiver<Result<Inspection, String>>>>,
    workers: Arc<AtomicUsize>,
    base: PathBuf,
    /// Reuses unchanged stages (file decode, atlas, geometry, VHACD) between the jobs of one session; freed in `end`.
    cache: ConvertCache,
    /// Folder the current model was saved to (library folder / name).
    pub saved: Option<PathBuf>,
    /// Something was saved this session: the list must be rescanned when going back.
    pub saved_any: bool,
}

pub(crate) fn base_options() -> Options {
    Options {
        overwrite: OverwritePolicy::Replace,
        // The sidebar owns the properties.cfg values, so always write them (INTEGRATION.md).
        keep_existing_properties: false,
        ..Options::default()
    }
}

impl ConvertState {
    pub fn new() -> Self {
        let base = std::env::temp_dir().join("votv-model-manager-convert");
        let _ = std::fs::remove_dir_all(&base); // leftovers of a crashed run
        Self {
            input: None,
            opts: base_options(),
            info: None,
            status: Status::Idle,
            diags: Vec::new(),
            report: None,
            pending: None,
            hold: 0,
            session: 0,
            job: 0,
            shown: false,
            restored: false,
            run: None,
            inspect_rx: None,
            workers: Arc::new(AtomicUsize::new(0)),
            base,
            cache: ConvertCache::new(),
            saved: None,
            saved_any: false,
        }
    }

    /// A previous run was cancelled but its thread has not exited yet.
    pub fn stopping(&self) -> bool {
        self.workers.load(Ordering::SeqCst) > 0
    }

    pub fn busy(&self) -> bool {
        self.inspect_rx.is_some() || self.run.is_some() || self.pending.is_some()
    }

    /// A new file was dropped: forget the old session, read the file, then convert it.
    pub fn begin(&mut self, path: PathBuf) {
        self.begin_with(path, None);
    }

    /// Like `begin`; with `restored` (options read back from a model's sidecar) the file's own presets are not applied.
    pub fn begin_with(&mut self, path: PathBuf, restored: Option<Options>) {
        self.cancel_all();
        self.session += 1;
        self.restored = restored.is_some();
        self.opts = match restored {
            Some(mut o) => {
                o.overwrite = OverwritePolicy::Replace;
                o.keep_existing_properties = false;
                o.output_name = None;
                o
            }
            None => base_options(),
        };
        self.cache.clear(); // a new session: free the previous model's decoded textures
        self.saved = None;
        self.info = None;
        self.diags.clear();
        self.report = None;
        self.shown = false;
        self.pending = None;
        self.hold = 0;
        self.status = Status::Inspecting;
        self.input = Some(path.clone());
        println!("[convert] {}", path.display());
        let (tx, rx) = channel();
        self.inspect_rx = Some(Mutex::new(rx));
        std::thread::spawn(move || {
            let _ = tx.send(inspect(&path).map_err(|e| e.to_string()));
        });
    }

    /// The name of the model that is open: the name set for it (Edit, rename, an earlier replace), else the folder it was
    /// saved to, else the name it will get.
    pub fn current_model_name(&self) -> Option<String> {
        self.opts
            .output_name
            .clone()
            .or_else(|| {
                self.saved
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy().into_owned())
            })
            .or_else(|| self.report.as_ref().map(|r| r.name.clone()))
            .or_else(|| {
                self.input
                    .as_ref()
                    .and_then(|p| p.file_stem())
                    .map(|n| n.to_string_lossy().into_owned())
            })
    }

    /// A new file was dropped on the converter screen: swap the input, keep the parameters, the old model on the
    /// stage and the camera, and regenerate (handy after re-exporting a model from Blender).
    pub fn replace_input(&mut self, path: PathBuf) {
        // The dropped file REPLACES the open model: it keeps the model's name (and folder), whatever the file is called.
        if let Some(name) = self.current_model_name() {
            self.opts.output_name = Some(name);
        }
        self.cancel_all();
        self.restored = true; // the user's parameters stay; the new file's presets are not applied
        self.info = None;
        self.diags.clear();
        self.pending = None;
        self.hold = 0;
        self.status = Status::Inspecting;
        self.input = Some(path.clone());
        println!("[convert] replaced input: {}", path.display());
        let (tx, rx) = channel();
        self.inspect_rx = Some(Mutex::new(rx));
        std::thread::spawn(move || {
            let _ = tx.send(inspect(&path).map_err(|e| e.to_string()));
        });
    }

    /// Leave the converter screen: stop everything and drop the session.
    pub fn end(&mut self) {
        self.cancel_all();
        self.session += 1;
        self.input = None;
        self.info = None;
        self.diags.clear();
        self.report = None;
        self.pending = None;
        self.hold = 0;
        self.shown = false;
        self.status = Status::Idle;
        self.saved = None;
        self.cache.clear();
    }

    fn cancel_all(&mut self) {
        if let Some(r) = self.run.take() {
            r.cancel.cancel();
        }
        self.inspect_rx = None;
    }

    /// A parameter was edited. Stops the running conversion / preview build now and queues a new one
    /// after `delay` seconds (0 for toggles and choices, `SLIDER_DELAY` for sliders; the latest edit wins).
    pub fn changed(&mut self, delay: f32) {
        if self.input.is_none() {
            return;
        }
        if let Some(r) = self.run.take() {
            r.cancel.cancel();
        }
        self.pending = Some(delay);
        if self.status != Status::Inspecting {
            self.status = Status::Queued;
        }
    }

    /// The pointer left a slider: the delay counts from now.
    pub fn slider_released(&mut self) {
        self.hold = self.hold.saturating_sub(1);
        if self.pending.is_some() {
            self.pending = Some(SLIDER_DELAY);
        }
    }

    /// One line for the title bar.
    pub fn summary(&self) -> String {
        let name = self
            .input
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        match &self.report {
            Some(r) => {
                let warn = r.warnings().count();
                format!(
                    "{name}   |   {} triangles   |   {} hulls   |   atlas {}x{}{}",
                    r.stats.triangles,
                    r.stats.hulls,
                    r.stats.atlas_width,
                    r.stats.atlas_height,
                    if warn > 0 {
                        format!("   |   {warn} warnings")
                    } else {
                        String::new()
                    }
                )
            }
            None => name,
        }
    }

    fn start_job(&mut self, root: PathBuf) {
        let Some(input) = self.input.clone() else {
            return;
        };
        self.pending = None;
        self.job += 1;
        let dir = self.base.join(format!("job{}", self.job));
        let mut opts = self.opts.clone();
        // Build in the temp folder under the final name (every file inside is named after the folder), install later.
        let base_name =
            votv_convert::sanitize_name(&opts.output_name.clone().unwrap_or_else(|| {
                input
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
            }));
        opts.output_name = Some(resolve_name(&root, &base_name));
        opts.output_dir = dir.clone();
        opts.write_sidecar = true; // marks the folder as ours and lets Edit restore these options
        if let Err(e) = opts.validate() {
            self.status = Status::Failed(e.to_string());
            return;
        }
        self.diags.clear();
        self.status = Status::Running {
            fraction: 0.0,
            message: "Starting".into(),
        };

        let (tx, rx) = channel::<Msg>();
        let cancel = CancelToken::new();
        // Mutex around the Sender: the hook closures must be Sync.
        let tx_progress = Mutex::new(tx.clone());
        let tx_diag = Mutex::new(tx.clone());
        let hooks = Hooks::default()
            .with_cancel(cancel.clone())
            .with_cache(self.cache.clone())
            .with_progress(move |p| {
                if let Ok(t) = tx_progress.lock() {
                    // The converter covers 0..1; the last tenth is parsing the result for the preview.
                    let _ = t.send(Msg::Progress(p.fraction * 0.9, p.message.clone()));
                }
            })
            .with_diagnostics(move |d| {
                if let Ok(t) = tx_diag.lock() {
                    let _ = t.send(Msg::Diag(d.clone()));
                }
            });

        self.workers.fetch_add(1, Ordering::SeqCst);
        let guard = WorkerGuard(self.workers.clone());
        let flag = cancel.clone();
        std::thread::spawn(move || {
            let _guard = guard;
            let res = run_job(&opts, &input, &hooks, &flag, &tx, &root);
            let _ = std::fs::remove_dir_all(&dir); // the temp job folder is never needed afterwards
            let _ = tx.send(Msg::Done(res));
        });
        self.run = Some(Run {
            cancel,
            rx: Mutex::new(rx),
        });
    }
}

/// Name to save under: `base` when it is free or was made by this program, otherwise `<base>_gen`
/// (which, if it exists, is replaced entirely).
pub fn resolve_name(root: &Path, base: &str) -> String {
    let dir = root.join(base);
    if !dir.exists() || is_ours(&dir) {
        base.to_string()
    } else {
        format!("{base}_gen")
    }
}

/// A model folder this program wrote carries the converter's sidecar.
pub fn is_ours(dir: &Path) -> bool {
    dir.join("votv_convert.json").is_file()
}

/// The options a model was converted with (from its sidecar), for the Edit button.
pub fn load_sidecar(dir: &Path) -> Option<Options> {
    let text = std::fs::read_to_string(dir.join("votv_convert.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    // `from_json` ignores retired fields and migrates old ones ("edible": true -> physical material 8).
    Options::from_json(&v.get("options")?.to_string()).ok()
}

/// The .glb saved inside a model folder: `<folder name>.glb`, else any .glb.
pub fn find_glb(dir: &Path) -> Option<PathBuf> {
    let name = dir.file_name()?.to_string_lossy().into_owned();
    let exact = dir.join(format!("{name}.glb"));
    if exact.is_file() {
        return Some(exact);
    }
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.is_file()
                && p.extension()
                    .map(|x| x.eq_ignore_ascii_case("glb"))
                    .unwrap_or(false)
        })
}

fn copy_dir_all(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let (p, t) = (e.path(), to.join(e.file_name()));
        if p.is_dir() {
            copy_dir_all(&p, &t)?;
        } else {
            std::fs::copy(&p, &t)?;
        }
    }
    Ok(())
}

/// Move the finished model folder into the library folder under `name`, replacing what is there. The old folder is
/// moved aside first and deleted only once the new one is in place (restored if that fails).
fn install(model_dir: &Path, root: &Path, name: &str) -> Result<PathBuf, String> {
    std::fs::create_dir_all(root)
        .map_err(|e| format!("Could not create the models folder {}: {e}", root.display()))?;
    let target = root.join(name);
    // resolve_name only picks a folder that is free, ours, or a `_gen` one; check again in case that changed meanwhile.
    if target.exists() && !is_ours(&target) && !name.ends_with("_gen") {
        return Err(format!("A folder named \"{name}\" appeared that this program did not make; nothing was overwritten."));
    }
    // Keep the model's icon across regenerations (the editor's "Regenerate icon" refreshes it on demand).
    let old_icon = target.join(crate::icons::ICON_FILE);
    if old_icon.is_file() && !model_dir.join(crate::icons::ICON_FILE).exists() {
        let _ = std::fs::copy(&old_icon, model_dir.join(crate::icons::ICON_FILE));
    }
    let aside = root.join(format!(".{name}.old{}", std::process::id()));
    let had_old = target.exists();
    if had_old {
        let _ = std::fs::remove_dir_all(&aside);
        if std::fs::rename(&target, &aside).is_err() {
            // could not move it (open in another program?): try to delete it outright
            let removed = if target.is_dir() {
                std::fs::remove_dir_all(&target)
            } else {
                std::fs::remove_file(&target)
            };
            removed.map_err(|e| {
                format!(
                    "Could not replace the existing folder {}: {e}",
                    target.display()
                )
            })?;
        }
    }
    let placed = std::fs::rename(model_dir, &target).or_else(|_| copy_dir_all(model_dir, &target));
    match placed {
        Ok(()) => {
            let _ = std::fs::remove_dir_all(&aside);
            Ok(target)
        }
        Err(e) => {
            let _ = std::fs::remove_dir_all(&target);
            if had_old {
                let _ = std::fs::rename(&aside, &target);
            }
            Err(format!(
                "Could not save the model to {}: {e}",
                target.display()
            ))
        }
    }
}

fn run_job(
    opts: &Options,
    input: &Path,
    hooks: &Hooks,
    cancel: &CancelToken,
    tx: &Sender<Msg>,
    root: &Path,
) -> Result<(Report, Loaded), (String, String)> {
    let cancelled = || ("cancelled".to_string(), "cancelled".to_string());
    let mut report =
        convert_with(opts, input, hooks).map_err(|e| (e.kind().to_string(), e.to_string()))?;
    if !report.cache_hits.is_empty() {
        println!(
            "[convert] reused from cache: {}",
            report.cache_hits.join(", ")
        );
    }
    if cancel.is_cancelled() {
        return Err(cancelled());
    }
    // Keep the source next to the model so it can be edited later.
    let glb = report.model_dir.join(format!("{}.glb", report.name));
    std::fs::copy(input, &glb).map_err(|e| {
        (
            "io".to_string(),
            format!("Could not save a copy of the .glb: {e}"),
        )
    })?;
    let _ = tx.send(Msg::Progress(0.93, "Building the preview".into()));
    let loaded = load_with_textures(&report.model_dir, &KINDS, None).map_err(|e| {
        (
            "preview".to_string(),
            format!("The converted model could not be previewed: {e}"),
        )
    })?;
    if cancel.is_cancelled() {
        return Err(cancelled());
    }
    let _ = tx.send(Msg::Progress(0.97, "Saving to the library".into()));
    let final_dir =
        install(&report.model_dir, root, &report.name).map_err(|e| ("install".to_string(), e))?;
    report.model_dir = final_dir;
    Ok((report, loaded))
}

/// Per-frame driver: inspect result, queue timer, job messages, preview hand-off.
#[allow(clippy::too_many_arguments)]
pub fn converter_system(
    mut commands: Commands,
    mut st: ResMut<ConvertState>,
    layout: Res<Layout>,
    time: Res<Time<Real>>,
    mouse: Res<ButtonInput<MouseButton>>,
    lib: Res<Library>,
    mut ps: ResMut<PreviewState>,
    opts: Res<Opts>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut orbit: ResMut<Orbit>,
    mut redraw: EventWriter<RequestRedraw>,
    mut last_session: Local<u64>,
) {
    if !layout.converter || st.input.is_none() {
        return;
    }
    // A new file replaced the old one: take the old model off the stage.
    if st.session != *last_session {
        *last_session = st.session;
        if let Some(old) = ps.scene.take() {
            commands.entity(old.built.root).despawn();
        }
    }
    if st.busy() {
        redraw.write(RequestRedraw); // the window only redraws on demand; keep polling while work is in flight
    }
    // Safety net: if a slider's DragEnd was lost (focus change), do not hold the queue forever.
    if st.hold > 0 && !mouse.pressed(MouseButton::Left) {
        st.hold = 0;
        if st.pending.is_some() {
            st.pending = Some(SLIDER_DELAY);
        }
    }

    // 1. Result of the inspection.
    let inspected = match st.inspect_rx.as_ref() {
        Some(m) => match m.lock() {
            Ok(rx) => match rx.try_recv() {
                Ok(r) => Some(r),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    Some(Err("Reading the file stopped unexpectedly.".to_string()))
                }
            },
            Err(_) => None,
        },
        None => None,
    };
    match inspected {
        Some(Ok(info)) => {
            // Same pre-selection as examples/gui_worker.rs (not when the options come from a saved model).
            if !st.restored {
                st.opts.humanoid = info.skins > 0;
                st.opts.drop_transparent = info.any_transparent;
            }
            for h in &info.hints {
                println!("[convert] hint: {}", h.message);
            }
            st.info = Some(info);
            st.inspect_rx = None;
            st.changed(0.0);
        }
        Some(Err(e)) => {
            println!("[convert] failed: {e}");
            st.inspect_rx = None;
            st.pending = None;
            st.status = Status::Failed(e);
        }
        None => {}
    }

    // 2. The queue: tick, then start once the sliders are released and the previous worker has exited.
    if st.inspect_rx.is_none() && st.hold == 0 {
        if let Some(t) = st.pending {
            let t = t - time.delta_secs();
            if t > 0.0 {
                st.pending = Some(t);
            } else if st.stopping() {
                st.pending = Some(0.0); // wait for the cancelled worker to finish
            } else {
                st.start_job(lib.root.clone());
            }
        }
    }

    // 3. Messages of the running job.
    let mut progress = None;
    let mut new_diags: Vec<Diagnostic> = Vec::new();
    let mut done = None;
    if let Some(run) = st.run.as_ref() {
        if let Ok(rx) = run.rx.lock() {
            loop {
                match rx.try_recv() {
                    Ok(Msg::Progress(f, m)) => progress = Some((f, m)),
                    Ok(Msg::Diag(d)) => new_diags.push(d),
                    Ok(Msg::Done(r)) => {
                        done = Some(r);
                        break;
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        done = Some(Err((
                            "internal".to_string(),
                            "The converter stopped unexpectedly.".to_string(),
                        )));
                        break;
                    }
                }
            }
        }
    }
    if let Some((fraction, message)) = progress {
        st.status = Status::Running { fraction, message };
    }
    for d in new_diags {
        println!(
            "[convert] ({:?}/{}) {}",
            d.severity,
            d.code.as_str(),
            d.message
        );
        st.diags.push(d);
    }
    if let Some(result) = done {
        st.run = None;
        match result {
            Ok((report, loaded)) => {
                let keep_camera = st.shown;
                show_loaded(
                    &mut commands,
                    &mut meshes,
                    &mut mats,
                    &mut images,
                    &mut ps,
                    &opts,
                    &mut orbit,
                    loaded,
                    keep_camera,
                );
                st.shown = true;
                // A rename saves the model under a new folder: remove the folder of the old name when this program made
                // it (it has the sidecar), after moving its icon over.
                if let Some(old) = st.saved.clone() {
                    // The input of an Edit session is the .glb INSIDE the model folder: when the folder is replaced by one
                    // with the new name, the input must point into the new folder (the old one is about to be removed).
                    if old != report.model_dir
                        && st.input.as_ref().and_then(|p| p.parent()) == Some(old.as_path())
                    {
                        st.input = Some(report.model_dir.join(format!("{}.glb", report.name)));
                    }
                    if old != report.model_dir
                        && old.parent() == report.model_dir.parent()
                        && is_ours(&old)
                    {
                        let (old_icon, new_icon) = (
                            old.join(crate::icons::ICON_FILE),
                            report.model_dir.join(crate::icons::ICON_FILE),
                        );
                        if old_icon.is_file() && !new_icon.exists() {
                            let _ = std::fs::copy(&old_icon, &new_icon);
                        }
                        let _ = std::fs::remove_dir_all(&old);
                    }
                }
                st.saved = Some(report.model_dir.clone());
                st.saved_any = true;
                st.report = Some(report);
                st.status = Status::Done;
                redraw.write(RequestRedraw);
            }
            Err((kind, message)) => {
                if kind != "cancelled" {
                    println!("[convert] failed [{kind}]: {message}");
                    st.status = Status::Failed(message);
                }
            }
        }
    }

    // 4. With no model on the stage, say what is going on in the middle of the preview.
    if ps.scene.is_none() {
        let hint = match &st.status {
            Status::Failed(m) => format!("Conversion failed\n\n{m}"),
            Status::Inspecting => "Reading the model...".to_string(),
            _ => "Converting...".to_string(),
        };
        if ps.hint != hint {
            ps.hint = hint;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> ConvertState {
        let mut s = ConvertState::new();
        s.input = Some(PathBuf::from("model.glb")); // no worker is started by these tests
        s
    }

    #[test]
    fn edits_without_a_file_are_ignored() {
        let mut s = ConvertState::new();
        s.changed(0.0);
        assert!(s.pending.is_none());
        assert_eq!(s.status, Status::Idle);
    }

    #[test]
    fn latest_edit_wins() {
        let mut s = session();
        s.changed(SLIDER_DELAY);
        assert_eq!(s.pending, Some(SLIDER_DELAY));
        s.changed(0.0); // a toggle after a slider edit starts at once
        assert_eq!(s.pending, Some(0.0));
        assert_eq!(s.status, Status::Queued);
    }

    #[test]
    fn slider_delay_counts_from_release() {
        let mut s = session();
        s.hold = 1;
        s.changed(SLIDER_DELAY);
        s.pending = Some(0.1); // pretend time passed while the queue was held
        s.slider_released();
        assert_eq!(s.hold, 0);
        assert_eq!(s.pending, Some(SLIDER_DELAY));
    }

    #[test]
    fn release_without_a_change_queues_nothing() {
        let mut s = session();
        s.hold = 1;
        s.slider_released();
        assert!(s.pending.is_none());
    }

    #[test]
    fn name_rule_gen_suffix() {
        let root = std::env::temp_dir().join(format!("votv-mm-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("chair")).unwrap();
        assert_eq!(resolve_name(&root, "lamp"), "lamp"); // free
        assert_eq!(resolve_name(&root, "chair"), "chair_gen"); // exists, not ours
        std::fs::write(root.join("chair").join("votv_convert.json"), "{}").unwrap();
        assert_eq!(resolve_name(&root, "chair"), "chair"); // exists and ours
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn leaving_the_screen_resets_the_session() {
        let mut s = session();
        s.changed(0.0);
        let before = s.session;
        s.end();
        assert!(s.input.is_none() && s.pending.is_none());
        assert_eq!(s.status, Status::Idle);
        assert!(s.session > before);
    }
}
