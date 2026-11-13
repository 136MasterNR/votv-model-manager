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
/// Lumens of the preview lamp per unit of the model's `lamp_intensity`: the physical candela-to-lumen conversion, 4 pi.
/// History: it was 200 (far too strong), then 2.0 (measured against a lamp with the default intensity 5001), which left
/// a real lamp invisible: the crystal lamp (130, attenuation 60 cm) lit its lily pad by about +0.045 linear where the game
/// shows +0.19 (lamp off (100,75,31) -> on (153,79,50), 2026-11-10). 4 pi is the first estimate that fits that model;
/// [?] a model that keeps the default intensity 5001 may now look too bright. Trim here.
pub const LAMP_GAIN: f32 = 4.0 * std::f32::consts::PI;

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
    image_from(p, p.img.data.clone(), usage)
}

/// An emissive map as the preview shows it for an emissive strength ABOVE 1.0.
///
/// The game multiplies the map by the strength. A plain multiply makes a bright colour overflow: (1, 0.2, 0.05) x 5 is
/// (5, 1, 0.25), and the display then clips (or tone-maps) every channel on its own, so the glow slides to yellow and
/// white and loses its colour. Here the strength is applied to each pixel in LINEAR colour, and a pixel whose brightest
/// channel goes over 1 is scaled back so that channel is exactly 1: the hue of every pixel is kept, dim pixels still
/// get brighter, and the brightest ones simply stay at full colour. Alpha is untouched. The result is a normal sRGB map
/// that the material then uses with a factor of 1.0.
fn boost_emissive(data: &[u8], strength: f32) -> Vec<u8> {
    let decode: [f32; 256] = std::array::from_fn(|i| {
        let c = i as f32 / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    });
    // linear 0..1 in 4096 steps -> sRGB byte (fine enough: one step is below one sRGB level even in the darks)
    let encode: Vec<u8> = (0..4096)
        .map(|i| {
            let l = i as f32 / 4095.0;
            let c = if l <= 0.003_130_8 {
                l * 12.92
            } else {
                1.055 * l.powf(1.0 / 2.4) - 0.055
            };
            (c * 255.0).round().clamp(0.0, 255.0) as u8
        })
        .collect();
    let mut out = data.to_vec();
    for px in out.chunks_exact_mut(4) {
        let mut v = [
            decode[px[0] as usize] * strength,
            decode[px[1] as usize] * strength,
            decode[px[2] as usize] * strength,
        ];
        let peak = v[0].max(v[1]).max(v[2]);
        if peak > 1.0 {
            v = v.map(|c| c / peak);
        }
        for (dst, c) in px.iter_mut().zip(v) {
            *dst = encode[(c * 4095.0).round() as usize];
        }
    }
    out
}

fn image_from(p: &PoolImg, data: Vec<u8>, usage: RenderAssetUsages) -> Image {
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
        data,
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

/// Lowest roughness the preview's world look allows (0..1). The game's world at night shows no sharp highlights on
/// the crystal lamp's vines and lily pad even where its pbr map asks for them (the red veins of the pad have a
/// roughness of about 0.2 in the map): the preview, with two strong lights and no flashlight, turned those into glossy
/// streaks. Raising the floor softens every highlight; 0 = the map as authored. Preview only (icons use the map as is).
pub const WORLD_ROUGHNESS_MIN: f32 = 0.7;

/// A texel whose metallic value (B, in Bevy's order) is at least this keeps its authored roughness in `soften_pbr`.
const METAL_KEEP: u8 = 77;

/// The pbr map (already in Bevy's order: R specular, G roughness, B metallic) with the roughness floor applied, EXCEPT
/// where the model's emissive map glows or where the texel is metal (the crystal itself): glass and metal keep their
/// authored smoothness, so they still show their reflections and the white glint like in the game, while vines and
/// leaves lose their sharp streaks. `glow` is the emissive map of the same atlas (RGBA, same size) when there is one.
fn soften_pbr(data: &[u8], glow: Option<&[u8]>) -> Vec<u8> {
    let floor = (WORLD_ROUGHNESS_MIN.clamp(0.0, 1.0) * 255.0).round() as u8;
    let glow = glow.filter(|g| g.len() == data.len());
    let mut out = data.to_vec();
    for (i, px) in out.chunks_exact_mut(4).enumerate() {
        let glows = glow.is_some_and(|g| g[i * 4].max(g[i * 4 + 1]).max(g[i * 4 + 2]) > 24);
        if !glows && px[2] < METAL_KEEP {
            px[1] = px[1].max(floor);
        }
    }
    out
}

/// Mean of the specular channel (stored in R by `model.rs`) of a decoded pbr map, 0..1, over the texels that HAVE
/// specular: matte texels (`model::MATTE_SPECULAR`, which `model::pbr_texel` already makes non-shiny) and empty atlas area
/// are left out, so a model's reflectance depends neither on how much empty space its atlas has nor on how much of it is
/// matte (the crystal lamp's vines and pad would dilute the crystal's 0.74 to 0.36). A map without any specular has none.
fn mean_specular(data: &[u8]) -> f32 {
    let (mut sum, mut n) = (0u64, 0u64);
    for px in data.chunks_exact(4).step_by(7) {
        if px[0] < MATTE_SPECULAR {
            continue;
        }
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
    let mut _bench = crate::benchmark::work("scene.spawn_model");
    _bench.detail_with(|| {
        format!(
            "{} full={full} sections={} images={}",
            l.data.name,
            l.data.sections.len(),
            l.pool.len()
        )
    });
    let m = &l.data;
    let usage = if full {
        RenderAssetUsages::default()
    } else {
        RenderAssetUsages::RENDER_WORLD
    };
    let strength = m.num("emissive_strength", 1.0);
    let _images_bench = crate::benchmark::work("scene.build_images");
    // The emissive map that goes with each pbr map (same atlas), to keep the glowing glass smooth (`soften_pbr`).
    let glow_of = |pbr_ix: usize| -> Option<usize> {
        (0..m.sections.len())
            .find(|&i| l.tex[i][Kind::Pbr.idx()] == Some(pbr_ix))
            .and_then(|i| l.tex[i][Kind::Emissive.idx()])
    };
    let handles: Vec<Handle<Image>> = l
        .pool
        .iter()
        .enumerate()
        .map(|(pi, p)| {
            // Above 1.0 the strength is baked into the emissive map with its hue kept (see `boost_emissive`).
            let mut img = if p.kind == Kind::Emissive && strength > 1.0 {
                image_from(p, boost_emissive(&p.img.data, strength), usage)
            } else if full && o.thumb_lights && p.kind == Kind::Pbr {
                let glow = glow_of(pi).map(|e| &l.pool[e].img.data[..]);
                image_from(p, soften_pbr(&p.img.data, glow), usage)
            } else {
                to_bevy_image(p, usage)
            };
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
    drop(_images_bench);

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
                // Up to 1.0 the strength is the material's factor; above it, it is already in the map (boost_emissive).
                let f = strength.min(1.0);
                LinearRgba::new(f, f, f, 0.0)
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
        let mesh = {
            let _t = crate::benchmark::sys("scene.build_mesh");
            meshes.add(build_mesh(s, o, full))
        };
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
    fn a_strong_emissive_keeps_its_hue() {
        // One orange pixel (sRGB 255, 51, 13, alpha 200) and one dim grey one (sRGB 60, alpha 255).
        let data = [255u8, 51, 13, 200, 60, 60, 60, 255];
        let out = boost_emissive(&data, 5.0);
        // The orange pixel overflows, so it is scaled back to its own colour (within rounding)...
        for (a, b) in out[..3].iter().zip(&data[..3]) {
            assert!((*a as i32 - *b as i32).abs() <= 2, "{out:?}");
        }
        // ...not clipped towards yellow/white: red stays far above green.
        assert!(out[0] > out[1] * 3);
        // The dim grey pixel gets brighter (linear x5) but stays grey.
        assert!(out[4] > 60 && out[4] == out[5] && out[5] == out[6]);
        // Alpha is untouched.
        assert_eq!((out[3], out[7]), (200, 255));
        // Strength 1 changes nothing (within rounding).
        let same = boost_emissive(&data, 1.0);
        assert!(same
            .iter()
            .zip(&data)
            .all(|(a, b)| (*a as i32 - *b as i32).abs() <= 1));
    }

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

#[cfg(test)]
mod pbr_scene_tests {
    use super::*;

    /// 14 texels, only every 7th is looked at by `mean_specular`: texel 0 has specular `first`, texel 7 has `second`.
    fn map(first: u8, second: u8) -> Vec<u8> {
        let mut d = vec![0u8; 14 * 4];
        d[0] = first;
        d[7 * 4] = second;
        d
    }

    #[test]
    fn matte_texels_are_not_part_of_the_specular_average() {
        let m = mean_specular(&map(190, 0));
        assert!((m - 190.0 / 255.0).abs() < 1e-6, "{m}");
    }

    #[test]
    fn glossy_texels_are_averaged() {
        let m = mean_specular(&map(200, 100));
        assert!((m - 150.0 / 255.0).abs() < 1e-6, "{m}");
    }

    #[test]
    fn a_map_without_any_specular_has_none() {
        assert_eq!(mean_specular(&map(0, 0)), 0.0);
        assert_eq!(mean_specular(&[]), 0.0);
    }

    // texels in Bevy's order: R specular, G roughness, B metallic, A
    #[test]
    fn the_roughness_floor_leaves_metal_and_glow_alone() {
        let data = [
            128u8, 20, 0, 255, // dielectric: gets the floor
            190, 60, 190, 255, // metal (the crystal): keeps its roughness
            128, 20, 0, 255, // dielectric where the emissive map glows: keeps its roughness
        ];
        let glow = [0u8, 0, 0, 255, 0, 0, 0, 255, 200, 10, 10, 255];
        let out = soften_pbr(&data, Some(&glow));
        let floor = (WORLD_ROUGHNESS_MIN * 255.0).round() as u8;
        assert!(out[1] >= floor - 1 && out[1] <= floor + 1, "{}", out[1]);
        assert_eq!(out[5], 60, "a metal keeps its roughness");
        assert_eq!(out[9], 20, "a glowing texel keeps its roughness");
        // nothing but the roughness changes
        assert_eq!((out[0], out[2], out[3]), (128, 0, 255));
    }
}

