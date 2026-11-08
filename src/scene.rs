//! Turning a loaded model into Bevy entities. Used by the icon renderer (lean: diffuse only, no
//! tangents) and by the 3D preview (full: every map, game-like tangents, lamp).

use std::f32::consts::FRAC_PI_2;

use bevy::{
    asset::RenderAssetUsages,
    image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor},
    pbr::NotShadowCaster,
    prelude::*,
    render::{
        mesh::{Indices, PrimitiveTopology},
        render_resource::{Extent3d, Face, TextureDimension, TextureFormat},
        view::RenderLayers,
    },
};

use crate::model::*;

/// VotV units are centimetres; Bevy works best in metres.
pub const UNIT: f32 = 0.01;
/// Lumens of the preview lamp per unit of the model's `lamp_intensity`. It was 200, which made a lamp colour of 0.01 look
/// like 1.0 in the game and a colour of 1.0 burn the texture to white; the user measured that as about 100 times too strong,
/// so it is 2 now. (A physical candela-to-lumen conversion would be 4 pi, about 12.6, but the preview's exposure is
/// not the game's, so this is calibrated against the game.) Raise it if lamps look too dim next to the game.
pub const LAMP_GAIN: f32 = 2.0;

/// Toggles for the things that are not verified yet (see references/open-questions.md).
#[derive(Resource, Clone, PartialEq)]
pub struct Opts {
    pub game_tangents: bool, // true: every tangent (1,0,0), what the game most likely does
    pub flip_v: bool,        // OBJ V is up, textures are top-down
    pub flip_winding: bool,
    pub double_sided: bool, // the game material is one-sided
    pub z_up: bool,         // treat the OBJ as Z-up (Unreal) instead of Y-up
    pub thumb_lights: bool, // the game's icon look: its two lights, no ambient light, no tone curve, grey background
    pub game_view: bool, // the game's icon camera: FOV 10 degrees, 12R away, from the front right above
    pub static_light: bool, // true: the preview lights stay where they are; false: they turn with the camera
    pub aa: bool, // anti-aliasing of the preview (4x MSAA); the list's icons always use it, the editor's Regenerate icon follows it
    pub show_hulls: bool,
    pub show_lamp: bool, // a marker at the lamp's position (preview.rs lamp_gizmos)
    pub printer_limits: bool, // the game printers' volumes around the model + the fit report (printer.rs)
    pub alpha: usize,
    pub drop_duplicates: bool, // converter rule: drop the untextured copy of duplicated faces
    pub debug: u8,             // 0 textures, 1 one colour per section, 2 UV checker pattern
}
impl Default for Opts {
    fn default() -> Self {
        Self {
            game_tangents: true,
            flip_v: true,
            flip_winding: false,
            double_sided: false,
            z_up: false,
            thumb_lights: true, // the preview shows the game's icon look by default (lights, no tone curve, pbr + diffuse only)
            game_view: false, // the game's icon camera (FOV 10, 12R) is opt-in (G): the studio camera is the default
            static_light: false,
            aa: true,
            show_hulls: false,
            show_lamp: false,
            printer_limits: false,
            alpha: 0,
            drop_duplicates: true,
            debug: 0,
        }
    }
}

pub const ALPHAS: [(&str, AlphaMode); 3] = [
    ("clip 0.333", AlphaMode::Mask(0.3333)),
    ("clip 0.42", AlphaMode::Mask(0.42)),
    ("no clip", AlphaMode::Opaque),
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OptKey {
    Tangents,
    FlipV,
    FlipWinding,
    DoubleSided,
    ZUp,
    Alpha,
    ThumbLights,
    GameView,
    StaticLight,
    Aa,
    Hulls,
    PrinterLimits,
    LampMarker,
    Debug,
    Duplicates,
}
pub const ALL_KEYS: [OptKey; 15] = [
    OptKey::Tangents,
    OptKey::FlipV,
    OptKey::FlipWinding,
    OptKey::DoubleSided,
    OptKey::ZUp,
    OptKey::Alpha,
    OptKey::ThumbLights,
    OptKey::GameView,
    OptKey::StaticLight,
    OptKey::Aa,
    OptKey::Hulls,
    OptKey::PrinterLimits,
    OptKey::LampMarker,
    OptKey::Debug,
    OptKey::Duplicates,
];
pub const KEY_BINDINGS: [(KeyCode, OptKey); 15] = [
    (KeyCode::KeyT, OptKey::Tangents),
    (KeyCode::KeyU, OptKey::FlipV),
    (KeyCode::KeyF, OptKey::FlipWinding),
    (KeyCode::KeyB, OptKey::DoubleSided),
    (KeyCode::KeyZ, OptKey::ZUp),
    (KeyCode::KeyA, OptKey::Alpha),
    (KeyCode::KeyL, OptKey::ThumbLights),
    (KeyCode::KeyG, OptKey::GameView),
    (KeyCode::KeyS, OptKey::StaticLight),
    (KeyCode::KeyM, OptKey::Aa),
    (KeyCode::KeyH, OptKey::Hulls),
    (KeyCode::KeyP, OptKey::PrinterLimits),
    (KeyCode::KeyI, OptKey::LampMarker),
    (KeyCode::KeyC, OptKey::Debug),
    (KeyCode::KeyD, OptKey::Duplicates),
];

pub fn toggle(k: OptKey, o: &mut Opts) {
    match k {
        OptKey::Tangents => o.game_tangents = !o.game_tangents,
        OptKey::FlipV => o.flip_v = !o.flip_v,
        OptKey::FlipWinding => o.flip_winding = !o.flip_winding,
        OptKey::DoubleSided => o.double_sided = !o.double_sided,
        OptKey::ZUp => o.z_up = !o.z_up,
        OptKey::Alpha => o.alpha = (o.alpha + 1) % ALPHAS.len(),
        OptKey::ThumbLights => o.thumb_lights = !o.thumb_lights,
        OptKey::GameView => o.game_view = !o.game_view,
        OptKey::StaticLight => o.static_light = !o.static_light,
        OptKey::Aa => o.aa = !o.aa,
        OptKey::Hulls => o.show_hulls = !o.show_hulls,
        OptKey::PrinterLimits => o.printer_limits = !o.printer_limits,
        OptKey::LampMarker => o.show_lamp = !o.show_lamp,
        OptKey::Debug => o.debug = (o.debug + 1) % 3,
        OptKey::Duplicates => o.drop_duplicates = !o.drop_duplicates,
    }
}

pub fn opt_text(k: OptKey, o: &Opts) -> String {
    let on = |b: bool| if b { "on" } else { "off" };
    match k {
        OptKey::Tangents => format!(
            "Tangents (T): {}",
            if o.game_tangents {
                "game (1,0,0)"
            } else {
                "generated"
            }
        ),
        OptKey::FlipV => format!("Flip UV V (U): {}", on(o.flip_v)),
        OptKey::FlipWinding => format!("Flip winding (F): {}", on(o.flip_winding)),
        OptKey::DoubleSided => format!("Two-sided (B): {}", on(o.double_sided)),
        OptKey::ZUp => format!("Z-up (Z): {}", on(o.z_up)),
        OptKey::Alpha => format!("Alpha (A): {}", ALPHAS[o.alpha].0),
        OptKey::ThumbLights => format!("Game renderer look (L): {}", on(o.thumb_lights)),
        OptKey::GameView => format!("Game renderer view (G): {}", on(o.game_view)),
        OptKey::StaticLight => format!("Static light (S): {}", on(o.static_light)),
        OptKey::Aa => format!("Anti-aliasing (M): {}", on(o.aa)),
        OptKey::Hulls => format!("Show hulls (H): {}", on(o.show_hulls)),
        OptKey::PrinterLimits => format!("Printer limits (P): {}", on(o.printer_limits)),
        OptKey::LampMarker => format!("Lamp position (I): {}", on(o.show_lamp)),
        OptKey::Duplicates => format!("Drop duplicate faces (D): {}", on(o.drop_duplicates)),
        OptKey::Debug => format!(
            "View (C): {}",
            ["textures", "section colours", "UV checker"][o.debug as usize % 3]
        ),
    }
}

pub fn build_mesh(s: &RawSection, o: &Opts, tangents: bool) -> Mesh {
    let mut idx = if o.drop_duplicates {
        s.dedup.clone().unwrap_or_else(|| s.indices.clone())
    } else {
        s.indices.clone()
    };
    if o.flip_winding {
        for t in idx.chunks_exact_mut(3) {
            t.swap(1, 2);
        }
    }
    let uvs: Vec<[f32; 2]> = s
        .uvs
        .iter()
        .map(|[u, v]| if o.flip_v { [*u, 1.0 - *v] } else { [*u, *v] })
        .collect();
    let mut m = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, s.positions.clone());
    m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, s.normals.clone());
    m.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    m.insert_indices(Indices::U32(idx));
    if tangents {
        if o.game_tangents {
            // The game passes no tangents to CreateMeshSection, which leaves them at the X axis.
            m.insert_attribute(
                Mesh::ATTRIBUTE_TANGENT,
                vec![[1.0f32, 0.0, 0.0, 1.0]; s.positions.len()],
            );
        } else {
            let _ = m.generate_tangents();
        }
    }
    m
}

/// Where the game puts a `lamp_offset` (Unreal axes, cm: X forward, Y right, Z UP) in the model's own axes, which is
/// where the preview draws it. The converter writes the three numbers to `properties.cfg` untouched and the game reads
/// them as Unreal axes, so the preview must convert them. An OBJ is Y-up: Unreal (x, y, z) = OBJ (x, z, y), i.e. the Y
/// and Z entries swap (reported by the user: Y and Z were swapped in-game compared with the editor). A Z-up model is
/// already in Unreal axes (the root's rotation stands it up), so its offset passes through.
/// NOTE: the icon rig (`gamerig::ue_to_model`) measured a different horizontal mapping, but its picture is the same under
/// both (its doc says so); the lamp is the first thing that tells them apart.
pub fn lamp_offset_to_model(ue: Vec3, z_up: bool) -> Vec3 {
    if z_up {
        ue
    } else {
        Vec3::new(ue.x, ue.z, ue.y)
    }
}

pub fn root_transform(m: &ModelData, z_up: bool) -> Transform {
    root_for(Vec3::from(m.center), z_up)
}

/// The model root for a bounding-box centre (in the model's own units): scales to metres and puts the centre at the world origin.
pub fn root_for(center: Vec3, z_up: bool) -> Transform {
    let rotation = if z_up {
        Quat::from_rotation_x(-FRAC_PI_2)
    } else {
        Quat::IDENTITY
    };
    let c = center * UNIT;
    Transform {
        translation: -(rotation * c),
        rotation,
        scale: Vec3::splat(UNIT),
    }
}

pub fn to_bevy_image(p: &PoolImg, usage: RenderAssetUsages) -> Image {
    let format = match p.kind {
        Kind::Diffuse | Kind::Emissive => TextureFormat::Rgba8UnormSrgb,
        _ => TextureFormat::Rgba8Unorm,
    };
    let mut img = Image::new(
        Extent3d {
            width: p.img.w,
            height: p.img.h,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        p.img.data.clone(),
        format,
        usage,
    );
    let f = if p.filter == 0 {
        ImageFilterMode::Nearest
    } else {
        ImageFilterMode::Linear
    };
    img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        address_mode_w: ImageAddressMode::Repeat,
        mag_filter: f,
        min_filter: f,
        mipmap_filter: f,
        ..default()
    });
    img
}

/// A distinct colour per section, for the "section colours" view.
pub fn section_color(i: usize) -> Color {
    Color::hsl((i as f32 * 137.5) % 360.0, 0.65, 0.55)
}

/// 512x512 test pattern for the "UV checker" view: red grows with U, green with V, blue alternates per square.
/// Stretched or squashed areas show up as long or thin squares.
pub fn checker_image() -> Image {
    let n = 512u32;
    let mut data = Vec::with_capacity((n * n * 4) as usize);
    for y in 0..n {
        for x in 0..n {
            let odd = ((x / 32) + (y / 32)) % 2 == 1;
            data.extend_from_slice(&[
                (60 + x * 190 / n) as u8,
                (60 + y * 190 / n) as u8,
                if odd { 230 } else { 40 },
                255,
            ]);
        }
    }
    let mut img = Image::new(
        Extent3d {
            width: n,
            height: n,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        address_mode_w: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Nearest,
        min_filter: ImageFilterMode::Nearest,
        ..default()
    });
    img
}

pub struct Built {
    pub meshes: Vec<Handle<Mesh>>,
    pub mats: Vec<Handle<StandardMaterial>>,
    pub root: Entity,
    pub orig_diffuse: Vec<Option<Handle<Image>>>,
    pub orig_color: Vec<Color>,
    /// Per section: the game's specular (mean of the pbr map's G channel, 0..1), None without a pbr map.
    pub spec: Vec<Option<f32>>,
}

/// Bevy `reflectance` for the game's icon look. The game's material takes Specular from the pbr map's G channel
/// (UE default 0.5 = 4 % reflectance = Bevy 0.5, reflectance = sqrt(specular / 2)); a model without a pbr map has no
/// specular at all (no grey sheen on dark surfaces) [? the default pbr texture is not known; the user saw a grey veil here].
pub fn icon_reflectance(spec: Option<f32>) -> f32 {
    spec.map_or(0.0, |s| (s.clamp(0.0, 1.0) * 0.5).sqrt())
}

/// Mean of the specular channel (stored in R by `model.rs`) of a decoded pbr map, 0..1.
fn mean_specular(data: &[u8]) -> f32 {
    let (mut sum, mut n) = (0u64, 0u64);
    for px in data.chunks_exact(4).step_by(7) {
        sum += px[0] as u64;
        n += 1;
    }
    if n == 0 {
        0.0
    } else {
        sum as f32 / n as f32 / 255.0
    }
}

/// Spawn a model on `layer`. `full` = every map, tangents and the lamp (preview); otherwise the lean
/// icon version: diffuse only, no tangents, textures dropped from CPU memory after upload.
#[allow(clippy::too_many_arguments)]
pub fn spawn_model(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    mats: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
    l: &Loaded,
    layer: RenderLayers,
    o: &Opts,
    full: bool,
) -> Built {
    let m = &l.data;
    let usage = if full {
        RenderAssetUsages::default()
    } else {
        RenderAssetUsages::RENDER_WORLD
    };
    let handles: Vec<Handle<Image>> = l
        .pool
        .iter()
        .map(|p| {
            let mut img = to_bevy_image(p, usage);
            if !full {
                // The game's icon samples the full texture at 128 px without averaging: unfiltered, no mips.
                if let ImageSampler::Descriptor(d) = &mut img.sampler {
                    d.mag_filter = ImageFilterMode::Nearest;
                    d.min_filter = ImageFilterMode::Nearest;
                    d.mipmap_filter = ImageFilterMode::Nearest;
                }
            }
            images.add(img)
        })
        .collect();
    let strength = m.num("emissive_strength", 1.0);

    let root = commands
        .spawn((
            root_transform(m, o.z_up),
            Visibility::default(),
            layer.clone(),
        ))
        .id();
    let (mut mesh_handles, mut mat_handles) = (vec![], vec![]);
    let (mut orig_diffuse, mut orig_color) = (vec![], vec![]);
    let mut spec_per_section: Vec<Option<f32>> = vec![];

    for (i, s) in m.sections.iter().enumerate() {
        let get = |k: Kind| l.tex[i][k.idx()].map(|ix| handles[ix].clone());
        let diffuse = get(Kind::Diffuse);
        // The game's icon material also has the pbr map (metals come out dark: no reflections in the capture); the icon
        // render loads the diffuse, pbr and emissive maps (`icons.rs::ICON_KINDS`).
        let pbr = get(Kind::Pbr);
        let spec = l.tex[i][Kind::Pbr.idx()].map(|ix| mean_specular(&l.pool[ix].img.data));
        spec_per_section.push(spec);
        // The normal map is left out of the icons (128 px), the emissive map is not: a lamp has to glow in its icon.
        let normal = if full { get(Kind::Normal) } else { None };
        let emissive = get(Kind::Emissive);
        let plain = diffuse.is_none() && !full; // icon of an untextured section: neutral grey
        let has_emissive = emissive.is_some();
        let base = if plain {
            Color::srgb(0.72, 0.74, 0.78)
        } else {
            Color::WHITE
        };
        orig_diffuse.push(diffuse.clone());
        orig_color.push(base);
        let mat = mats.add(StandardMaterial {
            base_color: base,
            base_color_texture: diffuse,
            metallic: if pbr.is_some() { 1.0 } else { 0.0 },
            perceptual_roughness: if pbr.is_some() {
                1.0
            } else if full {
                0.5
            } else {
                0.85
            },
            metallic_roughness_texture: pbr,
            normal_map_texture: normal,
            flip_normal_map_y: true, // the game expects DirectX-style (green down) normal maps
            emissive: if has_emissive {
                // Alpha 0 = "do not multiply by the camera exposure" (pbr_functions.wgsl: mix(1.0, exposure, emissive.a)).
                // With alpha 1 (what `LinearRgba::rgb` gives) the default exposure (about 1/1000) made the glow invisible.
                LinearRgba::new(strength, strength, strength, 0.0)
            } else {
                LinearRgba::BLACK
            },
            emissive_texture: emissive,
            reflectance: if full { 0.5 } else { icon_reflectance(spec) }, // the icon look in the preview sets it again (preview::SectionOrig)
            alpha_mode: ALPHAS[o.alpha].1,
            double_sided: o.double_sided,
            cull_mode: if o.double_sided {
                None
            } else {
                Some(Face::Back)
            },
            ..default()
        });
        let mesh = meshes.add(build_mesh(s, o, full));
        // The game's procedural mesh has CastShadow = false.
        let e = commands
            .spawn((
                Mesh3d(mesh.clone()),
                MeshMaterial3d(mat.clone()),
                Transform::default(),
                NotShadowCaster,
                layer.clone(),
            ))
            .id();
        commands.entity(root).add_child(e);
        mesh_handles.push(mesh);
        mat_handles.push(mat);
    }

    // Lamp (setProps: only when is_lamp). The offset is in Unreal axes in the cfg (Z up): `lamp_offset_to_model` puts it in the
    // model's axes. Intensity units differ from Unreal (see LAMP_GAIN).
    // (Icons get the lamp light too, so a lamp lights its own icon like in the preview.)
    if m.num("is_lamp", 0.0) != 0.0 {
        let lamp = commands
            .spawn((
                PointLight {
                    color: m.color("lamp_color"),
                    intensity: m.num("lamp_intensity", 5001.0) * LAMP_GAIN,
                    range: m.num("lamp_attenuation", 2500.0) * UNIT,
                    shadows_enabled: m.num("lamp_shadows", 0.0) != 0.0,
                    ..default()
                },
                Transform::from_translation(lamp_offset_to_model(m.vec3("lamp_offset"), o.z_up)),
                layer.clone(),
            ))
            .id();
        commands.entity(root).add_child(lamp);
    }

    Built {
        meshes: mesh_handles,
        mats: mat_handles,
        root,
        orig_diffuse,
        orig_color,
        spec: spec_per_section,
    }
}

#[cfg(test)]
mod lamp_axis_tests {
    use super::*;

    #[test]
    fn lamp_offset_z_is_up_in_a_y_up_model() {
        // 100 cm up in Unreal (Z) is along the model's Y; Unreal Y (sideways) is the model's Z; X is unchanged.
        assert_eq!(lamp_offset_to_model(Vec3::new(0.0, 0.0, 100.0), false), Vec3::new(0.0, 100.0, 0.0));
        assert_eq!(lamp_offset_to_model(Vec3::new(0.0, 100.0, 0.0), false), Vec3::new(0.0, 0.0, 100.0));
        assert_eq!(lamp_offset_to_model(Vec3::new(100.0, 0.0, 0.0), false), Vec3::new(100.0, 0.0, 0.0));
    }

    #[test]
    fn a_z_up_model_keeps_the_offset() {
        assert_eq!(lamp_offset_to_model(Vec3::new(1.0, 2.0, 3.0), true), Vec3::new(1.0, 2.0, 3.0));
    }
}
