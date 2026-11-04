//! Export a model as a `.zip`: a save dialog (Explorer on Windows) asks where to put it, then the model folder is
//! written into the archive as `<model name>/...`, WITHOUT the files the program keeps only for itself: the source
//! `.glb` and the `votv_convert.json` sidecar (what the game needs is the `.obj`, `.mtl`, textures, `icon.png`,
//! `properties.cfg`, ...). Started from the tile's context menu and from the Export button above the preview.
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

enum Outcome {
    Done { path: PathBuf, files: usize },
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

/// Opens the save dialog for the model folder `dir` and exports it. Ignored while an export is already running.
pub fn start(job: &mut ExportJob, dir: PathBuf) {
    if job.rx.is_some() {
        return;
    }
    let (tx, rx) = channel();
    job.rx = Some(Mutex::new(rx));
    std::thread::spawn(move || {
        let _ = tx.send(run(&dir));
    });
}

fn run(dir: &Path) -> Outcome {
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "model".to_string());
    // Start on the user's Desktop (known-folder lookup, so a redirected / OneDrive desktop works too). Without this the
    // dialog opens in whatever place Windows remembers, which can be a network location.
    let mut dialog = rfd::FileDialog::new()
        .set_title("Export model as .zip")
        .set_file_name(format!("{name}.zip"))
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
    match write_zip(dir, &name, &dest) {
        Ok(files) => Outcome::Done { path: dest, files },
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

fn write_zip(dir: &Path, name: &str, dest: &Path) -> Result<usize, String> {
    let mut files = Vec::new();
    collect(dir, "", &mut files).map_err(|e| format!("could not read {}: {e}", dir.display()))?;
    let file =
        File::create(dest).map_err(|e| format!("could not create {}: {e}", dest.display()))?;
    // The archive may be saved inside the model folder: never pack it into itself.
    let dest_canon = std::fs::canonicalize(dest).ok();
    files.retain(|(p, _)| dest_canon.is_none() || std::fs::canonicalize(p).ok() != dest_canon);

    let mut zip = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    zip.add_directory(format!("{name}/"), opts)
        .map_err(|e| e.to_string())?;
    for (path, rel) in &files {
        zip.start_file(format!("{name}/{rel}"), opts)
            .map_err(|e| e.to_string())?;
        let mut f =
            File::open(path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
        io::copy(&mut f, &mut zip).map_err(|e| e.to_string())?;
    }
    zip.finish().map_err(|e| e.to_string())?;
    Ok(files.len())
}

/// Click on the Export button above the preview: exports the selected model.
pub fn on_header_click(t: Trigger<Pointer<Click>>, lib: Res<Library>, mut job: ResMut<ExportJob>) {
    if t.event().button != PointerButton::Primary {
        return;
    }
    if let Some(e) = lib.selected.and_then(|i| lib.entries.get(i)) {
        start(&mut job, e.dir.clone());
    }
}

/// Shows the Export button exactly where the Edit button is shown: only for an EDITABLE model (one with a saved .glb)
/// on the list screen. Runs after `conv_ui::edit_button_system`, which keeps `EditTarget` up to date.
pub fn button_system(
    target: Res<EditTarget>,
    layout: Res<Layout>,
    mut q: Query<&mut Node, With<HeaderExport>>,
) {
    let want = if target.0.is_some() && layout.preview_open && !layout.converter {
        Display::Flex
    } else {
        Display::None
    };
    for mut n in &mut q {
        if n.display != want {
            n.display = want;
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
        Some(Outcome::Done { path, files }) => {
            layout.notice = Some(format!("Exported {files} files to {}", path.display()));
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
            .map(|i| z.by_index(i).unwrap().name().to_string())
            .collect();
        assert!(names.contains(&"chicken/chicken.obj".to_string()));
        assert!(!names
            .iter()
            .any(|n| n.ends_with(".glb") || n.ends_with("votv_convert.json")));
        let _ = std::fs::remove_dir_all(&root);
    }
}
