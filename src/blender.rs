//! "Open in Blender": a button in the editor's title bar (between the Settings slot and the Done button) that opens the
//! model being edited in Blender. Blender cannot open a `.glb` as a command line argument (only `.blend` files), so it is
//! started with a tiny Python script (`--python <script> -- <model>`) that empties the default scene, imports the glTF
//! and frames it. The user's preferences and startup file are left alone.
//!
//! The button only exists when Blender is known: the path chosen in Settings ("Blender path...", remembered in
//! `%LocalAppData%\VotV Model Manager\blender_path.txt`) wins; without one the newest
//! `Blender Foundation\Blender x.y\blender.exe` under Program Files / `%LocalAppData%\Programs`, else a Steam install, is
//! used (looked up once at start). Starting the process and the file dialog run on their own threads; results come back
//! through channels and are put on the status line.
//!
//! Colours: Blender's brand colours mark this as an integration with another program: blue button, orange border, white
//! text and the Blender logo.

use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::{
        mpsc::{channel, Receiver, TryRecvError},
        Mutex,
    },
};

use bevy::prelude::*;

use crate::{converter::ConvertState, Layout};

/// #e87d0d
pub const BLENDER_ORANGE: Color = Color::srgb(232.0 / 255.0, 125.0 / 255.0, 13.0 / 255.0);
/// #265787
pub const BLENDER_BLUE: Color = Color::srgb(38.0 / 255.0, 87.0 / 255.0, 135.0 / 255.0);
/// #ffffff
pub const BLENDER_WHITE: Color = Color::WHITE;
// Hover / pressed variants of the two brand colours.
const BLUE_HOVER: Color = Color::srgb(47.0 / 255.0, 106.0 / 255.0, 166.0 / 255.0);
const BLUE_PRESS: Color = Color::srgb(28.0 / 255.0, 68.0 / 255.0, 104.0 / 255.0);
const ORANGE_HOVER: Color = Color::srgb(1.0, 154.0 / 255.0, 46.0 / 255.0);

/// Marker of the title-bar button.
#[derive(Component)]
pub struct BlenderButton;

enum Outcome {
    Opened,
    Failed(String),
}

#[derive(Resource, Default)]
pub struct BlenderJob {
    rx: Option<Mutex<Receiver<Outcome>>>,
}

/// The Blender logo (`assets/blender_icon_64x64.png`, compiled into the exe). `None` if it cannot be decoded: the
/// button then shows the text only.
pub fn logo(images: &mut Assets<Image>) -> Option<Handle<Image>> {
    let rgba = image::load_from_memory_with_format(
        include_bytes!("../assets/blender_icon_64x64.png"),
        image::ImageFormat::Png,
    )
    .ok()?
    .to_rgba8();
    let (w, h) = rgba.dimensions();
    Some(images.add(Image::new(
        bevy::render::render_resource::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        bevy::render::render_resource::TextureDimension::D2,
        rgba.into_raw(),
        bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
        bevy::asset::RenderAssetUsages::default(),
    )))
}

/// Only glTF files go to Blender (the editor's input is always a `.glb`).
fn supported(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| {
            e.eq_ignore_ascii_case("glb") || e.eq_ignore_ascii_case("gltf")
        })
}

fn parse_version(name: &str) -> (u32, u32, u32) {
    let n: Vec<u32> = name
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect();
    (
        n.first().copied().unwrap_or(0),
        n.get(1).copied().unwrap_or(0),
        n.get(2).copied().unwrap_or(0),
    )
}

/// The newest standard install, else a Steam install.
fn detect_blender() -> Option<PathBuf> {
    let mut bases: Vec<PathBuf> = Vec::new();
    for var in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
        if let Some(v) = std::env::var_os(var) {
            bases.push(PathBuf::from(v));
        }
    }
    if let Some(v) = std::env::var_os("LOCALAPPDATA") {
        bases.push(PathBuf::from(v).join("Programs"));
    }
    let mut best: Option<((u32, u32, u32), PathBuf)> = None;
    for base in &bases {
        let Ok(rd) = std::fs::read_dir(base.join("Blender Foundation")) else {
            continue;
        };
        for e in rd.flatten() {
            let exe = e.path().join("blender.exe");
            if !exe.is_file() {
                continue;
            }
            let ver = parse_version(&e.file_name().to_string_lossy());
            if best.as_ref().is_none_or(|(v, _)| ver > *v) {
                best = Some((ver, exe));
            }
        }
    }
    if let Some((_, p)) = best {
        return Some(p);
    }
    bases
        .iter()
        .map(|b| {
            b.join("Steam")
                .join("steamapps")
                .join("common")
                .join("Blender")
                .join("blender.exe")
        })
        .find(|p| p.is_file())
}

fn saved_path_file() -> PathBuf {
    crate::settings_dir().join("blender_path.txt")
}

fn load_saved() -> Option<PathBuf> {
    let p = PathBuf::from(std::fs::read_to_string(saved_path_file()).ok()?.trim());
    p.is_file().then_some(p)
}

/// The Blender executable to use, `None` if there is none (the button is then not shown).
#[derive(Resource)]
pub struct BlenderInstall(pub Option<PathBuf>);

impl Default for BlenderInstall {
    fn default() -> Self {
        BlenderInstall(load_saved().or_else(detect_blender))
    }
}

const SCRIPT: &str = r##"# Written by VotV Model Manager ("Open in Blender"): opens the model in a clean scene.
import bpy, sys

path = sys.argv[sys.argv.index("--") + 1]


def frame_all():
    try:
        for window in bpy.context.window_manager.windows:
            for area in window.screen.areas:
                if area.type == "VIEW_3D":
                    region = next(r for r in area.regions if r.type == "WINDOW")
                    with bpy.context.temp_override(window=window, area=area, region=region):
                        bpy.ops.view3d.view_all()
    except Exception as e:
        print("view_all failed:", e)
    return None




class VOTV_OT_export_back(bpy.types.Operator):
    """Writes the model back to the .glb that VotV Model Manager is watching (it regenerates by itself)"""

    bl_idname = "votv.export_back"
    bl_label = "Export to VotV Model Manager"

    def execute(self, context):
        try:
            bpy.ops.export_scene.gltf(filepath=path, export_format="GLB")
        except Exception as e:
            self.report({"ERROR"}, "Export failed: %s" % e)
            return {"CANCELLED"}
        self.report({"INFO"}, "Exported to %s" % path)
        return {"FINISHED"}


def draw_in_menu(self, context):
    self.layout.operator(VOTV_OT_export_back.bl_idname, icon="EXPORT")
    self.layout.separator()


def draw_in_header(self, context):
    self.layout.operator(VOTV_OT_export_back.bl_idname, icon="EXPORT")


bpy.utils.register_class(VOTV_OT_export_back)
bpy.types.TOPBAR_MT_file.prepend(draw_in_menu)  # File > Export to VotV Model Manager
bpy.types.VIEW3D_HT_header.append(draw_in_header)  # and a button in the 3D view header

# Keep the user's preferences and startup file: only empty the default scene.
for obj in list(bpy.data.objects):
    bpy.data.objects.remove(obj, do_unlink=True)
bpy.ops.import_scene.gltf(filepath=path)
bpy.app.timers.register(frame_all, first_interval=0.5)
"##;

/// Starts Blender with `input`. Ignored while a previous start is still in progress.
pub fn start(job: &mut BlenderJob, exe: PathBuf, input: PathBuf) {
    if job.rx.is_some() {
        return;
    }
    let (tx, rx) = channel();
    job.rx = Some(Mutex::new(rx));
    std::thread::spawn(move || {
        let _ = tx.send(run(&exe, &input));
    });
}

fn run(exe: &Path, input: &Path) -> Outcome {
    if !input.is_file() {
        return Outcome::Failed(format!("{} does not exist (yet)", input.display()));
    }
    let dir = crate::settings_dir();
    let script = dir.join("open_in_blender.py");
    if let Err(e) = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&script, SCRIPT)) {
        return Outcome::Failed(format!("could not write {}: {e}", script.display()));
    }
    let mut cmd = Command::new(exe);
    cmd.arg("--python").arg(&script).arg("--").arg(input);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: no extra console window next to Blender
    }
    match cmd.spawn() {
        Ok(_) => Outcome::Opened,
        Err(e) => Outcome::Failed(format!("could not start {}: {e}", exe.display())),
    }
}

/// Click on the title-bar button.
pub fn on_click(
    t: Trigger<Pointer<Click>>,
    conv: Res<ConvertState>,
    install: Res<BlenderInstall>,
    mut job: ResMut<BlenderJob>,
) {
    if t.event().button != PointerButton::Primary {
        return;
    }
    if let (Some(exe), Some(input)) = (install.0.clone(), conv.input.clone()) {
        start(&mut job, exe, input);
    }
}

/// The button is shown in the editor, while a glTF is open and Blender is installed.
pub fn button_system(
    layout: Res<Layout>,
    conv: Res<ConvertState>,
    install: Res<BlenderInstall>,
    mut q: Query<&mut Node, With<BlenderButton>>,
) {
    let on =
        layout.converter && conv.input.as_deref().is_some_and(supported) && install.0.is_some();
    let want = if on { Display::Flex } else { Display::None };
    for mut n in &mut q {
        if n.display != want {
            n.display = want;
        }
    }
}

/// Hover / pressed colours (the generic `ui::button_colors` skips this button).
#[allow(clippy::type_complexity)]
pub fn button_colors(
    mut q: Query<
        (&Interaction, &mut BackgroundColor, &mut BorderColor),
        (Changed<Interaction>, With<BlenderButton>),
    >,
) {
    for (i, mut bg, mut border) in &mut q {
        let (b, o) = match *i {
            Interaction::Pressed => (BLUE_PRESS, BLENDER_ORANGE),
            Interaction::Hovered => (BLUE_HOVER, ORANGE_HOVER),
            Interaction::None => (BLENDER_BLUE, BLENDER_ORANGE),
        };
        bg.0 = b;
        border.0 = o;
    }
}

/// Collects the result of a start and reports it on the status line.
pub fn poll_system(mut job: ResMut<BlenderJob>, mut layout: ResMut<Layout>) {
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
        Some(Outcome::Opened) => {
            layout.notice = Some("Opened in Blender. To send changes back, click \"Export to VotV Model Manager\" (3D view header or File menu).".to_string())
        }
        Some(Outcome::Failed(e)) => layout.notice = Some(format!("Open in Blender failed: {e}")),
        None => {}
    }
}

/// The file dialog of the Settings item "Blender path..." (one at a time).
#[derive(Resource, Default)]
pub struct BlenderPick(Mutex<Option<Receiver<PathBuf>>>);

/// Click on the Settings item "Blender path...": asks for `blender.exe`.
pub fn on_pick_click(
    t: Trigger<Pointer<Click>>,
    install: Res<BlenderInstall>,
    pick: Res<BlenderPick>,
) {
    if t.event().button != PointerButton::Primary {
        return;
    }
    let mut slot = pick.0.lock().unwrap();
    if slot.is_some() {
        return; // a dialog is already open
    }
    let (tx, rx) = channel();
    *slot = Some(rx);
    let start_dir = install
        .0
        .as_ref()
        .and_then(|p| p.parent())
        .map(Path::to_path_buf);
    std::thread::spawn(move || {
        let mut d = rfd::FileDialog::new()
            .set_title("Choose blender.exe")
            .add_filter("Blender", &["exe"]);
        if let Some(dir) = start_dir {
            d = d.set_directory(dir);
        }
        if let Some(p) = d.pick_file() {
            let _ = tx.send(p);
        }
    });
}

/// Takes the chosen file: remembered on disk and used from now on (the title-bar button appears at once).
pub fn pick_system(
    pick: Res<BlenderPick>,
    mut install: ResMut<BlenderInstall>,
    mut layout: ResMut<Layout>,
) {
    let mut slot = pick.0.lock().unwrap();
    let Some(rx) = slot.as_ref() else { return };
    match rx.try_recv() {
        Ok(path) => {
            *slot = None;
            let name = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            if !path.is_file() || !name.contains("blender") {
                layout.notice = Some(format!(
                    "{} does not look like Blender (blender.exe)",
                    path.display()
                ));
                return;
            }
            let _ = std::fs::create_dir_all(crate::settings_dir());
            if let Err(e) = std::fs::write(saved_path_file(), path.to_string_lossy().as_bytes()) {
                layout.notice = Some(format!("Could not remember the Blender path: {e}"));
            } else {
                layout.notice = Some(format!("Blender set to {}", path.display()));
            }
            install.0 = Some(path);
        }
        Err(TryRecvError::Empty) => {}
        Err(TryRecvError::Disconnected) => *slot = None, // dialog cancelled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_numerically() {
        assert!(parse_version("Blender 4.10") > parse_version("Blender 4.9"));
        assert!(parse_version("Blender 4.2") > parse_version("Blender 3.6"));
        assert_eq!(parse_version("Blender"), (0, 0, 0));
    }

    #[test]
    fn only_gltf_files_are_supported() {
        assert!(supported(Path::new("a/b/chicken.glb")));
        assert!(supported(Path::new("CHICKEN.GLTF")));
        assert!(!supported(Path::new("chicken.obj")));
        assert!(!supported(Path::new("chicken")));
    }
}
