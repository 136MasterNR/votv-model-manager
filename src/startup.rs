//! Launch options.
//!
//! `--hide-window` (experimental, off by default): create the window hidden and show it after the first frames are
//! drawn, so the first sight of the app is the finished UI. A hidden window gets no redraw events on Windows, and
//! the app's reactive mode (`WinitSettings::desktop_app`) then waits for its 5 s timer: the first attempt at this
//! (redraw requests only) made launch take 5 s. This version runs the loop continuously while the window is hidden
//! and restores the reactive mode when it is shown.
//!
//! Graphics backend. On Windows the default is Vulkan only: measured on the RTX 5070 it starts the app about 270 ms
//! faster than letting wgpu initialise both DX12 and Vulkan (plugins built at 173 ms against 442 ms), and the first
//! preview does not freeze, which it does for 1.6-2.1 s on DX12 (shader compile). If a start with Vulkan never reaches
//! its first frame (no working Vulkan driver), the next start falls back to automatic selection and remembers that:
//! `startup-in-progress` is written before the window opens and removed after the first frame; if it is still there at the
//! next start, `backend-auto` is created in the settings folder (delete that file to try Vulkan again).
//!
//! `--backend=vulkan|dx12|auto` overrides all of this (`auto` = wgpu's own choice, both APIs).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use bevy::prelude::*;
use bevy::render::settings::{Backends, RenderCreation, WgpuSettings};
use bevy::window::PrimaryWindow;
use bevy::winit::{UpdateMode, WinitSettings};

/// Frames to run before the window is shown.
const FRAMES_BEFORE_SHOW: u32 = 3;

/// Whether the window should be created hidden (opt-in).
pub fn start_hidden(args: &[String]) -> bool {
    args.iter().any(|a| a == "--hide-window")
}

/// Set while `startup-in-progress` exists on disk because of this run.
static MARKER_SET: AtomicBool = AtomicBool::new(false);

fn marker_path() -> PathBuf {
    crate::settings_dir().join("startup-in-progress")
}

fn auto_flag_path() -> PathBuf {
    crate::settings_dir().join("backend-auto")
}

/// `--backend=...`: `Some(Some(b))` a backend, `Some(None)` automatic (wgpu decides), `None` the flag is absent.
fn parse_backend_flag(args: &[String]) -> Option<Option<Backends>> {
    let value = args.iter().find_map(|a| a.strip_prefix("--backend="))?;
    Some(match value.to_ascii_lowercase().as_str() {
        "dx12" => Some(Backends::DX12),
        "vulkan" => Some(Backends::VULKAN),
        _ => None,
    })
}

/// Windows default: Vulkan, unless the previous Vulkan start never reached its first frame.
#[cfg(windows)]
fn default_backend() -> Option<Backends> {
    let marker = marker_path();
    if marker.exists() {
        let _ = std::fs::write(
            auto_flag_path(),
            "The previous start with Vulkan did not reach its first frame; delete this file to try Vulkan again.\n",
        );
        let _ = std::fs::remove_file(&marker);
    }
    if auto_flag_path().exists() {
        println!("[manager] graphics: automatic (Vulkan did not start before; delete backend-auto to retry)");
        return None;
    }
    let _ = std::fs::create_dir_all(crate::settings_dir());
    if std::fs::write(&marker, "removed after the first frame\n").is_ok() {
        MARKER_SET.store(true, Ordering::Relaxed);
    }
    println!("[manager] graphics: Vulkan");
    Some(Backends::VULKAN)
}

#[cfg(not(windows))]
fn default_backend() -> Option<Backends> {
    None
}

/// Called once the first frame is drawn: the start worked, so the fallback marker goes.
fn start_succeeded() {
    if MARKER_SET.swap(false, Ordering::Relaxed) {
        let _ = std::fs::remove_file(marker_path());
    }
}

/// Renderer settings: an explicit `--backend=`, else the platform default (see the top of this file).
pub fn render_creation(args: &[String]) -> RenderCreation {
    let backends = match parse_backend_flag(args) {
        Some(explicit) => explicit,
        None => default_backend(),
    };
    let mut settings = WgpuSettings::default();
    if let Some(b) = backends {
        settings.backends = Some(b);
    }
    RenderCreation::Automatic(settings)
}

pub struct StartupPlugin;

impl Plugin for StartupPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ShowState>().add_systems(Last, show_when_ready);
    }
}

#[derive(Resource, Default)]
struct ShowState {
    frames: u32,
    done: bool,
}

fn show_when_ready(
    mut st: ResMut<ShowState>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
    mut settings: ResMut<WinitSettings>,
) {
    if st.done {
        return;
    }
    st.frames += 1;
    let hidden = windows.iter().any(|w| !w.visible);
    if !hidden {
        st.done = true; // the normal case: the window was never hidden, nothing to do
        start_succeeded();
        return;
    }
    if st.frames < FRAMES_BEFORE_SHOW {
        // A hidden window gets no redraw events, so keep the loop running until it is shown.
        settings.focused_mode = UpdateMode::Continuous;
        settings.unfocused_mode = UpdateMode::Continuous;
        return;
    }
    for mut w in &mut windows {
        w.visible = true;
    }
    *settings = WinitSettings::desktop_app();
    st.done = true;
    start_succeeded();
    let frames = st.frames;
    crate::benchmark::event("startup.window_shown", || format!("after {frames} frames"));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_window_is_visible_unless_hiding_is_asked_for() {
        assert!(!start_hidden(&args(&[])));
        assert!(!start_hidden(&args(&["--benchmark"])));
        assert!(start_hidden(&args(&["--hide-window"])));
    }

    #[test]
    fn backend_flag_parses() {
        assert_eq!(parse_backend_flag(&args(&["--backend=dx12"])), Some(Some(Backends::DX12)));
        assert_eq!(parse_backend_flag(&args(&["--backend=VULKAN"])), Some(Some(Backends::VULKAN)));
        assert_eq!(parse_backend_flag(&args(&["--backend=auto"])), Some(None));
        assert_eq!(parse_backend_flag(&args(&["--backend=nope"])), Some(None));
        assert_eq!(parse_backend_flag(&args(&["--benchmark"])), None);
    }
}
