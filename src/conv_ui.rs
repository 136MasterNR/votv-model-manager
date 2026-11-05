//! The converter screen: the preview fills the window except for the properties panel, a column of its own at the
//! right (Godot / Unity style, not floating over the preview). A small progress bar at the bottom of the preview shows
//! the generation steps (hidden when nothing is happening) and a logs button sits at its bottom left.
//!
//! Properties panel, top to bottom: the model's icon with a "Regenerate icon" button, the model name and a box of
//! technical info, then the "Properties" list: collapsible sections (click a header) of parameters.
//! The title bar swaps the Settings button for the Return button (the X); its toolbar stays empty for now.
//!
//! The parameter list is table driven: `rows()` lists the headers and parameters as plain data (label, how to read and
//! write the `votv_convert::Options` field, when to show it). Adding a parameter means adding one row.
//! Every edit goes through `ConvertState::changed`, which cancels the running job and queues the next one;
//! toggles and choices queue with delay 0, sliders with `SLIDER_DELAY` after release.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use bevy::{
    asset::RenderAssetUsages,
    core_pipeline::tonemapping::Tonemapping,
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
    text::{JustifyText, LineBreak},
    ui::OverflowAxis,
    window::{FileDragAndDrop, PrimaryWindow, RequestRedraw, SystemCursorIcon},
};
use votv_convert::model_core::PropValue;
use votv_convert::{CollisionQuality, Options, Severity};

use crate::converter::{find_glb, ConvertState, Status, SLIDER_DELAY};
use crate::icon_font::{g, icon, Glyph, IconFont};
use crate::icons::{self, IconPipe, IconView};
use crate::preview::{PreviewCamera, StudioLight, ThumbLight};
use crate::scene::Opts;
use crate::textedit::{field_drag, field_out, field_over, press, shift_down, Editing, Field, Geom};
use crate::ui::{
    button, child, pass, pointer_cursor, scroll_metrics, set_cursor, thumb_height, txt, ItemButton, ACCENT,
    BAR, BTN, DIM, EDGE, EDGE_HOVER, EXPORT_COL, ICON_BG, ITEM, PANEL, TEXT,
};
use crate::{Layout, Library};

// ---- the parameter table ---------------------------------------------------------------------------

pub struct ParamDef {
    pub label: &'static str,
    pub kind: ParamKind,
    /// Whether the row applies (e.g. VHACD settings while collision generation is off). When it does not, the row stays
    /// visible, nested under the option that controls it, and is grayed out and blocked by an overlay.
    pub show: fn(&Options) -> bool,
    /// Nesting under the option that controls the row (`rows()` sets it for every row whose `show` is not `always`).
    pub depth: u8,
}

pub enum ParamKind {
    Toggle {
        get: fn(&Options) -> bool,
        set: fn(&mut Options, bool),
    },
    Slider {
        min: f64,
        max: f64,
        step: f64,
        decimals: usize,
        unit: &'static str,
        get: fn(&Options) -> f64,
        set: fn(&mut Options, f64),
    },
    /// Click = next value, right click = previous.
    Choice {
        labels: &'static [&'static str],
        get: fn(&Options) -> usize,
        set: fn(&mut Options, usize),
    },
}

pub enum Row {
    Header(&'static str),
    Param(ParamDef),
}

#[derive(Resource)]
pub struct ParamRows(pub Vec<Row>);

fn always(_: &Options) -> bool {
    true
}
fn collision_on(o: &Options) -> bool {
    o.generate_collision
}
fn not_indestructible(o: &Options) -> bool {
    !o.properties.indestructible
}
fn vhacd_on(o: &Options) -> bool {
    o.generate_collision && o.collision_quality == CollisionQuality::High
}

const TILE_SIZES: [u32; 5] = [256, 512, 1024, 2048, 4096];
const TILE_LABELS: [&str; 5] = ["256 px", "512 px", "1024 px", "2048 px", "4096 px"];
const ATLAS_SIZES: [u32; 4] = [1024, 2048, 4096, 8192];
const ATLAS_LABELS: [&str; 4] = ["1024 px", "2048 px", "4096 px", "8192 px"];
const QUALITY_LABELS: [&str; 2] = ["Low (1 hull)", "High (VHACD)"];

fn toggle(
    label: &'static str,
    get: fn(&Options) -> bool,
    set: fn(&mut Options, bool),
    show: fn(&Options) -> bool,
) -> Row {
    Row::Param(ParamDef {
        label,
        kind: ParamKind::Toggle { get, set },
        show,
        depth: 0,
    })
}
#[allow(clippy::too_many_arguments)]
fn slider(
    label: &'static str,
    (min, max, step): (f64, f64, f64),
    (decimals, unit): (usize, &'static str),
    get: fn(&Options) -> f64,
    set: fn(&mut Options, f64),
    show: fn(&Options) -> bool,
) -> Row {
    Row::Param(ParamDef {
        label,
        kind: ParamKind::Slider {
            min,
            max,
            step,
            decimals,
            unit,
            get,
            set,
        },
        show,
        depth: 0,
    })
}
fn choice(
    label: &'static str,
    labels: &'static [&'static str],
    get: fn(&Options) -> usize,
    set: fn(&mut Options, usize),
    show: fn(&Options) -> bool,
) -> Row {
    Row::Param(ParamDef {
        label,
        kind: ParamKind::Choice { labels, get, set },
        show,
        depth: 0,
    })
}

/// First batch of parameters. Still to add (next steps): lamp settings, texture filters, health / resistances,
/// icon and sidecar output, output name and destination.
const FILTER_LABELS: [&str; 4] = ["Nearest", "Bilinear", "Trilinear", "Default"];

fn lamp_on(o: &Options) -> bool {
    o.properties
        .get("is_lamp")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

// Rows for properties.cfg keys through the generic `PropertiesCfg::get` / `set` (literal key, no field names).
macro_rules! pslider {
    ($label:expr, $range:expr, $fmt:expr, $key:literal, $show:expr) => {
        slider(
            $label,
            $range,
            $fmt,
            |o| {
                o.properties
                    .get($key)
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0)
            },
            |o, v| {
                let _ = o.properties.set($key, PropValue::Float(v));
            },
            $show,
        )
    };
}
macro_rules! ptoggle {
    ($label:expr, $key:literal, $show:expr) => {
        toggle(
            $label,
            |o| {
                o.properties
                    .get($key)
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
            },
            |o, v| {
                let _ = o.properties.set($key, PropValue::Bool(v));
            },
            $show,
        )
    };
}
macro_rules! pchoice {
    ($label:expr, $labels:expr, $key:literal, $show:expr) => {
        choice(
            $label,
            $labels,
            |o| {
                o.properties
                    .get($key)
                    .and_then(|v| v.as_int())
                    .unwrap_or(0)
                    .max(0) as usize
            },
            |o, i| {
                let _ = o.properties.set($key, PropValue::Int(i as i64));
            },
            $show,
        )
    };
}
/// One component (r/g/b or x/y/z) of a colour / vector property as a slider.
macro_rules! pcomp {
    ($label:expr, $key:literal, $var:ident, $f:ident, $range:expr, $fmt:expr, $show:expr) => {
        slider(
            $label,
            $range,
            $fmt,
            |o| match o.properties.get($key) {
                Some(PropValue::$var(c)) => c.$f,
                _ => 0.0,
            },
            |o, v| {
                if let Some(PropValue::$var(mut c)) = o.properties.get($key) {
                    c.$f = v;
                    let _ = o.properties.set($key, PropValue::$var(c));
                }
            },
            $show,
        )
    };
}

fn addr(f: fn(&Options) -> bool) -> usize {
    f as usize
}

/// The parameter table with the nesting filled in: every row that only applies under some condition is a sub-row of
/// the option right above it (the toggle or choice that decides), VHACD settings one level deeper (under Quality).
pub fn rows() -> Vec<Row> {
    let mut v = rows_flat();
    for row in v.iter_mut() {
        if let Row::Param(d) = row {
            if addr(d.show) != addr(always) {
                d.depth = if addr(d.show) == addr(vhacd_on) { 2 } else { 1 };
            }
        }
    }
    v
}

fn rows_flat() -> Vec<Row> {
    use votv_convert::model_core::PHYSICAL_MATERIALS;
    vec![
        Row::Header("MODEL"),
        slider(
            "Scale",
            (1.0, 400.0, 1.0),
            (0, ""),
            |o| o.scale,
            |o, v| o.scale = v,
            always,
        ),
        toggle(
            "Apply skeleton",
            |o| o.humanoid,
            |o, v| o.humanoid = v,
            always,
        ),
        toggle(
            "Drop transparent parts",
            |o| o.drop_transparent,
            |o, v| o.drop_transparent = v,
            always,
        ),
        Row::Header("TEXTURES"),
        choice(
            "Max tile size",
            &TILE_LABELS,
            |o| {
                TILE_SIZES
                    .iter()
                    .position(|&v| v == o.max_tile_size)
                    .unwrap_or(2)
            },
            |o, i| o.max_tile_size = TILE_SIZES[i.min(TILE_SIZES.len() - 1)],
            always,
        ),
        choice(
            "Max atlas size",
            &ATLAS_LABELS,
            |o| {
                ATLAS_SIZES
                    .iter()
                    .position(|&v| v == o.max_atlas_size)
                    .unwrap_or(2)
            },
            |o, i| o.max_atlas_size = ATLAS_SIZES[i.min(ATLAS_SIZES.len() - 1)],
            always,
        ),
        toggle(
            "Apply base colour factor",
            |o| o.apply_base_color_factor,
            |o, v| o.apply_base_color_factor = v,
            always,
        ),
        toggle(
            "Flip normal green",
            |o| o.flip_normal_green,
            |o, v| o.flip_normal_green = v,
            always,
        ),
        toggle(
            "Bake emissive map",
            |o| o.emissive,
            |o, v| o.emissive = v,
            always,
        ),
        slider(
            "Emissive strength",
            (0.0, 50.0, 0.5),
            (1, ""),
            |o| o.properties.emissive_strength,
            |o, v| o.properties.emissive_strength = v,
            |o| o.emissive,
        ),
        Row::Header("COLLISION"),
        toggle(
            "Generate collision",
            |o| o.generate_collision,
            |o, v| o.generate_collision = v,
            always,
        ),
        toggle(
            "Ignore UCX_",
            |o| o.force_generate_collision,
            |o, v| o.force_generate_collision = v,
            collision_on,
        ),
        choice(
            "Quality",
            &QUALITY_LABELS,
            |o| (o.collision_quality == CollisionQuality::High) as usize,
            |o, i| {
                o.collision_quality = if i == 0 {
                    CollisionQuality::Low
                } else {
                    CollisionQuality::High
                }
            },
            collision_on,
        ),
        slider(
            "VHACD resolution",
            (8.0, 256.0, 8.0),
            (0, ""),
            |o| o.vhacd_resolution as f64,
            |o, v| o.vhacd_resolution = v as u32,
            vhacd_on,
        ),
        slider(
            "Max hulls",
            (1.0, 128.0, 1.0),
            (0, ""),
            |o| o.max_decomp_hulls as f64,
            |o, v| o.max_decomp_hulls = v as u32,
            vhacd_on,
        ),
        slider(
            "Fallback box size",
            (5.0, 300.0, 1.0),
            (0, " cm"),
            |o| o.fallback_collision_size,
            |o, v| o.fallback_collision_size = v,
            always,
        ),
        slider(
            "Offset X",
            (-100.0, 100.0, 1.0),
            (0, " cm"),
            |o| o.collision_offset_x,
            |o, v| o.collision_offset_x = v,
            always,
        ),
        slider(
            "Offset Y",
            (-100.0, 100.0, 1.0),
            (0, " cm"),
            |o| o.collision_offset_y,
            |o, v| o.collision_offset_y = v,
            always,
        ),
        toggle(
            "Show collision (debug)",
            |o| o.show_collision,
            |o, v| o.show_collision = v,
            always,
        ),
        // The model's properties.cfg values, in the sections of `votv_model_core::PROPERTY_SPECS` (keys are literal so the
        // rows work through the generic `PropertiesCfg::get` / `set`). Edible is physical material 8 (Food).
        Row::Header("DURABILITY"),
        toggle(
            "Indestructible",
            |o| o.properties.indestructible,
            |o, v| o.properties.indestructible = v,
            always,
        ),
        slider(
            "Health (0 = auto)",
            (0.0, 2000.0, 10.0),
            (0, ""),
            |o| o.properties.health,
            |o, v| o.properties.health = v,
            not_indestructible,
        ),
        pslider!(
            "Impact resistance",
            (0.0, 100.0, 0.1),
            (1, ""),
            "impact_resistance",
            not_indestructible
        ),
        pslider!(
            "Damage resistance",
            (0.0, 100.0, 0.1),
            (1, ""),
            "damage_resistance",
            not_indestructible
        ),
        Row::Header("MATERIAL"),
        choice(
            "Physical material",
            &PHYSICAL_MATERIALS,
            |o| (o.properties.physical_material as usize).min(15),
            |o, i| o.properties.physical_material = i as u8,
            always,
        ),
        Row::Header("LIGHT"),
        ptoggle!("Lamp", "is_lamp", always),
        pslider!(
            "Intensity",
            (0.0, 50000.0, 100.0),
            (0, ""),
            "lamp_intensity",
            lamp_on
        ),
        pslider!(
            "Radius",
            (0.0, 10000.0, 50.0),
            (0, ""),
            "lamp_attenuation",
            lamp_on
        ),
        ptoggle!("Casts shadows", "lamp_shadows", lamp_on),
        ptoggle!("Switchable by the player", "light_toggle", lamp_on),
        pcomp!(
            "Colour red",
            "lamp_color",
            Color,
            r,
            (0.0, 1.0, 0.01),
            (2, ""),
            lamp_on
        ),
        pcomp!(
            "Colour green",
            "lamp_color",
            Color,
            g,
            (0.0, 1.0, 0.01),
            (2, ""),
            lamp_on
        ),
        pcomp!(
            "Colour blue",
            "lamp_color",
            Color,
            b,
            (0.0, 1.0, 0.01),
            (2, ""),
            lamp_on
        ),
        pcomp!(
            "Offset X",
            "lamp_offset",
            Vec3,
            x,
            (-500.0, 500.0, 1.0),
            (0, " cm"),
            lamp_on
        ),
        pcomp!(
            "Offset Y",
            "lamp_offset",
            Vec3,
            y,
            (-500.0, 500.0, 1.0),
            (0, " cm"),
            lamp_on
        ),
        pcomp!(
            "Offset Z",
            "lamp_offset",
            Vec3,
            z,
            (-500.0, 500.0, 1.0),
            (0, " cm"),
            lamp_on
        ),
        Row::Header("TEXTURE FILTERS"),
        pchoice!("Diffuse", &FILTER_LABELS, "filter_diffuse_", always),
        pchoice!("PBR", &FILTER_LABELS, "filter_pbr_", always),
        pchoice!("Normal", &FILTER_LABELS, "filter_normal_", always),
        pchoice!("Emissive", &FILTER_LABELS, "filter_emissive_", always),
    ]
}

// ---- references to the spawned widgets ----------------------------------------------------------------

#[derive(Clone, Copy)]
pub struct RowRefs {
    pub node: Entity,
    pub value: Option<Entity>, // value text (sliders, choices)
    pub fill: Option<Entity>,  // slider fill
    pub mark: Option<Entity>,  // toggle box
    pub list: Option<Entity>,  // dropdown option list (choices)
    pub chev: Option<Entity>,  // dropdown chevron (choices) / section chevron (headers)
    /// The section (index of its header) the row belongs to; a header has its own index.
    pub section: Option<usize>,
    /// Dims and blocks a nested row while its condition is off (only rows with depth > 0).
    pub overlay: Option<Entity>,
    /// A slider's track and its thumb (`slider_visuals_system`).
    pub track: Option<Entity>,
    pub thumb: Option<Entity>,
    /// The "reset to default" button of a parameter row (shown only while the value differs from the default).
    pub reset: Option<Entity>,
}

/// Room kept free at the right end of every parameter row for the reset button (so nothing moves when it appears).
const RESET_GUTTER: f32 = 30.0;

/// The "reset to default" button of a parameter row: just a floating circular-arrow icon (no background) in the right
/// gutter of the row, so it takes no room in the row's layout. It is hidden while the value is the default
/// (`reset_button_system`). A click puts the default value back and must not reach the row (a toggle would flip, a
/// dropdown would open). `top` is the offset from the row's top (rows differ in height).
fn spawn_reset(commands: &mut Commands, parent: Entity, font: &IconFont, i: usize, top: f32) -> Entity {
    let b = child(
        commands,
        parent,
        (
            Interaction::default(),
            Node {
                position_type: PositionType::Absolute,
                right: Val::Px(5.0),
                top: Val::Px(top),
                width: Val::Px(22.0),
                height: Val::Px(22.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            Visibility::Hidden,
        ),
    );
    let glyph = child(commands, b, (icon(font, g::RESTART_ALT, 18.0, ACCENT), Pickable::IGNORE));
    // The icon brightens under the pointer (there is no background to show the hover).
    commands.entity(b).observe(move |_: Trigger<Pointer<Over>>, mut c: Query<&mut TextColor>| {
        if let Ok(mut c) = c.get_mut(glyph) {
            c.0 = TEXT;
        }
    });
    commands.entity(b).observe(move |_: Trigger<Pointer<Out>>, mut c: Query<&mut TextColor>| {
        if let Ok(mut c) = c.get_mut(glyph) {
            c.0 = ACCENT;
        }
    });
    crate::ui::pointer_cursor(commands, b);
    commands.entity(b).observe(move |mut t: Trigger<Pointer<Click>>, rows: Res<ParamRows>, mut st: ResMut<ConvertState>| {
        t.propagate(false);
        if t.event().button == PointerButton::Primary {
            reset_row(&rows.0[i], &mut st);
        }
    });
    commands.entity(b).observe(|mut t: Trigger<Pointer<Pressed>>| t.propagate(false));
    b
}

/// Puts one parameter back to the converter's default (`Options::default()` as the editor starts from it).
fn reset_row(row: &Row, st: &mut ConvertState) {
    let Row::Param(def) = row else { return };
    let d = crate::converter::base_options();
    match &def.kind {
        ParamKind::Toggle { get, set } => set(&mut st.opts, get(&d)),
        ParamKind::Slider { get, set, .. } => set(&mut st.opts, get(&d)),
        ParamKind::Choice { get, set, .. } => set(&mut st.opts, get(&d)),
    }
    st.changed(0.0);
}

fn is_default(def: &ParamDef, cur: &Options, d: &Options) -> bool {
    match &def.kind {
        ParamKind::Toggle { get, .. } => get(cur) == get(d),
        ParamKind::Slider { get, .. } => (get(cur) - get(d)).abs() < 1e-6,
        ParamKind::Choice { get, .. } => get(cur) == get(d),
    }
}

/// Shows the reset button of exactly the rows whose value is not the default.
pub fn reset_button_system(
    st: Res<ConvertState>,
    rows: Res<ParamRows>,
    refs: Res<ConvUiRefs>,
    mut vis: Query<&mut Visibility>,
    mut defaults: Local<Option<Options>>,
) {
    if !st.is_changed() && !refs.is_changed() {
        return;
    }
    let d = defaults.get_or_insert_with(crate::converter::base_options);
    for (r, row) in refs.rows.iter().zip(rows.0.iter()) {
        let (Some(e), Row::Param(def)) = (r.reset, row) else {
            continue;
        };
        let want = if is_default(def, &st.opts, d) {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };
        if let Ok(mut v) = vis.get_mut(e) {
            if *v != want {
                *v = want;
            }
        }
    }
}

/// Marks the graying overlay of a row, so the mouse wheel over it still scrolls the properties list (`ui::wheel_system`).
#[derive(Component)]
pub struct RowOverlay;

/// Current width of the properties panel (changed with the splitter at its left edge).
#[derive(Resource)]
pub struct PropsWidth(pub f32);
impl Default for PropsWidth {
    fn default() -> Self {
        Self(PROPS_W)
    }
}

/// Which property sections are open (indexed like the `Row::Header`s; a missing entry = CLOSED, so all start folded).
#[derive(Resource, Default)]
pub struct OpenSections(pub Vec<bool>);

/// Index (among the section headers) of the COLLISION section, set by `build`; opening it shows the hull outlines.
#[derive(Resource, Default)]
pub struct CollisionSection(pub Option<usize>);

/// Index (among the section headers) of the MODEL section (it holds the Scale slider), set by `build`; opening it switches
/// the preview's printer volumes on (`Opts::printer_limits`), so the model can be sized against the game printers.
#[derive(Resource, Default)]
pub struct ModelSection(pub Option<usize>);

/// Index (among the section headers) of the LIGHT section, set by `build`; opening it switches the lamp marker on
/// (`Opts::show_lamp`: where the lamp is, drawn in the preview).
#[derive(Resource, Default)]
pub struct LightSection(pub Option<usize>);

/// The light bulb at the end of the LIGHT header: shown only while that section is open, the marker is on AND the model
/// is a lamp (there is nothing to show for a model that is not one). Clicking it switches the marker off.
#[derive(Component)]
pub struct LightIndicator;

pub fn light_indicator_system(
    sections: Res<OpenSections>,
    light: Res<LightSection>,
    opts: Res<Opts>,
    st: Res<ConvertState>,
    mut q: Query<&mut Node, With<LightIndicator>>,
) {
    let open = light
        .0
        .is_some_and(|l| sections.0.get(l).copied().unwrap_or(false));
    let want = if open && opts.show_lamp && st.opts.properties.is_lamp {
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

/// The "fullscreen" icon at the end of the MODEL header: shown only while that section is open AND the printer volumes
/// are on, so it tells the user why they appeared. Clicking it switches them off.
#[derive(Component)]
pub struct LimitsIndicator;

pub fn limits_indicator_system(
    sections: Res<OpenSections>,
    model: Res<ModelSection>,
    opts: Res<Opts>,
    mut q: Query<&mut Node, With<LimitsIndicator>>,
) {
    let open = model
        .0
        .is_some_and(|m| sections.0.get(m).copied().unwrap_or(false));
    let want = if open && opts.printer_limits {
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

/// The green "visible" eye at the end of the COLLISION header: shown only while that section is open AND the preview's
/// hull outlines are on (opening the section switches them on, so it tells the user why the hulls appeared).
#[derive(Component)]
pub struct HullIndicator;

pub fn hull_indicator_system(
    sections: Res<OpenSections>,
    collision: Res<CollisionSection>,
    opts: Res<Opts>,
    mut q: Query<&mut Node, With<HullIndicator>>,
) {
    let open = collision
        .0
        .is_some_and(|c| sections.0.get(c).copied().unwrap_or(false));
    let want = if open && opts.show_hulls {
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

/// Leaving the editor folds every section again (and, when COLLISION was open, switches the hull outlines off; when MODEL
/// was open, the printer volumes).
pub fn reset_sections_system(
    layout: Res<Layout>,
    collision: Res<CollisionSection>,
    model: Res<ModelSection>,
    light: Res<LightSection>,
    mut sections: ResMut<OpenSections>,
    mut opts: ResMut<Opts>,
    mut was_editor: Local<bool>,
) {
    if *was_editor && !layout.converter {
        let collision_open = collision
            .0
            .is_some_and(|c| sections.0.get(c).copied().unwrap_or(false));
        let model_open = model
            .0
            .is_some_and(|m| sections.0.get(m).copied().unwrap_or(false));
        if model_open {
            opts.printer_limits = false;
        }
        if light
            .0
            .is_some_and(|l| sections.0.get(l).copied().unwrap_or(false))
        {
            opts.show_lamp = false;
        }
        if sections.0.iter().any(|b| *b) {
            sections.0.clear();
        }
        if collision_open {
            opts.show_hulls = false;
        }
    }
    *was_editor = layout.converter;
}

/// Which dropdown row (index into the table) is open; at most one.
#[derive(Resource, Default)]
pub struct DropdownOpen(pub Option<usize>);

/// The .glb saved in the selected model's folder (if any); the Edit button needs it.
#[derive(Resource, Default)]
pub struct EditTarget(pub Option<PathBuf>);

#[derive(Resource)]
pub struct ConvUiRefs {
    pub sidebar: Entity,        // the properties panel
    pub props_splitter: Entity, // the drag handle at its left edge
    pub eye_btn: Entity,        // floating preview-options button (filled in by ui::setup)
    pub scroll: Entity,         // its scrolling list
    pub sb_track: Entity,
    pub sb_thumb: Entity,
    pub option_texts: Vec<Vec<Entity>>, // per row: the dropdown option labels
    pub option_checks: Vec<Vec<Entity>>, // per row: the check mark in front of each option
    pub progress: Entity,
    pub bar_fill: Entity,
    pub bar_text: Entity,
    pub rows: Vec<RowRefs>,
    // Title-bar and panel widgets created in ui::setup; it fills these in.
    pub btn_settings: Entity,
    /// Arrow icon inside the title-bar close button (shown instead of the X on the converter screen).
    pub btn_back: Entity,
    pub close_x: Entity,
    pub btn_edit: Entity,
    pub panel_header: Entity,
    pub regen_text: Entity,
    // Properties panel header
    pub icon_box: Entity,
    pub icon_hint: Entity,    // "No icon" text inside the icon box
    pub icon_overlay: Entity, // the "click to choose / drop a PNG" cover shown on hover
    pub icon_overlay_text: Entity,
    pub name_text: Entity,
    pub name_box: Entity, // the box around the name (it gets a field look while the name is edited)
    pub name_scroll: Entity, // the name's scrolling area (a long name scrolls on hover: name_marquee_system)
    pub info_text: Entity,
    pub info_scroll: Entity, // the info box's scrolling area
    pub info_sb_track: Entity,
    pub info_sb_thumb: Entity,
    /// Title-bar close/back button (wider on the converter screen, where it also says "Return").
    pub close_btn: Entity,
    // Log panel: opened by the logs button at the bottom left of the preview.
    pub log_panel: Entity,
    pub log_text: Entity,
    pub logs_icon: Entity,
    pub logs_btn: Entity,
}

/// Whether the converter log panel is shown (covers the preview area).
#[derive(Resource, Default)]
pub struct LogPanel {
    pub open: bool,
}

/// State of the slider tracks (they behave like HTML range inputs): which one is pressed, which one has the keyboard focus.
#[derive(Resource, Default)]
pub struct SliderDrag {
    held: bool,               // a track is pressed: the converter queue is held until release
    pub focus: Option<usize>, // the row whose slider has the keyboard focus (arrow keys, Home, End, PageUp/Down)
    hit: bool, // a track was pressed this frame (so `slider_keys_system` keeps the focus)
}

const THUMB_W: f32 = 12.0;
const THUMB: Color = Color::srgb(0.82, 0.85, 0.90);

/// Width of the properties panel.
pub const PROPS_W: f32 = 340.0;
/// The properties panel can be resized by dragging its left edge, between these widths.
const PROPS_MIN: f32 = 260.0;
const PROPS_MAX: f32 = 640.0;
/// At least this much of the window stays for the preview.
const PREVIEW_MIN: f32 = 360.0;
/// Size of the icon box (and of the Regenerate button under it).
const ICON_BOX: f32 = 124.0;
/// Height of the properties header: icon box + 8 px gap + the 26 px Regenerate button + 2 x 12 px padding.
const HEAD_H: f32 = ICON_BOX + 8.0 + 26.0 + 24.0;
const MARGIN: f32 = 24.0;
/// The properties panel no longer covers the preview, so the camera is not shifted any more (`preview::orbit_apply`).
pub const PREVIEW_SHIFT: f32 = 0.0;
/// Dark fill of check boxes and slider tracks (darker than the item background `ITEM`).
const OFF: Color = Color::srgb(0.04, 0.05, 0.07);
/// Left indent of the items below a section header.
const INDENT: f32 = 8.0;
/// Extra indent per nesting level of a dependent row.
const SUB_INDENT: f32 = 14.0;

/// Left border of a nested row (a thin guide line towards its parent option).
fn nest_border(depth: u8) -> UiRect {
    if depth > 0 {
        UiRect::left(Val::Px(2.0))
    } else {
        UiRect::ZERO
    }
}

/// The graying overlay of a nested row: hidden while the row applies, otherwise dims it and blocks the pointer.
fn row_overlay(commands: &mut Commands, parent: Entity, depth: u8) -> Option<Entity> {
    (depth > 0).then(|| {
        child(
            commands,
            parent,
            (
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    right: Val::Px(0.0),
                    top: Val::Px(0.0),
                    bottom: Val::Px(0.0),
                    display: Display::None,
                    ..default()
                },
                BackgroundColor(Color::srgba(0.09, 0.10, 0.12, 0.68)),
                BorderRadius::all(Val::Px(4.0)),
                RowOverlay,
            ),
        )
    })
}

/// Text that never wraps onto a second line.
fn line(s: &str, size: f32, color: Color) -> impl Bundle {
    (
        txt(s, size, color),
        TextLayout {
            linebreak: LineBreak::NoWrap,
            ..default()
        },
    )
}

fn snap(v: f64, min: f64, max: f64, step: f64) -> f64 {
    // `+ 0.0` turns a negative zero (e.g. round(-0.3) * step) into a plain 0.0.
    ((v / step).round() * step).clamp(min, max) + 0.0
}

/// Formats a slider value with `decimals` digits and never prints a negative zero ("-0", "-0.00").
pub fn fmt_num(decimals: usize, v: f64) -> String {
    let s = format!("{:.*}", decimals, v);
    match s.strip_prefix('-') {
        Some(rest) if rest.chars().all(|c| c == '0' || c == '.') => rest.to_string(),
        _ => s,
    }
}

/// The slider value for a pointer at window x over a track from `left` (`width` wide): the track's ends are min and max
/// (HTML range input), snapped to the step.
fn value_at(x: f32, left: f32, width: f32, min: f64, max: f64, step: f64) -> f64 {
    let frac = ((x - left) / width.max(1.0)).clamp(0.0, 1.0) as f64;
    snap(min + frac * (max - min), min, max, step)
}

/// The next grid value from `v` in direction `dir` (+1 / -1): an on-grid value moves one step, an off-grid one (typed by
/// hand) goes to the nearest grid value in that direction. Not clamped: typed values may lie outside the drawn range.
fn nudge(v: f64, step: f64, dir: f64) -> f64 {
    let q = v / step;
    let n = if dir > 0.0 {
        (q + 1.0e-9).floor() + 1.0
    } else {
        (q - 1.0e-9).ceil() - 1.0
    };
    let r = (n * step * 1.0e6).round() / 1.0e6; // 0.1 * 3 stays 0.3
    if r == 0.0 {
        0.0
    } else {
        r
    } // never -0
}

/// Sets slider row `i` from the pointer's window x over its `track`.
fn slide_to(
    rows: &ParamRows,
    i: usize,
    st: &mut ConvertState,
    boxes: &Query<(&GlobalTransform, &ComputedNode)>,
    track: Entity,
    x: f32,
) {
    let Some(Row::Param(ParamDef {
        kind:
            ParamKind::Slider {
                min,
                max,
                step,
                get,
                set,
                ..
            },
        ..
    })) = rows.0.get(i)
    else {
        return;
    };
    let Ok((gt, c)) = boxes.get(track) else {
        return;
    };
    let k = c.inverse_scale_factor;
    let v = value_at(
        x,
        (gt.translation().x - c.size.x * 0.5) * k,
        c.size.x * k,
        *min,
        *max,
        *step,
    );
    if (v - get(&st.opts)).abs() > 1.0e-9 {
        set(&mut st.opts, v);
        st.changed(SLIDER_DELAY);
    }
}

/// The mouse button of a slider went up: the converter queue may start (SLIDER_DELAY later). Runs once per press.
fn release(st: &mut ConvertState, d: &mut SliderDrag) {
    if d.held {
        d.held = false;
        st.slider_released();
    }
}

/// Keyboard for the slider that has the focus (set by pressing its track): Left / Down and Right / Up move one step,
/// PageDown / PageUp a tenth of the range, Home / End go to the ends; holding a key repeats it. Clicking anywhere else, or
/// typing in a number field, drops the focus. Also makes sure the converter queue is released when the mouse button is up.
#[allow(clippy::too_many_arguments)]
pub fn slider_keys_system(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    time: Res<Time<Real>>,
    rows: Res<ParamRows>,
    layout: Res<Layout>,
    ed: Res<Editing>,
    mut st: ResMut<ConvertState>,
    mut d: ResMut<SliderDrag>,
    mut repeat: Local<Option<(KeyCode, f64)>>,
) {
    if d.held && !mouse.pressed(MouseButton::Left) {
        release(&mut st, &mut d); // safety net: a release event that never came
    }
    if mouse.just_pressed(MouseButton::Left) && !d.hit && d.focus.is_some() {
        d.focus = None;
    }
    if d.hit {
        d.hit = false;
    }
    if !layout.converter || ed.active() {
        if d.focus.is_some() {
            d.focus = None;
        }
        *repeat = None;
        return;
    }
    let Some(i) = d.focus else {
        *repeat = None;
        return;
    };
    let Some(Row::Param(ParamDef {
        kind:
            ParamKind::Slider {
                min,
                max,
                step,
                get,
                set,
                ..
            },
        ..
    })) = rows.0.get(i)
    else {
        d.focus = None;
        return;
    };
    const KEYS: [KeyCode; 8] = [
        KeyCode::ArrowLeft,
        KeyCode::ArrowDown,
        KeyCode::ArrowRight,
        KeyCode::ArrowUp,
        KeyCode::PageDown,
        KeyCode::PageUp,
        KeyCode::Home,
        KeyCode::End,
    ];
    let now = time.elapsed_secs_f64();
    let mut fire = KEYS.into_iter().find(|k| keys.just_pressed(*k));
    if let Some(k) = fire {
        *repeat = Some((k, now + 0.4)); // first repeat after 0.4 s, then every 0.05 s
    } else if let Some((k, at)) = *repeat {
        if !keys.pressed(k) {
            *repeat = None;
        } else if now >= at {
            fire = Some(k);
            *repeat = Some((k, now + 0.05));
        }
    }
    let Some(k) = fire else { return };
    let v = get(&st.opts);
    let page = ((max - min) / 10.0 / step).round().max(1.0) * step;
    let new = match k {
        KeyCode::ArrowLeft | KeyCode::ArrowDown => nudge(v, *step, -1.0),
        KeyCode::ArrowRight | KeyCode::ArrowUp => nudge(v, *step, 1.0),
        KeyCode::PageDown => nudge(v - page + step, *step, -1.0),
        KeyCode::PageUp => nudge(v + page - step, *step, 1.0),
        KeyCode::Home => *min,
        _ => *max,
    };
    if (new - v).abs() > 1.0e-9 {
        set(&mut st.opts, new);
        st.changed(SLIDER_DELAY);
    }
}

/// Moves every slider's thumb to its value and shows the keyboard focus (accent border and thumb).
#[allow(clippy::too_many_arguments)]
pub fn slider_visuals_system(
    rows: Res<ParamRows>,
    refs: Res<ConvUiRefs>,
    st: Res<ConvertState>,
    d: Res<SliderDrag>,
    mut nodes: Query<&mut Node>,
    mut borders: Query<&mut BorderColor>,
    mut bgs: Query<&mut BackgroundColor>,
) {
    for (idx, (row, r)) in rows.0.iter().zip(refs.rows.iter()).enumerate() {
        let (
            Row::Param(ParamDef {
                kind: ParamKind::Slider { min, max, get, .. },
                ..
            }),
            Some(thumb),
            Some(track),
        ) = (row, r.thumb, r.track)
        else {
            continue;
        };
        let p =
            Val::Percent((((get(&st.opts) - min) / (max - min)).clamp(0.0, 1.0) * 100.0) as f32);
        if let Ok(mut n) = nodes.get_mut(thumb) {
            if n.left != p {
                n.left = p;
            }
        }
        let focused = d.focus == Some(idx);
        if let Ok(mut b) = borders.get_mut(track) {
            let want = if focused { ACCENT } else { EDGE };
            if b.0 != want {
                b.0 = want;
            }
        }
        if let Ok(mut c) = bgs.get_mut(thumb) {
            let want = if focused { ACCENT } else { THUMB };
            if c.0 != want {
                c.0 = want;
            }
        }
    }
}

// ---- building ---------------------------------------------------------------------------------------

/// A small icon at the right end of a section header that says "this section switched a preview overlay on". Hidden until
/// the matching system shows it (`marker` is its component). An ABSOLUTE overlay, so showing it never changes the
/// header's own layout. Clicking it runs `off` (switches the overlay off, the icon then hides itself) and does not fold
/// the section.
fn header_indicator(
    commands: &mut Commands,
    header: Entity,
    font: &IconFont,
    glyph: Glyph,
    color: Color,
    marker: impl Component,
    off: fn(&mut Opts),
) {
    let wrap = child(
        commands,
        header,
        (
            marker,
            Node {
                position_type: PositionType::Absolute,
                right: Val::Px(8.0),
                top: Val::Px(0.0),
                bottom: Val::Px(0.0),
                align_items: AlignItems::Center,
                display: Display::None,
                ..default()
            },
        ),
    );
    child(
        commands,
        wrap,
        (icon(font, glyph, 18.0, color), Pickable::IGNORE),
    );
    // The click must not reach the header below, which would fold the section.
    commands.entity(wrap).observe(
        move |mut t: Trigger<Pointer<Click>>, mut opts: ResMut<Opts>| {
            t.propagate(false);
            if t.event().button == PointerButton::Primary {
                off(&mut opts);
            }
        },
    );
    commands.entity(wrap).observe(
        |_: Trigger<Pointer<Over>>, mut commands: Commands, win: Query<Entity, With<PrimaryWindow>>| {
            set_cursor(&mut commands, &win, SystemCursorIcon::Pointer);
        },
    );
    commands.entity(wrap).observe(
        |_: Trigger<Pointer<Out>>, mut commands: Commands, win: Query<Entity, With<PrimaryWindow>>| {
            set_cursor(&mut commands, &win, SystemCursorIcon::Default);
        },
    );
}

/// Spawn the properties panel (a child of `body`, next to the preview) and the overlays inside the preview container.
pub fn build(
    commands: &mut Commands,
    container: Entity,
    body: Entity,
    rows: &[Row],
    font: &IconFont,
) -> ConvUiRefs {
    // The properties panel: a column of its own. Top: icon + regenerate button, name + technical info. Below: the
    // "Properties" title and the scrolling list of collapsible sections with a scrollbar.
    // Drag handle between the preview and the properties panel.
    let props_splitter = child(
        commands,
        body,
        (
            Node {
                width: Val::Px(6.0),
                flex_shrink: 0.0,
                display: Display::None,
                ..default()
            },
            BackgroundColor(BAR),
        ),
    );
    commands.entity(props_splitter).observe(props_splitter_drag);
    commands.entity(props_splitter).observe(
        |t: Trigger<Pointer<Over>>,
         mut commands: Commands,
         win: Query<Entity, With<PrimaryWindow>>,
         mut bg: Query<&mut BackgroundColor>| {
            set_cursor(&mut commands, &win, SystemCursorIcon::ColResize);
            if let Ok(mut b) = bg.get_mut(t.target()) {
                b.0 = EDGE_HOVER;
            }
        },
    );
    commands.entity(props_splitter).observe(
        |t: Trigger<Pointer<Out>>,
         mut commands: Commands,
         win: Query<Entity, With<PrimaryWindow>>,
         mut bg: Query<&mut BackgroundColor>| {
            set_cursor(&mut commands, &win, SystemCursorIcon::Default);
            if let Ok(mut b) = bg.get_mut(t.target()) {
                b.0 = BAR;
            }
        },
    );
    let sidebar = child(
        commands,
        body,
        (
            Node {
                width: Val::Px(PROPS_W),
                flex_shrink: 0.0,
                flex_direction: FlexDirection::Column,
                display: Display::None,
                ..default()
            },
            BackgroundColor(PANEL),
        ),
    );
    // The header has a DEFINITE height (the icon column: icon box + gap + Regenerate button, plus the padding). The info box
    // fills what is left next to it; with an automatic height the header would grow with the info tree instead of the tree
    // scrolling, because a flex item's content still counts towards its parent's automatic height.
    let head = child(
        commands,
        sidebar,
        Node {
            flex_shrink: 0.0,
            height: Val::Px(HEAD_H),
            column_gap: Val::Px(12.0),
            padding: UiRect::all(Val::Px(12.0)),
            ..default()
        },
    );
    let left_col = child(
        commands,
        head,
        Node {
            width: Val::Px(ICON_BOX),
            flex_shrink: 0.0,
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(8.0),
            ..default()
        },
    );
    let icon_box = child(
        commands,
        left_col,
        (
            Interaction::default(), // hover state for the "click to choose a PNG" hint
            Node {
                width: Val::Px(ICON_BOX),
                height: Val::Px(ICON_BOX),
                border: UiRect::all(Val::Px(2.0)),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(ICON_BG),
            BorderColor(EDGE_HOVER),
            BorderRadius::all(Val::Px(6.0)),
        ),
    );
    let icon_hint = child(
        commands,
        icon_box,
        (txt("No icon", 12.0, DIM), Pickable::IGNORE),
    );
    // Hover / drag hint over the icon: click = choose a PNG in the file explorer, or drop one (see `icon_input_system`).
    let icon_overlay = child(
        commands,
        icon_box,
        (
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                right: Val::Px(0.0),
                top: Val::Px(0.0),
                bottom: Val::Px(0.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                padding: UiRect::all(Val::Px(6.0)),
                display: Display::None,
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.66)),
            Pickable::IGNORE,
        ),
    );
    let icon_overlay_text = child(
        commands,
        icon_overlay,
        (
            txt("", 12.0, TEXT),
            TextLayout::new_with_justify(JustifyText::Center),
            Pickable::IGNORE,
        ),
    );
    commands.entity(icon_box).observe(field_over);
    commands.entity(icon_box).observe(field_out);
    commands.entity(icon_box).observe(on_icon_click);
    // Refreshes the saved model's icon.png (it is kept across regenerations, so it only changes when asked).
    let regen = child(
        commands,
        left_col,
        (
            Button,
            Node {
                width: Val::Px(ICON_BOX),
                height: Val::Px(26.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(BTN),
            BorderRadius::all(Val::Px(4.0)),
            pass(),
        ),
    );
    let regen_text = child(
        commands,
        regen,
        (line("Regenerate icon", 11.0, TEXT), Pickable::IGNORE),
    );
    // The icon is rendered the way the preview looks right now: camera, field of view, visible lights, options.
    commands.entity(regen).observe(
        |t: Trigger<Pointer<Click>>,
         st: Res<ConvertState>,
         mut pipe: ResMut<IconPipe>,
         opts: Res<Opts>,
         cam: Query<
            (&Transform, &Projection, &AmbientLight, &Tonemapping),
            With<PreviewCamera>,
        >,
         lights: Query<
            (&GlobalTransform, &DirectionalLight, &Visibility),
            Or<(With<StudioLight>, With<ThumbLight>)>,
        >| {
            if t.event().button != PointerButton::Primary {
                return;
            }
            let Some(dir) = &st.saved else { return };
            let view = cam.single().ok().map(|(tr, proj, amb, tone)| IconView {
                camera: *tr,
                fov: if let Projection::Perspective(p) = proj {
                    p.fov
                } else {
                    crate::gamerig::fov_rad()
                },
                lights: lights
                    .iter()
                    .filter(|(_, _, vis)| **vis != Visibility::Hidden)
                    .map(|(gt, d, _)| (gt.compute_transform().rotation, d.illuminance))
                    .collect(),
                ambient: amb.brightness,
                tonemapping: *tone,
                opts: opts.clone(),
            });
            icons::request_regen(&mut pipe, dir.clone(), None, 0, view);
        },
    );
    let right_col = child(
        commands,
        head,
        Node {
            flex_grow: 1.0,
            min_width: Val::Px(0.0),
            min_height: Val::Px(0.0),
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(8.0),
            ..default()
        },
    );
    // The model name is plain text, it does not look like a field. The pencil button next to it starts the rename and
    // turns the box into a text field (textedit.rs; `name_marquee_system` paints the field look while it is edited). A
    // name wider than the box scrolls slowly sideways while the pointer is over it (`name_marquee_system`), which never
    // takes a click: the text ignores the pointer.
    let name_box = child(
        commands,
        right_col,
        (
            Interaction::default(),
            Node {
                align_items: AlignItems::Center,
                column_gap: Val::Px(6.0),
                padding: UiRect::axes(Val::Px(8.0), Val::Px(4.0)),
                border: UiRect::all(Val::Px(1.0)), // (transparent until the name is edited; keeps the field geometry)
                overflow: Overflow::clip(),
                ..default()
            },
            BackgroundColor(Color::NONE),
            BorderColor(Color::NONE),
            BorderRadius::all(Val::Px(4.0)),
        ),
    );
    // A COLUMN: the scroll area below is stretched to the column's width whatever the text does. (As a row it was only as
    // wide as the text minus the negative margin textedit.rs uses to scroll the edited name, so the visible area shrank
    // while the caret moved right and the text vanished.)
    let name_col = child(
        commands,
        name_box,
        Node {
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
            flex_basis: Val::Px(0.0),
            min_width: Val::Px(0.0),
            ..default()
        },
    );
    let name_scroll = child(
        commands,
        name_col,
        (
            Node {
                width: Val::Percent(100.0),
                overflow: Overflow {
                    x: OverflowAxis::Scroll,
                    y: OverflowAxis::Clip,
                },
                ..default()
            },
            ScrollPosition::default(),
            Pickable::IGNORE,
        ),
    );
    let name_text = child(
        commands,
        name_scroll,
        (
            txt("", 16.0, TEXT),
            TextLayout {
                linebreak: LineBreak::NoWrap,
                ..default()
            },
            Node {
                flex_shrink: 0.0, // keeps its full width, so there is something to scroll
                ..default()
            },
            Pickable::IGNORE,
        ),
    );
    let pencil = child(commands, name_box, button(Val::Px(26.0), Val::Px(24.0)));
    child(commands, pencil, (icon(font, g::EDIT, 15.0, TEXT), Pickable::IGNORE));
    pointer_cursor(commands, pencil);
    commands.entity(pencil).observe(
        move |t: Trigger<Pointer<Click>>,
              st: Res<ConvertState>,
              time: Res<Time<Real>>,
              mut ed: ResMut<Editing>| {
            if t.event().button != PointerButton::Primary || st.input.is_none() {
                return;
            }
            if ed.widget != Some(name_box) {
                ed.start(
                    Field::Rename,
                    &model_name(&st),
                    Geom::rename(name_text, name_box),
                    true, // the whole name is selected, like the tile's Rename
                    time.elapsed_secs_f64(),
                );
            }
        },
    );
    // While the name is edited, drags select text and presses move the caret (nothing happens when it is not edited).
    commands.entity(name_box).observe(field_drag);
    commands.entity(name_box).observe(
        move |t: Trigger<Pointer<Pressed>>,
              boxes: Query<(&GlobalTransform, &ComputedNode)>,
              keys: Res<ButtonInput<KeyCode>>,
              time: Res<Time<Real>>,
              mut ed: ResMut<Editing>| {
            if t.event().button != PointerButton::Primary || ed.widget != Some(name_box) {
                return;
            }
            press(
                &mut ed,
                &boxes,
                name_box,
                t.event().pointer_location.position.x,
                shift_down(&keys),
                time.elapsed_secs_f64(),
            );
        },
    );
    // The info box: a row with the scrolling area on the left and a slim scrollbar on the right. flex_basis 0 + min_height 0:
    // the box takes the height the header already has (the icon column decides it) and never makes the header taller.
    let info_box = child(
        commands,
        right_col,
        (
            Node {
                flex_grow: 1.0,
                flex_basis: Val::Px(0.0),
                min_height: Val::Px(0.0),
                flex_direction: FlexDirection::Row,
                ..default()
            },
            BackgroundColor(Color::srgb(0.10, 0.11, 0.13)),
            BorderRadius::all(Val::Px(4.0)),
            pass(),
        ),
    );
    let info_scroll = child(
        commands,
        info_box,
        (
            Node {
                flex_grow: 1.0,
                min_width: Val::Px(0.0),
                min_height: Val::Px(0.0),
                flex_direction: FlexDirection::Column,
                padding: UiRect::new(Val::Px(8.0), Val::Px(4.0), Val::Px(8.0), Val::Px(8.0)),
                overflow: Overflow::scroll_y(),
                ..default()
            },
            ScrollPosition::default(),
            pass(),
        ),
    );
    let info_sb_track = child(
        commands,
        info_box,
        (
            Node {
                width: Val::Px(8.0),
                flex_shrink: 0.0,
                margin: UiRect::new(Val::Px(0.0), Val::Px(4.0), Val::Px(4.0), Val::Px(4.0)),
                ..default()
            },
            BackgroundColor(Color::srgb(0.07, 0.08, 0.10)),
            BorderRadius::all(Val::Px(4.0)),
        ),
    );
    let info_sb_thumb = child(
        commands,
        info_sb_track,
        (
            Button,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(1.0),
                right: Val::Px(1.0),
                top: Val::Px(0.0),
                height: Val::Px(30.0),
                display: Display::None,
                ..default()
            },
            BackgroundColor(BTN),
            BorderRadius::all(Val::Px(3.0)),
        ),
    );
    commands.entity(info_sb_thumb).observe(info_thumb_drag);
    // The info tree is rebuilt into this column by `info_tree_system` whenever its content changes.
    let info_text = child(
        commands,
        info_scroll,
        (
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(6.0),
                ..default()
            },
            pass(),
        ),
    );
    let title = child(
        commands,
        sidebar,
        Node {
            flex_shrink: 0.0,
            padding: UiRect::new(Val::Px(12.0), Val::Px(12.0), Val::Px(2.0), Val::Px(6.0)),
            ..default()
        },
    );
    child(commands, title, txt("Properties", 18.0, TEXT));
    let list_row = child(
        commands,
        sidebar,
        Node {
            flex_grow: 1.0,
            min_height: Val::Px(0.0),
            flex_direction: FlexDirection::Row,
            ..default()
        },
    );
    let scroll = child(
        commands,
        list_row,
        (
            Node {
                flex_grow: 1.0,
                min_width: Val::Px(0.0),
                min_height: Val::Px(0.0),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(6.0),
                padding: UiRect::new(Val::Px(8.0), Val::Px(4.0), Val::Px(4.0), Val::Px(12.0)),
                overflow: Overflow::scroll_y(),
                ..default()
            },
            ScrollPosition::default(),
        ),
    );
    let sb_track = child(
        commands,
        list_row,
        (
            Node {
                width: Val::Px(10.0),
                flex_shrink: 0.0,
                margin: UiRect::new(Val::Px(0.0), Val::Px(4.0), Val::Px(6.0), Val::Px(6.0)),
                ..default()
            },
            BackgroundColor(Color::srgb(0.08, 0.09, 0.11)),
            BorderRadius::all(Val::Px(4.0)),
        ),
    );
    let sb_thumb = child(
        commands,
        sb_track,
        (
            Button,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(1.0),
                right: Val::Px(1.0),
                top: Val::Px(0.0),
                height: Val::Px(30.0),
                display: Display::None,
                ..default()
            },
            BackgroundColor(BTN),
            BorderRadius::all(Val::Px(4.0)),
        ),
    );
    commands.entity(sb_thumb).observe(sidebar_thumb_drag);
    // From here on rows are added to the scrolling list.
    let sidebar_col = scroll;
    let mut option_texts: Vec<Vec<Entity>> = Vec::new();
    let mut option_checks: Vec<Vec<Entity>> = Vec::new();

    let mut refs = Vec::with_capacity(rows.len());
    let mut section: Option<usize> = None; // index of the section the current rows belong to
    let mut n_sections = 0usize;
    // The COLLISION section (index among the headers): opening it switches the preview's hull outlines on, closing it
    // (or opening another section, which closes it) switches them off.
    let n_sections_total = rows.iter().filter(|r| matches!(r, Row::Header(_))).count();
    let collision_sec = rows
        .iter()
        .filter_map(|r| {
            if let Row::Header(t) = r {
                Some(*t)
            } else {
                None
            }
        })
        .position(|t| t == "COLLISION");
    commands.insert_resource(CollisionSection(collision_sec));
    // The MODEL section: opening it switches the printer volumes on, like COLLISION does for the hull outlines.
    let model_sec = rows
        .iter()
        .filter_map(|r| {
            if let Row::Header(t) = r {
                Some(*t)
            } else {
                None
            }
        })
        .position(|t| t == "MODEL");
    commands.insert_resource(ModelSection(model_sec));
    // The LIGHT section: opening it switches the lamp marker on.
    let light_sec = rows
        .iter()
        .filter_map(|r| {
            if let Row::Header(t) = r {
                Some(*t)
            } else {
                None
            }
        })
        .position(|t| t == "LIGHT");
    commands.insert_resource(LightSection(light_sec));
    for (i, row) in rows.iter().enumerate() {
        let mut texts_of_row: Vec<Entity> = Vec::new();
        let mut checks_of_row: Vec<Entity> = Vec::new();
        match row {
            Row::Header(text) => {
                // A collapsible section header: click to fold / unfold the rows below it.
                let sec = n_sections;
                n_sections += 1;
                section = Some(sec);
                let node = child(
                    commands,
                    sidebar_col,
                    (
                        Button,
                        Node {
                            height: Val::Px(28.0),
                            flex_shrink: 0.0,
                            margin: UiRect::top(Val::Px(10.0)),
                            align_items: AlignItems::Center,
                            column_gap: Val::Px(4.0),
                            padding: UiRect::horizontal(Val::Px(4.0)),
                            overflow: Overflow::clip(),
                            ..default()
                        },
                        BackgroundColor(BTN),
                        BorderRadius::all(Val::Px(4.0)),
                        pass(),
                    ),
                );
                let chev = child(
                    commands,
                    node,
                    (icon(font, g::EXPAND_MORE, 20.0, TEXT), Pickable::IGNORE),
                );
                child(commands, node, (line(text, 13.0, TEXT), Pickable::IGNORE));
                if collision_sec == Some(sec) {
                    header_indicator(
                        commands,
                        node,
                        font,
                        g::VISIBILITY,
                        EXPORT_COL,
                        HullIndicator,
                        |o| o.show_hulls = false,
                    );
                }
                if model_sec == Some(sec) {
                    header_indicator(
                        commands,
                        node,
                        font,
                        g::FULLSCREEN,
                        ACCENT,
                        LimitsIndicator,
                        |o| o.printer_limits = false,
                    );
                }
                if light_sec == Some(sec) {
                    header_indicator(
                        commands,
                        node,
                        font,
                        g::LIGHTBULB,
                        Color::srgb(1.0, 0.82, 0.3),
                        LightIndicator,
                        |o| o.show_lamp = false,
                    );
                }
                commands.entity(node).observe(
                    move |t: Trigger<Pointer<Click>>,
                          mut s: ResMut<OpenSections>,
                          mut opts: ResMut<Opts>| {
                        if t.event().button != PointerButton::Primary {
                            return;
                        }
                        let is_open =
                            |s: &OpenSections, i: usize| s.0.get(i).copied().unwrap_or(false);
                        let collision_before = collision_sec.is_some_and(|c| is_open(&s, c));
                        let model_before = model_sec.is_some_and(|m| is_open(&s, m));
                        let light_before = light_sec.is_some_and(|l| is_open(&s, l));
                        // Accordion: at most one section is open. Clicking the open one folds it, clicking another
                        // one folds the open one and unfolds the clicked one.
                        let opening = !is_open(&s, sec);
                        s.0.clear();
                        s.0.resize(n_sections_total.max(sec + 1), false);
                        s.0[sec] = opening;
                        let collision_after = collision_sec.is_some_and(|c| is_open(&s, c));
                        if collision_after != collision_before {
                            opts.show_hulls = collision_after;
                        }
                        let model_after = model_sec.is_some_and(|m| is_open(&s, m));
                        if model_after != model_before {
                            opts.printer_limits = model_after;
                        }
                        let light_after = light_sec.is_some_and(|l| is_open(&s, l));
                        if light_after != light_before {
                            opts.show_lamp = light_after;
                        }
                    },
                );
                refs.push(RowRefs {
                    node,
                    value: None,
                    fill: None,
                    mark: None,
                    list: None,
                    chev: Some(chev),
                    section,
                    overlay: None,
                    track: None,
                    thumb: None,
                    reset: None,
                });
            }
            Row::Param(def) => {
                match &def.kind {
                    ParamKind::Toggle { .. } => {
                        let node = child(
                            commands,
                            sidebar_col,
                            (
                                Button,
                                Node {
                                    height: Val::Px(28.0),
                                    min_width: Val::Px(0.0),
                                    flex_shrink: 0.0,
                                    margin: UiRect::left(Val::Px(
                                        INDENT + def.depth as f32 * SUB_INDENT,
                                    )),
                                    border: nest_border(def.depth),
                                    align_items: AlignItems::Center,
                                    column_gap: Val::Px(8.0),
                                    padding: UiRect { left: Val::Px(6.0), right: Val::Px(RESET_GUTTER), ..default() },
                                    overflow: Overflow::clip(),
                                    ..default()
                                },
                                BackgroundColor(ITEM),
                                BorderColor(EDGE),
                                ItemButton,
                                BorderRadius::all(Val::Px(4.0)),
                                pass(),
                            ),
                        );
                        let mark = child(
                            commands,
                            node,
                            (
                                Node {
                                    width: Val::Px(14.0),
                                    height: Val::Px(14.0),
                                    flex_shrink: 0.0,
                                    border: UiRect::all(Val::Px(1.0)),
                                    ..default()
                                },
                                BackgroundColor(OFF),
                                BorderColor(EDGE_HOVER),
                                BorderRadius::all(Val::Px(3.0)),
                                Pickable::IGNORE,
                            ),
                        );
                        child(
                            commands,
                            node,
                            (line(def.label, 13.0, TEXT), Pickable::IGNORE),
                        );
                        let reset = spawn_reset(commands, node, font, i, 3.0);
                        commands.entity(node).observe(
                            move |_: Trigger<Pointer<Click>>,
                                  rows: Res<ParamRows>,
                                  mut st: ResMut<ConvertState>| {
                                if let Row::Param(ParamDef {
                                    kind: ParamKind::Toggle { get, set },
                                    ..
                                }) = &rows.0[i]
                                {
                                    let v = !get(&st.opts);
                                    set(&mut st.opts, v);
                                    st.changed(0.0);
                                }
                            },
                        );
                        let overlay = row_overlay(commands, node, def.depth);
                        refs.push(RowRefs {
                            node,
                            value: None,
                            fill: None,
                            mark: Some(mark),
                            list: None,
                            chev: None,
                            section,
                            overlay,
                            track: None,
                            thumb: None,
                            reset: Some(reset),
                        });
                    }
                    ParamKind::Choice { labels, .. } => {
                        // A dropdown: the head shows the value; clicking it opens the list of options right below it.
                        let node = child(
                            commands,
                            sidebar_col,
                            (
                                Node {
                                    flex_direction: FlexDirection::Column,
                                    row_gap: Val::Px(4.0),
                                    flex_shrink: 0.0,
                                    margin: UiRect::left(Val::Px(
                                        INDENT + def.depth as f32 * SUB_INDENT,
                                    )),
                                    border: nest_border(def.depth),
                                    ..default()
                                },
                                BorderColor(EDGE),
                                pass(),
                            ),
                        );
                        let head = child(
                            commands,
                            node,
                            (
                                Button,
                                Node {
                                    height: Val::Px(28.0),
                                    min_width: Val::Px(0.0),
                                    align_items: AlignItems::Center,
                                    justify_content: JustifyContent::SpaceBetween,
                                    column_gap: Val::Px(8.0),
                                    padding: UiRect { left: Val::Px(6.0), right: Val::Px(RESET_GUTTER), ..default() },
                                    overflow: Overflow::clip(),
                                    ..default()
                                },
                                BackgroundColor(ITEM),
                                ItemButton,
                                BorderRadius::all(Val::Px(4.0)),
                                pass(),
                            ),
                        );
                        child(
                            commands,
                            head,
                            (line(def.label, 13.0, TEXT), Pickable::IGNORE),
                        );
                        let right = child(
                            commands,
                            head,
                            (
                                Node {
                                    align_items: AlignItems::Center,
                                    column_gap: Val::Px(2.0),
                                    ..default()
                                },
                                Pickable::IGNORE,
                            ),
                        );
                        let reset = spawn_reset(commands, head, font, i, 3.0);
                        let value =
                            child(commands, right, (line("", 13.0, ACCENT), Pickable::IGNORE));
                        let chev = child(
                            commands,
                            right,
                            (icon(font, g::EXPAND_MORE, 18.0, ACCENT), Pickable::IGNORE),
                        );
                        commands.entity(head).observe(
                            move |_: Trigger<Pointer<Click>>, mut dd: ResMut<DropdownOpen>| {
                                dd.0 = if dd.0 == Some(i) { None } else { Some(i) };
                            },
                        );
                        let list = child(
                            commands,
                            node,
                            (
                                Node {
                                    flex_direction: FlexDirection::Column,
                                    row_gap: Val::Px(3.0),
                                    padding: UiRect::left(Val::Px(10.0)),
                                    display: Display::None,
                                    ..default()
                                },
                                pass(),
                            ),
                        );
                        for (k, label) in labels.iter().enumerate() {
                            let opt = child(
                                commands,
                                list,
                                (
                                    Button,
                                    Node {
                                        height: Val::Px(26.0),
                                        min_width: Val::Px(0.0),
                                        align_items: AlignItems::Center,
                                        column_gap: Val::Px(4.0),
                                        padding: UiRect::horizontal(Val::Px(8.0)),
                                        overflow: Overflow::clip(),
                                        ..default()
                                    },
                                    BackgroundColor(ITEM),
                                    ItemButton,
                                    BorderRadius::all(Val::Px(3.0)),
                                    pass(),
                                ),
                            );
                            // Check mark in front of the selected option (transparent on the others, so labels line up).
                            let ck = child(
                                commands,
                                opt,
                                (
                                    icon(font, g::CHECK, 16.0, Color::NONE),
                                    Node {
                                        width: Val::Px(16.0),
                                        ..default()
                                    },
                                    Pickable::IGNORE,
                                ),
                            );
                            checks_of_row.push(ck);
                            let t =
                                child(commands, opt, (line(label, 13.0, TEXT), Pickable::IGNORE));
                            texts_of_row.push(t);
                            commands.entity(opt).observe(move |_: Trigger<Pointer<Click>>, rows: Res<ParamRows>, mut st: ResMut<ConvertState>, mut dd: ResMut<DropdownOpen>| {
                            if let Row::Param(ParamDef { kind: ParamKind::Choice { get, set, .. }, .. }) = &rows.0[i] {
                                if get(&st.opts) != k {
                                    set(&mut st.opts, k);
                                    st.changed(0.0);
                                }
                            }
                            dd.0 = None;
                        });
                        }
                        let overlay = row_overlay(commands, node, def.depth);
                        refs.push(RowRefs {
                            node,
                            value: Some(value),
                            fill: None,
                            mark: None,
                            list: Some(list),
                            chev: Some(chev),
                            section,
                            overlay,
                            track: None,
                            thumb: None,
                            reset: Some(reset),
                        });
                    }
                    ParamKind::Slider { .. } => {
                        let node = child(
                            commands,
                            sidebar_col,
                            (
                                Node {
                                    flex_direction: FlexDirection::Column,
                                    row_gap: Val::Px(5.0),
                                    flex_shrink: 0.0,
                                    margin: UiRect::left(Val::Px(
                                        INDENT + def.depth as f32 * SUB_INDENT,
                                    )),
                                    border: nest_border(def.depth),
                                    padding: UiRect { left: Val::Px(8.0), right: Val::Px(RESET_GUTTER), top: Val::Px(6.0), bottom: Val::Px(6.0) },
                                    ..default()
                                },
                                BackgroundColor(ITEM),
                                BorderColor(EDGE),
                                BorderRadius::all(Val::Px(4.0)),
                                pass(),
                            ),
                        );
                        let top = child(
                            commands,
                            node,
                            (
                                Node {
                                    justify_content: JustifyContent::SpaceBetween,
                                    column_gap: Val::Px(8.0),
                                    overflow: Overflow::clip(),
                                    ..default()
                                },
                                pass(),
                            ),
                        );
                        child(commands, top, (line(def.label, 13.0, TEXT), pass()));
                        // The value is a field: click it to type an exact number (see textedit.rs).
                        let right_group = child(
                            commands,
                            top,
                            (
                                Node {
                                    align_items: AlignItems::Center,
                                    column_gap: Val::Px(6.0),
                                    ..default()
                                },
                                pass(),
                            ),
                        );
                        let reset = spawn_reset(commands, node, font, i, 4.0);
                        let vbox = child(
                            commands,
                            right_group,
                            (
                                Interaction::default(),
                                Node {
                                    min_width: Val::Px(64.0),
                                    padding: UiRect::axes(Val::Px(6.0), Val::Px(1.0)),
                                    border: UiRect::all(Val::Px(1.0)),
                                    justify_content: JustifyContent::FlexEnd,
                                    ..default()
                                },
                                BackgroundColor(OFF),
                                BorderColor(EDGE),
                                BorderRadius::all(Val::Px(3.0)),
                            ),
                        );
                        let value =
                            child(commands, vbox, (line("", 13.0, ACCENT), Pickable::IGNORE));
                        commands.entity(vbox).observe(field_over);
                        commands.entity(vbox).observe(field_out);
                        commands.entity(vbox).observe(field_drag);
                        commands.entity(vbox).observe(
                            move |t: Trigger<Pointer<Pressed>>,
                                  rows: Res<ParamRows>,
                                  st: Res<ConvertState>,
                                  boxes: Query<(&GlobalTransform, &ComputedNode)>,
                                  keys: Res<ButtonInput<KeyCode>>,
                                  time: Res<Time<Real>>,
                                  mut ed: ResMut<Editing>| {
                                if t.event().button != PointerButton::Primary {
                                    return;
                                }
                                let now = time.elapsed_secs_f64();
                                if ed.widget != Some(vbox) {
                                    if let Row::Param(ParamDef {
                                        kind: ParamKind::Slider { decimals, get, .. },
                                        ..
                                    }) = &rows.0[i]
                                    {
                                        ed.start(
                                            Field::Slider(i),
                                            &fmt_num(*decimals, get(&st.opts)),
                                            Geom::slider(value, vbox),
                                            false,
                                            now,
                                        );
                                    }
                                }
                                press(
                                    &mut ed,
                                    &boxes,
                                    vbox,
                                    t.event().pointer_location.position.x,
                                    shift_down(&keys),
                                    now,
                                );
                            },
                        );
                        let track = child(
                            commands,
                            node,
                            (
                                // The side margins leave room for the thumb's overhang at both ends of the track.
                                Node {
                                    height: Val::Px(14.0),
                                    margin: UiRect::horizontal(Val::Px(THUMB_W / 2.0)),
                                    ..default()
                                },
                                BackgroundColor(OFF),
                                BorderColor(EDGE),
                                BorderRadius::all(Val::Px(4.0)),
                                pass(),
                            ),
                        );
                        let fill = child(
                            commands,
                            track,
                            (
                                Node {
                                    position_type: PositionType::Absolute,
                                    left: Val::Px(0.0),
                                    top: Val::Px(0.0),
                                    bottom: Val::Px(0.0),
                                    width: Val::Percent(0.0),
                                    ..default()
                                },
                                BackgroundColor(Color::srgb(0.25, 0.45, 0.80)),
                                BorderRadius::all(Val::Px(4.0)),
                                Pickable::IGNORE,
                            ),
                        );
                        // The thumb sits at the value (`slider_visuals_system` moves it); it is not pickable, the track takes the pointer.
                        let thumb = child(
                            commands,
                            track,
                            (
                                Node {
                                    position_type: PositionType::Absolute,
                                    left: Val::Percent(0.0),
                                    margin: UiRect::left(Val::Px(-THUMB_W / 2.0)),
                                    top: Val::Px(-3.0),
                                    width: Val::Px(THUMB_W),
                                    height: Val::Px(20.0),
                                    border: UiRect::all(Val::Px(1.0)),
                                    ..default()
                                },
                                BackgroundColor(THUMB),
                                BorderColor(EDGE),
                                BorderRadius::all(Val::Px(4.0)),
                                Pickable::IGNORE,
                            ),
                        );
                        // HTML range behaviour: pressing anywhere on the track jumps the value to the pointer and gives the slider
                        // the keyboard focus; dragging then follows the pointer along the track (absolute, not relative). The
                        // converter queue is held while the button is down and starts SLIDER_DELAY after release. A press while a
                        // number field is being edited only ends that edit (the field applies its text), like clicking away.
                        commands.entity(track).observe(
                        move |t: Trigger<Pointer<Pressed>>,
                              rows: Res<ParamRows>,
                              mut st: ResMut<ConvertState>,
                              mut d: ResMut<SliderDrag>,
                              ed: Res<Editing>,
                              boxes: Query<(&GlobalTransform, &ComputedNode)>| {
                            if t.event().button != PointerButton::Primary || ed.active() {
                                return;
                            }
                            d.focus = Some(i);
                            d.hit = true;
                            if !d.held {
                                d.held = true;
                                st.hold += 1;
                            }
                            slide_to(&rows, i, &mut st, &boxes, track, t.event().pointer_location.position.x);
                        },
                    );
                        commands.entity(track).observe(
                        move |t: Trigger<Pointer<Drag>>, rows: Res<ParamRows>, mut st: ResMut<ConvertState>, d: Res<SliderDrag>, boxes: Query<(&GlobalTransform, &ComputedNode)>| {
                            if d.held {
                                slide_to(&rows, i, &mut st, &boxes, track, t.event().pointer_location.position.x);
                            }
                        },
                    );
                        // The release arrives as DragEnd after a drag and as Click after a plain click; `release` runs once.
                        commands.entity(track).observe(
                            |_: Trigger<Pointer<DragEnd>>,
                             mut st: ResMut<ConvertState>,
                             mut d: ResMut<SliderDrag>| {
                                release(&mut st, &mut d)
                            },
                        );
                        commands.entity(track).observe(
                            |_: Trigger<Pointer<Click>>,
                             mut st: ResMut<ConvertState>,
                             mut d: ResMut<SliderDrag>| {
                                release(&mut st, &mut d)
                            },
                        );
                        let overlay = row_overlay(commands, node, def.depth);
                        refs.push(RowRefs {
                            node,
                            value: Some(value),
                            fill: Some(fill),
                            mark: None,
                            list: None,
                            chev: None,
                            section,
                            overlay,
                            track: Some(track),
                            thumb: Some(thumb),
                            reset: Some(reset),
                        });
                    }
                }
            }
        }
        option_texts.push(texts_of_row);
        option_checks.push(checks_of_row);
    }

    // Logs button, bottom left of the preview: shows / hides the converter log (diagnostics of the last run).
    let logs_btn = child(
        commands,
        container,
        (
            Button,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(10.0),
                bottom: Val::Px(10.0),
                width: Val::Px(32.0),
                height: Val::Px(32.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                display: Display::None, // shown with the converter screen (conv_ui_system)
                ..default()
            },
            BackgroundColor(BTN),
            BorderRadius::all(Val::Px(4.0)),
        ),
    );
    let logs_icon = child(
        commands,
        logs_btn,
        (icon(font, g::DESCRIPTION, 20.0, TEXT), Pickable::IGNORE),
    );
    commands
        .entity(logs_btn)
        .observe(|t: Trigger<Pointer<Click>>, mut lp: ResMut<LogPanel>| {
            if t.event().button == PointerButton::Primary {
                lp.open = !lp.open;
            }
        });

    // Progress bar, bottom centre of the preview. Hidden unless something is happening.
    let progress = child(
        commands,
        container,
        (
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                right: Val::Px(0.0),
                bottom: Val::Px(14.0),
                justify_content: JustifyContent::Center,
                display: Display::None,
                ..default()
            },
            Pickable::IGNORE,
        ),
    );
    let bar = child(
        commands,
        progress,
        (
            Node {
                width: Val::Px(320.0),
                height: Val::Px(24.0),
                border: UiRect::all(Val::Px(2.0)),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                overflow: Overflow::clip(),
                ..default()
            },
            BackgroundColor(Color::srgba(0.07, 0.08, 0.10, 0.95)),
            BorderColor(EDGE_HOVER),
            BorderRadius::all(Val::Px(5.0)),
            Pickable::IGNORE,
        ),
    );
    let bar_fill = child(
        commands,
        bar,
        (
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                bottom: Val::Px(0.0),
                width: Val::Percent(0.0),
                ..default()
            },
            BackgroundColor(Color::srgba(0.30, 0.58, 0.98, 0.55)),
            Pickable::IGNORE,
        ),
    );
    let bar_text = child(
        commands,
        bar,
        (
            txt("", 12.0, TEXT),
            TextLayout {
                linebreak: LineBreak::NoWrap,
                ..default()
            },
            Pickable::IGNORE,
        ),
    );

    // Log panel: fills the preview (above the progress bar and the logs button) while open.
    let log_panel = child(
        commands,
        container,
        (
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(MARGIN),
                top: Val::Px(MARGIN),
                right: Val::Px(MARGIN),
                bottom: Val::Px(56.0),
                flex_direction: FlexDirection::Column,
                display: Display::None,
                ..default()
            },
            BackgroundColor(Color::srgba(0.11, 0.12, 0.15, 0.94)),
            BorderRadius::all(Val::Px(6.0)),
        ),
    );
    let log_head = child(
        commands,
        log_panel,
        Node {
            height: Val::Px(32.0),
            flex_shrink: 0.0,
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::SpaceBetween,
            padding: UiRect::horizontal(Val::Px(10.0)),
            ..default()
        },
    );
    child(commands, log_head, line("Converter log", 13.0, DIM));
    let log_close = child(commands, log_head, button(Val::Px(24.0), Val::Px(22.0)));
    child(
        commands,
        log_close,
        (icon(font, g::CLOSE, 16.0, TEXT), Pickable::IGNORE),
    );
    commands
        .entity(log_close)
        .observe(|_: Trigger<Pointer<Click>>, mut lp: ResMut<LogPanel>| lp.open = false);
    // The text lines ignore the pointer so the mouse wheel reaches this scrolling box (see ui::wheel_system).
    let log_scroll = child(
        commands,
        log_panel,
        (
            Node {
                flex_grow: 1.0,
                min_height: Val::Px(0.0),
                padding: UiRect::new(Val::Px(10.0), Val::Px(10.0), Val::Px(2.0), Val::Px(10.0)),
                overflow: Overflow::scroll_y(),
                ..default()
            },
            ScrollPosition::default(),
        ),
    );
    let log_text = child(
        commands,
        log_scroll,
        (txt("", 12.0, TEXT), Pickable::IGNORE),
    );

    ConvUiRefs {
        sidebar,
        props_splitter,
        eye_btn: Entity::PLACEHOLDER,
        scroll,
        sb_track,
        sb_thumb,
        option_texts,
        option_checks,
        progress,
        bar_fill,
        bar_text,
        rows: refs,
        btn_settings: Entity::PLACEHOLDER,
        btn_back: Entity::PLACEHOLDER,
        close_x: Entity::PLACEHOLDER,
        btn_edit: Entity::PLACEHOLDER,
        panel_header: Entity::PLACEHOLDER,
        regen_text,
        icon_box,
        icon_hint,
        icon_overlay,
        icon_overlay_text,
        name_text,
        name_box,
        name_scroll,
        info_text,
        info_scroll,
        info_sb_track,
        info_sb_thumb,
        close_btn: Entity::PLACEHOLDER,
        log_panel,
        log_text,
        logs_icon,
        logs_btn,
    }
}

/// The text of the log panel: the input, a failure if there was one, every diagnostic of the last run, cache reuse.
fn log_report(st: &ConvertState) -> String {
    let mut s = String::new();
    if let Some(p) = &st.input {
        s += &format!("Input: {}\n", p.display());
    }
    if let Status::Failed(e) = &st.status {
        s += &format!("\nFAILED: {e}\n");
    }
    if st.diags.is_empty() {
        s += "\nNo messages from the converter for this run (yet).\n";
    }
    for d in &st.diags {
        let tag = match d.severity {
            Severity::Info => "info",
            Severity::Warning => "warning",
            Severity::Attention => "ATTENTION",
        };
        s += &format!("\n[{tag}] {}: {}\n", d.code.as_str(), d.message);
    }
    if let Some(r) = &st.report {
        if !r.cache_hits.is_empty() {
            s += &format!("\nReused from cache: {}\n", r.cache_hits.join(", "));
        }
    }
    s
}

// ---- per-frame upkeep ------------------------------------------------------------------------------

/// Screen switching (title bar buttons, panel header, sidebar), the progress bar, and the values shown in the sidebar.
#[allow(clippy::too_many_arguments)]
pub fn conv_ui_system(
    st: Res<ConvertState>,
    layout: Res<Layout>,
    defs: Res<ParamRows>,
    refs: Res<ConvUiRefs>,
    dd: Res<DropdownOpen>,
    pipe: Res<IconPipe>,
    font: Res<IconFont>,
    lp: Res<LogPanel>,
    sections: Res<OpenSections>,
    pw: Res<PropsWidth>,
    mut nodes: Query<&mut Node>,
    mut texts: Query<&mut Text>,
    mut bgs: Query<&mut BackgroundColor>,
    mut text_colors: Query<&mut TextColor>,
) {
    if !st.is_changed()
        && !layout.is_changed()
        && !dd.is_changed()
        && !pipe.is_changed()
        && !lp.is_changed()
        && !sections.is_changed()
        && !pw.is_changed()
    {
        return;
    }
    let conv = layout.converter && st.input.is_some();
    let flex = |b: bool| if b { Display::Flex } else { Display::None };
    let show = |nodes: &mut Query<&mut Node>, e: Entity, d: Display| {
        if let Ok(mut n) = nodes.get_mut(e) {
            if n.display != d {
                n.display = d;
            }
        }
    };
    show(&mut nodes, refs.btn_settings, flex(!layout.converter));
    show(&mut nodes, refs.btn_back, flex(layout.converter));
    show(&mut nodes, refs.close_x, flex(!layout.converter));
    show(&mut nodes, refs.panel_header, flex(!layout.converter));
    show(&mut nodes, refs.sidebar, flex(conv));
    show(&mut nodes, refs.log_panel, flex(conv && lp.open));
    show(&mut nodes, refs.logs_btn, flex(conv));
    // The floating preview-options button: in the editor and under the list screen's preview.
    show(
        &mut nodes,
        refs.eye_btn,
        flex(conv || (!layout.converter && layout.preview_open)),
    );
    show(&mut nodes, refs.props_splitter, flex(conv));
    if let Ok(mut n) = nodes.get_mut(refs.sidebar) {
        let w = Val::Px(pw.0);
        if n.width != w {
            n.width = w;
        }
    }
    // The preview options fly-out sits above the eye button (bottom right of the preview), on both screens; it is
    // placed there when built, so nothing needs to move it.
    if let Ok(mut n) = nodes.get_mut(refs.close_btn) {
        // The X is a 32 px square; on the converter screen the button sizes itself to the arrow and "Return" (a fixed
        // width used to cut the text off) with some padding.
        let (w, pad) = if layout.converter {
            (Val::Auto, UiRect::horizontal(Val::Px(12.0)))
        } else {
            (Val::Px(26.0), UiRect::ZERO)
        };
        if n.width != w {
            n.width = w;
        }
        if n.padding != pad {
            n.padding = pad;
        }
        if n.flex_shrink != 0.0 {
            n.flex_shrink = 0.0;
        }
    }

    // Progress bar.
    // 0 = working, 1 = failed (red), 2 = generated and saved (green)
    let (visible, fraction, text, kind) = match &st.status {
        Status::Idle => (false, 0.0, String::new(), 0),
        Status::Inspecting => (true, 0.03, "Reading the model".to_string(), 0),
        Status::Queued => {
            let t = if st.stopping() {
                "Stopping the previous run..."
            } else {
                "Waiting for changes to settle..."
            };
            (true, 0.0, t.to_string(), 0)
        }
        Status::Running { fraction, message } => (
            true,
            *fraction,
            format!("{:.0}%   {}", fraction * 100.0, message),
            0,
        ),
        Status::Done => (true, 1.0, "Generated successfully".to_string(), 2),
        Status::Failed(m) => (
            true,
            1.0,
            m.lines().next().unwrap_or("Conversion failed").to_string(),
            1,
        ),
    };
    show(&mut nodes, refs.progress, flex(conv && visible));
    if let Ok(mut n) = nodes.get_mut(refs.bar_fill) {
        let w = Val::Percent(fraction.clamp(0.0, 1.0) * 100.0);
        if n.width != w {
            n.width = w;
        }
    }
    if let Ok(mut b) = bgs.get_mut(refs.bar_fill) {
        let c = match kind {
            1 => Color::srgba(0.80, 0.25, 0.25, 0.70),
            2 => Color::srgba(0.20, 0.68, 0.32, 0.80),
            _ => Color::srgba(0.30, 0.58, 0.98, 0.55),
        };
        if b.0 != c {
            b.0 = c;
        }
    }
    if let Ok(mut t) = texts.get_mut(refs.bar_text) {
        if t.0 != text {
            t.0 = text;
        }
    }

    // Sidebar rows.
    if !conv {
        return;
    }
    if let Ok(mut c) = text_colors.get_mut(refs.logs_icon) {
        let want = if lp.open { ACCENT } else { TEXT };
        if c.0 != want {
            c.0 = want;
        }
    }
    if lp.open {
        set_text(&mut texts, Some(refs.log_text), log_report(&st));
    }
    set_text(
        &mut texts,
        Some(refs.regen_text),
        if pipe.regen_busy() {
            "Generating..."
        } else {
            "Capture as icon"
        }
        .to_string(),
    );
    set_text(&mut texts, Some(refs.name_text), model_name(&st));
    if let Ok(mut c) = text_colors.get_mut(refs.regen_text) {
        let want = if st.saved.is_some() { TEXT } else { DIM };
        if c.0 != want {
            c.0 = want;
        }
    }
    let open = |sec: Option<usize>| {
        sec.map(|s| sections.0.get(s).copied().unwrap_or(false))
            .unwrap_or(true)
    };
    for (idx, (row, r)) in defs.0.iter().zip(refs.rows.iter()).enumerate() {
        let def = match row {
            Row::Header(_) => {
                set_text(
                    &mut texts,
                    r.chev,
                    font.text(if open(r.section) {
                        g::EXPAND_MORE
                    } else {
                        g::CHEVRON_RIGHT
                    }),
                );
                continue;
            }
            Row::Param(def) => def,
        };
        show(&mut nodes, r.node, flex(open(r.section)));
        if let Some(o) = r.overlay {
            show(&mut nodes, o, flex(!(def.show)(&st.opts)));
        }
        match &def.kind {
            ParamKind::Toggle { get, .. } => {
                if let Some(e) = r.mark {
                    if let Ok(mut b) = bgs.get_mut(e) {
                        let c = if get(&st.opts) { ACCENT } else { OFF };
                        if b.0 != c {
                            b.0 = c;
                        }
                    }
                }
            }
            ParamKind::Choice { labels, get, .. } => {
                let cur = get(&st.opts).min(labels.len() - 1);
                set_text(&mut texts, r.value, labels[cur].to_string());
                set_text(
                    &mut texts,
                    r.chev,
                    font.text(if dd.0 == Some(idx) {
                        g::EXPAND_LESS
                    } else {
                        g::EXPAND_MORE
                    }),
                );
                if let Some(cks) = refs.option_checks.get(idx) {
                    for (k, e) in cks.iter().enumerate() {
                        if let Ok(mut c) = text_colors.get_mut(*e) {
                            let want = if k == cur { ACCENT } else { Color::NONE };
                            if c.0 != want {
                                c.0 = want;
                            }
                        }
                    }
                }
                if let Some(list) = r.list {
                    show(&mut nodes, list, flex(dd.0 == Some(idx)));
                }
                if let Some(ots) = refs.option_texts.get(idx) {
                    for (k, e) in ots.iter().enumerate() {
                        if let Ok(mut c) = text_colors.get_mut(*e) {
                            let want = if k == cur {
                                TextColor(ACCENT)
                            } else {
                                TextColor(TEXT)
                            };
                            if c.0 != want.0 {
                                *c = want;
                            }
                        }
                    }
                }
            }
            ParamKind::Slider {
                min,
                max,
                decimals,
                unit,
                get,
                ..
            } => {
                let v = get(&st.opts);
                set_text(
                    &mut texts,
                    r.value,
                    format!("{}{}", fmt_num(*decimals, v), unit),
                );
                if let Some(e) = r.fill {
                    if let Ok(mut n) = nodes.get_mut(e) {
                        let w = Val::Percent(
                            (((v - min) / (max - min)).clamp(0.0, 1.0) * 100.0) as f32,
                        );
                        if n.width != w {
                            n.width = w;
                        }
                    }
                }
            }
        }
    }
}

/// The model's name for the properties panel header.
pub(crate) fn model_name(st: &ConvertState) -> String {
    // Once saved, the name the converter really used (sanitised, maybe with `_gen`); before that the name the user typed.
    if matches!(st.status, Status::Done) {
        if let Some(r) = &st.report {
            return r.name.clone();
        }
    }
    if let Some(n) = &st.opts.output_name {
        return n.clone();
    }
    if let Some(r) = &st.report {
        return r.name.clone();
    }
    st.input
        .as_ref()
        .and_then(|p| p.file_stem())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// A number without trailing zeros: 200 -> "200", 0.5 -> "0.5", 1.2345 -> "1.23".
fn num(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{v:.0}")
    } else {
        let s = format!("{v:.2}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// One group of the info tree: a title and its (label, value) items. An item with an empty label shows just the value.
#[derive(Clone, PartialEq, Default)]
pub struct InfoGroup {
    title: &'static str,
    items: Vec<(String, String)>,
}

/// The info box of the properties panel as a tree: the `properties.cfg` properties that are IN EFFECT for the current
/// settings, grouped (Material, Durability, Lamp, Emissive, Textures). In effect means what the game will actually use:
/// lamp values only when the model is a lamp, health and resistances only when it is not indestructible, the emissive
/// strength only when the model has an emissive map.
fn info_groups(st: &ConvertState) -> Vec<InfoGroup> {
    use votv_convert::model_core::{TextureFilter, PHYSICAL_MATERIALS};
    const FILTERS: [&str; 4] = ["Nearest", "Bilinear", "Trilinear", "Default"];
    let p = &st.opts.properties;
    let stats = st.report.as_ref().map(|r| &r.stats);
    let item = |l: &str, v: String| (l.to_string(), v);
    let mut groups: Vec<InfoGroup> = Vec::new();

    let material = PHYSICAL_MATERIALS
        .get(p.physical_material as usize)
        .copied()
        .unwrap_or("?");
    let mut items = vec![item("Type", material.to_string())];
    if p.is_edible() {
        items.push(item(
            "Edible",
            match stats {
                Some(s) => format!("yes, {} bites", s.bites),
                None => "yes".to_string(),
            },
        ));
    }
    groups.push(InfoGroup {
        title: "Material",
        items,
    });

    if p.indestructible {
        groups.push(InfoGroup {
            title: "Durability",
            items: vec![item("", "Indestructible".to_string())],
        });
    } else {
        let health = match stats.and_then(|s| s.health_in_game) {
            Some(h) => num(h),
            None if p.health > 0.0 => num(p.health),
            None => "auto".to_string(),
        };
        groups.push(InfoGroup {
            title: "Durability",
            items: vec![
                item("Health", health),
                item("Impact res.", num(p.impact_resistance)),
                item("Damage res.", num(p.damage_resistance)),
            ],
        });
    }

    if p.is_lamp {
        let c = p.lamp_color;
        let hex = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        groups.push(InfoGroup {
            title: "Lamp",
            items: vec![
                item(
                    "Mode",
                    if p.light_toggle {
                        "switchable"
                    } else {
                        "always lit"
                    }
                    .to_string(),
                ),
                item(
                    "Colour",
                    format!("#{:02X}{:02X}{:02X}", hex(c.r), hex(c.g), hex(c.b)),
                ),
                item("Intensity", num(p.lamp_intensity)),
                item("Radius", num(p.lamp_attenuation)),
                item(
                    "Shadows",
                    if p.lamp_shadows { "on" } else { "off" }.to_string(),
                ),
            ],
        });
    }

    if stats.is_some_and(|s| s.has_emissive) {
        groups.push(InfoGroup {
            title: "Emissive",
            items: vec![item("Strength", num(p.emissive_strength))],
        });
    }

    let name = |f: TextureFilter| {
        FILTERS
            .get(f.0 as usize)
            .copied()
            .unwrap_or("?")
            .to_string()
    };
    let all = [
        p.filter_diffuse_,
        p.filter_pbr_,
        p.filter_normal_,
        p.filter_emissive_,
    ];
    let items = if all.iter().all(|f| *f == all[0]) {
        vec![item("Filter", name(all[0]))]
    } else {
        vec![
            item("Diffuse", name(all[0])),
            item("PBR", name(all[1])),
            item("Normal", name(all[2])),
            item("Emissive", name(all[3])),
        ]
    };
    groups.push(InfoGroup {
        title: "Textures",
        items,
    });
    groups
}

const TREE_LINE: Color = Color::srgb(0.36, 0.38, 0.44);
const TREE_GUTTER: f32 = 14.0; // width of the column that holds the branch lines
const TREE_STUB_Y: f32 = 7.0; // height of the horizontal branch, from the row top: the middle of the first text line

/// Draws the info tree into the info box. It is rebuilt only when its content changes (the settings changed, a report
/// arrived), not every frame. Branch lines are small coloured nodes (the built-in fonts have no box-drawing characters):
/// a vertical line down the left of a group's items (to the elbow only for the last item) and a short horizontal stub.
pub fn info_tree_system(
    mut commands: Commands,
    st: Res<ConvertState>,
    refs: Res<ConvUiRefs>,
    mut last: Local<Option<Vec<InfoGroup>>>,
) {
    let groups = info_groups(&st);
    if last.as_ref() == Some(&groups) {
        return;
    }
    let list = refs.info_text;
    commands.entity(list).despawn_related::<Children>();
    let font = |size: f32| TextFont {
        font_size: size,
        ..default()
    };
    for g in &groups {
        let node = child(
            &mut commands,
            list,
            (
                Node {
                    flex_direction: FlexDirection::Column,
                    ..default()
                },
                pass(),
            ),
        );
        child(&mut commands, node, (txt(g.title, 12.0, TEXT), pass()));
        for (i, (label, value)) in g.items.iter().enumerate() {
            let last_item = i + 1 == g.items.len();
            let row = child(
                &mut commands,
                node,
                (
                    Node {
                        flex_direction: FlexDirection::Row,
                        padding: UiRect::vertical(Val::Px(1.0)),
                        ..default()
                    },
                    pass(),
                ),
            );
            let gutter = child(
                &mut commands,
                row,
                (
                    Node {
                        width: Val::Px(TREE_GUTTER),
                        flex_shrink: 0.0,
                        ..default()
                    },
                    Pickable::IGNORE,
                ),
            );
            child(
                &mut commands,
                gutter,
                (
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(5.0),
                        top: Val::Px(0.0),
                        width: Val::Px(1.0),
                        height: if last_item {
                            Val::Px(TREE_STUB_Y + 1.0)
                        } else {
                            Val::Percent(100.0)
                        },
                        ..default()
                    },
                    BackgroundColor(TREE_LINE),
                    Pickable::IGNORE,
                ),
            );
            child(
                &mut commands,
                gutter,
                (
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(5.0),
                        top: Val::Px(TREE_STUB_Y),
                        width: Val::Px(TREE_GUTTER - 6.0),
                        height: Val::Px(1.0),
                        ..default()
                    },
                    BackgroundColor(TREE_LINE),
                    Pickable::IGNORE,
                ),
            );
            // The label (dim) and the value (accent) are one text, so a long value wraps as a whole.
            // flex_basis 0: the text takes exactly the width left of the gutter and wraps there.
            let text_node = Node {
                flex_grow: 1.0,
                flex_basis: Val::Px(0.0),
                min_width: Val::Px(0.0),
                ..default()
            };
            if label.is_empty() {
                child(
                    &mut commands,
                    row,
                    (txt(value, 12.0, ACCENT), text_node, pass()),
                );
            } else {
                let t = child(
                    &mut commands,
                    row,
                    (txt(&format!("{label}: "), 12.0, DIM), text_node, pass()),
                );
                child(
                    &mut commands,
                    t,
                    (TextSpan::new(value.clone()), font(12.0), TextColor(ACCENT)),
                );
            }
        }
    }
    *last = Some(groups);
}

/// Shows the saved model's icon.png in the properties panel (reloaded after a save or an icon regeneration). A model
/// that has no icon yet gets one generated automatically, once.
#[allow(clippy::too_many_arguments)]
pub fn props_icon_system(
    mut commands: Commands,
    st: Res<ConvertState>,
    layout: Res<Layout>,
    refs: Res<ConvUiRefs>,
    mut pipe: ResMut<IconPipe>,
    mut images: ResMut<Assets<Image>>,
    mut nodes: Query<&mut Node>,
    edit: Res<IconEdit>,
    mut last: Local<(Option<PathBuf>, bool, bool, u32)>,
    mut requested: Local<Option<PathBuf>>,
) {
    let saved = if layout.converter {
        st.saved.clone()
    } else {
        None
    };
    let busy = pipe.regen_busy();
    let key = (
        saved.clone(),
        busy,
        matches!(st.status, Status::Done),
        edit.reload,
    );
    if *last == key {
        return;
    }
    *last = key;
    let hint = |nodes: &mut Query<&mut Node>, shown: bool| {
        if let Ok(mut n) = nodes.get_mut(refs.icon_hint) {
            let d = if shown { Display::Flex } else { Display::None };
            if n.display != d {
                n.display = d;
            }
        }
    };
    let Some(dir) = saved else {
        commands.entity(refs.icon_box).remove::<ImageNode>();
        hint(&mut nodes, true);
        return;
    };
    if busy {
        return; // a new icon is being rendered / written; this runs again when it is done
    }
    match icons::read_icon(&dir) {
        Some(rgba) => {
            let img = Image::new(
                Extent3d {
                    width: icons::ICON_PX,
                    height: icons::ICON_PX,
                    depth_or_array_layers: 1,
                },
                TextureDimension::D2,
                rgba,
                TextureFormat::Rgba8UnormSrgb,
                RenderAssetUsages::default(),
            );
            commands
                .entity(refs.icon_box)
                .insert(ImageNode::new(images.add(img)));
            hint(&mut nodes, false);
        }
        None => {
            commands.entity(refs.icon_box).remove::<ImageNode>();
            hint(&mut nodes, true);
            if requested.as_ref() != Some(&dir) {
                *requested = Some(dir.clone());
                icons::request_regen(&mut pipe, dir, None, 0, None); // first save of this model: make its icon in the game's view
            }
        }
    }
}

// ---- custom model icon: drop a PNG or pick one -------------------------------------------------------------

/// State of the "set the icon from a PNG" feature.
#[derive(Resource, Default)]
pub struct IconEdit {
    /// Bumped whenever the saved model's `icon.png` is replaced by hand, so `props_icon_system` reloads it.
    pub reload: u32,
    /// A PNG is being dragged over the window (it lights up the icon).
    pub png_hover: bool,
    /// The result of the "choose a PNG" dialog (it runs on its own thread, like the other pickers).
    pick: Mutex<Option<std::sync::mpsc::Receiver<PathBuf>>>,
}

/// Writes `png` as `dir/icon.png`: 128 x 128 like the icons the game makes. Another size is scaled to fit inside the
/// square, keeping its proportions, on a transparent background. Written to a temp file first, so a failure never
/// leaves a broken icon.
fn write_icon_png(png: &Path, dir: &Path) -> Result<(), String> {
    let img = image::open(png)
        .map_err(|e| format!("Could not read the PNG: {e}"))?
        .to_rgba8();
    if img.pixels().all(|p| p.0[3] == 0) {
        return Err("The PNG is completely transparent".to_string());
    }
    let px = icons::ICON_PX;
    let out = if img.width() == px && img.height() == px {
        img
    } else {
        let s = (px as f32 / img.width() as f32).min(px as f32 / img.height() as f32);
        let nw = ((img.width() as f32 * s).round() as u32).clamp(1, px);
        let nh = ((img.height() as f32 * s).round() as u32).clamp(1, px);
        let scaled = image::imageops::resize(&img, nw, nh, image::imageops::FilterType::Lanczos3);
        let mut canvas = image::RgbaImage::new(px, px); // transparent
        image::imageops::overlay(
            &mut canvas,
            &scaled,
            ((px - nw) / 2) as i64,
            ((px - nh) / 2) as i64,
        );
        canvas
    };
    let tmp = dir.join("icon.png.tmp");
    out.save_with_format(&tmp, image::ImageFormat::Png)
        .map_err(|e| format!("Could not write the icon: {e}"))?;
    std::fs::rename(&tmp, dir.join(icons::ICON_FILE)).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("Could not replace icon.png: {e}")
    })
}

/// Sets the open model's icon from `png` (status line message on failure).
fn apply_icon(png: &Path, conv: &mut ConvertState, layout: &mut Layout, edit: &mut IconEdit) {
    let Some(dir) = conv.saved.clone() else {
        layout.notice = Some(
            "The model is not saved yet: wait for the conversion to finish, then set the icon"
                .to_string(),
        );
        return;
    };
    match write_icon_png(png, &dir) {
        Ok(()) => {
            layout.notice = None;
            conv.saved_any = true; // the list is rescanned when going back, so the tile shows the new icon too
            edit.reload = edit.reload.wrapping_add(1);
        }
        Err(e) => layout.notice = Some(e),
    }
}

/// Click on the icon: choose a PNG in the file explorer.
fn on_icon_click(t: Trigger<Pointer<Click>>, st: Res<ConvertState>, edit: Res<IconEdit>) {
    if t.event().button != PointerButton::Primary || st.saved.is_none() {
        return;
    }
    let mut slot = edit.pick.lock().unwrap();
    if slot.is_some() {
        return; // a dialog is already open
    }
    let (tx, rx) = std::sync::mpsc::channel();
    *slot = Some(rx);
    std::thread::spawn(move || {
        if let Some(p) = rfd::FileDialog::new()
            .set_title("Choose a PNG image for the model's icon")
            .add_filter("PNG image", &["png"])
            .pick_file()
        {
            let _ = tx.send(p);
        }
    });
}

fn is_png(p: &Path) -> bool {
    p.extension().is_some_and(|e| e.eq_ignore_ascii_case("png"))
}

/// A PNG dropped on the editor (anywhere in the window: the OS does not say where) or chosen in the dialog becomes the
/// model's icon. Also tracks whether a PNG is being dragged over the window, for the highlight. `ui::drop_events` skips
/// PNGs while the editor is open, so they do not get its "only .glb files" message.
pub fn icon_input_system(
    mut events: EventReader<FileDragAndDrop>,
    mut edit: ResMut<IconEdit>,
    mut conv: ResMut<ConvertState>,
    mut layout: ResMut<Layout>,
) {
    for e in events.read() {
        match e {
            FileDragAndDrop::HoveredFile { path_buf, .. } => {
                let v = is_png(path_buf);
                if edit.png_hover != v {
                    edit.png_hover = v;
                }
            }
            FileDragAndDrop::HoveredFileCanceled { .. } => {
                if edit.png_hover {
                    edit.png_hover = false;
                }
            }
            FileDragAndDrop::DroppedFile { path_buf, .. } => {
                if edit.png_hover {
                    edit.png_hover = false;
                }
                if is_png(path_buf) && layout.converter && conv.input.is_some() {
                    apply_icon(path_buf, &mut conv, &mut layout, &mut edit);
                }
            }
        }
    }
    // The dialog's result (a cancelled dialog just drops its sender).
    let picked = {
        let mut slot = edit.pick.lock().unwrap();
        match slot.as_ref().map(|rx| rx.try_recv()) {
            Some(Ok(p)) => {
                *slot = None;
                Some(p)
            }
            Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => {
                *slot = None;
                None
            }
            _ => None,
        }
    };
    if let Some(p) = picked {
        apply_icon(&p, &mut conv, &mut layout, &mut edit);
    }
}

/// The cover over the icon: "Click to choose a PNG or drop one here" while the pointer is on it, "Drop the PNG to set
/// the icon" (and an accent border) while a PNG is dragged over the window. Only when the model is saved.
#[allow(clippy::too_many_arguments)]
pub fn icon_hint_system(
    edit: Res<IconEdit>,
    st: Res<ConvertState>,
    layout: Res<Layout>,
    refs: Res<ConvUiRefs>,
    ints: Query<&Interaction>,
    mut nodes: Query<&mut Node>,
    mut texts: Query<&mut Text>,
    mut borders: Query<&mut BorderColor>,
) {
    let active = layout.converter && st.saved.is_some();
    let hovered = ints
        .get(refs.icon_box)
        .map(|i| *i != Interaction::None)
        .unwrap_or(false);
    let dragging = active && edit.png_hover;
    let want = if active && (hovered || dragging) {
        Display::Flex
    } else {
        Display::None
    };
    if let Ok(mut n) = nodes.get_mut(refs.icon_overlay) {
        if n.display != want {
            n.display = want;
        }
    }
    set_text(
        &mut texts,
        Some(refs.icon_overlay_text),
        if dragging {
            "Drop the PNG\nto set the icon"
        } else {
            "Click to choose a PNG\nor drop one here"
        }
        .to_string(),
    );
    let border = if dragging { ACCENT } else { EDGE_HOVER };
    if let Ok(mut b) = borders.get_mut(refs.icon_box) {
        if b.0 != border {
            b.0 = border;
        }
    }
}

fn set_text(texts: &mut Query<&mut Text>, e: Option<Entity>, s: String) {
    if let Some(e) = e {
        if let Ok(mut t) = texts.get_mut(e) {
            if t.0 != s {
                t.0 = s;
            }
        }
    }
}

/// Shows the Edit button in the preview header when the selected model has a .glb saved in its folder.
#[allow(clippy::too_many_arguments)]
pub fn edit_button_system(
    lib: Res<Library>,
    layout: Res<Layout>,
    refs: Res<ConvUiRefs>,
    mut target: ResMut<EditTarget>,
    mut nodes: Query<&mut Node>,
    mut last: Local<(Option<usize>, u64, bool)>,
) {
    let key = (lib.selected, lib.generation, layout.converter);
    if *last != key {
        *last = key;
        let t = if layout.converter {
            None
        } else {
            lib.selected
                .and_then(|i| lib.entries.get(i))
                .and_then(|e| find_glb(&e.dir))
        };
        if target.0 != t {
            target.0 = t;
        }
    }
    let want = if target.0.is_some() && layout.preview_open && !layout.converter {
        Display::Flex
    } else {
        Display::None
    };
    if let Ok(mut n) = nodes.get_mut(refs.btn_edit) {
        if n.display != want {
            n.display = want;
        }
    }
}

/// Sizes and places a scrollbar thumb from the scroll area's position; hidden when everything fits.
fn place_thumb(sp: &ScrollPosition, area: &ComputedNode, track: &ComputedNode, n: &mut Node) {
    let (visible, content, track_h) = scroll_metrics(area, track);
    let max = (content - visible).max(0.0);
    if max < 1.0 || track_h < 30.0 {
        if n.display != Display::None {
            n.display = Display::None;
        }
        return;
    }
    let h = thumb_height(visible, content, track_h);
    let top = (sp.offset_y.clamp(0.0, max) / max) * (track_h - h);
    if n.display != Display::Flex {
        n.display = Display::Flex;
    }
    if n.height != Val::Px(h) {
        n.height = Val::Px(h);
    }
    if n.top != Val::Px(top) {
        n.top = Val::Px(top);
    }
}

/// Scrolls `area` by a thumb drag of `delta_y` pixels along `track`.
fn drag_thumb(
    delta_y: f32,
    area: Entity,
    track: Entity,
    nodes: &Query<&ComputedNode>,
    sps: &mut Query<&mut ScrollPosition>,
) {
    let (Ok(g), Ok(tr)) = (nodes.get(area), nodes.get(track)) else {
        return;
    };
    let (visible, content, track_h) = scroll_metrics(g, tr);
    let max = (content - visible).max(0.0);
    let travel = (track_h - thumb_height(visible, content, track_h)).max(1.0);
    if let Ok(mut sp) = sps.get_mut(area) {
        sp.offset_y = (sp.offset_y + delta_y * max / travel).clamp(0.0, max);
    }
}

/// Size and place the sidebar's scrollbar thumb; hidden when everything fits.
pub fn sidebar_scrollbar_system(
    refs: Res<ConvUiRefs>,
    scrolls: Query<(&ScrollPosition, &ComputedNode)>,
    computed: Query<&ComputedNode>,
    mut nodes: Query<&mut Node>,
) {
    let (Ok((sp, g)), Ok(tr)) = (scrolls.get(refs.scroll), computed.get(refs.sb_track)) else {
        return;
    };
    if let Ok(mut n) = nodes.get_mut(refs.sb_thumb) {
        place_thumb(sp, g, tr, &mut n);
    }
}

/// Scrolling speed of a long model name while the pointer is over it (logical px per second), the speed of the way back,
/// and the pause before it starts.
const MARQUEE_SPEED: f32 = 38.0;
const MARQUEE_BACK: f32 = 150.0;
const MARQUEE_DELAY: f32 = 0.45;

/// The model name: while the pointer is over a name that does not fit, it scrolls slowly and smoothly to its end (after a
/// short pause) and glides back when the pointer leaves. It is only a scroll position: the text ignores the pointer, so
/// the pencil (rename) and everything else behave the same. While the name is edited it stays at 0 (textedit.rs scrolls the
/// text itself) and the box gets its field look (dark background; textedit.rs paints the border).
pub fn name_marquee_system(
    time: Res<Time<Real>>,
    refs: Res<ConvUiRefs>,
    ed: Res<Editing>,
    hovers: Query<&Interaction>,
    scrolls: Query<&ComputedNode>,
    mut positions: Query<&mut ScrollPosition>,
    mut bgs: Query<&mut BackgroundColor>,
    mut state: Local<(f32, f32)>, // (offset, seconds the pointer has been over the name)
    mut redraw: EventWriter<RequestRedraw>,
) {
    let editing = ed.is(Field::Rename);
    if let Ok(mut bg) = bgs.get_mut(refs.name_box) {
        let want = if editing { Color::srgb(0.10, 0.11, 0.13) } else { Color::NONE };
        if bg.0 != want {
            bg.0 = want;
        }
    }
    let (Ok(view), Ok(mut sp)) = (scrolls.get(refs.name_scroll), positions.get_mut(refs.name_scroll)) else {
        return;
    };
    let max = ((view.content_size.x - view.size.x) * view.inverse_scale_factor).max(0.0);
    let hovered = !editing && max > 1.0 && hovers.get(refs.name_box).is_ok_and(|i| *i != Interaction::None);
    let dt = time.delta_secs().min(0.05); // the window redraws on demand: never jump after an idle moment
    let (mut offset, mut held) = *state;
    let mut animating = false;
    if editing {
        offset = 0.0;
        held = 0.0;
    } else if hovered {
        held += dt;
        if held > MARQUEE_DELAY && offset < max {
            offset = (offset + MARQUEE_SPEED * dt).min(max);
        }
        animating = held <= MARQUEE_DELAY || offset < max;
    } else {
        held = 0.0;
        if offset > 0.0 {
            offset = (offset - MARQUEE_BACK * dt).max(0.0);
            animating = offset > 0.0;
        }
    }
    offset = offset.min(max);
    *state = (offset, held);
    if sp.offset_x != offset {
        sp.offset_x = offset;
    }
    if animating {
        redraw.write(RequestRedraw); // keep the animation running while the window waits for input
    }
}

/// The same for the scrollbar of the info box under the model name.
pub fn info_scrollbar_system(
    refs: Res<ConvUiRefs>,
    scrolls: Query<(&ScrollPosition, &ComputedNode)>,
    computed: Query<&ComputedNode>,
    mut nodes: Query<&mut Node>,
) {
    let (Ok((sp, g)), Ok(tr)) = (
        scrolls.get(refs.info_scroll),
        computed.get(refs.info_sb_track),
    ) else {
        return;
    };
    if let Ok(mut n) = nodes.get_mut(refs.info_sb_thumb) {
        place_thumb(sp, g, tr, &mut n);
    }
}

/// Dragging the handle at the left edge of the properties panel resizes it (left = wider).
fn props_splitter_drag(
    t: Trigger<Pointer<Drag>>,
    mut w: ResMut<PropsWidth>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    let window_w = windows.single().map(|win| win.width()).unwrap_or(1200.0);
    let max = (window_w - PREVIEW_MIN - 6.0).clamp(PROPS_MIN, PROPS_MAX);
    let new = (w.0 - t.event().delta.x).clamp(PROPS_MIN, max);
    if (new - w.0).abs() > 0.01 {
        w.0 = new;
    }
}

fn sidebar_thumb_drag(
    t: Trigger<Pointer<Drag>>,
    refs: Res<ConvUiRefs>,
    nodes: Query<&ComputedNode>,
    mut sps: Query<&mut ScrollPosition>,
) {
    drag_thumb(
        t.event().delta.y,
        refs.scroll,
        refs.sb_track,
        &nodes,
        &mut sps,
    );
}

fn info_thumb_drag(
    t: Trigger<Pointer<Drag>>,
    refs: Res<ConvUiRefs>,
    nodes: Query<&ComputedNode>,
    mut sps: Query<&mut ScrollPosition>,
) {
    drag_thumb(
        t.event().delta.y,
        refs.info_scroll,
        refs.info_sb_track,
        &nodes,
        &mut sps,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snap_rounds_to_step_and_clamps() {
        assert_eq!(snap(13.0, 8.0, 256.0, 8.0), 16.0);
        assert_eq!(snap(2.0, 8.0, 256.0, 8.0), 8.0);
        assert_eq!(snap(999.0, 8.0, 256.0, 8.0), 256.0);
        assert!((snap(0.74, 0.0, 50.0, 0.5) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn icon_png_is_scaled_to_fit_the_icon_square() {
        let dir = std::env::temp_dir().join(format!("votv-icon-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // 300 x 100, opaque red: scaled to 128 x 43, centred, transparent above and below.
        let src = dir.join("wide.png");
        image::RgbaImage::from_pixel(300, 100, image::Rgba([255, 0, 0, 255]))
            .save(&src)
            .unwrap();
        write_icon_png(&src, &dir).unwrap();
        let out = image::open(dir.join("icon.png")).unwrap().to_rgba8();
        assert_eq!((out.width(), out.height()), (128, 128));
        assert_eq!(out.get_pixel(64, 64).0[3], 255);
        assert_eq!(out.get_pixel(64, 2).0[3], 0);
        assert!(!dir.join("icon.png.tmp").exists());
        // A completely transparent image is refused and leaves the icon alone.
        let clear = dir.join("clear.png");
        image::RgbaImage::new(16, 16).save(&clear).unwrap();
        assert!(write_icon_png(&clear, &dir).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn slider_pointer_and_keys() {
        // A 200 px track from x = 50: the ends are min and max, the middle is the middle, clamped outside.
        assert_eq!(value_at(50.0, 50.0, 200.0, 0.0, 10.0, 1.0), 0.0);
        assert_eq!(value_at(250.0, 50.0, 200.0, 0.0, 10.0, 1.0), 10.0);
        assert_eq!(value_at(150.0, 50.0, 200.0, 0.0, 10.0, 1.0), 5.0);
        assert_eq!(value_at(-30.0, 50.0, 200.0, 0.0, 10.0, 1.0), 0.0);
        assert_eq!(value_at(900.0, 50.0, 200.0, 0.0, 10.0, 1.0), 10.0);
        // Arrow keys: one step on the grid, to the next grid value off it, never -0, no 0.30000000000000004.
        assert_eq!(nudge(3.0, 1.0, 1.0), 4.0);
        assert_eq!(nudge(3.0, 1.0, -1.0), 2.0);
        assert_eq!(nudge(3.2, 0.5, 1.0), 3.5);
        assert_eq!(nudge(3.2, 0.5, -1.0), 3.0);
        assert_eq!(nudge(0.2, 0.1, 1.0), 0.3);
        assert!(nudge(1.0, 1.0, -1.0).is_sign_positive());
        assert_eq!(nudge(1000.0, 1.0, 1.0), 1001.0); // typed values outside the drawn range keep working
    }

    #[test]
    fn no_negative_zero() {
        assert!(snap(-0.3, -5.0, 5.0, 1.0).is_sign_positive());
        assert_eq!(fmt_num(0, -0.0), "0");
        assert_eq!(fmt_num(2, -0.004), "0.00");
        assert_eq!(fmt_num(1, -0.5), "-0.5");
        assert_eq!(fmt_num(0, -12.0), "-12");
    }

    #[test]
    fn every_param_reads_back_what_it_writes() {
        let mut o = Options::default();
        for row in rows() {
            let Row::Param(def) = row else { continue };
            match def.kind {
                ParamKind::Toggle { get, set } => {
                    let v = !get(&o);
                    set(&mut o, v);
                    assert_eq!(get(&o), v, "{}", def.label);
                }
                ParamKind::Slider {
                    min,
                    max,
                    step,
                    get,
                    set,
                    ..
                } => {
                    let v = snap((min + max) / 2.0, min, max, step);
                    set(&mut o, v);
                    assert!((get(&o) - v).abs() < 1e-9, "{}", def.label);
                }
                ParamKind::Choice { labels, get, set } => {
                    for i in 0..labels.len() {
                        set(&mut o, i);
                        assert_eq!(get(&o), i, "{}", def.label);
                    }
                }
            }
            assert!(o.validate().is_ok(), "{} left invalid options", def.label);
        }
    }

    #[test]
    fn reset_button_logic_matches_the_defaults() {
        let d = crate::converter::base_options();
        for row in rows() {
            let Row::Param(def) = row else { continue };
            // A fresh options set equals the defaults on every row: no reset button anywhere.
            assert!(is_default(&def, &d, &d), "{}", def.label);
            // Changing the value makes the row differ, and writing the default back makes it equal again.
            let mut o = base_opts_for_test();
            match &def.kind {
                ParamKind::Toggle { get, set } => {
                    set(&mut o, !get(&d));
                    assert!(!is_default(&def, &o, &d), "{}", def.label);
                    set(&mut o, get(&d));
                }
                ParamKind::Slider { get, set, min, max, .. } => {
                    // The bound farthest from the default (always a value the row accepts).
                    let far = if (get(&d) - min).abs() > (max - get(&d)).abs() { *min } else { *max };
                    if (far - get(&d)).abs() > 1e-6 {
                        set(&mut o, far);
                        assert!(!is_default(&def, &o, &d), "{}", def.label);
                    }
                    set(&mut o, get(&d));
                }
                ParamKind::Choice { labels, get, set } => {
                    if labels.len() < 2 {
                        continue;
                    }
                    set(&mut o, (get(&d) + 1) % labels.len());
                    assert!(!is_default(&def, &o, &d), "{}", def.label);
                    set(&mut o, get(&d));
                }
            }
            assert!(is_default(&def, &o, &d), "{}", def.label);
        }
    }

    fn base_opts_for_test() -> Options {
        crate::converter::base_options()
    }
}
