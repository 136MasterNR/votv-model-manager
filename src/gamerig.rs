//! The camera and light rig the game uses to render a model's `icon.png` (3dPrinterProcessor, reverse engineered;
//! see `NOTES-FOR-AGENTS.md` and the `votv-printer-thumbnails` skill). Used by the icon generator (`icons.rs`) and by
//! the preview's "game icon look / view" options (`preview.rs`), so both show what the game shows.
//!
//! Facts used (all read from the game's Blueprint, labels from the skill): [V] verified, [I] inferred, [?] unknown.
//!
//! * R = half the diagonal of the model's bounding box, C = the box centre. Everything is placed relative to R,
//!   so the framing does not depend on the model's size [V; "half diagonal" is [I]].
//! * Camera: FOV 10 degrees, 12R from C, direction (cos30 cos45, cos30 sin45, sin30) in Unreal axes
//!   (X forward, Y right, Z up), looking at C [V].
//! * Lights (point lights, white, no distance falloff inside 4R, no shadows) at (2R, 2R, R) intensity 3 and
//!   (-2R, -2R, R) intensity 1, measured from the MESH ORIGIN, not from C [V].
//! * No sky light, ambient, reflections, shadows, AO, anti-aliasing, bloom or tone curve (the manager adds a global
//!   ambient brightness on top, `AMBIENT_FRACTION`, so unlit parts are not black); background: the Blueprint clears to 0.2 grey, but the measured icon is black (see below).
//!
//! Approximations made here:
//! * The lights are DIRECTIONAL lights pointing from the light's position to C. The game's point lights have
//!   no distance falloff, so brightness does not change along the model; a Bevy point light always falls off
//!   with the square of the distance. The only loss is that the light direction does not vary across the model.
//! * Brightness: the game's intensity I is treated as a diffuse factor I/pi (a face looking straight at the key
//!   light gets 3/pi = 0.95 of its albedo). Bevy's default exposure divides lux by about 998, so lux = I * 998.
//!   [?] This scale must be calibrated against a real icon (`LUX_PER_UNIT`).
//!
//! Measured against a real game icon (the chicken, `game-generated-icon.png`, compared with a software render of the
//! same model from every one of the 48 signed axis permutations): [V by measurement]
//! * Axis mapping: Unreal (x, y, z) = OBJ (z, x, y). The silhouette matched with IoU 0.975; the next best
//!   permutation reached 0.72. (OBJ (x, z, y) draws exactly the same picture: the rig is symmetric under swapping
//!   Unreal x and y, so a mirror across that plane cannot be seen. Only a model with a different rig would tell.)
//! * Background: pure black (0, 0, 0, 255) in the game, not the 0.2 grey of the clear colour. The manager's icons and
//!   its preview "icon look" use a transparent background instead (user decision, 2026-11-01).
//! * Output is sRGB-encoded (an sRGB fit gave RMSE 0.053, a linear one 0.148).
//! * Brightness: the key light adds 0.342 of albedo per unit of intensity (about 1.07 / pi), which is Bevy
//!   `LUX_PER_UNIT` = 0.342 * pi * 998. The fill light barely shows in that view, so the 3 : 1 ratio is unchecked.
//! * Textures: the game's icon is sharper (the full texture is sampled at 128 px, no mip averaging).

use bevy::prelude::*;

use crate::scene::{root_for, UNIT};

pub const FOV_DEG: f32 = 10.0;
pub const CAMERA_DISTANCE_R: f32 = 12.0;
pub const CAMERA_PITCH_DEG: f32 = 30.0;
pub const CAMERA_YAW_DEG: f32 = 45.0;
pub const KEY_INTENSITY: f32 = 3.0;
pub const FILL_INTENSITY: f32 = 1.0;
/// How many degrees both lights are raised above the positions the Blueprint gives them (`Rig::lights`). 0 is exactly
/// the game's numbers. The user compared renders with the game and found that 12 degrees higher looks more accurate; it
/// applies to the generated icons and to the preview alike. Positive = higher, negative = lower (never above 80).
pub const LIGHT_RAISE_DEG: f32 = 12.0;
/// Bevy lux that give the same diffuse brightness as one unit of Unreal light intensity (see the module notes).
pub const LUX_PER_UNIT: f32 = 1072.0;
/// Bevy's default exposure (ev100 9.7) divides every light value by about 998 before it reaches the screen.
pub const EXPOSURE_DIVISOR: f32 = 998.0;
/// Global brightness of the whole scene, in the preview and in every generated icon: the fraction of its own colour
/// that a surface shows when no light reaches it (33%: a shadowed part is dark, not black). The game's own icon rig has
/// no ambient light at all (measured), so this is a deliberate departure from it, decided by the user on 2026-11-06;
/// set it to 0.0 for the game's exact look.
pub const AMBIENT_FRACTION: f32 = 0.33;
/// The same thing as a Bevy `AmbientLight::brightness`: Bevy's ambient term is albedo x brightness x exposure, so the
/// brightness that shows `AMBIENT_FRACTION` of the albedo is the fraction times the exposure divisor (about 329).
pub const AMBIENT_BRIGHTNESS: f32 = AMBIENT_FRACTION * EXPOSURE_DIVISOR;
/// The icon's background. The game's icon.png is pure black (measured; the Blueprint says 0.2 grey), but the manager
/// leaves it transparent on the user's request: black or not, nothing is drawn behind the model.
pub const BACKGROUND: Color = Color::NONE;

pub fn fov_rad() -> f32 {
    FOV_DEG.to_radians()
}

pub fn key_lux() -> f32 {
    KEY_INTENSITY * LUX_PER_UNIT
}

pub fn fill_lux() -> f32 {
    FILL_INTENSITY * LUX_PER_UNIT
}

/// An Unreal-space direction or offset expressed in the model's own axes (the axes of the OBJ as the manager shows
/// them). Measured: Unreal (x, y, z) = OBJ (z, x, y), so OBJ (x, y, z) = Unreal (y, z, x). `z_up` means the OBJ is
/// already in Unreal axes (a guess; nothing was measured for that case).
pub fn ue_to_model(v: Vec3, z_up: bool) -> Vec3 {
    if z_up {
        v
    } else {
        Vec3::new(v.y, v.z, v.x)
    }
}

/// The rig for one model, in the manager's world space (metres, the model's bounding-box centre at the origin).
#[derive(Clone, Copy, Debug)]
pub struct Rig {
    /// R in metres.
    pub radius: f32,
    /// Unit vector from the centre towards the camera.
    pub view_dir: Vec3,
    /// 12R.
    pub distance: f32,
    /// Direction the key / fill light travels (from the light towards the centre).
    pub key_travel: Vec3,
    pub fill_travel: Vec3,
}

impl Rig {
    /// `center_cm` and `radius_cm`: the model's bounding-box centre and R in the model's own units (centimetres).
    pub fn new(center_cm: Vec3, radius_cm: f32, z_up: bool) -> Rig {
        let r = (radius_cm * UNIT).max(0.001);
        let root = root_for(center_cm, z_up);
        let world = |ue: Vec3| root.rotation * ue_to_model(ue, z_up);
        let (p, y) = (CAMERA_PITCH_DEG.to_radians(), CAMERA_YAW_DEG.to_radians());
        let forward = Vec3::new(p.cos() * y.cos(), p.cos() * y.sin(), p.sin());
        let view_dir = world(forward).normalize();
        // Light positions are measured from the mesh origin, which is at `root.translation`; the centre is the origin.
        let key_pos = root.translation + world(Vec3::new(2.0, 2.0, 1.0)) * r;
        let fill_pos = root.translation + world(Vec3::new(-2.0, -2.0, 1.0)) * r;
        let toward_centre =
            |from: Vec3, fallback: Vec3| (-from).try_normalize().unwrap_or(fallback);
        Rig {
            radius: r,
            view_dir,
            distance: CAMERA_DISTANCE_R * r,
            key_travel: toward_centre(key_pos, -view_dir),
            fill_travel: toward_centre(fill_pos, view_dir),
        }
    }

    /// The directions the key and the fill light shine, with `LIGHT_RAISE_DEG` applied. The icon render and the preview
    /// use these (the raw game numbers stay in `key_travel` / `fill_travel`).
    pub fn lights(&self) -> (Vec3, Vec3) {
        (raise_light(self.key_travel, LIGHT_RAISE_DEG), raise_light(self.fill_travel, LIGHT_RAISE_DEG))
    }

    pub fn camera_pos(&self) -> Vec3 {
        self.view_dir * self.distance
    }

    /// Near and far planes that keep depth precision for a camera 12R away.
    pub fn clip_planes(&self) -> (f32, f32) {
        (self.radius, self.radius * 30.0)
    }

    /// The camera transform: at `camera_pos`, looking at the centre, up = world +Y (the model's up in both axis modes).
    pub fn camera_transform(&self) -> Transform {
        Transform::from_translation(self.camera_pos()).looking_at(Vec3::ZERO, Vec3::Y)
    }

    /// (yaw, pitch) of the manager's orbit camera (`preview::Orbit`) that looks along this rig's view.
    pub fn orbit_angles(&self) -> (f32, f32) {
        orbit_angles(self.view_dir)
    }
}

/// Yaw and pitch (radians) for `preview::Orbit` so that the camera sits in direction `d` (unit) from its target.
/// The orbit camera's position is `Quat::from_euler(YXZ, yaw, -pitch, 0) * (0, 0, dist)`.
pub fn orbit_angles(d: Vec3) -> (f32, f32) {
    (d.x.atan2(d.z), d.y.clamp(-1.0, 1.0).asin())
}

/// `travel` (the direction a light shines) with the light moved `deg` degrees higher on its circle around the centre:
/// its elevation grows (never beyond 80 degrees), its direction around the vertical axis stays.
pub fn raise_light(travel: Vec3, deg: f32) -> Vec3 {
    let to_light = -travel;
    let flat = Vec3::new(to_light.x, 0.0, to_light.z);
    if flat.length_squared() < 1.0e-6 {
        return travel; // straight above or below: nothing to raise
    }
    let elevation = (to_light.y.clamp(-1.0, 1.0).asin() + deg.to_radians()).min(80.0_f32.to_radians());
    -(flat.normalize() * elevation.cos() + Vec3::Y * elevation.sin())
}

/// A rotation whose forward (-Z) is `travel`, for a directional light.
pub fn light_rotation(travel: Vec3) -> Quat {
    let up = if travel.dot(Vec3::Y).abs() > 0.99 {
        Vec3::Z
    } else {
        Vec3::Y
    };
    Transform::IDENTITY.looking_to(travel, up).rotation
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec3, b: Vec3) -> bool {
        a.distance(b) < 1.0e-4
    }

    #[test]
    fn ambient_brightness_shows_the_chosen_fraction_of_the_albedo() {
        // Bevy: radiance = albedo x brightness x exposure, and the exposure is 1 / EXPOSURE_DIVISOR.
        let shown = AMBIENT_BRIGHTNESS / EXPOSURE_DIVISOR;
        assert!((shown - AMBIENT_FRACTION).abs() < 1.0e-4);
        assert!((AMBIENT_BRIGHTNESS - 329.34).abs() < 0.01);
    }

    #[test]
    fn camera_direction_matches_the_skill_numbers() {
        // Unreal (0.61237, 0.61237, 0.5) becomes (y, z, x) = (0.61237, 0.5, 0.61237) in the manager's axes.
        let rig = Rig::new(Vec3::new(10.0, 20.0, 30.0), 86.603, false);
        assert!(close(rig.view_dir, Vec3::new(0.61237, 0.5, 0.61237)));
        assert!((rig.distance - 12.0 * 0.86603).abs() < 1.0e-3);
        assert!((rig.radius - 0.86603).abs() < 1.0e-4);
    }

    #[test]
    fn z_up_models_use_unreal_axes_directly() {
        let rig = Rig::new(Vec3::ZERO, 100.0, true);
        // Unreal (x, y, z) -> world (x, z, -y) through the Z-up root rotation.
        assert!(close(rig.view_dir, Vec3::new(0.61237, 0.5, -0.61237)));
    }

    #[test]
    fn lights_measure_from_the_mesh_origin_not_from_the_centre() {
        // A model whose centre is 1 m above its origin: the key light sits at origin + (2R, R, 2R) in world space.
        let rig = Rig::new(Vec3::new(0.0, 100.0, 0.0), 100.0, false);
        let origin = Vec3::new(0.0, -1.0, 0.0); // the mesh origin in world space (centre at 0)
        let key_pos = origin + Vec3::new(2.0, 1.0, 2.0); // R = 1 m
        assert!(close(rig.key_travel, (-key_pos).normalize()));
        // For a centred model the key light travels along -(2, 1, 2)/3.
        let centred = Rig::new(Vec3::ZERO, 100.0, false);
        assert!(close(centred.key_travel, Vec3::new(-2.0, -1.0, -2.0) / 3.0));
        assert!(close(centred.fill_travel, Vec3::new(2.0, -1.0, 2.0) / 3.0));
    }

    #[test]
    fn raising_a_light_lifts_it_and_keeps_its_direction_around_the_vertical_axis() {
        let travel = Vec3::new(-2.0, -1.0, -2.0).normalize(); // the game's key light, shining down towards the centre
        let raised = raise_light(travel, 12.0);
        let (from, to) = (-travel, -raised);
        assert!(to.y > from.y, "the light must be higher");
        let (a, b) = (Vec2::new(from.x, from.z).normalize(), Vec2::new(to.x, to.z).normalize());
        assert!((a - b).length() < 1.0e-4, "the direction around the vertical axis must not change");
        assert!((raised.length() - 1.0).abs() < 1.0e-4);
        assert!((raise_light(travel, 0.0) - travel).length() < 1.0e-4, "0 degrees is the game's position");
        // The rig's lights are the game's lights raised by LIGHT_RAISE_DEG.
        let rig = Rig::new(Vec3::ZERO, 100.0, false);
        let (key, _) = rig.lights();
        let lift = (-key).y.asin() - (-rig.key_travel).y.asin();
        assert!((lift - LIGHT_RAISE_DEG.to_radians()).abs() < 1.0e-3);
    }

    #[test]
    fn key_to_fill_ratio_is_three_to_one() {
        assert!((key_lux() / fill_lux() - 3.0).abs() < 1.0e-6);
    }

    #[test]
    fn orbit_angles_reproduce_the_view_direction() {
        let rig = Rig::new(Vec3::ZERO, 50.0, false);
        let (yaw, pitch) = rig.orbit_angles();
        let from_orbit = Quat::from_euler(EulerRot::YXZ, yaw, -pitch, 0.0) * Vec3::Z;
        assert!(close(from_orbit, rig.view_dir));
    }

    #[test]
    fn sphere_of_radius_r_fills_about_95_percent_of_the_frame() {
        // Half-width of the view at the centre is D * tan(FOV/2) = 12R * tan(5 deg) = 1.0499R.
        let half = CAMERA_DISTANCE_R * (fov_rad() / 2.0).tan();
        assert!((half - 1.0499).abs() < 1.0e-3);
    }

    #[test]
    fn light_rotation_points_forward_along_travel() {
        let t = Vec3::new(-2.0, -1.0, 2.0).normalize();
        assert!(close(light_rotation(t) * Vec3::NEG_Z, t));
        let down = Vec3::NEG_Y;
        assert!(close(light_rotation(down) * Vec3::NEG_Z, down));
    }
}
