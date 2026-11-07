//! Export a model as a `.zip`: a save dialog (Explorer on Windows) asks where to put it, then the model folder is
//! written into the archive as `<model name>/...`, WITHOUT the files the program keeps only for itself: the source
//! `.glb` and the `votv_convert.json` sidecar (what the game needs is the `.obj`, `.mtl`, textures, `icon.png`,
//! `properties.cfg`, ...). Started from the Export button above the preview.
//!
//! With several tiles selected the same button reads **Export Bundle** and writes ONE `.zip` holding every selected model
//! as its own `<model name>/...` folder (same filter, same save dialog).
//!
//! The dialog and the zipping run on their own thread (a native dialog blocks), the result comes back through a channel
//! and `poll_system` puts it on the status line.

use std::{
    fs::File,
    io,
    path::{Path, PathBuf},
    sync::{
        mpsc::{channel, Receiver, TryRecvError},
        Mutex,
    },
};

use bevy::prelude::*;

use crate::conv_ui::EditTarget;
use crate::{Layout, Library};

/// Marker of the Export button above the preview (shown while a model is selected).
#[derive(Component)]
pub struct HeaderExport;

/// Marker of the text inside that button (it says "Export" for one model and "Export Bundle" for several).
#[derive(Component)]
pub struct HeaderExportLabel;

enum Outcome {
    Done { path: PathBuf, files: usize, models: usize },
    Cancelled,
    Failed(String),
}

#[derive(Resource, Default)]
pub struct ExportJob {
    rx: Option<Mutex<Receiver<Outcome>>>,
}

/// Files only this program needs: the source .glb (any case) and the options sidecar.
pub fn is_internal(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.ends_with(".glb") || n == "votv_convert.json"
}

/// Opens the save dialog and exports the model folders `folders` (one model: `<name>.zip`; several: one bundle with every
/// model in its own folder). Ignored while an export is already running or when `folders` is empty.
pub fn start(job: &mut ExportJob, folders: Vec<PathBuf>) {
    if job.rx.is_some() || folders.is_empty() {
        return;
    }
    let (tx, rx) = channel();
    job.rx = Some(Mutex::new(rx));
    std::thread::spawn(move || {
        let _ = tx.send(run(&folders));
    });
}

/// The name of a model folder (what the model is called inside the archive).
fn folder_name(dir: &Path) -> String {
    dir.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "model".to_string())
}

fn run(folders: &[PathBuf]) -> Outcome {
    let models: Vec<(PathBuf, String)> = folders.iter().map(|d| (d.clone(), folder_name(d))).collect();
    let bundle = models.len() > 1;
    let suggested = if bundle { "VotV models bundle".to_string() } else { models[0].1.clone() };
    // Start on the user's Desktop (known-folder lookup, so a redirected / OneDrive desktop works too). Without this the
    // dialog opens in whatever place Windows remembers, which can be a network location.
    let mut dialog = rfd::FileDialog::new()
        .set_title(if bundle { "Export models as a .zip bundle" } else { "Export model as .zip" })
        .set_file_name(format!("{suggested}.zip"))
        .add_filter("Zip archive", &["zip"]);
    if let Some(desktop) = dirs::desktop_dir().filter(|d| d.is_dir()) {
        dialog = dialog.set_directory(desktop);
    }
    let Some(mut dest) = dialog.save_file() else {
        return Outcome::Cancelled;
    };
    if dest.extension().is_none() {
        dest.set_extension("zip");
    }
    match write_bundle(&models, &dest) {
        Ok(files) => Outcome::Done { path: dest, files, models: models.len() },
        Err(e) => {
            let _ = std::fs::remove_file(&dest); // no half-written archive
            Outcome::Failed(e)
        }
    }
}

/// Every file under `dir` that is not internal, as (path on disk, name inside the folder with `/` separators).
fn collect(dir: &Path, prefix: &str, out: &mut Vec<(PathBuf, String)>) -> io::Result<()> {
    let mut entries = std::fs::read_dir(dir)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let fname = e.file_name().to_string_lossy().into_owned();
        let path = e.path();
        if path.is_dir() {
            collect(&path, &format!("{prefix}{fname}/"), out)?;
        } else if !is_internal(&fname) {
            out.push((path, format!("{prefix}{fname}")));
        }
    }
    Ok(())
}

/// One model into a fresh archive: `<name>/...`. See `write_bundle`.
#[cfg(test)]
fn write_zip(dir: &Path, name: &str, dest: &Path) -> Result<usize, String> {
    write_bundle(&[(dir.to_path_buf(), name.to_string())], dest)
}

/// Writes every `(model folder, name)` into ONE archive, each as its own `<name>/...` folder, without the internal files.
/// Returns how many files were packed in total.
fn write_bundle(models: &[(PathBuf, String)], dest: &Path) -> Result<usize, String> {
    let mut all = Vec::new(); // (name, files of that model)
    for (dir, name) in models {
        let mut files = Vec::new();
        collect(dir, "", &mut files).map_err(|e| format!("could not read {}: {e}", dir.display()))?;
        all.push((name, files));
    }
    let file =
        File::create(dest).map_err(|e| format!("could not create {}: {e}", dest.display()))?;
    // The archive may be saved inside a model folder: never pack it into itself.
    let dest_canon = std::fs::canonicalize(dest).ok();
    for (_, files) in &mut all {
        files.retain(|(p, _)| dest_canon.is_none() || std::fs::canonicalize(p).ok() != dest_canon);
    }

    let mut zip = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let mut total = 0;
    for (name, files) in &all {
        zip.add_directory(format!("{name}/"), opts)
            .map_err(|e| e.to_string())?;
        for (path, rel) in files {
            zip.start_file(format!("{name}/{rel}"), opts)
                .map_err(|e| e.to_string())?;
            let mut f =
                File::open(path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
            io::copy(&mut f, &mut zip).map_err(|e| e.to_string())?;
        }
        total += files.len();
    }
    zip.finish().map_err(|e| e.to_string())?;
    Ok(total)
}

/// The models the Export button acts on: every selected model when several are selected (a bundle), else the one
/// selected model.
fn export_dirs(lib: &Library) -> Vec<PathBuf> {
    let many: Vec<PathBuf> = lib
        .selection
        .iter()
        .filter_map(|&i| lib.entries.get(i))
        .map(|e| e.dir.clone())
        .collect();
    if many.len() > 1 {
        return many;
    }
    lib.selected
        .and_then(|i| lib.entries.get(i))
        .map(|e| vec![e.dir.clone()])
        .unwrap_or_default()
}

/// Click on the Export button above the preview: exports the selected model, or the selected models as one bundle.
pub fn on_header_click(t: Trigger<Pointer<Click>>, lib: Res<Library>, mut job: ResMut<ExportJob>) {
    if t.event().button != PointerButton::Primary {
        return;
    }
    start(&mut job, export_dirs(&lib));
}

/// Shows the Export button for an EDITABLE model (one with a saved .glb) on the list screen, as the Edit button, and
/// for any selection of two or more models, where it reads "Export Bundle". Runs after `conv_ui::edit_button_system`,
/// which keeps `EditTarget` up to date.
pub fn button_system(
    target: Res<EditTarget>,
    layout: Res<Layout>,
    lib: Res<Library>,
    mut q: Query<&mut Node, With<HeaderExport>>,
    mut labels: Query<&mut Text, With<HeaderExportLabel>>,
) {
    let several = lib.selection.iter().filter(|&&i| i < lib.entries.len()).count() > 1;
    let shown = layout.preview_open && !layout.converter && (several || target.0.is_some());
    let want = if shown { Display::Flex } else { Display::None };
    for mut n in &mut q {
        if n.display != want {
            n.display = want;
        }
    }
    let text = if several { "Export Bundle" } else { "Export" };
    for mut t in &mut labels {
        if t.0 != text {
            t.0 = text.to_string();
        }
    }
}

/// Collects the finished export and reports it on the status line.
pub fn poll_system(mut job: ResMut<ExportJob>, mut layout: ResMut<Layout>) {
    let outcome = match job.rx.as_ref() {
        None => return,
        Some(rx) => match rx.lock().unwrap().try_recv() {
            Ok(o) => Some(o),
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => None,
        },
    };
    job.rx = None;
    match outcome {
        Some(Outcome::Done { path, files, models }) => {
            layout.notice = Some(if models > 1 {
                format!("Exported {models} models ({files} files) to {}", path.display())
            } else {
                format!("Exported {files} files to {}", path.display())
            });
        }
        Some(Outcome::Failed(e)) => layout.notice = Some(format!("Export failed: {e}")),
        Some(Outcome::Cancelled) | None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_files_are_skipped() {
        assert!(is_internal("chicken.glb"));
        assert!(is_internal("CHICKEN.GLB"));
        assert!(is_internal("votv_convert.json"));
        assert!(!is_internal("chicken.obj"));
        assert!(!is_internal("icon.png"));
        assert!(!is_internal("other.json"));
    }

    #[test]
    fn zip_holds_the_folder_without_internal_files() {
        let root = std::env::temp_dir().join(format!("votv_export_test_{}", std::process::id()));
        let model = root.join("chicken");
        std::fs::create_dir_all(&model).unwrap();
        for f in [
            "chicken.obj",
            "chicken.mtl",
            "icon.png",
            "chicken.glb",
            "votv_convert.json",
        ] {
            std::fs::write(model.join(f), b"x").unwrap();
        }
        let dest = root.join("out.zip");
        let n = write_zip(&model, "chicken", &dest).unwrap();
        assert_eq!(n, 3);
        let mut z = zip::ZipArchive::new(File::open(&dest).unwrap()).unwrap();
        let names: Vec<String> = (0..z.len())
            .map(|i| z.by_index(i).unwrap().name().unwrap().to_string())
            .collect();
        assert!(names.contains(&"chicken/chicken.obj".to_string()));
        assert!(!names
            .iter()
            .any(|n| n.ends_with(".glb") || n.ends_with("votv_convert.json")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn bundle_holds_every_model_in_its_own_folder() {
        let root = std::env::temp_dir().join(format!("votv_bundle_test_{}", std::process::id()));
        let mut models = Vec::new();
        for name in ["chicken", "brownie"] {
            let dir = root.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            for f in [format!("{name}.obj"), "icon.png".to_string(), format!("{name}.glb"), "votv_convert.json".to_string()] {
                std::fs::write(dir.join(f), b"x").unwrap();
            }
            models.push((dir, name.to_string()));
        }
        // The archive is saved inside the first model's folder (and an older one is already there): it must not end up
        // in itself.
        let inside = models[0].0.join("bundle.zip");
        std::fs::write(&inside, b"old").unwrap();
        let n = write_bundle(&models, &inside).unwrap();
        assert_eq!(n, 4); // obj + icon.png per model
        let mut z = zip::ZipArchive::new(File::open(&inside).unwrap()).unwrap();
        let names: Vec<String> = (0..z.len())
            .map(|i| z.by_index(i).unwrap().name().unwrap().to_string())
            .collect();
        for want in ["chicken/chicken.obj", "chicken/icon.png", "brownie/brownie.obj", "brownie/icon.png"] {
            assert!(names.contains(&want.to_string()), "missing {want}: {names:?}");
        }
        assert!(!names.iter().any(|n| n.ends_with(".glb") || n.ends_with("votv_convert.json") || n.ends_with("bundle.zip")));
        let _ = std::fs::remove_dir_all(&root);
    }
}
