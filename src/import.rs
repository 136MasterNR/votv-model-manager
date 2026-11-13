//! Importing ready-made models: drop (or pick) one or more `.zip`, `.7z` or `.rar` files on the list and they are UNPACKED
//! into the VotV printer folder, nothing is converted.
//!
//! * An archive is unpacked: every folder inside that holds an `.obj` (or `.obj.disabled`) becomes a model folder; an
//!   `.obj` at the root of the archive makes the archive's own name the folder name. Only the files of those folders are
//!   written (a `.blend` or `.psd` next to the model is not).
//! * A name that already exists is never overwritten: the copy is called `name_2`, `name_3`, ...
//! * A model is REFUSED unless its name matches its diffuse texture: the model folder (or the archive, for an `.obj` at the
//!   root) must be called like the `diffuse_<name>.png` it holds (letter case does not matter). The game finds a model's
//!   textures by that name, so a model that does not follow it would arrive without its texture. The user is always asked
//!   (even with no diffuse texture at all). When the folder holds OTHER diffuse textures (`diffuse_<something else>.png`), the user is asked which one is the model's diffuse
//!   (`importpick.rs`): a copy of the chosen file named `diffuse_<model folder>.png` is added (the original stays, the
//!   .mtl may refer to it). The dialog also offers **Import anyway** (the model is copied as it is, without a diffuse of
//!   its name) and **Skip this model**, which is the only way a model is left out: it is listed in the status line
//!   ("skipped") and the other models of the same archive are still imported (`requires_matching_diffuse`).
//! * `.zip` is read by the `zip` crate. `.7z` (`sevenz-rust2`, pure Rust) and `.rar` (`unrar`, rarlab's UnRAR C++ code) go
//!   through the same plan (`plan_import`): the entries are listed first, the model folders are chosen, then each wanted
//!   file is written by US, to a path that was checked (`safe_relative`: no `..`, no drive or absolute path, no odd
//!   characters), with a limit per file, in total and on the number of entries. The libraries never write to the disk.
//!   Not supported (reported as such): password-protected archives, multi-volume RAR sets.
//! * The work runs on a thread; `poll_system` shows the result in the status line and rescans the list. A RAR / 7z import that
//!   fails half way removes the folders it had started (nothing half-unpacked is left).
//!
//! The fake tile of the EDITABLE section (`ui::spawn_drop_tile`) opens the file dialog (a `.glb` or archives);
//! `ui::poll_file_pick` and `ui::drop_events` hand the archives (`is_archive`) to `start`.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::Mutex;

use bevy::prelude::*;
use bevy::window::RequestRedraw;

use crate::{Layout, Library};

#[derive(Default)]
pub struct Outcome {
    /// Names of the model folders that were created.
    pub copied: Vec<String>,
    /// One line per dropped item that could not be used.
    pub problems: Vec<String>,
}

/// What one archive gave: the model folders that were created and the models that were refused (one line each).
#[derive(Default)]
struct Imported {
    names: Vec<String>,
    refused: Vec<String>,
}

/// What the import rule says about one model folder.
#[derive(Debug, PartialEq)]
enum Verdict {
    Accept,
    /// No `diffuse_<name>.png`: the user decides. The list holds the other diffuse textures of the folder (these file
    /// names; empty when there is none) the user can pick from, besides importing anyway or skipping the model.
    Ask(Vec<String>),
}

/// The user's answer to a `Verdict::Ask` (`importpick.rs`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Choice {
    /// Use this candidate (an index into the list asked with) as the diffuse: it is also written as `diffuse_<model>.png`.
    Pick(usize),
    /// Import the model as it is, without a diffuse of its name.
    Anyway,
    /// Leave the model out.
    Skip,
}

/// Decides about a model folder: its name (the folder, or the archive's name for a model at the archive root) and the names
/// of the files directly inside it.
type Rule = fn(&str, &[String]) -> Verdict;

/// Asked for a `Verdict::Ask`: the model name and the candidate file names.
type Chooser<'a> = &'a mut dyn FnMut(&str, &[String]) -> Choice;

/// The import rule: the name must match a diffuse texture, i.e. the folder holds `diffuse_<name>.png`. Else, when it holds
/// other `diffuse_*.png` files, the user chooses among them.
fn requires_matching_diffuse(name: &str, files: &[String]) -> Verdict {
    let want = format!("diffuse_{name}.png").to_lowercase();
    if files.iter().any(|f| f.to_lowercase() == want) {
        return Verdict::Accept;
    }
    let mut others: Vec<String> = files
        .iter()
        .filter(|f| {
            let l = f.to_lowercase();
            l.starts_with("diffuse_") && l.ends_with(".png")
        })
        .cloned()
        .collect();
    others.sort();
    Verdict::Ask(others)
}

/// No rule (the readers' own tests use it: their fixtures are about the archive formats, not about textures).
#[cfg(test)]
fn rule_any(_: &str, _: &[String]) -> Verdict {
    Verdict::Accept
}

/// A chooser that always skips (tests).
#[cfg(test)]
fn no_choice(_: &str, _: &[String]) -> Choice {
    Choice::Skip
}

fn skipped(name: &str) -> String {
    format!("\"{name}\" skipped: no diffuse_{name}.png (the folder name, or the archive name, must match the diffuse texture)")
}

/// The file a chosen diffuse is also written as: `diffuse_<model folder>.png`.
fn matching_diffuse_name(dest: &Path) -> String {
    format!("diffuse_{}.png", file_name(dest))
}

/// The name a model folder of an archive gets: its own folder name, or the archive's for a model at the root.
fn model_dir_name(dir: &Path, stem: &str) -> String {
    dir.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| stem.to_string())
}

/// What the worker thread tells the main thread.
enum Msg {
    /// A model has no diffuse of its name but has others: the worker waits for the user's pick.
    Ask(Question),
    Done(Outcome),
}

/// A pending "which texture is the diffuse?" question (shown by `importpick.rs`).
pub struct Question {
    /// The archive the model comes from, and the model.
    pub archive: String,
    pub model: String,
    /// The diffuse files the model folder has (none of them is called `diffuse_<model>.png`).
    pub candidates: Vec<String>,
    reply: Mutex<Sender<Choice>>,
}

#[derive(Resource, Default)]
pub struct ImportJob {
    rx: Option<Mutex<Receiver<Msg>>>,
    /// The question the user has to answer before the import goes on.
    pub ask: Option<Question>,
    /// Counts the questions asked (the dialog rebuilds its buttons when it changes).
    pub asked: u64,
}

impl ImportJob {
    pub fn busy(&self) -> bool {
        self.rx.is_some()
    }

    /// The user's answer. Lets the worker go on.
    pub fn answer(&mut self, choice: Choice) {
        if let Some(q) = self.ask.take() {
            let _ = q.reply.lock().unwrap().send(choice);
        }
    }
}

/// Asks the user (through the main thread) and waits for the answer. A closed channel counts as "skip".
fn ask_user(tx: &Sender<Msg>, archive: &str, model: &str, candidates: &[String]) -> Choice {
    let (reply, wait) = channel();
    let sent = tx.send(Msg::Ask(Question {
        archive: archive.to_string(),
        model: model.to_string(),
        candidates: candidates.to_vec(),
        reply: Mutex::new(reply),
    }));
    if sent.is_err() {
        return Choice::Skip;
    }
    wait.recv().unwrap_or(Choice::Skip)
}

/// True for a file the importer can unpack (by extension): `.zip`, `.7z`, `.rar`.
pub fn is_archive(p: &Path) -> bool {
    matches!(ext_lower(p).as_deref(), Some("zip" | "7z" | "rar"))
}

fn ext_lower(p: &Path) -> Option<String> {
    p.extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
}

/// Unpack `items` (archives, see `is_archive`) into `root` on a worker thread. Ignored while another import runs.
pub fn start(job: &mut ImportJob, root: PathBuf, items: Vec<PathBuf>) {
    if job.rx.is_some() || items.is_empty() {
        return;
    }
    let (tx, rx) = channel::<Msg>();
    job.rx = Some(Mutex::new(rx));
    std::thread::spawn(move || {
        let mut out = Outcome::default();
        for item in items {
            let archive = file_name(&item);
            let mut choose = |model: &str, candidates: &[String]| ask_user(&tx, &archive, model, candidates);
            match import_item(&root, &item, &mut choose) {
                Ok(imp) => {
                    if imp.names.is_empty() && imp.refused.is_empty() {
                        out.problems.push(format!(
                            "{}: no model found (an .obj file is needed)",
                            file_name(&item)
                        ));
                    }
                    for r in &imp.refused {
                        out.problems.push(format!("{}: {r}", file_name(&item)));
                    }
                    out.copied.extend(imp.names);
                }
                Err(e) => out.problems.push(format!("{}: {e}", file_name(&item))),
            }
        }
        let _ = tx.send(Msg::Done(out));
    });
}

/// Reports a finished import in the status line and rescans the list.
pub fn poll_system(
    mut job: ResMut<ImportJob>,
    mut lib: ResMut<Library>,
    mut layout: ResMut<Layout>,
    mut redraw: EventWriter<RequestRedraw>,
) {
    let _bench = crate::benchmark::sys("sys.import.poll_system");
    if job.ask.is_some() {
        return; // the worker waits for the user's answer (importpick.rs)
    }
    if let Some(rx) = &job.rx {
        let r = rx.lock().unwrap().try_recv();
        match r {
            Ok(Msg::Ask(q)) => {
                job.ask = Some(q);
                job.asked += 1;
            }
            Ok(Msg::Done(out)) => {
                job.rx = None;
                layout.notice = Some(summary(&out));
                if !out.copied.is_empty() {
                    lib.dirty = true; // rescan: the new models show up
                    lib.select_name = out.copied.last().cloned();
                }
            }
            Err(TryRecvError::Disconnected) => {
                job.rx = None;
                layout.notice = Some("Copying failed".to_string());
            }
            Err(TryRecvError::Empty) => {
                redraw.write(RequestRedraw);
            }
        }
    }
}

fn summary(o: &Outcome) -> String {
    let mut s = match o.copied.len() {
        0 => "Nothing was copied".to_string(),
        1 => format!("Copied 1 model: {}", o.copied[0]),
        n => format!("Copied {n} models: {}", o.copied.join(", ")),
    };
    if let Some(first) = o.problems.first() {
        s += &format!("   |   {} item(s) skipped, e.g. {first}", o.problems.len());
    }
    s
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.display().to_string())
}

// ---- the work --------------------------------------------------------------------------------------

fn import_item(root: &Path, item: &Path, choose: Chooser) -> Result<Imported, String> {
    import_item_with(root, item, requires_matching_diffuse, choose)
}

fn import_item_with(root: &Path, item: &Path, rule: Rule, choose: Chooser) -> Result<Imported, String> {
    let mut _bench = crate::benchmark::work("import.item");
    _bench.detail_with(|| {
        let bytes = std::fs::metadata(item).map(|m| m.len()).unwrap_or(0);
        format!("{} bytes={bytes}", item.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
    });
    match ext_lower(item).as_deref() {
        Some("zip") => import_zip_with(root, item, rule, choose),
        Some("7z") => import_7z_with(root, item, rule, choose),
        Some("rar") => import_rar_with(root, item, rule, choose),
        _ => Err("only .zip, .7z and .rar files can be copied".to_string()),
    }
}

fn is_model_file(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    // `._name.obj` is the resource-fork stub a Mac puts into an archive next to the real file: not a model.
    !n.starts_with("._") && (n.ends_with(".obj") || n.ends_with(".obj.disabled"))
}

/// `root/name`, or `root/name_2`, `name_3`, ... when that exists already.
fn unique_dest(root: &Path, name: &str) -> PathBuf {
    let first = root.join(name);
    if !first.exists() {
        return first;
    }
    (2..1000)
        .map(|n| root.join(format!("{name}_{n}")))
        .find(|p| !p.exists())
        .unwrap_or(first)
}

// ---- .7z and .rar: one plan, our own writer ----------------------------------------------------------

/// Limits for the formats whose reader is not the `zip` crate. A model is a few files of some megabytes; these are far above
/// that and far below what a decompression bomb needs.
const MAX_ENTRIES: usize = 50_000;
/// One file, also the most that is ever held in memory for a RAR entry.
const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;
/// Everything that is written for one archive.
const MAX_TOTAL_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// A regular file of an archive: its checked relative path and its size as the archive declares it.
struct Listed {
    path: PathBuf,
    size: u64,
}

/// The archive path of an entry made safe, or `None` when it must not be unpacked: a `..` component, a drive letter or
/// another `:`, a control character or one of `< > " | ? *`, an empty name, or anything inside `__MACOSX`. Both `/` and
/// `\` separate; a leading `/` is dropped (the entry is then relative, it can no longer leave the folder it is written to).
fn safe_relative(raw: &str) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for part in raw.replace('\\', "/").split('/') {
        match part {
            "" | "." => {}
            ".." => return None,
            p if p.eq_ignore_ascii_case("__MACOSX") => return None,
            p if p
                .chars()
                .any(|c| c.is_control() || matches!(c, ':' | '<' | '>' | '"' | '|' | '?' | '*')) =>
            {
                return None
            }
            p => out.push(p),
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

/// What to write where: every wanted entry (by its checked archive path) and the files it goes to, and the new model folders.
struct Plan {
    /// More than one target when model folders are nested (the inner files belong to both).
    targets: HashMap<PathBuf, Vec<PathBuf>>,
    dests: Vec<PathBuf>,
    /// Model folders the rule turned away, or the user skipped (one line each, see `skipped`).
    refused: Vec<String>,
}

impl Plan {
    fn names(&self) -> Vec<String> {
        self.dests.iter().map(|d| file_name(d)).collect()
    }

    /// Removes the folders this import created (after a failure): they did not exist before, `plan_import` chose fresh names.
    fn remove_all(&self) {
        for d in &self.dests {
            let _ = fs::remove_dir_all(d);
        }
    }
}

/// `root/name`, or `root/name_2`, ... when that exists on disk or is already taken by another model folder of the same
/// archive (compared without regard to case: Windows).
fn unique_dest_avoiding(root: &Path, name: &str, taken: &HashSet<String>) -> PathBuf {
    let free = |p: &PathBuf| {
        !p.exists()
            && !taken.contains(&file_name(p).to_ascii_lowercase())
    };
    let first = root.join(name);
    if free(&first) {
        return first;
    }
    (2..1000)
        .map(|n| root.join(format!("{name}_{n}")))
        .find(|p| free(p))
        .unwrap_or(first)
}

/// Chooses the model folders of an archive from its file list (the same rule as the zip import: a folder holding an `.obj` or
/// `.obj.disabled`; an `.obj` at the root makes `stem` the name) and the files each one gets. Nothing is written here.
#[cfg(test)]
fn plan_import(root: &Path, stem: &str, files: &[Listed]) -> Result<Plan, String> {
    plan_import_with(root, stem, files, rule_any, &mut no_choice)
}

fn plan_import_with(
    root: &Path,
    stem: &str,
    files: &[Listed],
    rule: Rule,
    choose: Chooser,
) -> Result<Plan, String> {
    if files.len() > MAX_ENTRIES {
        return Err(format!(
            "too many files in the archive (more than {MAX_ENTRIES})"
        ));
    }
    let mut model_dirs: Vec<PathBuf> = Vec::new();
    for f in files {
        if is_model_file(&file_name(&f.path)) {
            let parent = f.path.parent().map(Path::to_path_buf).unwrap_or_default();
            if !model_dirs.contains(&parent) {
                model_dirs.push(parent);
            }
        }
    }
    // The rule: a model whose name does not match a diffuse texture of its own folder is left out, unless the user picks
    // one of the folder's other diffuse textures (`chosen`: the file name, written again as diffuse_<model>.png below).
    let mut refused: Vec<String> = Vec::new();
    let mut chosen: Vec<Option<String>> = Vec::new();
    let mut accepted_dirs: Vec<PathBuf> = Vec::new();
    for dir in model_dirs {
        let name = model_dir_name(&dir, stem);
        let inside: Vec<String> = files
            .iter()
            .filter(|f| f.path.parent() == Some(dir.as_path()))
            .map(|f| file_name(&f.path))
            .collect();
        match rule(&name, &inside) {
            Verdict::Accept => {
                accepted_dirs.push(dir);
                chosen.push(None);
            }
            Verdict::Ask(candidates) => match choose(&name, &candidates) {
                Choice::Pick(i) if i < candidates.len() => {
                    accepted_dirs.push(dir);
                    chosen.push(Some(candidates[i].clone()));
                }
                Choice::Anyway => {
                    accepted_dirs.push(dir);
                    chosen.push(None);
                }
                _ => refused.push(skipped(&name)),
            },
        }
    }
    let model_dirs = accepted_dirs;
    let mut taken: HashSet<String> = HashSet::new();
    let mut dests: Vec<PathBuf> = Vec::new();
    for dir in &model_dirs {
        let name = model_dir_name(dir, stem);
        let dest = unique_dest_avoiding(root, &name, &taken);
        taken.insert(file_name(&dest).to_ascii_lowercase());
        dests.push(dest);
    }
    let mut targets: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
    let mut total = 0u64;
    for f in files {
        for (dir, dest) in model_dirs.iter().zip(&dests) {
            let Ok(rel) = f.path.strip_prefix(dir) else {
                continue;
            };
            if rel.as_os_str().is_empty() {
                continue;
            }
            if f.size > MAX_FILE_BYTES {
                return Err(format!(
                    "{} is larger than {} MB, which is more than a model needs",
                    file_name(&f.path),
                    MAX_FILE_BYTES / (1024 * 1024)
                ));
            }
            total = total.saturating_add(f.size);
            if total > MAX_TOTAL_BYTES {
                return Err(format!(
                    "the model files add up to more than {} GB",
                    MAX_TOTAL_BYTES / (1024 * 1024 * 1024)
                ));
            }
            let target = dest.join(rel);
            let list = targets.entry(f.path.clone()).or_default();
            if !list.contains(&target) {
                list.push(target);
            }
        }
    }
    // A chosen diffuse is also written under the name the model needs: diffuse_<model folder>.png (the original stays).
    for ((dir, pick), dest) in model_dirs.iter().zip(&chosen).zip(&dests) {
        if let Some(file) = pick {
            if let Some(list) = targets.get_mut(&dir.join(file)) {
                let extra = dest.join(matching_diffuse_name(dest));
                if !list.contains(&extra) {
                    list.push(extra);
                }
            }
        }
    }
    Ok(Plan {
        targets,
        dests,
        refused,
    })
}

/// Writes one entry of an archive: to the first of its targets (read from `data`, at most the per-file limit and what is left
/// of `budget`), then copied to the others. An entry the plan does not want is read to its end and thrown away, which a 7z
/// stream needs (the next entry starts where this one ends).
fn write_entry(
    plan: &Plan,
    path: &Path,
    data: &mut dyn Read,
    budget: &mut u64,
) -> Result<(), String> {
    let Some(targets) = plan.targets.get(path) else {
        io::copy(data, &mut io::sink()).map_err(|e| format!("{}: {e}", file_name(path)))?;
        return Ok(());
    };
    let limit = MAX_FILE_BYTES.min(*budget);
    let first = &targets[0];
    if let Some(parent) = first.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut f = fs::File::create(first).map_err(|e| format!("{}: {e}", file_name(first)))?;
    // `(&mut *data).take(..)`: a reborrow of the trait object, read at most one byte past the limit to notice an overrun.
    let mut limited = (&mut *data).take(limit.saturating_add(1));
    let n = io::copy(&mut limited, &mut f).map_err(|e| format!("{}: {e}", file_name(path)))?;
    drop(f);
    if n > limit {
        return Err(format!(
            "{} holds more data than the import limit allows (a damaged archive?)",
            file_name(path)
        ));
    }
    *budget -= n;
    for other in &targets[1..] {
        if let Some(parent) = other.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        fs::copy(first, other).map_err(|e| format!("{}: {e}", file_name(other)))?;
    }
    Ok(())
}

fn stem_of(p: &Path) -> String {
    p.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "model".to_string())
}

// ---- .7z (sevenz-rust2) ----

fn seven_err(e: sevenz_rust2::Error) -> String {
    use sevenz_rust2::Error as E;
    match e {
        E::PasswordRequired | E::MaybeBadPassword(_) => {
            "password-protected archives are not supported".to_string()
        }
        E::MaxMemLimited { .. } => {
            "the archive needs more memory than allowed to unpack".to_string()
        }
        E::UnsupportedCompressionMethod(m) => format!("unsupported compression method ({m})"),
        E::BadSignature(_) => "not a 7z archive".to_string(),
        other => format!("not a readable 7z ({other})"),
    }
}

/// A regular file of a 7z archive: not a folder, not a deletion marker, not a symbolic link (7z keeps a Unix mode in the high
/// 16 bits of the attributes when bit 0x8000 is set, and Windows marks reparse points with 0x400).
fn listed_7z(e: &sevenz_rust2::ArchiveEntry) -> Option<Listed> {
    if e.is_directory() || e.is_anti_item() {
        return None;
    }
    let attrs = e.windows_attributes();
    let unix_mode = (attrs & 0x8000 != 0).then_some(attrs >> 16);
    if attrs & 0x400 != 0 || unix_mode.is_some_and(|m| m & 0o170000 == 0o120000) {
        return None;
    }
    Some(Listed {
        path: safe_relative(e.name())?,
        size: e.size(),
    })
}

#[cfg(test)]
fn import_7z(root: &Path, path: &Path) -> Result<Vec<String>, String> {
    import_7z_with(root, path, rule_any, &mut no_choice).map(|i| i.names)
}

fn import_7z_with(root: &Path, path: &Path, rule: Rule, choose: Chooser) -> Result<Imported, String> {
    use sevenz_rust2::{ArchiveReader, Password};
    let mut reader = ArchiveReader::open(path, Password::empty()).map_err(seven_err)?;
    let listed: Vec<Listed> = reader
        .archive()
        .files
        .iter()
        .filter_map(listed_7z)
        .collect();
    let plan = plan_import_with(root, &stem_of(path), &listed, rule, choose)?;
    if plan.dests.is_empty() {
        return Ok(Imported {
            names: Vec::new(),
            refused: plan.refused,
        });
    }
    let mut budget = MAX_TOTAL_BYTES;
    let mut failure: Option<String> = None;
    let done = reader.for_each_entries(|entry, data| {
        // Every entry must be read to its end, wanted or not: `write_entry` does that for the ones it throws away.
        let wanted = if entry.is_directory() {
            None
        } else {
            safe_relative(entry.name())
        };
        match wanted {
            Some(p) => {
                if let Err(e) = write_entry(&plan, &p, data, &mut budget) {
                    failure = Some(e);
                    return Ok(false);
                }
            }
            None => {
                io::copy(data, &mut io::sink())?;
            }
        }
        Ok(true)
    });
    let result = match (done, failure) {
        (_, Some(e)) => Err(e),
        (Err(e), None) => Err(seven_err(e)),
        (Ok(()), None) => Ok(()),
    };
    match result {
        Ok(()) => Ok(Imported {
            names: plan.names(),
            refused: plan.refused,
        }),
        Err(e) => {
            plan.remove_all();
            Err(e)
        }
    }
}

// ---- .rar (unrar) ----

fn rar_err(e: unrar::error::UnrarError) -> String {
    use unrar::error::Code;
    match e.code {
        Code::MissingPassword | Code::BadPassword => {
            "password-protected archives are not supported".to_string()
        }
        Code::BadArchive | Code::UnknownFormat => "not a RAR archive".to_string(),
        Code::NoMemory => "not enough memory to unpack the archive".to_string(),
        Code::EOpen => "could not open the archive (a multi-volume set needs all its parts)".to_string(),
        _ => format!("not a readable RAR ({e})"),
    }
}

#[cfg(test)]
fn import_rar(root: &Path, path: &Path) -> Result<Vec<String>, String> {
    import_rar_with(root, path, rule_any, &mut no_choice).map(|i| i.names)
}

fn import_rar_with(root: &Path, path: &Path, rule: Rule, choose: Chooser) -> Result<Imported, String> {
    use unrar::Archive;
    // Pass 1: the headers only (no data is unpacked).
    let mut listed: Vec<Listed> = Vec::new();
    for entry in Archive::new(path).open_for_listing().map_err(rar_err)? {
        let e = entry.map_err(rar_err)?;
        if e.is_encrypted() {
            return Err("password-protected archives are not supported".to_string());
        }
        if e.is_split() {
            return Err("multi-volume RAR sets are not supported (join them or use a .zip / .7z)".to_string());
        }
        if e.is_directory() {
            continue;
        }
        if let Some(p) = safe_relative(&e.filename.to_string_lossy()) {
            listed.push(Listed {
                path: p,
                size: e.unpacked_size,
            });
        }
    }
    let plan = plan_import_with(root, &stem_of(path), &listed, rule, choose)?;
    if plan.dests.is_empty() {
        return Ok(Imported {
            names: Vec::new(),
            refused: plan.refused,
        });
    }
    // Pass 2: walk the archive in order, unpack the wanted entries into memory (at most MAX_FILE_BYTES each, checked against the
    // header first) and write them ourselves: UnRAR never touches the disk, so it cannot create a link or leave the folder.
    let run = || -> Result<(), String> {
        let mut budget = MAX_TOTAL_BYTES;
        let mut archive = Archive::new(path).open_for_processing().map_err(rar_err)?;
        while let Some(cursor) = archive.read_header().map_err(rar_err)? {
            let (wanted, size) = {
                let e = cursor.entry();
                let wanted = if e.is_directory() {
                    None
                } else {
                    safe_relative(&e.filename.to_string_lossy())
                        .filter(|p| plan.targets.contains_key(p))
                };
                (wanted, e.unpacked_size)
            };
            archive = match wanted {
                Some(p) => {
                    if size > MAX_FILE_BYTES {
                        return Err(format!(
                            "{} is larger than {} MB, which is more than a model needs",
                            file_name(&p),
                            MAX_FILE_BYTES / (1024 * 1024)
                        ));
                    }
                    let (data, next) = cursor.read().map_err(rar_err)?;
                    let mut bytes: &[u8] = &data;
                    write_entry(&plan, &p, &mut bytes, &mut budget)?;
                    next
                }
                None => cursor.skip().map_err(rar_err)?,
            };
        }
        Ok(())
    };
    match run() {
        Ok(()) => Ok(Imported {
            names: plan.names(),
            refused: plan.refused,
        }),
        Err(e) => {
            plan.remove_all();
            Err(e)
        }
    }
}

#[cfg(test)]
fn import_zip(root: &Path, zip_path: &Path) -> Result<Vec<String>, String> {
    import_zip_with(root, zip_path, rule_any, &mut no_choice).map(|i| i.names)
}

fn import_zip_with(root: &Path, zip_path: &Path, rule: Rule, choose: Chooser) -> Result<Imported, String> {
    let file = fs::File::open(zip_path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(BufReader::new(file))
        .map_err(|e| format!("not a readable zip ({e})"))?;
    // Folders (relative to the zip root) that hold an .obj, and every file path (for the files directly in a folder).
    let mut model_dirs: Vec<PathBuf> = Vec::new();
    let mut all_files: Vec<PathBuf> = Vec::new();
    for i in 0..archive.len() {
        let Ok(entry) = archive.by_index(i) else {
            continue;
        };
        let Some(path) = entry.enclosed_name() else {
            continue;
        };
        if !entry.is_file() {
            continue;
        }
        if is_model_file(&file_name(&path)) {
            let parent = path.parent().map(Path::to_path_buf).unwrap_or_default();
            if !model_dirs.contains(&parent) {
                model_dirs.push(parent);
            }
        }
        all_files.push(path);
    }
    let stem = zip_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "model".to_string());
    let mut names = Vec::new();
    let mut refused = Vec::new();
    for dir in model_dirs {
        let name = model_dir_name(&dir, &stem);
        // The rule: the name must match a diffuse texture of the model's own folder.
        let inside: Vec<String> = all_files
            .iter()
            .filter(|p| p.parent() == Some(dir.as_path()))
            .map(|p| file_name(p))
            .collect();
        let picked: Option<String> = match rule(&name, &inside) {
            Verdict::Accept => None,
            Verdict::Ask(candidates) => match choose(&name, &candidates) {
                Choice::Pick(i) if i < candidates.len() => Some(candidates[i].clone()),
                Choice::Anyway => None,
                _ => {
                    refused.push(skipped(&name));
                    continue;
                }
            },
        };
        let dest = unique_dest(root, &name);
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
            let Some(path) = entry.enclosed_name() else {
                continue;
            };
            let Ok(rel) = path.strip_prefix(&dir) else {
                continue;
            };
            if !entry.is_file() || rel.as_os_str().is_empty() {
                continue;
            }
            let out = dest.join(rel);
            if let Some(parent) = out.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let mut f = fs::File::create(&out).map_err(|e| e.to_string())?;
            io::copy(&mut entry, &mut f).map_err(|e| e.to_string())?;
            drop(f);
            // The chosen diffuse is also written as diffuse_<model folder>.png (the original stays).
            if picked.as_deref().is_some_and(|c| rel == Path::new(c)) {
                fs::copy(&out, dest.join(matching_diffuse_name(&dest))).map_err(|e| e.to_string())?;
            }
        }
        names.push(file_name(&dest));
    }
    Ok(Imported { names, refused })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let p =
            std::env::temp_dir().join(format!("votv-import-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn a_zip_with_a_model_folder_or_a_root_level_model_is_unpacked() {
        use std::io::Write;
        let dir = temp("zip");
        let root = temp("zroot");
        let make = |name: &str, files: &[&str]| {
            let path = dir.join(name);
            let mut w = zip::ZipWriter::new(fs::File::create(&path).unwrap());
            let opt = zip::write::SimpleFileOptions::default();
            for f in files {
                w.start_file(*f, opt).unwrap();
                w.write_all(b"x").unwrap();
            }
            w.finish().unwrap();
            path
        };
        let nested = make(
            "pack.zip",
            &["lamp/lamp.obj", "lamp/lamp.mtl", "lamp/tex/d.png"],
        );
        let flat = make("flatmodel.zip", &["m.obj", "m.mtl"]);
        assert_eq!(
            import_zip(&root, &nested).unwrap(),
            vec!["lamp".to_string()]
        );
        assert!(root.join("lamp").join("tex").join("d.png").is_file());
        assert_eq!(
            import_zip(&root, &flat).unwrap(),
            vec!["flatmodel".to_string()]
        );
        assert!(root.join("flatmodel").join("m.obj").is_file());
        for d in [&dir, &root] {
            let _ = fs::remove_dir_all(d);
        }
    }

    // ---- .7z / .rar ----

    /// Made by 7-Zip 23.01 (`7z a -snl -mx=1 pack.7z pack`): `pack/chair/{chair.obj, chair.mtl, tex/c.png, chair_link.obj}`,
    /// `pack/lamp/lamp.obj.disabled`, `pack/readme.txt`, `pack/src/chair.blend` (300 bytes). `chair_link.obj` is a SYMBOLIC LINK
    /// to /etc/hostname: it must not become a file.
    const PACK_7Z: &[u8] = &[
        0x37, 0x7a, 0xbc, 0xaf, 0x27, 0x1c, 0x00, 0x04, 0x2d, 0x8b, 0x43, 0x48, 0x7b, 0x01, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x22, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xf9, 0x35, 0xac, 0x29,
        0xe0, 0x01, 0xae, 0x00, 0x76, 0x5d, 0x00, 0x37, 0x19, 0x4b, 0x55, 0x8a, 0xfe, 0x09, 0xf5, 0x3d,
        0x4c, 0x00, 0x17, 0x1b, 0x97, 0x68, 0x59, 0xe1, 0x99, 0x66, 0x54, 0xea, 0x90, 0x0b, 0x26, 0x19,
        0xa3, 0x2c, 0x22, 0x87, 0x90, 0x0f, 0xac, 0xc5, 0xff, 0xee, 0x50, 0xac, 0x58, 0xfe, 0x76, 0x5c,
        0xee, 0x9d, 0x2c, 0xff, 0x52, 0x58, 0x3e, 0xc4, 0xd0, 0x1d, 0xbb, 0x6b, 0x4f, 0xb7, 0x0b, 0xfe,
        0xa5, 0x86, 0x91, 0xcb, 0x14, 0x78, 0x4c, 0x83, 0x57, 0x0f, 0x99, 0xa9, 0x0d, 0x97, 0xe9, 0x62,
        0xd5, 0x1b, 0x4f, 0xf9, 0xe2, 0xa3, 0x17, 0x3f, 0xf2, 0xbf, 0xa0, 0x38, 0x4a, 0x0f, 0xed, 0xbb,
        0xc9, 0x42, 0x38, 0xd5, 0x19, 0xf0, 0x64, 0xa3, 0x22, 0xa1, 0xf4, 0x35, 0xa0, 0xe7, 0xc0, 0x66,
        0x3c, 0xf4, 0xb7, 0x8f, 0x52, 0x4d, 0x8b, 0xe1, 0x41, 0x10, 0x13, 0x08, 0x00, 0x00, 0x00, 0x00,
        0x81, 0x33, 0x07, 0xae, 0x0f, 0xd5, 0x13, 0x02, 0x51, 0x42, 0x15, 0xf6, 0xb3, 0x45, 0x7d, 0x69,
        0x4d, 0x8f, 0x3a, 0xba, 0x25, 0x66, 0xb1, 0x05, 0xad, 0x71, 0xfb, 0x6f, 0x06, 0x62, 0x39, 0x86,
        0x20, 0x12, 0x7e, 0xee, 0xc0, 0xcd, 0x32, 0x1e, 0xa8, 0x23, 0xa9, 0xbf, 0xb6, 0xd8, 0xfc, 0x95,
        0xb2, 0x62, 0xf3, 0xb5, 0x2f, 0xbd, 0x6d, 0xf5, 0x6c, 0xdb, 0xda, 0x85, 0x49, 0xf3, 0x77, 0x2d,
        0x32, 0x61, 0xd3, 0xc2, 0x3e, 0xa5, 0x6e, 0x3c, 0x3b, 0xf0, 0x51, 0x8a, 0x8f, 0x9d, 0xa4, 0x71,
        0x5d, 0xc3, 0xa4, 0x84, 0x9b, 0xb7, 0xc4, 0x85, 0xa1, 0x5f, 0x55, 0x30, 0x02, 0xa0, 0x75, 0xd4,
        0xe5, 0x77, 0xf1, 0x8a, 0xf3, 0x45, 0xfd, 0x3a, 0xc3, 0x04, 0xae, 0x43, 0x17, 0xa5, 0x16, 0xda,
        0x0a, 0x85, 0xe2, 0x97, 0x86, 0x0e, 0xc9, 0x81, 0x2d, 0x74, 0xc4, 0x3a, 0xb2, 0xc6, 0x28, 0x62,
        0x24, 0xfd, 0x36, 0x7f, 0xe2, 0x69, 0xe5, 0x55, 0x7f, 0x19, 0xf1, 0x76, 0xc1, 0x0c, 0x4a, 0x60,
        0x6f, 0xcc, 0x0b, 0xfd, 0x95, 0x14, 0xa6, 0xae, 0x7c, 0x32, 0x83, 0x52, 0xe3, 0x42, 0x97, 0xa6,
        0xd8, 0xd3, 0xe5, 0x46, 0x32, 0xfb, 0xae, 0x49, 0x86, 0x31, 0xed, 0xef, 0x0f, 0x2e, 0x52, 0xc5,
        0x3e, 0x6d, 0xbd, 0x83, 0xf2, 0x17, 0x80, 0x1e, 0xa5, 0x39, 0xb7, 0x60, 0x48, 0x73, 0xc0, 0xb1,
        0x80, 0x60, 0xbd, 0x3c, 0xf1, 0xd5, 0xa2, 0x87, 0xfc, 0x97, 0xb7, 0xed, 0x1b, 0x78, 0xac, 0xc8,
        0xe4, 0x90, 0x13, 0x82, 0xf8, 0x8d, 0x4c, 0xad, 0x57, 0x57, 0x5a, 0x5a, 0x34, 0xd0, 0xfa, 0x0d,
        0x2d, 0xc1, 0xda, 0x23, 0x1a, 0x3d, 0x65, 0xf8, 0xa6, 0x8e, 0x18, 0x9a, 0x3d, 0xc0, 0x32, 0x23,
        0xb1, 0xd9, 0x06, 0x18, 0xb6, 0xa1, 0x85, 0x91, 0x00, 0x00, 0x00, 0x17, 0x06, 0x7e, 0x01, 0x09,
        0x80, 0xfd, 0x00, 0x07, 0x0b, 0x01, 0x00, 0x01, 0x23, 0x03, 0x01, 0x01, 0x05, 0x5d, 0x00, 0x10,
        0x00, 0x00, 0x0c, 0x82, 0x86, 0x0a, 0x01, 0x39, 0xd6, 0xce, 0xd0, 0x00, 0x00,
    ];

    /// A stored RAR5 archive with the same files except the link (built by hand, checked with 7-Zip 23.01: `Type = Rar5`,
    /// every CRC right, the files extract identically).
    const PACK_RAR: &[u8] = &[
        0x52, 0x61, 0x72, 0x21, 0x1a, 0x07, 0x01, 0x00, 0xc5, 0x1a, 0x33, 0x32, 0x03, 0x01, 0x00, 0x00,
        0x89, 0xc7, 0x3b, 0x1f, 0x21, 0x02, 0x02, 0x28, 0x04, 0x28, 0x20, 0xb4, 0x0c, 0x45, 0x16, 0x00,
        0x01, 0x14, 0x70, 0x61, 0x63, 0x6b, 0x2f, 0x63, 0x68, 0x61, 0x69, 0x72, 0x2f, 0x63, 0x68, 0x61,
        0x69, 0x72, 0x2e, 0x6f, 0x62, 0x6a, 0x6f, 0x20, 0x63, 0x68, 0x61, 0x69, 0x72, 0x0a, 0x76, 0x20,
        0x30, 0x20, 0x30, 0x20, 0x30, 0x0a, 0x76, 0x20, 0x31, 0x20, 0x30, 0x20, 0x30, 0x0a, 0x76, 0x20,
        0x30, 0x20, 0x31, 0x20, 0x30, 0x0a, 0x66, 0x20, 0x31, 0x20, 0x32, 0x20, 0x33, 0x0a, 0xe7, 0x68,
        0xcb, 0x72, 0x21, 0x02, 0x02, 0x12, 0x04, 0x12, 0x20, 0xc7, 0x8d, 0x09, 0x4f, 0x00, 0x01, 0x14,
        0x70, 0x61, 0x63, 0x6b, 0x2f, 0x63, 0x68, 0x61, 0x69, 0x72, 0x2f, 0x63, 0x68, 0x61, 0x69, 0x72,
        0x2e, 0x6d, 0x74, 0x6c, 0x6e, 0x65, 0x77, 0x6d, 0x74, 0x6c, 0x20, 0x6d, 0x0a, 0x4b, 0x64, 0x20,
        0x31, 0x20, 0x30, 0x20, 0x30, 0x0a, 0x44, 0xfd, 0x27, 0x1b, 0x21, 0x02, 0x02, 0x25, 0x04, 0x25,
        0x20, 0x62, 0x4a, 0xe0, 0xd1, 0x00, 0x01, 0x14, 0x70, 0x61, 0x63, 0x6b, 0x2f, 0x63, 0x68, 0x61,
        0x69, 0x72, 0x2f, 0x74, 0x65, 0x78, 0x2f, 0x63, 0x2e, 0x70, 0x6e, 0x67, 0x5c, 0x78, 0x38, 0x39,
        0x50, 0x4e, 0x47, 0x20, 0x66, 0x61, 0x6b, 0x65, 0x20, 0x74, 0x65, 0x78, 0x74, 0x75, 0x72, 0x65,
        0x20, 0x62, 0x79, 0x74, 0x65, 0x73, 0x20, 0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38,
        0x39, 0x92, 0x0a, 0xab, 0x97, 0x28, 0x02, 0x02, 0x0f, 0x04, 0x0f, 0x20, 0x34, 0x82, 0x54, 0x44,
        0x00, 0x01, 0x1b, 0x70, 0x61, 0x63, 0x6b, 0x2f, 0x6c, 0x61, 0x6d, 0x70, 0x2f, 0x6c, 0x61, 0x6d,
        0x70, 0x2e, 0x6f, 0x62, 0x6a, 0x2e, 0x64, 0x69, 0x73, 0x61, 0x62, 0x6c, 0x65, 0x64, 0x6f, 0x20,
        0x6c, 0x61, 0x6d, 0x70, 0x0a, 0x76, 0x20, 0x30, 0x20, 0x30, 0x20, 0x30, 0x0a, 0x30, 0x24, 0x1f,
        0x37, 0x1c, 0x02, 0x02, 0x08, 0x04, 0x08, 0x20, 0x05, 0xa6, 0xa1, 0xe5, 0x00, 0x01, 0x0f, 0x70,
        0x61, 0x63, 0x6b, 0x2f, 0x72, 0x65, 0x61, 0x64, 0x6d, 0x65, 0x2e, 0x74, 0x78, 0x74, 0x72, 0x65,
        0x61, 0x64, 0x20, 0x6d, 0x65, 0x0a, 0xfc, 0xa3, 0x67, 0x10, 0x23, 0x02, 0x02, 0xac, 0x02, 0x04,
        0xac, 0x02, 0x20, 0xcc, 0x0a, 0xaf, 0x04, 0x00, 0x01, 0x14, 0x70, 0x61, 0x63, 0x6b, 0x2f, 0x73,
        0x72, 0x63, 0x2f, 0x63, 0x68, 0x61, 0x69, 0x72, 0x2e, 0x62, 0x6c, 0x65, 0x6e, 0x64, 0x42, 0x42,
        0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42,
        0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42,
        0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42,
        0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42,
        0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42,
        0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42,
        0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42,
        0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42,
        0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42,
        0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42,
        0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42,
        0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42,
        0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42,
        0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42,
        0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42,
        0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42,
        0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42,
        0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42,
        0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x19, 0xb2, 0x3a, 0x35, 0x03, 0x05,
        0x00, 0x00,
    ];

    fn fixture(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let p = dir.join(name);
        fs::write(&p, bytes).unwrap();
        p
    }

    /// What both fixtures must give: two model folders with their files, nothing from outside them.
    fn check_pack(root: &Path, mut names: Vec<String>) {
        names.sort();
        assert_eq!(names, ["chair", "lamp"]);
        let obj = fs::read_to_string(root.join("chair").join("chair.obj")).unwrap();
        assert!(obj.starts_with("o chair"), "{obj}");
        assert_eq!(
            fs::read(root.join("chair").join("tex").join("c.png")).unwrap().len(),
            37
        );
        assert!(root.join("chair").join("chair.mtl").is_file());
        assert!(root.join("lamp").join("lamp.obj.disabled").is_file());
        assert!(!root.join("readme.txt").exists(), "files outside the model folders are left out");
        assert!(!root.join("src").exists() && !root.join("chair").join("src").exists());
    }

    #[test]
    fn a_7z_with_model_folders_is_unpacked_and_a_link_is_not() {
        let dir = temp("7z");
        let root = dir.join("printer");
        fs::create_dir_all(&root).unwrap();
        let a = fixture(&dir, "pack.7z", PACK_7Z);
        let names = import_7z(&root, &a).unwrap();
        check_pack(&root, names);
        assert!(
            !root.join("chair").join("chair_link.obj").exists(),
            "a symbolic link in the archive must not be written"
        );
        // a second import never overwrites: the copies are numbered
        let mut again = import_7z(&root, &a).unwrap();
        again.sort();
        assert_eq!(again, ["chair_2", "lamp_2"]);
    }

    #[test]
    fn a_rar_with_model_folders_is_unpacked() {
        let dir = temp("rar");
        let root = dir.join("printer");
        fs::create_dir_all(&root).unwrap();
        let a = fixture(&dir, "pack.rar", PACK_RAR);
        let names = import_rar(&root, &a).unwrap();
        check_pack(&root, names);
        let mut again = import_rar(&root, &a).unwrap();
        again.sort();
        assert_eq!(again, ["chair_2", "lamp_2"]);
    }

    #[test]
    fn import_item_picks_the_reader_by_extension() {
        let dir = temp("dispatch");
        let root = dir.join("printer");
        fs::create_dir_all(&root).unwrap();
        let a = fixture(&dir, "PACK.7Z", PACK_7Z);
        let r = fixture(&dir, "pack.RAR", PACK_RAR);
        assert_eq!(import_item_with(&root, &a, rule_any, &mut no_choice).unwrap().names.len(), 2);
        assert_eq!(import_item_with(&root, &r, rule_any, &mut no_choice).unwrap().names.len(), 2);
        let other = fixture(&dir, "pack.tar", b"x");
        assert!(import_item_with(&root, &other, rule_any, &mut no_choice).is_err());
    }

    #[test]
    fn a_damaged_or_wrong_archive_is_an_error_and_leaves_nothing() {
        let dir = temp("damaged");
        let root = dir.join("printer");
        fs::create_dir_all(&root).unwrap();
        let junk7 = fixture(&dir, "junk.7z", b"this is not a 7z archive at all");
        let junkr = fixture(&dir, "junk.rar", b"this is not a rar archive at all");
        assert!(import_7z(&root, &junk7).is_err());
        assert!(import_rar(&root, &junkr).is_err());
        // a real 7z cut in half
        let cut = fixture(&dir, "cut.7z", &PACK_7Z[..PACK_7Z.len() / 2]);
        assert!(import_7z(&root, &cut).is_err());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0, "nothing is left behind");
        // a missing file
        assert!(import_7z(&root, &dir.join("missing.7z")).is_err());
        assert!(import_rar(&root, &dir.join("missing.rar")).is_err());
    }

    // ---- the shared plan and writer ----

    fn listed(names: &[&str]) -> Vec<Listed> {
        names
            .iter()
            .map(|n| Listed {
                path: PathBuf::from(n),
                size: 10,
            })
            .collect()
    }

    #[test]
    fn archive_extensions_are_recognised_in_any_case() {
        for ok in ["a.zip", "b.7z", "c.RAR", "d.Zip", "e.7Z"] {
            assert!(is_archive(Path::new(ok)), "{ok}");
        }
        for no in ["a.tar", "b.glb", "c", "d.zip.txt", "e.gz"] {
            assert!(!is_archive(Path::new(no)), "{no}");
        }
    }

    #[test]
    fn unsafe_entry_paths_are_refused() {
        for bad in [
            "../x.obj",
            "a/../../x.obj",
            "..\\x.obj",
            "C:\\x.obj",
            "c:/x.obj",
            "a/b:stream",
            "a/b*.obj",
            "a/b?.obj",
            "a/\u{1}b.obj",
            "__MACOSX/m/m.obj",
            "m/__macosx/m.obj",
            "",
            "/",
            "./",
        ] {
            assert_eq!(safe_relative(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn safe_entry_paths_are_normalised() {
        assert_eq!(safe_relative("a/b/c.obj"), Some(PathBuf::from("a/b/c.obj")));
        assert_eq!(safe_relative("a\\b\\c.obj"), Some(PathBuf::from("a/b/c.obj")));
        assert_eq!(safe_relative("/abs/m.obj"), Some(PathBuf::from("abs/m.obj")));
        assert_eq!(safe_relative("./m/./m.obj"), Some(PathBuf::from("m/m.obj")));
        assert_eq!(safe_relative("m//m.obj"), Some(PathBuf::from("m/m.obj")));
    }

    #[test]
    fn mac_resource_fork_stubs_are_not_models() {
        assert!(is_model_file("chair.obj"));
        assert!(is_model_file("CHAIR.OBJ.DISABLED"));
        assert!(!is_model_file("._chair.obj"));
        assert!(!is_model_file("chair.obj.bak"));
    }

    #[test]
    fn a_plan_picks_the_model_folders_and_only_their_files() {
        let root = temp("plan-folders");
        let files = listed(&[
            "pack/chair/chair.obj",
            "pack/chair/chair.mtl",
            "pack/chair/tex/c.png",
            "pack/readme.txt",
            "pack/src/chair.blend",
        ]);
        let plan = plan_import(&root, "pack", &files).unwrap();
        assert_eq!(plan.names(), ["chair"]);
        assert!(plan.targets.contains_key(Path::new("pack/chair/tex/c.png")));
        assert!(!plan.targets.contains_key(Path::new("pack/readme.txt")));
        assert!(!plan.targets.contains_key(Path::new("pack/src/chair.blend")));
        assert_eq!(
            plan.targets[Path::new("pack/chair/chair.obj")],
            vec![root.join("chair").join("chair.obj")]
        );
    }

    #[test]
    fn a_model_at_the_root_of_the_archive_is_named_after_it() {
        let root = temp("plan-root");
        let plan = plan_import(&root, "flat", &listed(&["m.obj", "m.mtl", "tex/t.png"])).unwrap();
        assert_eq!(plan.names(), ["flat"]);
        assert_eq!(
            plan.targets[Path::new("tex/t.png")],
            vec![root.join("flat").join("tex").join("t.png")]
        );
    }

    #[test]
    fn existing_and_repeated_names_get_a_number() {
        let root = temp("plan-names");
        fs::create_dir_all(root.join("chair")).unwrap();
        let plan = plan_import(
            &root,
            "p",
            &listed(&["a/chair/c.obj", "b/chair/c.obj"]),
        )
        .unwrap();
        assert_eq!(plan.names(), ["chair_2", "chair_3"]);
    }

    #[test]
    fn nested_model_folders_share_the_inner_files() {
        let root = temp("plan-nested");
        let plan = plan_import(
            &root,
            "p",
            &listed(&["m/m.obj", "m/sub/n.obj", "m/sub/n.png"]),
        )
        .unwrap();
        assert_eq!(plan.names(), ["m", "sub"]);
        assert_eq!(plan.targets[Path::new("m/sub/n.png")].len(), 2);
        assert_eq!(plan.targets[Path::new("m/m.obj")].len(), 1);
    }

    #[test]
    fn an_archive_without_a_model_gives_an_empty_plan() {
        let root = temp("plan-none");
        let plan = plan_import(&root, "p", &listed(&["a/readme.txt", "a/pic.png"])).unwrap();
        assert!(plan.dests.is_empty() && plan.targets.is_empty());
    }

    #[test]
    fn the_limits_are_enforced() {
        let root = temp("plan-limits");
        let many: Vec<Listed> = (0..=MAX_ENTRIES)
            .map(|i| Listed {
                path: PathBuf::from(format!("m/f{i}.txt")),
                size: 1,
            })
            .collect();
        assert!(plan_import(&root, "p", &many).is_err(), "too many entries");
        let big = vec![
            Listed {
                path: PathBuf::from("m/m.obj"),
                size: MAX_FILE_BYTES + 1,
            },
        ];
        assert!(plan_import(&root, "p", &big).is_err(), "one file too large");
        let sum: Vec<Listed> = (0..17)
            .map(|i| Listed {
                path: PathBuf::from(format!("m/f{i}.obj")),
                size: MAX_FILE_BYTES,
            })
            .collect();
        assert!(plan_import(&root, "p", &sum).is_err(), "too much in total");
    }

    #[test]
    fn write_entry_writes_every_target_and_drains_unwanted_entries() {
        let root = temp("write-entry");
        let mut targets = HashMap::new();
        targets.insert(
            PathBuf::from("m/a.png"),
            vec![
                root.join("x").join("a.png"),
                root.join("y").join("sub").join("a.png"),
            ],
        );
        let plan = Plan {
            targets,
            dests: Vec::new(),
            refused: Vec::new(),
        };
        let mut budget = 100u64;
        let mut data = io::Cursor::new(vec![7u8; 10]);
        write_entry(&plan, Path::new("m/a.png"), &mut data, &mut budget).unwrap();
        assert_eq!(fs::read(root.join("x").join("a.png")).unwrap(), vec![7u8; 10]);
        assert_eq!(
            fs::read(root.join("y").join("sub").join("a.png")).unwrap(),
            vec![7u8; 10]
        );
        assert_eq!(budget, 90);
        // an entry the plan does not want is read to its end (a 7z stream needs that) and writes nothing
        let mut other = io::Cursor::new(vec![1u8; 10]);
        write_entry(&plan, Path::new("m/other.txt"), &mut other, &mut budget).unwrap();
        assert_eq!(other.position(), 10);
        assert!(!root.join("m").exists());
        // more data than the budget allows is an error
        let mut big = io::Cursor::new(vec![1u8; 10]);
        let mut small = 5u64;
        assert!(write_entry(&plan, Path::new("m/a.png"), &mut big, &mut small).is_err());
    }

    // ---- the diffuse-name rule ----

    fn files(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_name_must_match_a_diffuse_png_in_the_folder_in_any_letter_case() {
        let ok = |n: &str, f: &[&str]| requires_matching_diffuse(n, &files(f)) == Verdict::Accept;
        assert!(ok("lamp", &["lamp.obj", "diffuse_lamp.png"]));
        assert!(ok("Lamp", &["DIFFUSE_LAMP.PNG"]));
        assert!(ok("lamp", &["Diffuse_Lamp.png"]));
        assert!(!ok("lamp", &["diffuse_lamp.jpg"]), "only png");
        assert!(!ok("lamp", &["lamp.obj"]), "no texture at all");
        assert!(!ok("lamp", &["diffuse_lamp_2.png"]), "the whole name must match");
    }

    #[test]
    fn other_diffuse_pngs_are_offered_and_the_user_is_asked_even_without_any() {
        // sorted by plain string order: the upper case D of Diffuse_chair.png comes first
        assert_eq!(
            requires_matching_diffuse("lamp", &files(&["lamp.obj", "diffuse_bulb.png", "Diffuse_chair.png", "pbr_lamp.png"])),
            Verdict::Ask(vec!["Diffuse_chair.png".to_string(), "diffuse_bulb.png".to_string()])
        );
        let none = Verdict::Ask(Vec::new());
        assert_eq!(requires_matching_diffuse("lamp", &files(&["lamp.obj", "pbr_lamp.png", "normal_lamp.png"])), none);
        assert_eq!(requires_matching_diffuse("lamp", &files(&["lamp.obj"])), none);
        assert_eq!(requires_matching_diffuse("lamp", &files(&["diffuse_lamp.jpg"])), none);
    }

    fn make_zip(dir: &Path, name: &str, files: &[&str]) -> PathBuf {
        use std::io::Write;
        let path = dir.join(name);
        let mut w = zip::ZipWriter::new(fs::File::create(&path).unwrap());
        let opt = zip::write::SimpleFileOptions::default();
        for f in files {
            w.start_file(*f, opt).unwrap();
            w.write_all(f.as_bytes()).unwrap(); // the content is the entry's own name: tells the copies apart
        }
        w.finish().unwrap();
        path
    }

    #[test]
    fn a_zip_imports_the_matching_models_and_refuses_the_others() {
        let dir = temp("rule-zip");
        let root = temp("rule-zip-root");
        let z = make_zip(
            &dir,
            "pack.zip",
            &["lamp/lamp.obj", "lamp/Diffuse_Lamp.png", "desk/desk.obj", "bare/bare.obj"],
        );
        let r = import_zip_with(&root, &z, requires_matching_diffuse, &mut no_choice).unwrap();
        assert_eq!(r.names, ["lamp"]);
        assert_eq!(r.refused.len(), 2, "{:?}", r.refused);
        assert!(r.refused.iter().any(|m| m.contains("\"desk\"")));
        assert!(r.refused.iter().any(|m| m.contains("\"bare\"")));
        assert!(!root.join("desk").exists() && !root.join("bare").exists(), "a refused model writes nothing");
    }

    #[test]
    fn a_model_at_the_root_of_a_zip_must_match_the_zip_name() {
        let dir = temp("rule-zip-flat");
        let root = temp("rule-zip-flat-root");
        let good = make_zip(&dir, "flat.zip", &["m.obj", "diffuse_flat.png"]);
        let bad = make_zip(&dir, "other.zip", &["m.obj", "pbr_flat.png"]);
        assert_eq!(import_zip_with(&root, &good, requires_matching_diffuse, &mut no_choice).unwrap().names, ["flat"]);
        let r = import_zip_with(&root, &bad, requires_matching_diffuse, &mut no_choice).unwrap();
        assert!(r.names.is_empty() && r.refused.len() == 1, "{:?}", r.refused);
        assert!(!root.join("other").exists());
    }

    #[test]
    fn a_zip_model_with_other_diffuse_textures_uses_the_one_the_user_picks() {
        let dir = temp("pick-zip");
        let root = temp("pick-zip-root");
        let z = make_zip(
            &dir,
            "pack.zip",
            &["desk/desk.obj", "desk/diffuse_chair.png", "desk/diffuse_stool.png", "desk/pbr_desk.png"],
        );
        let mut asked: Vec<(String, Vec<String>)> = Vec::new();
        // candidates come sorted: diffuse_chair.png, diffuse_stool.png; pick the second
        let mut choose = |m: &str, c: &[String]| {
            asked.push((m.to_string(), c.to_vec()));
            Choice::Pick(1)
        };
        let r = import_zip_with(&root, &z, requires_matching_diffuse, &mut choose).unwrap();
        assert_eq!(r.names, ["desk"]);
        assert!(r.refused.is_empty());
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].0, "desk");
        assert_eq!(asked[0].1, ["diffuse_chair.png", "diffuse_stool.png"]);
        // the picked file is there under the name the model needs, with the picked file's content; the originals stay
        assert_eq!(fs::read(root.join("desk").join("diffuse_desk.png")).unwrap(), b"desk/diffuse_stool.png");
        assert!(root.join("desk").join("diffuse_stool.png").is_file() && root.join("desk").join("diffuse_chair.png").is_file());
    }

    #[test]
    fn skipping_the_question_refuses_the_model_and_a_name_clash_names_the_copy_after_the_new_folder() {
        let dir = temp("skip-zip");
        let root = temp("skip-zip-root");
        let z = make_zip(&dir, "pack.zip", &["desk/desk.obj", "desk/diffuse_chair.png"]);
        let r = import_zip_with(&root, &z, requires_matching_diffuse, &mut no_choice).unwrap();
        assert!(r.names.is_empty() && r.refused.len() == 1 && r.refused[0].contains("skipped"), "{:?}", r.refused);
        assert!(!root.join("desk").exists());
        // `desk` exists already: the import becomes desk_2 and the picked texture is diffuse_desk_2.png
        fs::create_dir_all(root.join("desk")).unwrap();
        let r = import_zip_with(&root, &z, requires_matching_diffuse, &mut |_: &str, _: &[String]| Choice::Pick(0)).unwrap();
        assert_eq!(r.names, ["desk_2"]);
        assert!(root.join("desk_2").join("diffuse_desk_2.png").is_file());
    }

    #[test]
    fn a_picked_diffuse_in_a_7z_or_rar_plan_gets_a_second_target() {
        let root = temp("pick-plan");
        let plan = plan_import_with(
            &root,
            "p",
            &listed(&["m/m.obj", "m/diffuse_x.png", "m/diffuse_y.png"]),
            requires_matching_diffuse,
            &mut |_: &str, c: &[String]| {
                assert_eq!(c, ["diffuse_x.png", "diffuse_y.png"]);
                Choice::Pick(1)
            },
        )
        .unwrap();
        assert_eq!(plan.names(), ["m"]);
        assert_eq!(
            plan.targets[Path::new("m/diffuse_y.png")],
            vec![root.join("m").join("diffuse_y.png"), root.join("m").join("diffuse_m.png")]
        );
        assert_eq!(plan.targets[Path::new("m/diffuse_x.png")].len(), 1);
        // skipped: nothing planned
        let plan = plan_import_with(
            &root,
            "p",
            &listed(&["m/m.obj", "m/diffuse_x.png"]),
            requires_matching_diffuse,
            &mut no_choice,
        )
        .unwrap();
        assert!(plan.dests.is_empty() && plan.refused.len() == 1);
    }

    #[test]
    fn only_files_directly_in_the_model_folder_count() {
        let root = temp("rule-nested");
        // the diffuse is in a sub folder of the model folder: it is not the model's own texture
        let plan = plan_import_with(
            &root,
            "p",
            &listed(&["m/m.obj", "m/tex/diffuse_m.png"]),
            requires_matching_diffuse,
            &mut no_choice,
        )
        .unwrap();
        assert!(plan.dests.is_empty());
        assert_eq!(plan.refused.len(), 1);
        let plan = plan_import_with(
            &root,
            "p",
            &listed(&["m/m.obj", "m/diffuse_m.png", "m/tex/t.png"]),
            requires_matching_diffuse,
            &mut no_choice,
        )
        .unwrap();
        assert_eq!(plan.names(), ["m"]);
        assert!(plan.refused.is_empty());
    }

    #[test]
    fn the_7z_and_rar_readers_apply_the_same_rule() {
        // the real 7z / rar fixtures hold chair/chair.obj with only tex/c.png: no diffuse at all, so both are refused
        let dir = temp("rule-7z");
        let root = dir.join("printer");
        fs::create_dir_all(&root).unwrap();
        let a = fixture(&dir, "pack.7z", PACK_7Z);
        let r = import_7z_with(&root, &a, requires_matching_diffuse, &mut no_choice).unwrap();
        assert!(r.names.is_empty());
        assert_eq!(r.refused.len(), 2, "{:?}", r.refused);
        let b = fixture(&dir, "pack.rar", PACK_RAR);
        let r = import_rar_with(&root, &b, requires_matching_diffuse, &mut no_choice).unwrap();
        assert!(r.names.is_empty() && r.refused.len() == 2);
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0, "nothing was written");
    }

    #[test]
    fn import_anyway_copies_the_model_without_a_diffuse_of_its_name() {
        let dir = temp("anyway-zip");
        let root = temp("anyway-zip-root");
        let z = make_zip(
            &dir,
            "pack.zip",
            &["bare/bare.obj", "bare/pbr_bare.png", "desk/desk.obj", "desk/diffuse_chair.png"],
        );
        let mut asked: Vec<(String, usize)> = Vec::new();
        let mut choose = |m: &str, c: &[String]| {
            asked.push((m.to_string(), c.len()));
            Choice::Anyway
        };
        let r = import_zip_with(&root, &z, requires_matching_diffuse, &mut choose).unwrap();
        assert_eq!(r.names, ["bare", "desk"]);
        assert!(r.refused.is_empty());
        assert_eq!(asked, [("bare".to_string(), 0), ("desk".to_string(), 1)], "asked even with no candidate at all");
        assert!(root.join("bare").join("pbr_bare.png").is_file());
        // nothing is added: no diffuse of the model's name appears
        assert!(!root.join("bare").join("diffuse_bare.png").exists());
        assert!(!root.join("desk").join("diffuse_desk.png").exists());
        assert!(root.join("desk").join("diffuse_chair.png").is_file());
    }

    #[test]
    fn each_model_gets_its_own_answer() {
        let dir = temp("mixed-zip");
        let root = temp("mixed-zip-root");
        let z = make_zip(
            &dir,
            "pack.zip",
            &["a/a.obj", "b/b.obj", "b/diffuse_x.png", "c/c.obj", "d/d.obj", "d/diffuse_d.png"],
        );
        let mut choose = |m: &str, _: &[String]| match m {
            "a" => Choice::Anyway,
            "b" => Choice::Pick(0),
            _ => Choice::Skip, // c; d matches by name and is never asked
        };
        let r = import_zip_with(&root, &z, requires_matching_diffuse, &mut choose).unwrap();
        assert_eq!(r.names, ["a", "b", "d"]);
        assert_eq!(r.refused.len(), 1);
        assert!(r.refused[0].contains("\"c\"") && r.refused[0].contains("skipped"));
        assert!(root.join("b").join("diffuse_b.png").is_file());
        assert!(!root.join("c").exists());
    }

    #[test]
    fn import_anyway_works_for_7z_and_rar_too() {
        // the real fixtures have no diffuse at all: both models are imported when the user says so
        let dir = temp("anyway-7z");
        let root = dir.join("printer");
        fs::create_dir_all(&root).unwrap();
        let a = fixture(&dir, "pack.7z", PACK_7Z);
        let r = import_7z_with(&root, &a, requires_matching_diffuse, &mut |_: &str, _: &[String]| Choice::Anyway).unwrap();
        assert_eq!(r.names.len(), 2, "{:?} {:?}", r.names, r.refused);
        assert!(r.refused.is_empty());
        let b = fixture(&dir, "pack.rar", PACK_RAR);
        let r = import_rar_with(&root, &b, requires_matching_diffuse, &mut |_: &str, _: &[String]| Choice::Anyway).unwrap();
        assert_eq!(r.names.len(), 2);
    }
}
