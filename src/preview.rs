//! The 3D preview panel. The scene renders into an image (render layer 1) that the UI shows inside
//! the preview panel, so the panel can be any size and the camera never fights the UI for the screen.

use std::sync::mpsc::{channel, Receiver};
use std::sync::Mutex;

use bevy::{
    asset::RenderAssetUsages,
    core_pipeline::tonemapping::Tonemapping,
    prelude::*,
    render::{
        render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages},
        view::RenderLayers,
    },
    window::RequestRedraw,
};

use crate::gamerig::{self, Rig};
use crate::model::{load_with_textures, Loaded, KINDS};
use crate::scene::*;
use crate::ui::{OptLabel, UiRefs};
use crate::{Layout, Library};

pub const PREVIEW_LAYER: usize = 1;
/// Ambient brightness of the normal preview (the game's icon look uses none).
const STUDIO_AMBIENT: f32 = 350.0;
const STUDIO_FOV: f32 = std::f32::consts::FRAC_PI_4;
const STUDIO_CLEAR: Color = Color::srgb(0.07, 0.08, 0.09);

#[derive(Component)]
pub struct PreviewCamera;
#[derive(Component)]
pub struct StudioLight;
#[derive(Component)]
pub struct ThumbLight {
    pub front: bool,
}

#[derive(Resource)]
pub struct Orbit {
    pub yaw: f32,
    pub pitch: f32,
    pub dist: f32,
    pub target: Vec3,
}
impl Default for Orbit {
    fn default() -> Self {
        Self {
            yaw: 0.6,
            pitch: 0.35,
            dist: 1.0,
            target: Vec3::ZERO,
        }
    }
}

pub struct PreviewScene {
    pub loaded: Loaded,
    pub built: Built,
}

#[derive(Resource)]
pub struct PreviewState {
    pub target: Handle<Image>,
    pub target_size: (u32, u32),
    pub gen: u64,
    pub rx: Option<Mutex<Receiver<(u64, Result<Loaded, String>)>>>,
    pub loading: bool,
    pub scene: Option<PreviewScene>,
    pub header: String,
    pub hint: String,
    pub checker: Handle<Image>,
    pub scene_gen: u64,   // bumped whenever a new model is shown
    pub last_radius: f32, // radius (m) of the model on stage; lets a regenerated model keep the camera framing
}

/// Puts the preview camera back to the view a freshly loaded model gets: the studio view, or the game's icon camera
/// when the game renderer view (G) is on. Does nothing while no model is shown.
pub fn reset_camera(ps: &PreviewState, opts: &Opts, orbit: &mut Orbit) {
    let Some(scene) = ps.scene.as_ref() else {
        return;
    };
    let data = &scene.loaded.data;
    *orbit = if opts.game_view {
        let rig = Rig::new(Vec3::from(data.center), data.radius, opts.z_up);
        let (yaw, pitch) = rig.orbit_angles();
        Orbit {
            yaw,
            pitch,
            dist: rig.distance,
            target: Vec3::ZERO,
        }
    } else {
        Orbit {
            yaw: 0.6,
            pitch: 0.35,
            dist: (data.radius * UNIT * 2.8).max(0.15),
            target: Vec3::ZERO,
        }
    };
}

/// Camera, render target and lights for the preview. Returns the state resource to insert.
pub fn spawn_stage(commands: &mut Commands, images: &mut Assets<Image>) -> PreviewState {
    let mut image = Image::new_fill(
        Extent3d {
            width: 640,
            height: 480,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        &[0, 0, 0, 0],
        TextureFormat::Bgra8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.texture_descriptor.usage =
        TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST | TextureUsages::RENDER_ATTACHMENT;
    let target = images.add(image);
    let layer = RenderLayers::layer(PREVIEW_LAYER);

    commands.spawn((
        PreviewCamera,
        Camera3d::default(),
        Camera {
            target: target.clone().into(),
            clear_color: Color::srgb(0.07, 0.08, 0.09).into(),
            order: -1, // render before the UI camera samples the result
            ..default()
        },
        Projection::Perspective(PerspectiveProjection {
            near: 0.001,
            ..default()
        }),
        AmbientLight {
            brightness: STUDIO_AMBIENT,
            ..default()
        }, // per camera, so the icon look can switch it off
        Transform::from_xyz(0.0, 0.0, 1.0),
        layer.clone(),
    ));
    commands.spawn((
        StudioLight,
        DirectionalLight {
            illuminance: 9000.0,
            shadows_enabled: false,
            ..default()
        },
        Transform::from_xyz(2.0, 3.0, 2.5).looking_at(Vec3::ZERO, Vec3::Y),
        layer.clone(),
    ));
    // The game's icon lights (3dPrinterProcessor): key : fill = 3 : 1. Directions are set per model by `thumb_rig_system`.
    for front in [true, false] {
        commands.spawn((
            ThumbLight { front },
            DirectionalLight {
                illuminance: if front {
                    gamerig::key_lux()
                } else {
                    gamerig::fill_lux()
                },
                shadows_enabled: false,
                ..default()
            },
            Transform::default(),
            Visibility::Hidden,
            layer.clone(),
        ));
    }

    PreviewState {
        target,
        target_size: (640, 480),
        gen: 0,
        rx: None,
        loading: false,
        scene: None,
        header: String::new(),
        hint: "Select a model".into(),
        checker: images.add(checker_image()),
        scene_gen: 0,
        last_radius: 0.0,
    }
}

/// Drag on the preview: left = orbit, right/middle = pan.
pub fn surface_drag(t: Trigger<Pointer<Drag>>, mut o: ResMut<Orbit>, opts: Res<Opts>) {
    let d = t.event().delta;
    match t.event().button {
        PointerButton::Primary => {
            o.yaw -= d.x * 0.006;
            o.pitch = (o.pitch + d.y * 0.006).clamp(-1.5, 1.5);
        }
        PointerButton::Secondary | PointerButton::Middle => {
            let rot = Quat::from_euler(EulerRot::YXZ, o.yaw, -o.pitch, 0.0);
            // World units per pixel grow with the width of the view at the target (distance x tan(fov/2)); 0.00386 is the
            // old feel at the studio's 45 degrees. With the game's 10 degree view the old constant moved the model ~5x too far.
            let k = o.dist * (fov_for(&opts) * 0.5).tan() * 0.00386;
            o.target += (-(rot * Vec3::X) * d.x + (rot * Vec3::Y) * d.y) * k;
        }
    }
}

/// Vertical field of view of the preview camera for the current options.
pub fn fov_for(opts: &Opts) -> f32 {
    if opts.game_view {
        gamerig::fov_rad()
    } else {
        STUDIO_FOV
    }
}

pub fn orbit_apply(
    o: Res<Orbit>,
    opts: Res<Opts>,
    layout: Res<Layout>,
    ps: Res<PreviewState>,
    refs: Res<UiRefs>,
    nodes: Query<&ComputedNode>,
    mut cam: Query<&mut Transform, With<PreviewCamera>>,
) {
    if !o.is_changed() && !layout.is_changed() && !ps.is_changed() && !opts.is_changed() {
        return;
    }
    let fov = fov_for(&opts);
    if let Ok(mut t) = cam.single_mut() {
        let rot = Quat::from_euler(EulerRot::YXZ, o.yaw, -o.pitch, 0.0);
        t.translation = o.target + rot * Vec3::new(0.0, 0.0, o.dist);
        t.rotation = rot;
        // Converter screen: the sidebar covers the right side, so slide the view right by half of it; the model then
        // sits in the middle of the free area. (Moving the camera right moves the picture left.)
        if layout.converter {
            if let Ok(c) = nodes.get(refs.container) {
                let h = c.size.y * c.inverse_scale_factor;
                if h > 1.0 {
                    let view_h = 2.0 * o.dist * (fov * 0.5).tan();
                    let right: Vec3 = *t.right();
                    t.translation += right * (crate::conv_ui::PREVIEW_SHIFT * view_h / h);
                }
            }
        }
    }
}

/// Selection changed (or the converter screen was entered / left): drop the old preview and, on the library
/// screen, load the selected model on a background thread. On the converter screen the converter owns the stage.
pub fn preview_request(
    mut commands: Commands,
    lib: Res<Library>,
    layout: Res<Layout>,
    mut ps: ResMut<PreviewState>,
    mut last: Local<(Option<usize>, bool)>,
) {
    let now = (lib.selected, layout.converter);
    if *last == now {
        return;
    }
    *last = now;
    if let Some(old) = ps.scene.take() {
        commands.entity(old.built.root).despawn();
    }
    ps.rx = None; // cancels any load still running
    if layout.converter {
        ps.loading = false;
        ps.header.clear();
        ps.hint = "Converting...".into();
        return;
    }
    match lib.selected {
        None => {
            ps.loading = false;
            ps.header.clear();
            ps.hint = "Select a model".into();
        }
        Some(i) => {
            let (dir, name) = (lib.entries[i].dir.clone(), lib.entries[i].name.clone());
            ps.gen += 1;
            ps.loading = true;
            ps.header = format!("Loading {name}...");
            ps.hint = ps.header.clone();
            let gen = ps.gen;
            let (tx, rx) = channel();
            ps.rx = Some(Mutex::new(rx));
            std::thread::spawn(move || {
                let r = load_with_textures(&dir, &KINDS, None);
                let _ = tx.send((gen, r));
            });
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn preview_receive(
    mut commands: Commands,
    mut ps: ResMut<PreviewState>,
    opts: Res<Opts>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut orbit: ResMut<Orbit>,
    mut redraw: EventWriter<RequestRedraw>,
) {
    if !ps.loading {
        return;
    }
    redraw.write(RequestRedraw); // keep polling the loader thread
    let msg = match ps.rx.as_ref() {
        Some(m) => match m.lock() {
            Ok(g) => g.try_recv().ok(),
            Err(_) => None,
        },
        None => None,
    };
    let Some((gen, res)) = msg else { return };
    if gen != ps.gen {
        return;
    }
    ps.loading = false;
    ps.rx = None;
    match res {
        Err(e) => {
            eprintln!("[preview] {e}   (run with --diagnose <folder> for details)");
            ps.hint = format!("Could not load: {e}");
            ps.header = ps.hint.clone();
        }
        Ok(loaded) => {
            show_loaded(
                &mut commands,
                &mut meshes,
                &mut mats,
                &mut images,
                &mut ps,
                &opts,
                &mut orbit,
                loaded,
                false,
            );
        }
    }
}

/// Put a loaded model on the stage (used by the library preview and by the converter). Replaces whatever model is
/// shown, frames it, sizes the thumbnail lights to it and writes the header. With `keep_camera` (a regenerated
/// model) the viewing angle and target stay, and the distance scales with the change of model size, so a parameter
/// edit does not throw the user's view away.
#[allow(clippy::too_many_arguments)]
pub fn show_loaded(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    mats: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
    ps: &mut PreviewState,
    opts: &Opts,
    orbit: &mut Orbit,
    loaded: Loaded,
    keep_camera: bool,
) {
    if let Some(old) = ps.scene.take() {
        commands.entity(old.built.root).despawn();
    }
    let built = spawn_model(
        commands,
        meshes,
        mats,
        images,
        &loaded,
        RenderLayers::layer(PREVIEW_LAYER),
        opts,
        true,
    );
    let r = loaded.data.radius * UNIT;
    if keep_camera && ps.last_radius > 1.0e-4 {
        orbit.dist = (orbit.dist * r / ps.last_radius).max(0.02);
    } else if opts.game_view {
        // The game's icon camera: 12R away, from the front right above.
        let rig = Rig::new(
            Vec3::from(loaded.data.center),
            loaded.data.radius,
            opts.z_up,
        );
        let (yaw, pitch) = rig.orbit_angles();
        *orbit = Orbit {
            yaw,
            pitch,
            dist: rig.distance,
            target: Vec3::ZERO,
        };
    } else {
        *orbit = Orbit {
            yaw: 0.6,
            pitch: 0.35,
            dist: (r * 2.8).max(0.15),
            target: Vec3::ZERO,
        };
    }
    ps.last_radius = r;
    let dups = loaded.dup_removed;
    let untextured = loaded.tex.iter().filter(|t| t[0].is_none()).count();
    ps.header = format!(
        "{}  |  {} sections, {} hulls{}{}",
        loaded.data.name,
        loaded.data.sections.len(),
        loaded.data.hulls.len(),
        if dups > 0 {
            format!("  |  {dups} duplicate faces dropped (D shows them)")
        } else {
            String::new()
        },
        if untextured > 0 {
            format!("  |  {untextured} untextured sections")
        } else {
            String::new()
        }
    );
    // Legend for the "section colours" view (press C in the preview).
    for (i, s) in loaded.data.sections.iter().enumerate() {
        println!(
            "[preview] section {i}: material '{}', {} triangles, colour {}",
            s.mat_name,
            s.indices.len() / 3,
            section_color(i).to_srgba().to_hex()
        );
    }
    ps.scene = Some(PreviewScene { loaded, built });
    ps.scene_gen += 1;
}

pub fn controls(
    keys: Res<ButtonInput<KeyCode>>,
    mut opts: ResMut<Opts>,
    search: Res<crate::listmenu::Search>,
    dialog: Res<crate::dialog::ReplaceDialog>,
    editing: Res<crate::textedit::Editing>,
) {
    if search.focused || dialog.pending.is_some() || editing.active() {
        return; // typing in the search bar must not toggle the preview options
    }
    let mut o = opts.bypass_change_detection().clone();
    for (code, key) in KEY_BINDINGS {
        if keys.just_pressed(code) {
            toggle(key, &mut o);
        }
    }
    if o != *opts {
        *opts = o; // marks the resource changed only when something actually toggled
    }
}

#[allow(clippy::too_many_arguments)]
pub fn apply_opts(
    opts: Res<Opts>,
    ps: Res<PreviewState>,
    mut last_gen: Local<u64>,
    mut last_look: Local<bool>,
    mut orig: Local<Vec<SectionOrig>>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    mut roots: Query<&mut Transform>,
    mut studio: Query<&mut Visibility, (With<StudioLight>, Without<ThumbLight>)>,
    mut thumb: Query<&mut Visibility, (With<ThumbLight>, Without<StudioLight>)>,
) {
    let opts_changed = opts.is_changed();
    let scene_new = ps.scene_gen != *last_gen;
    *last_gen = ps.scene_gen;
    if !opts_changed && !scene_new {
        return;
    }
    if opts_changed {
        for mut v in &mut studio {
            *v = if opts.thumb_lights {
                Visibility::Hidden
            } else {
                Visibility::Inherited
            };
        }
        for mut v in &mut thumb {
            *v = if opts.thumb_lights {
                Visibility::Inherited
            } else {
                Visibility::Hidden
            };
        }
    }
    let Some(sc) = &ps.scene else { return };
    if scene_new {
        // Remember what the full-detail materials look like, so the icon look can be switched off again.
        orig.clear();
        for h in &sc.built.mats {
            orig.push(
                mats.get(h)
                    .map(|m| SectionOrig::capture(m, &images))
                    .unwrap_or_default(),
            );
        }
        *last_look = false;
    }
    let look_changed = opts.thumb_lights != *last_look;
    *last_look = opts.thumb_lights;
    for (i, s) in sc.loaded.data.sections.iter().enumerate() {
        if opts_changed {
            meshes.insert(&sc.built.meshes[i], build_mesh(s, &opts, true));
        }
        if let Some(m) = mats.get_mut(&sc.built.mats[i]) {
            if look_changed {
                if let Some(o) = orig.get(i) {
                    o.apply(
                        m,
                        &mut images,
                        opts.thumb_lights,
                        sc.built.spec.get(i).copied().flatten(),
                    );
                }
            }
            if opts_changed {
                m.alpha_mode = ALPHAS[opts.alpha].1;
                m.double_sided = opts.double_sided;
                m.cull_mode = if opts.double_sided {
                    None
                } else {
                    Some(bevy::render::render_resource::Face::Back)
                };
            }
            match opts.debug {
                1 => {
                    m.base_color = section_color(i);
                    m.base_color_texture = None;
                }
                2 => {
                    m.base_color = Color::WHITE;
                    m.base_color_texture = Some(ps.checker.clone());
                }
                _ => {
                    m.base_color = sc.built.orig_color[i];
                    m.base_color_texture = sc.built.orig_diffuse[i].clone();
                }
            }
        }
    }
    if opts_changed {
        if let Ok(mut t) = roots.get_mut(sc.built.root) {
            *t = root_transform(&sc.loaded.data, opts.z_up);
        }
    }
}

/// What one section's material looked like before the game's icon look was applied.
pub struct SectionOrig {
    normal: Option<Handle<Image>>,
    emissive_tex: Option<Handle<Image>>,
    emissive: LinearRgba,
    roughness: f32,
    reflectance: f32,
    samplers: Vec<(Handle<Image>, bevy::image::ImageSampler)>,
}

impl Default for SectionOrig {
    fn default() -> Self {
        SectionOrig {
            normal: None,
            emissive_tex: None,
            emissive: LinearRgba::BLACK,
            roughness: 0.5,
            reflectance: 0.5,
            samplers: Vec::new(),
        }
    }
}

impl SectionOrig {
    fn capture(m: &StandardMaterial, images: &Assets<Image>) -> SectionOrig {
        let handles = [
            &m.base_color_texture,
            &m.metallic_roughness_texture,
            &m.normal_map_texture,
            &m.emissive_texture,
        ];
        SectionOrig {
            normal: m.normal_map_texture.clone(),
            emissive_tex: m.emissive_texture.clone(),
            emissive: m.emissive,
            roughness: m.perceptual_roughness,
            reflectance: m.reflectance,
            samplers: handles
                .into_iter()
                .flatten()
                .filter_map(|h| images.get(h).map(|im| (h.clone(), im.sampler.clone())))
                .collect(),
        }
    }

    /// The icon render (see `icons.rs`) uses the diffuse and pbr maps only, samples them without filtering and uses
    /// roughness 1 with a pbr map, 0.85 without. `on` applies that; off puts the full-detail material back.
    fn apply(
        &self,
        m: &mut StandardMaterial,
        images: &mut Assets<Image>,
        on: bool,
        spec: Option<f32>,
    ) {
        use bevy::image::{ImageFilterMode, ImageSampler};
        if on {
            m.normal_map_texture = None;
            m.emissive_texture = None;
            m.emissive = LinearRgba::BLACK;
            m.perceptual_roughness = if m.metallic_roughness_texture.is_some() {
                1.0
            } else {
                0.85
            };
            m.reflectance = crate::scene::icon_reflectance(spec);
        } else {
            m.normal_map_texture = self.normal.clone();
            m.emissive_texture = self.emissive_tex.clone();
            m.emissive = self.emissive;
            m.perceptual_roughness = self.roughness;
            m.reflectance = self.reflectance;
        }
        for (h, original) in &self.samplers {
            if let Some(im) = images.get_mut(h) {
                im.sampler = if on {
                    match original {
                        ImageSampler::Descriptor(d) => {
                            let mut d = d.clone();
                            d.mag_filter = ImageFilterMode::Nearest;
                            d.min_filter = ImageFilterMode::Nearest;
                            d.mipmap_filter = ImageFilterMode::Nearest;
                            ImageSampler::Descriptor(d)
                        }
                        other => other.clone(),
                    }
                } else {
                    original.clone()
                };
            }
        }
    }
}

/// Keep the render target the same size as the panel (physical pixels) so the preview stays sharp.
pub fn preview_resize(
    refs: Res<UiRefs>,
    nodes: Query<&ComputedNode>,
    mut images: ResMut<Assets<Image>>,
    mut ps: ResMut<PreviewState>,
) {
    let Ok(c) = nodes.get(refs.container) else {
        return;
    };
    let (w, h) = (c.size.x.round() as u32, c.size.y.round() as u32);
    if w < 16 || h < 16 || (w, h) == ps.target_size {
        return;
    }
    if let Some(img) = images.get_mut(&ps.target) {
        img.resize(Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        });
        ps.target_size = (w, h);
    }
}

/// The game's icon look and view (options L and G) and the light that turns with the camera (S switches it off).
/// Look: its two lights aimed for the model on stage, no ambient light, no tone curve, flat background. View: its camera
/// (FOV 10 degrees, 12R away, from the front right above); switching it on moves the orbit camera there once,
/// switching it off goes back to the normal distance. Lights: unless "Static light" is on, every light turns with the
/// camera around the model, so the lighting seen from the camera stays the same while orbiting; when the camera is at
/// the game's icon angle the two icon lights are exactly where the game puts them.
#[allow(clippy::type_complexity)]
pub fn thumb_rig_system(
    opts: Res<Opts>,
    ps: Res<PreviewState>,
    mut orbit: ResMut<Orbit>,
    mut last: Local<(u64, bool, bool, bool, bool, bool)>, // scene_gen, thumb_lights, game_view, z_up, static_light, aa
    mut lights: Query<(&ThumbLight, &mut Transform, &mut DirectionalLight), Without<StudioLight>>,
    mut studio: Query<&mut Transform, (With<StudioLight>, Without<ThumbLight>)>,
    mut cam: Query<
        (
            &mut Projection,
            &mut Camera,
            &mut AmbientLight,
            &mut Tonemapping,
            &mut Msaa,
        ),
        With<PreviewCamera>,
    >,
) {
    let now = (
        ps.scene_gen,
        opts.thumb_lights,
        opts.game_view,
        opts.z_up,
        opts.static_light,
        opts.aa,
    );
    let prev = *last;
    let settings_changed = prev != now;
    if !settings_changed && (opts.static_light || !orbit.is_changed()) {
        return;
    }
    *last = now;
    if settings_changed {
        if let Ok((mut proj, mut camera, mut ambient, mut tone, mut msaa)) = cam.single_mut() {
            let look = opts.thumb_lights;
            // Anti-aliasing is its own option (M), independent of the icon look: the preview is smoothed, the generated
            // icons (icons.rs, Msaa::Off) are not.
            *msaa = if opts.aa { Msaa::Sample4 } else { Msaa::Off };
            ambient.brightness = if look { 0.0 } else { STUDIO_AMBIENT };
            *tone = if look {
                Tonemapping::None
            } else {
                Tonemapping::default()
            };
            let clear = if look {
                gamerig::BACKGROUND
            } else {
                STUDIO_CLEAR
            };
            camera.clear_color = ClearColorConfig::Custom(clear);
            if let Projection::Perspective(p) = &mut *proj {
                p.fov = if opts.game_view {
                    gamerig::fov_rad()
                } else {
                    STUDIO_FOV
                };
            }
        }
    }
    let rig = ps.scene.as_ref().map(|sc| {
        let data = &sc.loaded.data;
        Rig::new(Vec3::from(data.center), data.radius, opts.z_up)
    });
    if settings_changed && prev.2 != now.2 {
        if let Some(rig) = &rig {
            if now.2 {
                let (yaw, pitch) = rig.orbit_angles();
                *orbit = Orbit {
                    yaw,
                    pitch,
                    dist: rig.distance,
                    target: Vec3::ZERO,
                };
            } else {
                orbit.dist = (rig.radius * 2.8).max(0.15);
            }
        }
    }
    // Where the camera looks from now.
    let cur = Quat::from_euler(EulerRot::YXZ, orbit.yaw, -orbit.pitch, 0.0);
    if let Some(rig) = &rig {
        // The icon lights, turned by the angle the camera has moved away from the game's icon angle.
        let delta = if opts.static_light {
            Quat::IDENTITY
        } else {
            let (gy, gp) = rig.orbit_angles();
            cur * Quat::from_euler(EulerRot::YXZ, gy, -gp, 0.0).inverse()
        };
        for (l, mut t, mut d) in &mut lights {
            t.rotation = gamerig::light_rotation(
                delta
                    * if l.front {
                        rig.key_travel
                    } else {
                        rig.fill_travel
                    },
            );
            d.illuminance = if l.front {
                gamerig::key_lux()
            } else {
                gamerig::fill_lux()
            };
        }
    }
    // The studio light: from above right of the camera (it follows the camera), or the old fixed spot.
    if let Ok(mut t) = studio.single_mut() {
        t.rotation = if opts.static_light {
            Transform::from_xyz(2.0, 3.0, 2.5)
                .looking_at(Vec3::ZERO, Vec3::Y)
                .rotation
        } else {
            let toward_light = Vec3::new(0.45, 0.55, 0.70).normalize(); // camera space: right, up, towards the viewer
            gamerig::light_rotation(-(cur * toward_light))
        };
    }
}

/// Hull outlines. Two things made a toggle show up only after the next input event (the window redraws on demand):
/// (1) Bevy hands a gizmo to the renderer one frame after it was drawn, so turning the hulls ON (or loading a model)
/// needs one more frame: `RequestRedraw` whenever what is drawn changes; (2) the model root's `GlobalTransform` is only
/// updated in `PostUpdate`, after this system, so a Z-up toggle drew the old orientation: the root is a top-level entity,
/// so its `Transform` is the same thing and is already current (`apply_opts` runs before this system).
pub fn hull_gizmos(
    opts: Res<Opts>,
    ps: Res<PreviewState>,
    transforms: Query<&Transform>,
    mut gizmos: Gizmos,
    mut redraw: EventWriter<RequestRedraw>,
    mut last: Local<Option<(bool, u64, Option<Transform>)>>,
) {
    let root_tf = ps
        .scene
        .as_ref()
        .and_then(|sc| transforms.get(sc.built.root).ok())
        .copied();
    let now = (opts.show_hulls, ps.scene_gen, root_tf);
    if *last != Some(now) {
        *last = Some(now);
        redraw.write(RequestRedraw);
    }
    if !opts.show_hulls {
        return;
    }
    let Some(sc) = &ps.scene else { return };
    let Some(g) = root_tf else { return };
    for h in &sc.loaded.data.hulls {
        for (a, b) in &h.edges {
            gizmos.line(
                g.transform_point(Vec3::from(*a)),
                g.transform_point(Vec3::from(*b)),
                Color::srgb(0.1, 1.0, 0.3),
            );
        }
    }
}

/// Header text above the preview, the hint over an empty preview, and the option labels.
pub fn preview_text(
    ps: Res<PreviewState>,
    layout: Res<Layout>,
    opts: Res<Opts>,
    refs: Res<UiRefs>,
    mut texts: Query<&mut Text, Without<OptLabel>>,
    mut labels: Query<(&OptLabel, &mut Text)>,
) {
    if ps.is_changed() || layout.is_changed() {
        let header = ps.header.as_str();
        if let Ok(mut t) = texts.get_mut(refs.header_text) {
            if t.0 != header {
                t.0 = header.to_string();
            }
        }
    }
    if ps.is_changed() {
        if let Ok(mut t) = texts.get_mut(refs.hint_text) {
            if t.0 != ps.hint {
                t.0 = ps.hint.clone();
            }
        }
    }
    if opts.is_changed() {
        for (l, mut t) in &mut labels {
            t.0 = opt_text(l.0, &opts);
        }
    }
}
