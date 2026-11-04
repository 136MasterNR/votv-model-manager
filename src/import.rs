//! Importing ready-made models: drop (or pick) one or more `.zip` files on the list and they are UNPACKED into the VotV
//! printer folder, nothing is converted.
//!
//! * A `.zip` is unpacked: every folder inside that holds an `.obj` (or `.obj.disabled`) becomes a model folder; an
//!   `.obj` at the root of the zip makes the zip's own name the folder name.
//! * A name that already exists is never overwritten: the copy is called `name_2`, `name_3`, ...
//! * The work runs on a thread; `poll_system` shows the result in the status line and rescans the list.
//!
//! The fake tile of the EDITABLE section (`ui::spawn_drop_tile`) opens the file dialog (a `.glb` or `.zip` files);
//! `ui::poll_file_pick` and `ui::drop_events` hand the zip files to `start`.

use std::fs;
use std::io::{self, BufReader};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
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

#[derive(Resource, Default)]
pub struct ImportJob {
    rx: Option<Mutex<Receiver<Outcome>>>,
}

impl ImportJob {
    pub fn busy(&self) -> bool {
        self.rx.is_some()
    }
}

/// Unpack `items` (`.zip` files) into `root` on a worker thread. Ignored while another import runs.
pub fn start(job: &mut ImportJob, root: PathBuf, items: Vec<PathBuf>) {
    if job.rx.is_some() || items.is_empty() {
        return;
    }
    let (tx, rx) = channel();
    job.rx = Some(Mutex::new(rx));
    std::thread::spawn(move || {
        let mut out = Outcome::default();
        for item in items {
            match import_item(&root, &item) {
                Ok(names) if names.is_empty() => out.problems.push(format!(
                    "{}: no model found (an .obj file is needed)",
                    file_name(&item)
                )),
                Ok(mut names) => out.copied.append(&mut names),
                Err(e) => out.problems.push(format!("{}: {e}", file_name(&item))),
            }
        }
        let _ = tx.send(out);
    });
}

/// Reports a finished import in the status line and rescans the list.
pub fn poll_system(
    mut job: ResMut<ImportJob>,
    mut lib: ResMut<Library>,
    mut layout: ResMut<Layout>,
    mut redraw: EventWriter<RequestRedraw>,
) {
    if let Some(rx) = &job.rx {
        let r = rx.lock().unwrap().try_recv();
        match r {
            Ok(out) => {
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

fn import_item(root: &Path, item: &Path) -> Result<Vec<String>, String> {
    if item
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("zip"))
    {
        import_zip(root, item)
    } else {
        Err("only .zip files can be copied".to_string())
    }
}

fn is_model_file(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.ends_with(".obj") || n.ends_with(".obj.disabled")
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

fn import_zip(root: &Path, zip_path: &Path) -> Result<Vec<String>, String> {
    let file = fs::File::open(zip_path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(BufReader::new(file))
        .map_err(|e| format!("not a readable zip ({e})"))?;
    // Folders (relative to the zip root) that hold an .obj.
    let mut model_dirs: Vec<PathBuf> = Vec::new();
    for i in 0..archive.len() {
        let Ok(entry) = archive.by_index(i) else {
            continue;
        };
        let Some(path) = entry.enclosed_name() else {
            continue;
        };
        if entry.is_file() && is_model_file(&file_name(&path)) {
            let parent = path.parent().map(Path::to_path_buf).unwrap_or_default();
            if !model_dirs.contains(&parent) {
                model_dirs.push(parent);
            }
        }
    }
    let stem = zip_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "model".to_string());
    let mut names = Vec::new();
    for dir in model_dirs {
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| stem.clone());
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
        }
        names.push(file_name(&dest));
    }
    Ok(names)
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
}
