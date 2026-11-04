//! VotV Model Manager: browse the printer models folder as a grid of generated icons, with a 3D preview.
//!
//! Usage:
//!   votv-model-manager [models-folder]         open the manager (default: %LocalAppData%\VotV\Assets\meshes\printer)
//!   votv-model-manager --check <model-folder>  print what the loader finds for one model, no window
//!   votv-model-manager --diagnose <model-folder>  detailed report on the OBJ/MTL files (paste it when a model fails)
//!   votv-model-manager --reset-folder          forget the saved folder and start in the default one again
//!
//! Dropping a .glb file on the window converts it right away (see converter.rs) and shows the result.
//!
//! The folder you pick with "Browse..." is remembered in %LocalAppData%\VotV Model Manager\config.txt.

// Release builds on Windows are GUI programs: no console window. (The --check / --diagnose modes attach to the console
// they were started from, see `attach_parent_console`.)
// Lints that do not fit a Bevy app: systems take many parameters, queries have long types, the 3/4-wide `chunks_exact`
// loops read better than `as_chunks`, and the Done / icon messages are sent once per job (their size does not matter).
// `chunks_exact_to_as_chunks` only exists in newer Clippy versions; `unknown_lints` keeps older toolchains quiet.
#![allow(unknown_lints)]
#![allow(
    clippy::too_many_arguments,
    clippy::type_complexity,
    clippy::chunks_exact_to_as_chunks,
    clippy::large_enum_variant
)]
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod blender;
mod conv_ui;
mod converter;
mod diagnose;
mod dialog;
mod export;
mod gamerig;
mod glbwatch;
mod icon_font;
mod icons;
mod import;
mod listmenu;
mod model;
mod preview;
mod printer;
mod scene;
mod textedit;
mod ui;
mod window_style;

use std::path::{Path, PathBuf};

use bevy::{
    prelude::*,
    render::{view::RenderLayers, RenderPlugin},
    window::WindowResizeConstraints,
    winit::WinitSettings,
};

use crate::icons::IconPipe;
use crate::model::Entry;
use crate::preview::Orbit;
use crate::scene::Opts;
use crate::ui::{FilePick, FolderPick};

// ---- shared state ----------------------------------------------------------------------------------

pub struct TileRefs {
    pub tile: Entity,
    pub icon_box: Entity,
    pub placeholder: Entity,
    /// The name under the icon (a text field while a tile is being renamed).
    pub name_box: Entity,
    pub name_text: Entity,
}

#[derive(Resource)]
pub struct Library {
    pub root: PathBuf,
    pub entries: Vec<Entry>,
    pub tiles: Vec<TileRefs>,
    /// The ACTIVE tile: the one the preview and the editor show.
    pub selected: Option<usize>,
    /// Every selected tile (Ctrl / Shift + click); contains `selected`. The toolbar acts on all of them.
    pub selection: std::collections::BTreeSet<usize>,
    /// Where a Shift + click range starts.
    pub anchor: Option<usize>,
    pub dirty: bool,     // true: rescan the folder and rebuild the tiles
    pub generation: u64, // bumped on every rescan so stale icon jobs are ignored
    /// Model (folder name) to select after the next rescan, e.g. the one just edited or renamed; otherwise the current
    /// selection is kept by name.
    pub select_name: Option<String>,
}

#[derive(Resource)]
pub struct Layout {
    pub preview_open: bool,
    pub list_w: f32, // width of the model list; the preview takes the rest
    pub options_open: bool,
    pub converter: bool, // the converter screen (a dropped .glb) replaces the list and preview screen
    pub drop_hover: bool, // a file is being dragged over the window
    pub notice: Option<String>, // shown in the drop zone / status bar, e.g. "only .glb files can be converted"
}

// ---- settings --------------------------------------------------------------------------------------------

/// Every place %LocalAppData%\\VotV\\Assets\\meshes\\printer could be, without duplicates: the Windows known-folder
/// API first, then the LOCALAPPDATA variable, then %UserProfile%\\AppData\\Local, then ~/.local/share.
fn default_candidates() -> Vec<PathBuf> {
    let tail = |base: PathBuf| {
        base.join("VotV")
            .join("Assets")
            .join("meshes")
            .join("printer")
    };
    let mut bases: Vec<PathBuf> = Vec::new();
    if let Some(d) = dirs::data_local_dir() {
        bases.push(d);
    }
    if let Some(v) = std::env::var_os("LOCALAPPDATA") {
        bases.push(PathBuf::from(v));
    }
    if let Some(v) = std::env::var_os("USERPROFILE") {
        bases.push(PathBuf::from(v).join("AppData").join("Local"));
    }
    if let Some(v) = std::env::var_os("HOME") {
        bases.push(PathBuf::from(v).join(".local").join("share"));
    }
    let mut out: Vec<PathBuf> = Vec::new();
    for b in bases {
        let p = tail(b);
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

/// The VotV printer folder: the first candidate that exists (or the first one, to show in the "not found" message).
pub fn default_root() -> PathBuf {
    let c = default_candidates();
    c.iter()
        .find(|p| p.is_dir())
        .or(c.first())
        .cloned()
        .unwrap_or_else(|| PathBuf::from("."))
}

fn local_app_data() -> PathBuf {
    dirs::data_local_dir()
        .or_else(|| std::env::var_os("LOCALAPPDATA").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// The folder of the app's own settings files (%LocalAppData%\\VotV Model Manager).
pub(crate) fn settings_dir() -> PathBuf {
    local_app_data().join("VotV Model Manager")
}

fn config_path() -> PathBuf {
    local_app_data()
        .join("VotV Model Manager")
        .join("config.txt")
}

fn load_config() -> Option<PathBuf> {
    let text = std::fs::read_to_string(config_path()).ok()?;
    let p = PathBuf::from(text.trim());
    p.is_dir().then_some(p)
}

pub fn save_config(root: &Path) {
    let path = config_path();
    if root == default_root() {
        let _ = std::fs::remove_file(path); // choosing the default means "no saved override"
        return;
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, root.to_string_lossy().as_bytes());
}

// ---- startup ---------------------------------------------------------------------------------------------

/// Hull overlay lines belong to the preview only; keep them out of the icon renders.
fn gizmo_layers(mut store: ResMut<GizmoConfigStore>) {
    let (config, _) = store.config_mut::<DefaultGizmoConfigGroup>();
    config.render_layers = RenderLayers::layer(preview::PREVIEW_LAYER);
    // The lamp marker: preview only too, and always on top of the model (a negative bias skips the depth test).
    let (lamp, _) = store.config_mut::<preview::LampGizmos>();
    lamp.render_layers = RenderLayers::layer(preview::PREVIEW_LAYER);
    lamp.depth_bias = -1.0;
}

/// With `windows_subsystem = "windows"` the process has no console, so `println!` output of the command-line modes
/// would vanish. When started with a `--` option from a terminal, attach to that terminal and point stdout/stderr at
/// it (unless they were redirected to a file or pipe).
#[cfg(all(windows, not(debug_assertions)))]
fn attach_parent_console() {
    use std::ffi::c_void;
    use std::ptr::null;
    #[link(name = "kernel32")]
    extern "system" {
        fn AttachConsole(process_id: u32) -> i32;
        fn CreateFileW(
            name: *const u16,
            access: u32,
            share: u32,
            security: *const c_void,
            disposition: u32,
            flags: u32,
            template: *const c_void,
        ) -> *mut c_void;
        fn GetStdHandle(which: u32) -> *mut c_void;
        fn SetStdHandle(which: u32, handle: *mut c_void) -> i32;
    }
    const ATTACH_PARENT_PROCESS: u32 = 0xFFFF_FFFF;
    const STD_OUTPUT_HANDLE: u32 = 0xFFFF_FFF5; // -11
    const STD_ERROR_HANDLE: u32 = 0xFFFF_FFF4; // -12
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
            return; // not started from a console
        }
        let name: Vec<u16> = "CONOUT$\0".encode_utf16().collect();
        for which in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            let cur = GetStdHandle(which);
            if cur.is_null() || cur as isize == -1 {
                // GENERIC_WRITE, FILE_SHARE_READ | FILE_SHARE_WRITE, OPEN_EXISTING
                let h = CreateFileW(name.as_ptr(), 0x4000_0000, 3, null(), 3, 0, null());
                if !h.is_null() && h as isize != -1 {
                    SetStdHandle(which, h);
                }
            }
        }
    }
}

#[cfg(not(all(windows, not(debug_assertions))))]
fn attach_parent_console() {}

/// Also used to find the window handle (`window_style`).
pub const WINDOW_TITLE: &str = "VotV Model Manager";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a.starts_with("--")) {
        attach_parent_console();
    }

    if args.first().map(String::as_str) == Some("--check") {
        let Some(folder) = args.get(1) else {
            eprintln!("usage: votv-model-manager --check <model-folder>");
            std::process::exit(2);
        };
        match model::load_model(Path::new(folder)) {
            Ok(m) => model::check(&m),
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    if args.first().map(String::as_str) == Some("--diagnose") {
        let Some(folder) = args.get(1) else {
            eprintln!("usage: votv-model-manager --diagnose <model-folder>");
            std::process::exit(2);
        };
        diagnose::run(Path::new(folder));
        return;
    }

    if args.iter().any(|a| a == "--reset-folder") {
        let _ = std::fs::remove_file(config_path());
    }
    // Which folder to open. Candidates in priority order: command line, the folder chosen last time, then the
    // VotV default locations. The first one that actually contains models wins; failing that, the first that exists.
    let mut candidates: Vec<(PathBuf, &str)> = Vec::new();
    if let Some(p) = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .map(PathBuf::from)
    {
        candidates.push((p, "command line"));
    }
    if let Some(p) = load_config() {
        candidates.push((p, "saved choice (--reset-folder forgets it)"));
    }
    for p in default_candidates() {
        candidates.push((p, "default"));
    }
    let mut counted = Vec::new();
    for (p, why) in &candidates {
        let n = if p.is_dir() {
            model::scan_models(p).len()
        } else {
            0
        };
        println!(
            "[manager] candidate ({why}): {}  ->  {}",
            p.display(),
            if p.is_dir() {
                format!("{n} models")
            } else {
                "not found".into()
            }
        );
        counted.push(n);
    }
    let pick = counted
        .iter()
        .position(|&n| n > 0)
        .or_else(|| candidates.iter().position(|(p, _)| p.is_dir()))
        .unwrap_or(0);
    let root = candidates
        .get(pick)
        .map(|(p, _)| p.clone())
        .unwrap_or_else(default_root);
    println!("[manager] using: {}", root.display());

    App::new()
        .add_plugins(
            DefaultPlugins
                // Compile shaders before first use. Without this, the first icons are rendered before their
                // pipelines exist, come out blank, and only show up after Refresh.
                .set(RenderPlugin {
                    synchronous_pipeline_compilation: true,
                    ..default()
                })
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: WINDOW_TITLE.into(),
                        resolution: (1200.0_f32, 760.0_f32).into(),
                        resize_constraints: WindowResizeConstraints {
                            min_width: 760.0,
                            min_height: 440.0,
                            ..default()
                        },
                        decorations: false, // the UI draws its own title bar; see ui.rs
                        resizable: true,
                        ..default()
                    }),
                    ..default()
                }),
        )
        // Redraw only on input, or while the icon/preview loaders ask for a frame.
        .insert_resource(WinitSettings::desktop_app())
        .insert_resource(ClearColor(Color::srgb(0.10, 0.11, 0.13)))
        .insert_resource(AmbientLight {
            color: Color::WHITE,
            brightness: 350.0,
            ..default()
        })
        .insert_resource(Library {
            root,
            entries: vec![],
            tiles: vec![],
            selected: None,
            selection: Default::default(),
            anchor: None,
            dirty: true,
            generation: 0,
            select_name: None,
        })
        .insert_resource(Layout {
            preview_open: true,
            list_w: 602.0, // exactly 4 icon columns: 2*12 padding + 4*132 tiles + 3*12 gaps + 14 scrollbar
            options_open: false,
            converter: false,
            drop_hover: false,
            notice: None,
        })
        .insert_resource(Opts::default())
        .insert_resource(Orbit::default())
        .insert_resource(IconPipe::default())
        .insert_resource(FolderPick::default())
        .insert_resource(FilePick::default())
        .insert_resource(converter::ConvertState::new())
        .insert_resource(conv_ui::ParamRows(conv_ui::rows()))
        .insert_resource(conv_ui::SliderDrag::default())
        .insert_resource(conv_ui::DropdownOpen::default())
        .insert_resource(conv_ui::EditTarget::default())
        .insert_resource(conv_ui::LogPanel::default())
        .insert_resource(conv_ui::OpenSections::default())
        .insert_resource(conv_ui::PropsWidth::default())
        .insert_resource(conv_ui::IconEdit::default())
        .insert_resource(textedit::Editing::default())
        .insert_resource(listmenu::Search::default())
        .insert_resource(listmenu::Toolbar::default())
        .insert_resource(listmenu::SettingsMenu::default())
        .insert_resource(export::ExportJob::default())
        .insert_resource(blender::BlenderJob::default())
        .insert_resource(blender::BlenderInstall::default())
        .insert_resource(blender::BlenderPick::default())
        .insert_resource(import::ImportJob::default())
        .insert_resource(listmenu::SortState::default())
        .add_systems(
            Update,
            (
                export::button_system.after(conv_ui::edit_button_system),
                export::poll_system,
                import::poll_system,
                blender::button_system,
                blender::poll_system,
                blender::pick_system,
                glbwatch::watch_system,
                glbwatch::pacing_system,
                blender::button_colors,
                conv_ui::reset_sections_system,
                conv_ui::hull_indicator_system,
            ),
        )
        .insert_resource(dialog::ReplaceDialog::load())
        .init_gizmo_group::<preview::LampGizmos>()
        .add_systems(Startup, (ui::setup, gizmo_layers))
        // Order matters: input and loaders first, then the converter (it feeds the preview stage), then the
        // layout and per-frame UI upkeep. The inner tuple is nested only to stay under Bevy's 20-system tuple limit.
        .add_systems(Update, window_style::round_corners)
        .add_systems(
            Update,
            (
                conv_ui::icon_input_system,
                conv_ui::icon_hint_system,
                conv_ui::slider_keys_system,
                conv_ui::slider_visuals_system,
            )
                .chain(),
        )
        // The game printers' volumes and fit report in the preview (printer.rs), and the MODEL header's indicator.
        .add_systems(
            Update,
            (
                conv_ui::limits_indicator_system,
                conv_ui::light_indicator_system,
                preview::printer_gizmos,
                preview::lamp_gizmos,
                preview::printer_hud_system,
            ),
        )
        .add_systems(
            Update,
            (
                ui::poll_folder_pick,
                ui::poll_file_pick,
                ui::drop_events,
                listmenu::search_input_system,
                textedit::edit_input_system,
                ui::rescan,
                icons::icon_system,
                preview::preview_request,
                preview::preview_receive,
                converter::converter_system,
                (
                    (
                        preview::controls,
                        preview::apply_opts,
                        preview::thumb_rig_system,
                        ui::layout_system,
                        preview::preview_resize,
                        ui::wheel_system,
                        ui::scrollbar_system,
                        preview::orbit_apply,
                        preview::hull_gizmos,
                        ui::tile_styles,
                        ui::button_colors,
                        ui::status_text,
                    )
                        .chain(),
                    listmenu::filter_system,
                    listmenu::search_visuals_system,
                    listmenu::toolbar_system,
                    listmenu::settings_menu_system,
                    conv_ui::props_icon_system,
                    dialog::dialog_system,
                    preview::preview_text,
                    conv_ui::conv_ui_system,
                    conv_ui::info_tree_system,
                    textedit::edit_visuals_system,
                    conv_ui::edit_button_system,
                    conv_ui::sidebar_scrollbar_system,
                    conv_ui::info_scrollbar_system,
                    conv_ui::name_marquee_system,
                    conv_ui::reset_button_system,
                )
                    .chain(),
            )
                .chain(),
        )
        .run();
}
