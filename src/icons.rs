//! Low-cost icons.
//!
//! * A model folder that already has an `icon.png` (the game writes one; so do we) uses it: the worker thread just
//!   decodes it (resized to 128 px if needed) and the tile shows it as it is.
//! * Otherwise a worker thread parses the model and decodes only its diffuse and pbr maps (at most 1024 px), and the main
//!   thread renders ONE model at a time into a 128x128 image (own render layer, 4x MSAA (ICON_MSAA), no shadows, no
//!   normal/emissive maps) with the game's own camera and lights (gamerig.rs) on its grey background, waits a few
//!   frames, then despawns the temporary scene.
//!   The finished image is shown by the tile, and saved as `icon.png` in the model folder (a screenshot of the image).
//! * "Regenerate icon" in the converter queues the same render for a saved model and overwrites its `icon.png`.
//! * Tiles appear immediately with a placeholder; icons fill in as they finish.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, sync_channel, Receiver, TryRecvError};
use std::sync::Mutex;

use bevy::{
    asset::RenderAssetUsages,
    core_pipeline::tonemapping::Tonemapping,
    prelude::*,
    render::{
        render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages},
        view::{
            screenshot::{Screenshot, ScreenshotCaptured},
            RenderLayers,
        },
    },
    window::RequestRedraw,
};

use crate::gamerig::{self, Rig};
use crate::model::{load_with_textures, Kind, Loaded};
use crate::scene::{spawn_model, Opts};
use crate::Library;

pub const ICON_PX: u32 = 128;
const ICON_LAYER: usize = 2;
const FRAMES_PER_ICON: u32 = 4; // spawn, upload, render, safety
const MIN_ICON_SECS: f32 = 0.15; // also wait this long, so a very fast display cannot cut the render short

pub const ICON_FILE: &str = "icon.png";

/// The maps an icon render needs: the game's icon material uses the diffuse and pbr maps (metals come out dark, there
/// is no environment light); normal and emissive are skipped (128 px, and the game's emissive strength is 0 there).
const ICON_KINDS: &[Kind] = &[Kind::Diffuse, Kind::Pbr];
/// Longest side of the textures decoded for an icon. The game samples the full texture at 128 px (sharp, aliased),
/// so icons keep far more than 128 px of it and sample it without filtering (see `scene::spawn_model`).
/// Anti-aliasing of the icon render: 4x MSAA smooths the model's edges (the game's own icon has none; the user asked for it).
/// `Msaa::Off` gives the game's hard edges back.
const ICON_MSAA: Msaa = Msaa::Sample4;
const ICON_TEX_PX: u32 = 1024;

/// What the worker found for one model.
pub enum IconSource {
    /// An existing icon.png, decoded to 128x128 RGBA.
    File(Vec<u8>),
    /// No usable icon.png: the parsed model, to be rendered (and saved).
    Model(Loaded),
}

pub struct IconMsg {
    pub gen: u64,
    pub index: usize,
    pub result: Result<IconSource, String>,
}

/// What the editor's preview shows, captured when *Regenerate icon* is pressed there, so the icon is rendered the
/// way the preview looks now (camera position and angle, field of view, the visible lights, ambient light, tone curve,
/// preview options) instead of from the game's fixed view. The list's own icon generation never uses it.
#[derive(Clone)]
pub struct IconView {
    pub camera: Transform,
    pub fov: f32,
    /// The visible lights: world rotation and illuminance.
    pub lights: Vec<(Quat, f32)>,
    pub ambient: f32,
    pub tonemapping: Tonemapping,
    pub opts: Opts,
}

/// A "regenerate this folder's icon" request (always renders, always overwrites icon.png).
pub struct RegenMsg {
    pub dir: PathBuf,
    /// The list tile to refresh with the new icon (None: only the file is rewritten, e.g. from the converter).
    pub index: Option<usize>,
    pub gen: u64,
    pub result: Result<Loaded, String>,
}

pub struct InFlight {
    gen: u64,
    index: Option<usize>, // the tile to update, if any
    regen: bool,          // a regeneration request (not counted in the list progress)
    save_to: Option<PathBuf>,
    frames: u32,
    started: f32,
    capturing: Option<f32>, // when the screenshot was requested (the scene stays alive until it arrives)
    ents: Vec<Entity>,
    image: Handle<Image>,
}

#[derive(Resource, Default)]
pub struct IconPipe {
    pub rx: Option<Mutex<Receiver<IconMsg>>>,
    pub regen_rx: Option<Mutex<Receiver<RegenMsg>>>,
    pub current: Option<InFlight>,
    pub done: usize,
    pub total: usize,
    screens: usize, // screenshots still being written
    /// The preview view to render the pending regeneration with (None: the game's icon view).
    regen_view: Option<IconView>,
    /// Icons still to regenerate (a toolbar batch); the next one starts when the previous one has been picked up.
    regen_queue: std::collections::VecDeque<(PathBuf, Option<usize>, u64)>,
}

impl IconPipe {
    /// Drop whatever is in flight (used on rescan). Returns the entities to despawn.
    pub fn reset(&mut self, total: usize) -> Vec<Entity> {
        self.rx = None;
        self.done = 0;
        self.total = total;
        self.current.take().map(|c| c.ents).unwrap_or_default()
    }

    /// How many icon regenerations are still to finish: the queued ones plus the one loading, rendering or being saved.
    /// The status line derives its "Regenerating..." text from this, so the text goes away by itself when the last
    /// icon is written (it used to be a notice that nothing ever cleared).
    pub fn regen_remaining(&self) -> usize {
        // `screens` is not counted: it also counts the list's own icon saves, and while a regeneration's screenshot is
        // pending its `current` job is still alive anyway.
        let running = self.regen_rx.is_some() || self.current.as_ref().map(|c| c.regen).unwrap_or(false);
        self.regen_queue.len() + usize::from(running)
    }

    /// An icon regeneration is queued, rendering or being saved.
    pub fn regen_busy(&self) -> bool {
        self.regen_rx.is_some()
            || !self.regen_queue.is_empty()
            || self.screens > 0
            || self.current.as_ref().map(|c| c.regen).unwrap_or(false)
    }
}

/// Add a regeneration to the batch queue: they run one after the other (`icon_system` starts the next one).
pub fn queue_regen(pipe: &mut IconPipe, dir: PathBuf, index: Option<usize>, gen: u64) {
    pipe.regen_queue.push_back((dir, index, gen));
}

/// Queue a regeneration of `dir/icon.png` from the model in that folder.
pub fn request_regen(
    pipe: &mut IconPipe,
    dir: PathBuf,
    index: Option<usize>,
    gen: u64,
    view: Option<IconView>,
) {
    let (tx, rx) = channel();
    pipe.regen_rx = Some(Mutex::new(rx));
    pipe.regen_view = view;
    std::thread::spawn(move || {
        let result = load_with_textures(&dir, ICON_KINDS, Some(ICON_TEX_PX));
        let _ = tx.send(RegenMsg {
            dir,
            index,
            gen,
            result,
        });
    });
}

/// Prepare the bytes read back from the render target for saving. Returns false when the image is blank.
/// If the render wrote colour but no alpha at all, the coloured pixels are made opaque (and the rest stay transparent).
pub fn finish_icon(raw: &mut [u8]) -> bool {
    let any_alpha = raw.chunks_exact(4).any(|p| p[3] != 0);
    let any_color = raw.chunks_exact(4).any(|p| p[0] | p[1] | p[2] != 0);
    if !any_alpha && any_color {
        for p in raw.chunks_exact_mut(4) {
            if p[0] | p[1] | p[2] != 0 {
                p[3] = 255;
            }
        }
    }
    any_alpha || any_color
}

/// Decode an existing icon to 128x128 RGBA bytes (alpha kept, background left alone).
pub fn read_icon(dir: &Path) -> Option<Vec<u8>> {
    let bytes = std::fs::read(dir.join(ICON_FILE)).ok()?;
    let img = image::load_from_memory(&bytes).ok()?.to_rgba8();
    if img.pixels().all(|p| p.0[3] == 0) {
        return None; // a completely transparent file (an earlier failed save): treat as missing and render a new one
    }
    let img = if img.width() != ICON_PX || img.height() != ICON_PX {
        image::imageops::resize(
            &img,
            ICON_PX,
            ICON_PX,
            image::imageops::FilterType::Triangle,
        )
    } else {
        img
    };
    Some(img.into_raw())
}

/// Bounded channel: the worker blocks when 3 parsed models are waiting, which caps memory.
pub fn start_worker(jobs: Vec<(usize, PathBuf)>, gen: u64) -> Receiver<IconMsg> {
    let (tx, rx) = sync_channel::<IconMsg>(3);
    std::thread::spawn(move || {
        for (index, dir) in jobs {
            let result = match read_icon(&dir) {
                Some(rgba) => Ok(IconSource::File(rgba)),
                None => {
                    load_with_textures(&dir, ICON_KINDS, Some(ICON_TEX_PX)).map(IconSource::Model)
                }
            };
            if tx.send(IconMsg { gen, index, result }).is_err() {
                break; // receiver dropped: a rescan replaced this worker
            }
        }
    });
    rx
}

pub fn icon_system(
    mut commands: Commands,
    mut pipe: ResMut<IconPipe>,
    lib: Res<Library>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut redraw: EventWriter<RequestRedraw>,
    time: Res<Time>,
) {
    if pipe.screens > 0 {
        redraw.write(RequestRedraw); // a saved icon is being captured; the window only redraws on demand
    }
    // A toolbar batch: start the next queued regeneration once the previous one has been picked up.
    if pipe.regen_rx.is_none() {
        if let Some((dir, index, gen)) = pipe.regen_queue.pop_front() {
            request_regen(&mut pipe, dir, index, gen, None);
        }
    }
    if !pipe.regen_queue.is_empty() {
        redraw.write(RequestRedraw);
    }
    // 1. A job is rendering: count frames; when it has rendered, request the screenshot for icon.png and keep the
    //    camera and scene alive until it has been captured (the capture is taken from a camera that renders into the
    //    image in that frame; with the camera already gone it came back blank); then hand the image to the tile.
    let state = pipe.bypass_change_detection().current.as_mut().map(|c| {
        c.frames += 1;
        (
            c.frames,
            time.elapsed_secs() - c.started,
            c.capturing,
            c.save_to.clone(),
            c.image.clone(),
        )
    });
    if let Some((f, secs, capturing, save_to, image)) = state {
        redraw.write(RequestRedraw);
        match capturing {
            None => {
                if f < FRAMES_PER_ICON || secs < MIN_ICON_SECS {
                    return;
                }
                if let Some(path) = save_to {
                    pipe.screens += 1;
                    let keep = image.clone();
                    commands.spawn(Screenshot::image(image)).observe(move |t: Trigger<ScreenshotCaptured>, mut pipe: ResMut<IconPipe>| {
                        let _keep = &keep;
                        match t.event().0.clone().try_into_dynamic() {
                            Ok(img) => {
                                let rgba = img.to_rgba8();
                                let (w, h) = (rgba.width(), rgba.height());
                                let mut raw = rgba.into_raw();
                                if !finish_icon(&mut raw) {
                                    eprintln!("[icon] the rendered icon for {} came back blank; not saved", path.display());
                                } else if let Err(e) = image::save_buffer(&path, &raw, w, h, image::ColorType::Rgba8) {
                                    eprintln!("[icon] could not save {}: {e}", path.display());
                                }
                            }
                            Err(e) => eprintln!("[icon] could not read the rendered icon back: {e}"),
                        }
                        pipe.screens = pipe.screens.saturating_sub(1);
                    });
                    if let Some(c) = pipe.current.as_mut() {
                        c.capturing = Some(time.elapsed_secs());
                    }
                    return;
                }
            }
            Some(t0) => {
                if pipe.screens > 0 {
                    if time.elapsed_secs() - t0 < 3.0 {
                        return; // still waiting for the capture
                    }
                    eprintln!("[icon] the screenshot did not arrive in time; continuing");
                    pipe.screens = 0;
                }
            }
        }
        let cur = pipe.current.take().unwrap();
        for e in &cur.ents {
            commands.entity(*e).despawn();
        }
        if let Some(index) = cur.index {
            if cur.gen == lib.generation {
                if let Some(t) = lib.tiles.get(index) {
                    commands
                        .entity(t.icon_box)
                        .insert(ImageNode::new(cur.image.clone()));
                    if let Ok(mut p) = commands.get_entity(t.placeholder) {
                        p.despawn(); // gone already when the tile had an icon before (regeneration)
                    }
                }
            }
        }
        if !cur.regen {
            pipe.done += 1;
        }
        return;
    }

    // 2. Idle: take the next parsed model from a worker. Icons read from files are cheap, so several are applied per
    //    frame; a model that must be rendered starts a job and ends the loop.
    for _ in 0..16 {
        // (generation, tile index, folder, result)
        let mut next: Option<(
            u64,
            Option<usize>,
            PathBuf,
            Result<IconSource, String>,
            bool,
        )> = None;
        let mut waiting = false;
        let mut regen_closed = false;
        if let Some(rx) = pipe.regen_rx.as_ref() {
            if let Ok(g) = rx.lock() {
                match g.try_recv() {
                    Ok(m) => {
                        next = Some((m.gen, m.index, m.dir, m.result.map(IconSource::Model), true))
                    }
                    Err(TryRecvError::Empty) => waiting = true,
                    Err(TryRecvError::Disconnected) => regen_closed = true,
                }
            }
        }
        if regen_closed || next.is_some() {
            pipe.regen_rx = None;
        }
        let mut list_closed = false;
        if next.is_none() {
            if let Some(rx) = pipe.rx.as_ref() {
                if let Ok(g) = rx.lock() {
                    match g.try_recv() {
                        Ok(m) => {
                            let dir = lib
                                .entries
                                .get(m.index)
                                .map(|e| e.dir.clone())
                                .unwrap_or_default();
                            next = Some((m.gen, Some(m.index), dir, m.result, false));
                        }
                        Err(TryRecvError::Empty) => waiting = true,
                        Err(TryRecvError::Disconnected) => list_closed = true,
                    }
                }
            }
        }
        if list_closed {
            pipe.rx = None; // worker finished
        }
        let Some((gen, index, dir, result, regen)) = next else {
            if waiting {
                redraw.write(RequestRedraw); // keep polling the worker threads
            }
            return;
        };
        if index.is_some() && gen != lib.generation {
            continue; // from before a rescan
        }
        let loaded = match result {
            Ok(IconSource::File(rgba)) => {
                // An existing icon: show it as it is.
                if let Some(t) = index.and_then(|i| lib.tiles.get(i)) {
                    let img = Image::new(
                        Extent3d {
                            width: ICON_PX,
                            height: ICON_PX,
                            depth_or_array_layers: 1,
                        },
                        TextureDimension::D2,
                        rgba,
                        TextureFormat::Rgba8UnormSrgb,
                        RenderAssetUsages::default(),
                    );
                    commands
                        .entity(t.icon_box)
                        .insert(ImageNode::new(images.add(img)));
                    commands.entity(t.placeholder).despawn();
                }
                pipe.done += 1;
                redraw.write(RequestRedraw);
                continue;
            }
            Ok(IconSource::Model(l)) => l,
            Err(e) => {
                let name = dir
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                eprintln!("[icon] {name}: {e}   (run with --diagnose for details)");
                if let Some(t) = index.and_then(|i| lib.tiles.get(i)) {
                    commands.entity(t.placeholder).insert(Text::new("!"));
                }
                if !regen {
                    pipe.done += 1;
                }
                redraw.write(RequestRedraw);
                continue;
            }
        };

        // 3. Render it: temporary scene on its own layer, one camera aimed at a 128x128 image.
        let layer = RenderLayers::layer(ICON_LAYER);
        // A regeneration from the editor carries the preview's view and options; everything else uses the game's rig.
        let view = if regen { pipe.regen_view.take() } else { None };
        let opts = view.as_ref().map(|v| v.opts.clone()).unwrap_or_default();
        let built = spawn_model(
            &mut commands,
            &mut meshes,
            &mut mats,
            &mut images,
            &loaded,
            layer.clone(),
            &opts,
            false,
        );

        let mut image = Image::new_fill(
            Extent3d {
                width: ICON_PX,
                height: ICON_PX,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            &[0, 0, 0, 0],
            TextureFormat::Bgra8UnormSrgb,
            RenderAssetUsages::default(),
        );
        // COPY_SRC: the finished image is read back to be saved as icon.png.
        image.texture_descriptor.usage = TextureUsages::TEXTURE_BINDING
            | TextureUsages::COPY_DST
            | TextureUsages::COPY_SRC
            | TextureUsages::RENDER_ATTACHMENT;
        let handle = images.add(image);

        // The game's own icon rig (see gamerig.rs): FOV 10 degrees, 12R away from the front right above, two lights
        // 3 : 1, the global ambient brightness (gamerig::AMBIENT_FRACTION; the game has none), no tone curve, transparent
        // background. The bounds centre is at the world origin.
        let rig = Rig::new(
            Vec3::from(loaded.data.center),
            loaded.data.radius,
            opts.z_up,
        );
        let (cam_t, fov, near, far, tone, ambient, light_specs) = match &view {
            Some(v) => {
                let dist = v.camera.translation.length().max(0.01);
                let r = rig.radius;
                (
                    v.camera,
                    v.fov,
                    (dist - 2.0 * r).max(dist * 0.02),
                    dist + 4.0 * r,
                    v.tonemapping,
                    v.ambient,
                    v.lights.clone(),
                )
            }
            None => {
                let (near, far) = rig.clip_planes();
                let (key_travel, fill_travel) = rig.lights();
                let lights = vec![
                    (gamerig::light_rotation(key_travel), gamerig::key_lux()),
                    (
                        gamerig::light_rotation(fill_travel),
                        gamerig::fill_lux(),
                    ),
                ];
                (
                    rig.camera_transform(),
                    gamerig::fov_rad(),
                    near,
                    far,
                    Tonemapping::None,
                    gamerig::AMBIENT_BRIGHTNESS,
                    lights,
                )
            }
        };
        // Anti-aliasing: the list's icons always use 4x MSAA; the editor's Regenerate icon follows the preview's option (M).
        let msaa = match &view {
            Some(v) if !v.opts.aa => Msaa::Off,
            _ => ICON_MSAA,
        };
        let camera = commands
            .spawn((
                Camera3d::default(),
                Camera {
                    target: handle.clone().into(),
                    clear_color: ClearColorConfig::Custom(gamerig::BACKGROUND),
                    order: -1, // render before the UI camera samples the result
                    ..default()
                },
                Projection::Perspective(PerspectiveProjection {
                    fov,
                    near,
                    far,
                    ..default()
                }),
                msaa,
                tone,
                AmbientLight {
                    brightness: ambient,
                    ..default()
                }, // per camera; the same global brightness as the preview
                cam_t,
                layer.clone(),
            ))
            .id();
        let mut ents = vec![built.root, camera];
        for (rotation, illuminance) in light_specs {
            ents.push(
                commands
                    .spawn((
                        DirectionalLight {
                            illuminance,
                            shadows_enabled: false,
                            ..default()
                        },
                        Transform::from_rotation(rotation),
                        layer.clone(),
                    ))
                    .id(),
            );
        }

        pipe.current = Some(InFlight {
            gen,
            index,
            regen,
            // A model without a usable icon.png gets one; a regeneration request overwrites it.
            save_to: Some(dir.join(ICON_FILE)),
            frames: 0,
            started: time.elapsed_secs(),
            capturing: None,
            ents,
            image: handle,
        });
        redraw.write(RequestRedraw);
        return;
    }
    redraw.write(RequestRedraw); // more icons are waiting
}

#[cfg(test)]
mod tests {
    use super::finish_icon;

    #[test]
    fn blank_icons_are_rejected() {
        assert!(!finish_icon(&mut [0u8; 16]));
    }

    #[test]
    fn colour_without_alpha_becomes_opaque() {
        let mut px = [10, 20, 30, 0, 0, 0, 0, 0];
        assert!(finish_icon(&mut px));
        assert_eq!(px, [10, 20, 30, 255, 0, 0, 0, 0]);
    }

    #[test]
    fn real_alpha_is_left_alone() {
        let mut px = [10, 20, 30, 128, 0, 0, 0, 0];
        assert!(finish_icon(&mut px));
        assert_eq!(px, [10, 20, 30, 128, 0, 0, 0, 0]);
    }
}
