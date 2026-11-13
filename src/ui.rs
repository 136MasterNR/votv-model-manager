//! The window layout from the mockups.
//!
//! List / preview screen:
//!   +--------------------------------------------------------------+
//!   | Title  [Change folder..] [Refresh]   path | count          [X] |  title bar = drag area
//!   +-----------------------------+--+-----------------------------+
//!   | icon grid (scrolls)         |  | [x] model name            [=]|  [x] closes the preview,
//!   |                             |::|                              |  [=] opens the options fly-out
//!   |-----------------------------|  |          PREVIEW             |
//!   | [ Search models...      ]   |  |                              |
//!   +-----------------------------+--+------------------------------+
//!
//! Converter screen (a .glb was dropped; widgets in conv_ui.rs, logic in converter.rs):
//!   +--------------------------------------------------------------+
//!   | Title   file | triangles | hulls | atlas   [Settings] [X]  | on the converter screen the X becomes a back arrow
//!   +--------------------------------------------------+-----------+
//!   |                                                  | PARAMETERS|
//!   |                    PREVIEW                       | (scrolls) |
//!   |                                                  |           |
//!   |            [ generation progress bar ]           |           |  hidden when nothing is happening
//!   +--------------------------------------------------+-----------+
//!
//! The list has a fixed width (drag the splitter to change it); when the window is resized the preview takes the
//! extra space. With the preview closed the list takes the whole width.
//!
//! Window decorations are off; the title bar drags the window and thin handles on every edge and corner
//! resize it (they change the cursor on hover; a dotted grip marks the bottom-right corner). The splitter (::) resizes the preview panel.

use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Mutex;

use bevy::{
    input::mouse::{MouseScrollUnit, MouseWheel},
    math::CompassOctant,
    picking::hover::HoverMap,
    prelude::*,
    text::{JustifyText, LineBreak},
    ui::OverflowAxis,
    window::{FileDragAndDrop, PrimaryWindow, RequestRedraw, SystemCursorIcon},
    // In Bevy 0.17+ this moved to bevy::window::CursorIcon.
    winit::cursor::CursorIcon,
};

use crate::blender::{self, BlenderButton};
use crate::conv_ui::{self, ConvUiRefs, EditTarget, ParamRows};
use crate::converter::{load_sidecar, ConvertState};
use crate::export::{self, HeaderExport, HeaderExportLabel};
use crate::icon_font::{g, icon, IconFont};
use crate::icons::{start_worker, IconPipe};
use crate::listmenu::{self, DropTile, SectionEmpty, SectionHeader};
use crate::model::scan_models;
use crate::model::{sort_entries, SortMode};
use crate::preview::{self, Orbit, PreviewState};
use crate::scene::*;
use crate::{save_config, Layout, Library, TileRefs};

// ---- palette ---------------------------------------------------------------------------------
pub(crate) const BG: Color = Color::srgb(0.10, 0.11, 0.13);
pub(crate) const PANEL: Color = Color::srgb(0.13, 0.14, 0.17);
pub(crate) const BAR: Color = Color::srgb(0.18, 0.19, 0.23);
pub(crate) const BTN: Color = Color::srgb(0.24, 0.26, 0.31);
pub(crate) const BTN_HOVER: Color = Color::srgb(0.31, 0.34, 0.41);
pub(crate) const BTN_PRESS: Color = Color::srgb(0.25, 0.45, 0.80);
/// Items inside a property section: darker than the section header buttons (BTN) so they read as its contents.
pub(crate) const ITEM: Color = Color::srgb(0.085, 0.095, 0.125);
pub(crate) const ITEM_HOVER: Color = Color::srgb(0.15, 0.165, 0.21);

/// Marks a button that uses the darker item colours (see `button_colors`).
#[derive(Component)]
pub struct ItemButton;
pub(crate) const TILE: Color = Color::srgb(0.17, 0.18, 0.22);
pub(crate) const ICON_BG: Color = Color::srgb(0.16, 0.17, 0.20);
pub(crate) const EDGE: Color = Color::srgb(0.22, 0.24, 0.29);
pub(crate) const EDGE_HOVER: Color = Color::srgb(0.45, 0.50, 0.60);
pub(crate) const ACCENT: Color = Color::srgb(0.30, 0.58, 0.98);
/// Edit (blue: "change it") and Export / Enable (green: "take it out / turn it on"); readable on the dark buttons.
pub(crate) const EDIT_COL: Color = Color::srgb(0.47, 0.73, 1.0);
/// The Properties button (a model without a saved .glb: edit `properties.cfg` only, see `propsedit.rs`).
pub(crate) const PROPS_COL: Color = Color::srgb(1.0, 0.62, 0.22);
pub(crate) const EXPORT_COL: Color = Color::srgb(0.46, 0.86, 0.56);
pub(crate) const TEXT: Color = Color::srgb(0.90, 0.91, 0.93);
pub(crate) const DIM: Color = Color::srgb(0.58, 0.60, 0.66);
/// The build number next to the app title: quieter than `DIM`.
const BUILD_GRAY: Color = Color::srgb(0.46, 0.48, 0.53);

const TILE_W: f32 = 132.0;
const ICON_SIZE: f32 = 120.0;
const TILE_H: f32 = 156.0;
const RESIZE_EDGE: f32 = 6.0;
const RESIZE_CORNER: f32 = 12.0;

// ---- components & resources --------------------------------------------------------------------

#[derive(Resource)]
pub struct UiRefs {
    pub grid: Entity,
    pub panel: Entity,
    pub splitter: Entity,
    pub options: Entity,
    pub container: Entity,
    pub hint: Entity,
    pub hint_text: Entity,
    pub header_text: Entity,
    pub limits_box: Entity,          // the printer report (printer.rs) at the preview's top left
    pub limits_lines: [Entity; 4],
    pub status: Entity,
    pub track: Entity,
    pub thumb: Entity,
    pub left: Entity,
    pub drop_overlay: Entity,
    pub conv_drop_overlay: Entity, // same hint over the preview on the converter screen
}

#[derive(Component)]
pub struct Tile(pub usize);
#[derive(Component)]
pub struct PreviewSurface;
#[derive(Component)]
pub struct OptLabel(pub OptKey);

/// A folder dialog running on its own thread; the chosen path arrives through the channel.
#[derive(Resource, Default)]
pub struct FolderPick(pub Mutex<Option<Receiver<std::path::PathBuf>>>);

/// The file dialog of the fake tile (a .glb to convert, or model archives - .zip, .7z, .rar - to copy), running on its own thread.
#[derive(Resource, Default)]
pub struct FilePick(pub Mutex<Option<Receiver<Vec<std::path::PathBuf>>>>);

// ---- small helpers -------------------------------------------------------------------------------

pub(crate) fn txt(s: &str, size: f32, color: Color) -> impl Bundle {
    (
        Text::new(s),
        TextFont {
            font_size: size,
            ..default()
        },
        TextColor(color),
    )
}

pub(crate) fn button(w: Val, h: Val) -> impl Bundle {
    (
        Button,
        Node {
            width: w,
            height: h,
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            flex_shrink: 0.0,
            ..default()
        },
        BackgroundColor(BTN),
        BorderRadius::all(Val::Px(4.0)),
    )
}

pub(crate) fn child(commands: &mut Commands, parent: Entity, b: impl Bundle) -> Entity {
    let e = commands.spawn(b).id();
    commands.entity(parent).add_child(e);
    e
}

/// Children of a scrolling list must not block the pointer from reaching the list below them,
/// or the mouse wheel would stop scrolling when the cursor is over an icon.
pub(crate) fn pass() -> Pickable {
    Pickable {
        should_block_lower: false,
        ..default()
    }
}

pub(crate) fn short(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let t: String = s.chars().take(n.saturating_sub(2)).collect();
        format!("{t}..")
    }
}

fn resize_node(dir: CompassOctant) -> Node {
    use CompassOctant::*;
    let (e, c) = (Val::Px(RESIZE_EDGE), Val::Px(RESIZE_CORNER));
    let (z, a) = (Val::Px(0.0), Val::Auto);
    let (left, right, top, bottom, width, height) = match dir {
        North => (c, c, z, a, a, e),
        South => (c, c, a, z, a, e),
        West => (z, a, c, c, e, a),
        East => (a, z, c, c, e, a),
        NorthWest => (z, a, z, a, c, c),
        NorthEast => (a, z, z, a, c, c),
        SouthWest => (z, a, a, z, c, c),
        SouthEast => (a, z, a, z, c, c),
    };
    Node {
        position_type: PositionType::Absolute,
        left,
        right,
        top,
        bottom,
        width,
        height,
        ..default()
    }
}

fn cursor_for(dir: CompassOctant) -> SystemCursorIcon {
    use CompassOctant::*;
    match dir {
        North | South => SystemCursorIcon::NsResize,
        East | West => SystemCursorIcon::EwResize,
        NorthWest | SouthEast => SystemCursorIcon::NwseResize,
        NorthEast | SouthWest => SystemCursorIcon::NeswResize,
    }
}

pub(crate) fn set_cursor(
    commands: &mut Commands,
    win: &Query<Entity, With<PrimaryWindow>>,
    icon: SystemCursorIcon,
) {
    if let Ok(e) = win.single() {
        commands.entity(e).insert(CursorIcon::from(icon));
    }
}

// ---- observers (button and drag behaviour) ----------------------------------------------------------

fn drag_window(_: Trigger<Pointer<DragStart>>, mut w: Query<&mut Window, With<PrimaryWindow>>) {
    if let Ok(mut win) = w.single_mut() {
        win.start_drag_move();
    }
}
/// Title-bar X: closes the app, or on the converter screen goes back to the list (see `leave_converter`).
fn on_close(
    _: Trigger<Pointer<Click>>,
    mut exit: EventWriter<AppExit>,
    mut l: ResMut<Layout>,
    mut conv: ResMut<ConvertState>,
    mut lib: ResMut<Library>,
) {
    let _bench = crate::benchmark::work("ui.ui.on_close");
    if l.converter {
        leave_converter(&mut l, &mut conv, &mut lib);
    } else {
        exit.write(AppExit::Success);
    }
}
fn on_close_preview(_: Trigger<Pointer<Click>>, mut l: ResMut<Layout>) {
    let _bench = crate::benchmark::work("ui.ui.on_close_preview");
    l.preview_open = false;
}
fn on_toggle_options(_: Trigger<Pointer<Click>>, mut l: ResMut<Layout>) {
    let _bench = crate::benchmark::work("ui.ui.on_toggle_options");
    l.options_open = !l.options_open;
}
/// Converter screen -> list/preview screen. Stops every running job; `preview_request` reloads the selected model.
fn leave_converter(l: &mut Layout, conv: &mut ConvertState, lib: &mut Library) {
    l.converter = false;
    // Back on the list the model that was edited (under its current name) stays selected. (Properties-only mode without
    // a rename rescans nothing, so a name left in `select_name` would hijack the selection of a later rescan.)
    if conv.props.is_none() || conv.saved_any {
        if let Some(name) = conv
            .saved
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
        {
            lib.select_name = Some(name);
        }
    }
    if conv.saved_any {
        lib.dirty = true; // models were saved: rescan the folder so they show up
        conv.saved_any = false;
    }
    conv.end();
}

/// Edit button of the preview: reopen the model's saved .glb in the converter with the options it was made with.
fn on_edit(
    _: Trigger<Pointer<Click>>,
    target: Res<EditTarget>,
    mut l: ResMut<Layout>,
    mut conv: ResMut<ConvertState>,
) {
    let _bench = crate::benchmark::work("ui.ui.on_edit");
    let Some(glb) = target.0.clone() else { return };
    let restored = glb.parent().and_then(load_sidecar);
    l.converter = true;
    l.preview_open = true;
    conv.begin_with(glb.clone(), restored);
    // The model being edited IS this folder: whatever the .glb file is called, the save goes back to the same model.
    conv.opts.output_name = glb
        .parent()
        .and_then(|d| d.file_name())
        .map(|n| n.to_string_lossy().into_owned());
}
/// Properties button of the preview: edit only the `properties.cfg` of a model without a saved .glb (no converter).
fn on_properties(
    t: Trigger<Pointer<Click>>,
    target: Res<crate::propsedit::PropsTarget>,
    mut l: ResMut<Layout>,
    mut conv: ResMut<ConvertState>,
) {
    let _bench = crate::benchmark::work("ui.ui.on_properties");
    if t.event().button != PointerButton::Primary {
        return;
    }
    let Some(dir) = target.0.clone() else { return };
    l.converter = true;
    l.preview_open = true;
    conv.begin_props(dir);
}
fn on_refresh(_: Trigger<Pointer<Click>>, mut lib: ResMut<Library>) {
    let _bench = crate::benchmark::work("ui.ui.on_refresh");
    lib.dirty = true;
}
fn on_pick_glb(t: Trigger<Pointer<Click>>, pick: Res<FilePick>) {
    let _bench = crate::benchmark::work("ui.ui.on_pick_glb");
    if t.event().button != PointerButton::Primary {
        return;
    }
    let mut slot = pick.0.lock().unwrap();
    if slot.is_some() {
        return; // a dialog is already open
    }
    let (tx, rx) = channel();
    *slot = Some(rx);
    std::thread::spawn(move || {
        if let Some(list) = rfd::FileDialog::new()
            .set_title("Choose a .glb to convert, or model .zip, .7z or .rar files to copy")
            .add_filter("glb or archive", &["glb", "zip", "7z", "rar"])
            .pick_files()
        {
            let _ = tx.send(list);
        }
    });
}
fn on_browse(_: Trigger<Pointer<Click>>, lib: Res<Library>, pick: Res<FolderPick>) {
    let _bench = crate::benchmark::work("ui.ui.on_browse");
    let mut slot = pick.0.lock().unwrap();
    if slot.is_some() {
        return; // a dialog is already open
    }
    let (tx, rx) = channel();
    *slot = Some(rx);
    let start = lib.root.clone();
    std::thread::spawn(move || {
        if let Some(p) = rfd::FileDialog::new()
            .set_title("Choose the VotV printer models folder")
            .set_directory(&start)
            .pick_folder()
        {
            let _ = tx.send(p);
        }
    });
}
fn splitter_drag(t: Trigger<Pointer<Drag>>, mut l: ResMut<Layout>) {
    l.list_w = (l.list_w + t.event().delta.x).clamp(260.0, 4000.0);
}

/// Logical pixels: (visible height, content height, track height) of the grid and its scrollbar.
pub(crate) fn scroll_metrics(grid: &ComputedNode, track: &ComputedNode) -> (f32, f32, f32) {
    let isf = grid.inverse_scale_factor;
    (
        grid.size.y * isf,
        grid.content_size.y * isf,
        track.size.y * track.inverse_scale_factor,
    )
}

pub(crate) fn thumb_height(visible: f32, content: f32, track_h: f32) -> f32 {
    if content <= 0.0 {
        return track_h;
    }
    (visible / content * track_h).clamp(28.0, track_h.max(28.0))
}

fn thumb_drag(
    t: Trigger<Pointer<Drag>>,
    refs: Res<UiRefs>,
    nodes: Query<&ComputedNode>,
    mut sps: Query<&mut ScrollPosition>,
) {
    let (Ok(g), Ok(tr)) = (nodes.get(refs.grid), nodes.get(refs.track)) else {
        return;
    };
    let (visible, content, track_h) = scroll_metrics(g, tr);
    let max = (content - visible).max(0.0);
    let travel = (track_h - thumb_height(visible, content, track_h)).max(1.0);
    if let Ok(mut sp) = sps.get_mut(refs.grid) {
        sp.offset_y = (sp.offset_y + t.event().delta.y * max / travel).clamp(0.0, max);
    }
}

// ---- building the window -----------------------------------------------------------------------------

/// The app icon for the title bar: `assets/icon.png` compiled into the exe and decoded once. `None` if it cannot be decoded
/// (the title then shows the text only).
fn title_icon(images: &mut Assets<Image>) -> Option<Handle<Image>> {
    let rgba = image::load_from_memory_with_format(
        include_bytes!("../assets/icon.png"),
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

pub fn setup(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    layout: Res<Layout>,
    opts: Res<Opts>,
    rows: Res<ParamRows>,
    mut fonts: ResMut<Assets<Font>>,
) {
    let _bench = crate::benchmark::work("startup.ui.setup");
    let iconfont = IconFont::new(&mut fonts);
    commands.spawn((Camera2d, IsDefaultUiCamera));
    let stage = preview::spawn_stage(&mut commands, &mut images);
    let target = stage.target.clone();
    commands.insert_resource(stage);

    let root = commands
        .spawn((
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                ..default()
            },
            BackgroundColor(BG),
        ))
        .id();

    // -- title bar: title | toolbar | path and count | close ---------------------------------------
    let bar = child(
        &mut commands,
        root,
        (
            Node {
                height: Val::Px(40.0),
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(8.0),
                padding: UiRect::horizontal(Val::Px(10.0)),
                flex_shrink: 0.0,
                ..default()
            },
            BackgroundColor(BAR),
        ),
    );
    let title = child(
        &mut commands,
        bar,
        Node {
            height: Val::Percent(100.0),
            align_items: AlignItems::Center,
            padding: UiRect::right(Val::Px(8.0)),
            flex_shrink: 0.0,
            ..default()
        },
    );
    commands.entity(title).observe(drag_window);
    // The app icon (embedded in the exe, same picture as the window / taskbar icon) in front of the name.
    if let Some(handle) = title_icon(&mut images) {
        child(
            &mut commands,
            title,
            (
                Node {
                    width: Val::Px(22.0),
                    height: Val::Px(22.0),
                    margin: UiRect::right(Val::Px(8.0)),
                    flex_shrink: 0.0,
                    ..default()
                },
                ImageNode::new(handle),
                Pickable::IGNORE,
            ),
        );
    }
    // The name, with the build number (GitHub Actions run number) as a small gray "superscript" stuck to its top right;
    // nothing for a local build. Both are top-aligned, so the number rides at the height of the name's capitals.
    let name_group = child(
        &mut commands,
        title,
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::FlexStart,
            flex_shrink: 0.0,
            ..default()
        },
    );
    child(&mut commands, name_group, txt("VotV Model Manager", 16.0, TEXT));
    if let Some(n) = crate::build_number() {
        child(
            &mut commands,
            name_group,
            (
                txt(&n.to_string(), 10.0, BUILD_GRAY),
                Node {
                    margin: UiRect::left(Val::Px(3.0)),
                    flex_shrink: 0.0,
                    ..default()
                },
            ),
        );
    }

    let info = child(
        &mut commands,
        bar,
        Node {
            flex_grow: 1.0,
            min_width: Val::Px(0.0),
            height: Val::Percent(100.0),
            align_items: AlignItems::Center,
            padding: UiRect::left(Val::Px(8.0)),
            overflow: Overflow::clip(),
            ..default()
        },
    );
    commands.entity(info).observe(drag_window);
    let status = child(
        &mut commands,
        info,
        (
            txt("", 13.0, DIM),
            TextLayout {
                linebreak: LineBreak::NoWrap,
                ..default()
            },
        ),
    );

    // Settings drop-down (Change folder..., Refresh) next to the close button; the menu itself is built below.
    let btn_settings = child(&mut commands, bar, button(Val::Px(96.0), Val::Px(26.0)));
    child(
        &mut commands,
        btn_settings,
        icon(&iconfont, g::SETTINGS, 18.0, TEXT),
    );
    child(
        &mut commands,
        btn_settings,
        (
            txt("Settings", 14.0, TEXT),
            Node {
                margin: UiRect::left(Val::Px(6.0)),
                ..default()
            },
        ),
    );
    commands
        .entity(btn_settings)
        .observe(listmenu::toggle_settings);

    // "Open in Blender" (editor only): Blender's brand colours (blue button, orange border, white text, the logo) mark it
    // as an integration with another program. Sits before the Done button, which stays at the far right.
    let btn_blender = child(
        &mut commands,
        bar,
        (
            BlenderButton,
            Button,
            Node {
                height: Val::Px(26.0),
                padding: UiRect::horizontal(Val::Px(10.0)),
                border: UiRect::all(Val::Px(1.0)),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                column_gap: Val::Px(6.0),
                flex_shrink: 0.0,
                display: Display::None, // shown by blender::button_system in the editor
                ..default()
            },
            BackgroundColor(blender::BLENDER_BLUE),
            BorderColor(blender::BLENDER_ORANGE),
            BorderRadius::all(Val::Px(4.0)),
        ),
    );
    if let Some(logo) = blender::logo(&mut images) {
        child(
            &mut commands,
            btn_blender,
            (
                Node {
                    width: Val::Px(20.0),
                    height: Val::Px(20.0),
                    flex_shrink: 0.0,
                    ..default()
                },
                ImageNode::new(logo),
                Pickable::IGNORE,
            ),
        );
    }
    child(
        &mut commands,
        btn_blender,
        (
            txt("Open in Blender", 14.0, blender::BLENDER_WHITE),
            TextLayout {
                linebreak: LineBreak::NoWrap,
                ..default()
            },
            Pickable::IGNORE,
        ),
    );
    commands.entity(btn_blender).observe(blender::on_click);

    // (Named `close_btn`: later code reuses the name `b`, and the width changes of conv_ui_system must reach THIS button.)
    // A 1:1 button (26 x 26) with a plain X; in the editor it becomes the green "Done" button (conv_ui_system).
    let close_btn = child(&mut commands, bar, button(Val::Px(26.0), Val::Px(26.0)));
    // On the converter screen this button is the back button: the X is hidden and the arrow shown (conv_ui_system).
    let close_x = child(
        &mut commands,
        close_btn,
        icon(&iconfont, g::CLOSE, 20.0, TEXT),
    );
    let back_arrow = child(
        &mut commands,
        close_btn,
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            display: Display::None,
            ..default()
        },
    );
    let done_green = Color::srgb(0.30, 0.92, 0.45);
    child(
        &mut commands,
        back_arrow,
        icon(&iconfont, g::CHECK, 20.0, done_green),
    );
    child(
        &mut commands,
        back_arrow,
        (
            txt("Done", 14.0, done_green),
            TextLayout {
                linebreak: LineBreak::NoWrap,
                ..default()
            },
            Node {
                margin: UiRect::left(Val::Px(6.0)),
                ..default()
            },
        ),
    );
    commands.entity(close_btn).observe(on_close);

    // -- body: grid | splitter | preview ---------------------------------------------------------
    let body = child(
        &mut commands,
        root,
        Node {
            flex_grow: 1.0,
            min_height: Val::Px(0.0),
            flex_direction: FlexDirection::Row,
            ..default()
        },
    );
    let left = child(
        &mut commands,
        body,
        Node {
            width: Val::Px(layout.list_w),
            min_width: Val::Px(0.0),
            flex_shrink: 0.0,
            flex_direction: FlexDirection::Column,
            ..default()
        },
    );
    // The list (grid + scrollbar) takes all the height above the search bar.
    let list_row = child(
        &mut commands,
        left,
        Node {
            flex_grow: 1.0,
            flex_basis: Val::Px(0.0),
            min_height: Val::Px(0.0),
            flex_direction: FlexDirection::Row,
            ..default()
        },
    );
    let grid = child(
        &mut commands,
        list_row,
        (
            Node {
                flex_grow: 1.0,
                min_height: Val::Px(0.0),
                flex_direction: FlexDirection::Row,
                flex_wrap: FlexWrap::Wrap,
                align_content: AlignContent::FlexStart, // do not stretch rows to fill the height
                overflow: Overflow::scroll_y(),
                padding: UiRect::all(Val::Px(12.0)),
                row_gap: Val::Px(12.0),
                column_gap: Val::Px(12.0),
                ..default()
            },
            BackgroundColor(PANEL),
        ),
    );
    // Scrollbar: a track beside the grid with a draggable thumb (sized and placed by `scrollbar_system`).
    let track = child(
        &mut commands,
        list_row,
        (
            Node {
                width: Val::Px(14.0),
                flex_shrink: 0.0,
                ..default()
            },
            BackgroundColor(Color::srgb(0.10, 0.11, 0.13)),
        ),
    );
    let thumb = child(
        &mut commands,
        track,
        (
            Button,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(2.0),
                right: Val::Px(2.0),
                top: Val::Px(0.0),
                height: Val::Px(40.0),
                display: Display::None,
                ..default()
            },
            BackgroundColor(BTN),
            BorderRadius::all(Val::Px(5.0)),
        ),
    );
    commands.entity(thumb).observe(thumb_drag);
    // Search bar under the list (replaces the old drop zone; a .glb can still be dropped anywhere on the list area).
    let search_refs = listmenu::build_search(&mut commands, left, &iconfont);
    commands.insert_resource(search_refs);
    // The toolbar with the batch actions goes to the TOP of the list column.
    let (toolbar, toolbar_refs) = listmenu::build_toolbar(&mut commands, &iconfont);
    commands.entity(left).insert_children(0, &[toolbar]);
    commands.insert_resource(toolbar_refs);
    // While a file is dragged over the window this covers the whole list area (list, scrollbar and search bar):
    // the whole list is the drop target. It is the last child of `left`, so it draws on top.
    let drop_overlay = child(
        &mut commands,
        left,
        (
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                right: Val::Px(0.0),
                top: Val::Px(0.0),
                bottom: Val::Px(0.0),
                border: UiRect::all(Val::Px(3.0)),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                display: Display::None,
                ..default()
            },
            BackgroundColor(Color::srgba(0.30, 0.58, 0.98, 0.16)),
            BorderColor(ACCENT),
            BorderRadius::all(Val::Px(8.0)),
            Pickable::IGNORE,
        ),
    );
    child(
        &mut commands,
        drop_overlay,
        (
            txt(
                "Release to convert a .glb, or to copy a model .zip / .7z / .rar",
                18.0,
                TEXT,
            ),
            Pickable::IGNORE,
        ),
    );

    let splitter = child(
        &mut commands,
        body,
        (
            Node {
                width: Val::Px(6.0),
                flex_shrink: 0.0,
                ..default()
            },
            BackgroundColor(BAR),
        ),
    );
    commands.entity(splitter).observe(splitter_drag);
    commands.entity(splitter).observe(
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
    commands.entity(splitter).observe(
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

    let panel = child(
        &mut commands,
        body,
        (
            Node {
                flex_grow: 1.0,
                min_width: Val::Px(260.0),
                flex_direction: FlexDirection::Column,
                ..default()
            },
            BackgroundColor(PANEL),
        ),
    );
    let panel_header = child(
        &mut commands,
        panel,
        (
            Node {
                height: Val::Px(32.0),
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(6.0),
                padding: UiRect::horizontal(Val::Px(6.0)),
                flex_shrink: 0.0,
                ..default()
            },
            BackgroundColor(BAR),
        ),
    );
    let b = child(
        &mut commands,
        panel_header,
        button(Val::Px(24.0), Val::Px(22.0)),
    );
    child(&mut commands, b, icon(&iconfont, g::CLOSE, 16.0, TEXT));
    commands.entity(b).observe(on_close_preview);
    let header_area = child(
        &mut commands,
        panel_header,
        Node {
            flex_grow: 1.0,
            min_width: Val::Px(0.0),
            overflow: Overflow::clip(),
            ..default()
        },
    );
    let header_text = child(
        &mut commands,
        header_area,
        (
            txt("", 13.0, TEXT),
            TextLayout {
                linebreak: LineBreak::NoWrap,
                ..default()
            },
        ),
    );
    let btn_edit = child(
        &mut commands,
        panel_header,
        (
            Button,
            Node {
                width: Val::Px(66.0),
                height: Val::Px(22.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                flex_shrink: 0.0,
                display: Display::None, // shown by edit_button_system when the model has a saved .glb
                ..default()
            },
            BackgroundColor(BTN),
            BorderRadius::all(Val::Px(4.0)),
        ),
    );
    child(
        &mut commands,
        btn_edit,
        icon(&iconfont, g::EDIT, 16.0, EDIT_COL),
    );
    child(
        &mut commands,
        btn_edit,
        (
            txt("Edit", 13.0, EDIT_COL),
            Node {
                margin: UiRect::left(Val::Px(4.0)),
                ..default()
            },
        ),
    );
    commands.entity(btn_edit).observe(on_edit);
    // Same place as Edit: a model has one or the other (`edit_button_system`).
    let btn_props = child(
        &mut commands,
        panel_header,
        (
            Button,
            Node {
                width: Val::Auto,
                padding: UiRect::horizontal(Val::Px(10.0)),
                height: Val::Px(22.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                flex_shrink: 0.0,
                display: Display::None, // shown by edit_button_system when the model has no saved .glb
                ..default()
            },
            BackgroundColor(BTN),
            BorderRadius::all(Val::Px(4.0)),
        ),
    );
    child(
        &mut commands,
        btn_props,
        icon(&iconfont, g::TUNE, 16.0, PROPS_COL),
    );
    child(
        &mut commands,
        btn_props,
        (
            txt("Properties", 13.0, PROPS_COL),
            Node {
                margin: UiRect::left(Val::Px(4.0)),
                ..default()
            },
        ),
    );
    commands.entity(btn_props).observe(on_properties);
    let btn_export = child(
        &mut commands,
        panel_header,
        (
            HeaderExport,
            Button,
            Node {
                width: Val::Auto, // "Export" for one model, "Export Bundle" for several (export::button_system)
                padding: UiRect::horizontal(Val::Px(10.0)),
                height: Val::Px(22.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                flex_shrink: 0.0,
                display: Display::None, // shown by export::button_system while a model is selected
                ..default()
            },
            BackgroundColor(BTN),
            BorderRadius::all(Val::Px(4.0)),
        ),
    );
    child(
        &mut commands,
        btn_export,
        icon(&iconfont, g::FILE_UPLOAD, 16.0, EXPORT_COL),
    );
    child(
        &mut commands,
        btn_export,
        (
            HeaderExportLabel,
            txt("Export", 13.0, EXPORT_COL),
            Node {
                margin: UiRect::left(Val::Px(4.0)),
                ..default()
            },
        ),
    );
    commands.entity(btn_export).observe(export::on_header_click);

    let container = child(
        &mut commands,
        panel,
        (
            Node {
                flex_grow: 1.0,
                min_height: Val::Px(0.0),
                ..default()
            },
            BackgroundColor(Color::srgb(0.07, 0.08, 0.09)),
        ),
    );
    // The 3D view: an absolutely positioned image so its size never feeds back into the layout.
    let surface = child(
        &mut commands,
        container,
        (
            PreviewSurface,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                right: Val::Px(0.0),
                top: Val::Px(0.0),
                bottom: Val::Px(0.0),
                ..default()
            },
            ImageNode::new(target),
        ),
    );
    commands.entity(surface).observe(preview::surface_drag);
    let hint = child(
        &mut commands,
        container,
        (
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                right: Val::Px(0.0),
                top: Val::Percent(45.0),
                justify_content: JustifyContent::Center,
                ..default()
            },
            Pickable::IGNORE,
        ),
    );
    let hint_text = child(
        &mut commands,
        hint,
        (txt("Select a model", 15.0, DIM), Pickable::IGNORE),
    );

    // The printer report: model size, one verdict per printer, a warning (`preview::printer_hud_system`). Hidden until it
    // has something to say; not pickable, so the preview can still be dragged underneath it.
    let limits_box = child(
        &mut commands,
        container,
        (
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(10.0),
                top: Val::Px(10.0),
                max_width: Val::Percent(70.0),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(2.0),
                padding: UiRect::all(Val::Px(6.0)),
                display: Display::None,
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.45)),
            BorderRadius::all(Val::Px(4.0)),
            Pickable::IGNORE,
        ),
    );
    let limits_lines: [Entity; 4] = std::array::from_fn(|_| {
        child(
            &mut commands,
            limits_box,
            (txt("", 12.0, TEXT), Pickable::IGNORE),
        )
    });

    // Options fly-out (hidden until the eye button is pressed).
    // A floating "preview options" (eye) button at the preview's bottom right, on the list screen and in the editor
    // (conv_ui_system shows it); the fly-out opens above it.
    let eye_btn = child(
        &mut commands,
        container,
        (
            Button,
            Node {
                position_type: PositionType::Absolute,
                right: Val::Px(10.0),
                bottom: Val::Px(10.0),
                width: Val::Px(32.0),
                height: Val::Px(32.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                display: Display::None,
                ..default()
            },
            BackgroundColor(BTN),
            BorderRadius::all(Val::Px(4.0)),
        ),
    );
    child(
        &mut commands,
        eye_btn,
        (icon(&iconfont, g::VISIBILITY, 20.0, TEXT), Pickable::IGNORE),
    );
    commands.entity(eye_btn).observe(on_toggle_options);

    let options = child(
        &mut commands,
        container,
        (
            Node {
                position_type: PositionType::Absolute,
                bottom: Val::Px(52.0), // above the eye button
                right: Val::Px(10.0),
                width: Val::Px(250.0),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(4.0),
                padding: UiRect::all(Val::Px(8.0)),
                display: Display::None,
                ..default()
            },
            BackgroundColor(Color::srgba(0.11, 0.12, 0.15, 0.96)),
            BorderRadius::all(Val::Px(6.0)),
        ),
    );
    child(&mut commands, options, txt("Preview options", 13.0, DIM));
    // First row: put the camera back to the view a freshly loaded model gets.
    let reset = child(
        &mut commands,
        options,
        (
            Button,
            Node {
                height: Val::Px(26.0),
                align_items: AlignItems::Center,
                padding: UiRect::horizontal(Val::Px(8.0)),
                ..default()
            },
            BackgroundColor(BTN),
            BorderRadius::all(Val::Px(4.0)),
        ),
    );
    child(&mut commands, reset, (txt("Reset camera", 13.0, TEXT), Pickable::IGNORE));
    commands.entity(reset).observe(
        |_: Trigger<Pointer<Click>>,
         ps: Res<PreviewState>,
         opts: Res<Opts>,
         mut orbit: ResMut<Orbit>| {
            crate::preview::reset_camera(&ps, &opts, &mut orbit);
        },
    );
    for key in ALL_KEYS {
        let b = child(
            &mut commands,
            options,
            (
                Button,
                Node {
                    height: Val::Px(26.0),
                    align_items: AlignItems::Center,
                    padding: UiRect::horizontal(Val::Px(8.0)),
                    ..default()
                },
                BackgroundColor(BTN),
                BorderRadius::all(Val::Px(4.0)),
            ),
        );
        child(
            &mut commands,
            b,
            (txt(&opt_text(key, &opts), 13.0, TEXT), OptLabel(key)),
        );
        commands
            .entity(b)
            .observe(move |_: Trigger<Pointer<Click>>, mut o: ResMut<Opts>| toggle(key, &mut o));
    }

    // -- resize handles on every edge and corner: highlight + resize cursor on hover -----------------
    use CompassOctant::*;
    for dir in [
        North, South, West, East, NorthWest, NorthEast, SouthWest, SouthEast,
    ] {
        let e = child(&mut commands, root, (resize_node(dir), GlobalZIndex(100)));
        commands.entity(e).observe(
            move |_: Trigger<Pointer<DragStart>>,
                  mut w: Query<&mut Window, With<PrimaryWindow>>| {
                if let Ok(mut win) = w.single_mut() {
                    win.start_drag_resize(dir);
                }
            },
        );
        // Feedback is the resize cursor only (no coloured highlight).
        commands.entity(e).observe(
            move |_: Trigger<Pointer<Over>>,
                  mut commands: Commands,
                  win: Query<Entity, With<PrimaryWindow>>| {
                set_cursor(&mut commands, &win, cursor_for(dir));
            },
        );
        commands.entity(e).observe(
            |_: Trigger<Pointer<Out>>,
             mut commands: Commands,
             win: Query<Entity, With<PrimaryWindow>>| {
                set_cursor(&mut commands, &win, SystemCursorIcon::Default);
            },
        );
    }
    // A small dotted grip in the bottom-right corner: always visible, so it is clear the window resizes.
    for (r, b) in [
        (3.0, 3.0),
        (7.0, 3.0),
        (11.0, 3.0),
        (3.0, 7.0),
        (7.0, 7.0),
        (3.0, 11.0),
    ] {
        child(
            &mut commands,
            root,
            (
                Node {
                    position_type: PositionType::Absolute,
                    right: Val::Px(r),
                    bottom: Val::Px(b),
                    width: Val::Px(2.0),
                    height: Val::Px(2.0),
                    ..default()
                },
                BackgroundColor(DIM),
                GlobalZIndex(90),
                Pickable::IGNORE,
            ),
        );
    }

    // Converter screen widgets: the properties panel is a column next to the preview (child of `body`); the progress
    // bar, logs button and log panel sit inside the preview container.
    let mut conv_refs: ConvUiRefs =
        conv_ui::build(&mut commands, container, body, &rows.0, &iconfont);
    conv_refs.btn_settings = btn_settings;
    conv_refs.btn_back = back_arrow;
    conv_refs.close_x = close_x;
    conv_refs.close_btn = close_btn;
    conv_refs.eye_btn = eye_btn;
    conv_refs.btn_edit = btn_edit;
    conv_refs.btn_props = btn_props;
    conv_refs.panel_header = panel_header;
    commands.insert_resource(conv_refs);
    // Dragging a .glb over the converter screen: the preview is the drop target (added last so it draws on top).
    let conv_drop_overlay = child(
        &mut commands,
        container,
        (
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                right: Val::Px(0.0),
                top: Val::Px(0.0),
                bottom: Val::Px(0.0),
                border: UiRect::all(Val::Px(3.0)),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                display: Display::None,
                ..default()
            },
            BackgroundColor(Color::srgba(0.30, 0.58, 0.98, 0.16)),
            BorderColor(ACCENT),
            Pickable::IGNORE,
        ),
    );
    child(
        &mut commands,
        conv_drop_overlay,
        (
            txt("Release to replace the model", 18.0, TEXT),
            Pickable::IGNORE,
        ),
    );

    // Right-click menu of the list tiles (drawn above everything, hidden until opened).
    let overlay = crate::textedit::build_overlay(&mut commands, root);
    commands.insert_resource(overlay);
    let dialog_refs = crate::dialog::build_dialog(&mut commands, root, &iconfont);
    commands.insert_resource(dialog_refs);
    let pick_refs = crate::importpick::build(&mut commands, root);
    commands.insert_resource(pick_refs);
    let settings = listmenu::build_settings_menu(&mut commands, root, btn_settings, &iconfont);
    commands.entity(settings.folder).observe(on_browse);
    commands.entity(settings.refresh).observe(on_refresh);
    commands
        .entity(settings.blender)
        .observe(blender::on_pick_click);
    commands.insert_resource(settings);

    commands.insert_resource(iconfont);
    commands.insert_resource(UiRefs {
        grid,
        panel,
        splitter,
        options,
        container,
        hint,
        hint_text,
        header_text,
        limits_box,
        limits_lines,
        status,
        track,
        thumb,
        left,
        drop_overlay,
        conv_drop_overlay,
    });
}

// ---- model list --------------------------------------------------------------------------------------

/// Re-scan the folder and rebuild the tiles. Triggered by Refresh, Browse, or the first frame.
/// Section header (and, except for EDITABLE, the row that is shown while the section has nothing to list).
fn spawn_section(commands: &mut Commands, grid: Entity, rank: u8, sort: SortMode, font: &IconFont) {
    let header = commands
        .spawn((
            SectionHeader,
            Node {
                width: Val::Percent(100.0),
                flex_shrink: 0.0,
                padding: UiRect::top(Val::Px(6.0)),
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::SpaceBetween,
                ..default()
            },
            pass(),
        ))
        .id();
    commands.entity(grid).add_child(header);
    child(
        commands,
        header,
        (
            txt(listmenu::SECTION_NAMES[rank as usize], 12.0, DIM),
            pass(),
        ),
    );
    // The sort button: the icon and name of the current mode; a click goes to the next one (the list is rescanned).
    let sort_btn = child(
        commands,
        header,
        (
            Button,
            Node {
                height: Val::Px(22.0),
                padding: UiRect::horizontal(Val::Px(8.0)),
                column_gap: Val::Px(5.0),
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(Color::srgb(0.14, 0.15, 0.18)),
            BorderRadius::all(Val::Px(4.0)),
            pass(),
        ),
    );
    let glyph = match sort {
        SortMode::LastAdded => g::MORE_TIME,
        SortMode::LastModified => g::UPDATE,
        SortMode::NameAz => g::SORT_BY_ALPHA,
    };
    child(
        commands,
        sort_btn,
        (icon(font, glyph, 15.0, TEXT), Pickable::IGNORE),
    );
    child(
        commands,
        sort_btn,
        (txt(sort.label(), 11.0, TEXT), Pickable::IGNORE),
    );
    commands.entity(sort_btn).observe(
        move |t: Trigger<Pointer<Click>>,
              mut sorting: ResMut<listmenu::SortState>,
              mut lib: ResMut<Library>| {
            if t.event().button == PointerButton::Primary {
                sorting.modes[rank as usize] = sorting.modes[rank as usize].next(rank);
                lib.dirty = true;
            }
        },
    );
    pointer_cursor(commands, sort_btn);
    if rank > 0 {
        let row = commands
            .spawn((
                Node {
                    width: Val::Percent(100.0),
                    flex_shrink: 0.0,
                    padding: UiRect::axes(Val::Px(4.0), Val::Px(10.0)),
                    display: Display::None,
                    ..default()
                },
                pass(),
            ))
            .id();
        commands.entity(grid).add_child(row);
        let text = child(commands, row, (txt("", 13.0, DIM), pass()));
        commands.entity(row).insert(SectionEmpty { rank, text });
    }
}

/// The mouse pointer becomes a hand while it is over `entity` (buttons and fake tiles).
pub(crate) fn pointer_cursor(commands: &mut Commands, entity: Entity) {
    commands.entity(entity).observe(
        |_: Trigger<Pointer<Over>>,
         mut commands: Commands,
         win: Query<Entity, With<PrimaryWindow>>| {
            set_cursor(&mut commands, &win, SystemCursorIcon::Pointer);
        },
    );
    commands.entity(entity).observe(
        |_: Trigger<Pointer<Out>>,
         mut commands: Commands,
         win: Query<Entity, With<PrimaryWindow>>| {
            set_cursor(&mut commands, &win, SystemCursorIcon::Default);
        },
    );
}

/// A fake tile (dashed look, "+" icon, a few centred lines of text) at the start of a section.
fn spawn_fake_tile(commands: &mut Commands, grid: Entity, font: &IconFont, lines: &[&str]) -> Entity {
    let tile = commands
        .spawn((
            Button,
            DropTile,
            Node {
                width: Val::Px(TILE_W),
                height: Val::Px(TILE_H),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                row_gap: Val::Px(6.0),
                border: UiRect::all(Val::Px(2.0)),
                flex_shrink: 0.0,
                ..default()
            },
            BackgroundColor(Color::srgb(0.10, 0.11, 0.14)),
            BorderColor(EDGE_HOVER),
            BorderRadius::all(Val::Px(6.0)),
            pass(),
        ))
        .id();
    commands.entity(grid).add_child(tile);
    child(
        commands,
        tile,
        (icon(font, g::ADD, 44.0, DIM), Pickable::IGNORE),
    );
    // One line each, centred (no wrapping).
    let one_line = TextLayout {
        justify: JustifyText::Center,
        linebreak: LineBreak::NoWrap,
    };
    for line in lines {
        child(
            commands,
            tile,
            (txt(line, 11.0, DIM), one_line, Pickable::IGNORE),
        );
    }
    pointer_cursor(commands, tile);
    tile
}

/// The one fake tile of the list (first in EDITABLE): drop a .glb (opens the editor) or a model archive (.zip, .7z, .rar:
/// copied into the printer folder, see import.rs) on the window, or click it to choose either with the file explorer.
fn spawn_drop_tile(commands: &mut Commands, grid: Entity, font: &IconFont) {
    let tile = spawn_fake_tile(
        commands,
        grid,
        font,
        &["Drop a glb or", "an archive here"],
    );
    commands.entity(tile).observe(on_pick_glb);
}

pub fn rescan(
    mut commands: Commands,
    mut lib: ResMut<Library>,
    refs: Res<UiRefs>,
    mut pipe: ResMut<IconPipe>,
    extras: Query<Entity, Or<(With<SectionHeader>, With<SectionEmpty>, With<DropTile>)>>,
    iconfont: Res<IconFont>,
    sorting: Res<listmenu::SortState>,
) {
    if !lib.dirty {
        return;
    }
    lib.dirty = false;
    lib.generation += 1;
    let mut _bench = crate::benchmark::work("lib.rescan");
    // Keep the selection across the rescan: by name (a renamed / edited model is named by `select_name`).
    let explicit = lib.select_name.take();
    // A just-edited / renamed / imported model replaces the selection; otherwise every selected model is kept (by name).
    let keep_all: Vec<String> = if explicit.is_some() {
        Vec::new()
    } else {
        lib.selection.iter().filter_map(|&i| lib.entries.get(i)).map(|e| e.name.clone()).collect()
    };
    let keep_active = explicit.or_else(|| lib.selected.and_then(|i| lib.entries.get(i)).map(|e| e.name.clone()));
    lib.selected = None;
    lib.selection.clear();

    for t in lib.tiles.drain(..) {
        commands.entity(t.tile).despawn();
    }
    for h in &extras {
        commands.entity(h).despawn();
    }
    let mut list = {
        let _b = crate::benchmark::work("lib.scan_models");
        scan_models(&lib.root)
    };
    {
        let _b = crate::benchmark::work("lib.sort_entries");
        sort_entries(&mut list, sorting.modes);
    }
    lib.entries = list;
    _bench.detail_with(|| format!("{} models", lib.entries.len()));
    lib.selected = keep_active.and_then(|n| lib.entries.iter().position(|e| e.name == n));
    let mut selection: std::collections::BTreeSet<usize> =
        keep_all.iter().filter_map(|n| lib.entries.iter().position(|e| &e.name == n)).collect();
    if let Some(i) = lib.selected {
        selection.insert(i);
    }
    lib.anchor = lib.selected;
    lib.selection = selection;

    let mut _tiles_bench = crate::benchmark::work("lib.spawn_tiles");
    _tiles_bench.detail_with(|| format!("{} tiles", lib.entries.len()));
    let mut tiles = Vec::with_capacity(lib.entries.len());
    // Three permanent sections, in this order: EDITABLE (models with a saved .glb; starts with the drop / pick tile),
    // MODELS. Entries are sorted by section, so a section is opened when the first entry of it (or of a
    // later one) is reached; empty sections still get their header and an empty-message row (see listmenu.rs).
    let mut section = 0u8;
    spawn_section(&mut commands, refs.grid, 0, sorting.modes[0], &iconfont);
    spawn_drop_tile(&mut commands, refs.grid, &iconfont);
    for (i, e) in lib.entries.iter().enumerate() {
        while section < e.section() {
            section += 1;
            spawn_section(
                &mut commands,
                refs.grid,
                section,
                sorting.modes[section as usize],
                &iconfont,
            );
        }
        let tile = commands
            .spawn((
                Button,
                Tile(i),
                Node {
                    width: Val::Px(TILE_W),
                    height: Val::Px(TILE_H),
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::Center,
                    padding: UiRect::all(Val::Px(4.0)),
                    border: UiRect::all(Val::Px(2.0)),
                    row_gap: Val::Px(4.0),
                    flex_shrink: 0.0,
                    ..default()
                },
                BackgroundColor(if e.disabled {
                    Color::srgb(0.14, 0.15, 0.18)
                } else {
                    TILE
                }),
                BorderColor(EDGE),
                BorderRadius::all(Val::Px(6.0)),
                pass(),
            ))
            .id();
        commands.entity(refs.grid).add_child(tile);
        let icon_box = child(
            &mut commands,
            tile,
            (
                Node {
                    width: Val::Px(ICON_SIZE),
                    height: Val::Px(ICON_SIZE),
                    justify_content: JustifyContent::Center,
                    align_items: AlignItems::Center,
                    flex_shrink: 0.0,
                    ..default()
                },
                BackgroundColor(ICON_BG),
                BorderRadius::all(Val::Px(4.0)),
                pass(),
            ),
        );
        let placeholder = child(&mut commands, icon_box, (txt("...", 16.0, DIM), pass()));
        if e.disabled {
            // Greyed out: a dark veil over the icon and a "block" symbol at the top left.
            child(
                &mut commands,
                icon_box,
                (
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(0.0),
                        right: Val::Px(0.0),
                        top: Val::Px(0.0),
                        bottom: Val::Px(0.0),
                        ..default()
                    },
                    BackgroundColor(Color::srgba(0.10, 0.11, 0.14, 0.62)),
                    BorderRadius::all(Val::Px(4.0)),
                    Pickable::IGNORE,
                ),
            );
            child(
                &mut commands,
                icon_box,
                (
                    icon(&iconfont, g::BLOCK, 22.0, Color::srgb(0.95, 0.75, 0.25)),
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(4.0),
                        top: Val::Px(4.0),
                        ..default()
                    },
                    Pickable::IGNORE,
                ),
            );
        }
        // The name is a box so that "Rename" in the toolbar can turn it into a text field (textedit.rs).
        let name_box = child(
            &mut commands,
            tile,
            (
                Interaction::default(),
                Node {
                    width: Val::Percent(100.0),
                    height: Val::Px(20.0),
                    flex_shrink: 0.0,
                    align_items: AlignItems::Center,
                    padding: UiRect::horizontal(Val::Px(3.0)),
                    border: UiRect::all(Val::Px(1.0)),
                    overflow: Overflow::clip(),
                    ..default()
                },
                BackgroundColor(Color::NONE),
                BorderColor(Color::NONE),
                BorderRadius::all(Val::Px(3.0)),
                pass(),
            ),
        );
        let name_text = child(
            &mut commands,
            name_box,
            (
                txt(
                    &short(&e.name, 17),
                    12.0,
                    if e.disabled { DIM } else { TEXT },
                ),
                TextLayout {
                    justify: JustifyText::Center,
                    linebreak: LineBreak::NoWrap,
                },
                Node {
                    flex_grow: 1.0,
                    min_width: Val::Px(0.0),
                    ..default()
                },
                pass(),
            ),
        );
        commands
            .entity(name_box)
            .observe(crate::textedit::field_drag);
        commands.entity(name_box).observe(
            move |t: Trigger<Pointer<Pressed>>,
                  boxes: Query<(&GlobalTransform, &ComputedNode)>,
                  keys: Res<ButtonInput<KeyCode>>,
                  time: Res<Time<Real>>,
                  mut ed: ResMut<crate::textedit::Editing>| {
                // Only while this tile's name is being edited: move the caret / select.
                if t.event().button == PointerButton::Primary && ed.widget == Some(name_box) {
                    crate::textedit::press(
                        &mut ed,
                        &boxes,
                        name_box,
                        t.event().pointer_location.position.x,
                        crate::textedit::shift_down(&keys),
                        time.elapsed_secs_f64(),
                    );
                }
            },
        );
        commands.entity(tile).observe(
            move |t: Trigger<Pointer<Click>>,
                  mut lib: ResMut<Library>,
                  mut layout: ResMut<Layout>,
                  keys: Res<ButtonInput<KeyCode>>,
                  nodes: Query<&Node>,
                  time: Res<Time<Real>>,
                  mut last: Local<(f64, Vec2)>,
                  ed: Res<crate::textedit::Editing>| {
                // Bevy sends a Click to EVERY hovered entity under the cursor (the tile and each pickable part of it:
                // icon, name, ...), and each one bubbles up to this observer. A toggle (Ctrl + click) would run twice
                // and cancel itself, depending on where on the tile the click lands: handle one click once.
                let (now, pos) = (time.elapsed_secs_f64(), t.event().pointer_location.position);
                if *last == (now, pos) {
                    return;
                }
                *last = (now, pos);
                crate::benchmark::event("ui.tile_click", String::new);
                let secondary = match t.event().button {
                    PointerButton::Primary => false,
                    PointerButton::Secondary => true,
                    _ => return,
                };
                if ed.active() {
                    return; // a click while a name is being edited only moves its caret
                }
                let ctrl = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
                let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
                if secondary {
                    // There is no context menu any more (the toolbar replaces it): a right click selects, like in
                    // a file explorer, but keeps a selection the tile is already part of.
                    if lib.selection.contains(&i) {
                        return;
                    }
                    lib.selection.clear();
                    lib.selection.insert(i);
                    lib.anchor = Some(i);
                    lib.selected = Some(i);
                } else if shift {
                    // The range from the anchor to this tile, over the tiles that are visible (search filter).
                    let from = lib.anchor.or(lib.selected).unwrap_or(i);
                    let (lo, hi) = (from.min(i), from.max(i));
                    let visible = |j: usize| lib.tiles.get(j).and_then(|r| nodes.get(r.tile).ok()).map_or(true, |n| n.display != Display::None);
                    let range: Vec<usize> = (lo..=hi).filter(|&j| visible(j)).collect();
                    if !ctrl {
                        lib.selection.clear();
                    }
                    lib.selection.extend(range);
                    lib.selection.insert(i);
                    lib.selected = Some(i);
                } else if ctrl {
                    if lib.selection.remove(&i) {
                        if lib.selected == Some(i) {
                            lib.selected = lib.selection.iter().next_back().copied(); // the preview moves to another selected tile
                        }
                    } else {
                        lib.selection.insert(i);
                        lib.selected = Some(i);
                    }
                    lib.anchor = Some(i);
                } else {
                    lib.selection.clear();
                    lib.selection.insert(i);
                    lib.anchor = Some(i);
                    lib.selected = Some(i);
                }
                if lib.selected.is_some() {
                    layout.preview_open = true;
                    layout.converter = false;
                }
            },
        );
        tiles.push(TileRefs {
            tile,
            icon_box,
            placeholder,
            name_box,
            name_text,
        });
    }
    while section < 1 {
        section += 1;
        spawn_section(
            &mut commands,
            refs.grid,
            section,
            sorting.modes[section as usize],
            &iconfont,
        );
    }
    lib.tiles = tiles;
    drop(_tiles_bench);
    // The scroll position is kept on a refresh (the layout clamps it if the list got shorter).

    // Restart icon generation for the new list.
    let jobs: Vec<_> = lib
        .entries
        .iter()
        .enumerate()
        .map(|(i, e)| (i, e.dir.clone()))
        .collect();
    for e in pipe.reset(jobs.len()) {
        commands.entity(e).despawn();
    }
    if !jobs.is_empty() {
        pipe.rx = Some(Mutex::new(start_worker(jobs, lib.generation)));
    }
}

/// True if `p` has the extension `ext` (any case).
fn has_ext(p: &std::path::Path, ext: &str) -> bool {
    p.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext))
}

/// The file dialog of the fake tile finished: convert the chosen .glb and copy the chosen archives, like dropped ones.
pub fn poll_file_pick(
    pick: Res<FilePick>,
    lib: Res<Library>,
    mut layout: ResMut<Layout>,
    mut conv: ResMut<ConvertState>,
    mut import: ResMut<crate::import::ImportJob>,
    mut redraw: EventWriter<RequestRedraw>,
) {
    let _bench = crate::benchmark::sys("sys.ui.poll_file_pick");
    let mut slot = pick.0.lock().unwrap();
    let res = slot.as_ref().map(|rx| rx.try_recv());
    match res {
        Some(Ok(list)) => {
            *slot = None;
            layout.notice = None;
            let archives: Vec<std::path::PathBuf> = list
                .iter()
                .filter(|p| crate::import::is_archive(p))
                .cloned()
                .collect();
            if !archives.is_empty() {
                if import.busy() {
                    layout.notice = Some("Still copying the previous drop".to_string());
                } else {
                    layout.notice = Some("Copying...".to_string());
                    crate::import::start(&mut import, lib.root.clone(), archives);
                }
            }
            if let Some(glb) = list.into_iter().find(|p| has_ext(p, "glb")) {
                layout.converter = true;
                layout.preview_open = true;
                conv.begin(glb);
            }
        }
        Some(Err(TryRecvError::Disconnected)) => *slot = None, // dialog cancelled
        Some(Err(TryRecvError::Empty)) => {
            redraw.write(RequestRedraw);
        }
        None => {}
    }
}

pub fn poll_folder_pick(
    pick: Res<FolderPick>,
    mut lib: ResMut<Library>,
    mut redraw: EventWriter<RequestRedraw>,
) {
    let _bench = crate::benchmark::sys("sys.ui.poll_folder_pick");
    let mut slot = pick.0.lock().unwrap();
    let res = slot.as_ref().map(|rx| rx.try_recv());
    match res {
        Some(Ok(p)) => {
            *slot = None;
            save_config(&p);
            lib.root = p;
            lib.dirty = true;
        }
        Some(Err(TryRecvError::Disconnected)) => *slot = None, // dialog cancelled
        Some(Err(TryRecvError::Empty)) => {
            redraw.write(RequestRedraw);
        }
        None => {}
    }
}

// ---- per-frame UI upkeep -------------------------------------------------------------------------------

pub fn layout_system(
    layout: Res<Layout>,
    refs: Res<UiRefs>,
    ps: Res<PreviewState>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut nodes: Query<&mut Node>,
) {
    let _bench = crate::benchmark::sys("sys.ui.layout_system");
    fn set_display(nodes: &mut Query<&mut Node>, e: Entity, d: Display) {
        if let Ok(mut n) = nodes.get_mut(e) {
            if n.display != d {
                n.display = d;
            }
        }
    }
    let ww = windows.single().map(|w| w.width()).unwrap_or(1200.0);
    let flex_if = |b: bool| if b { Display::Flex } else { Display::None };

    // On the converter screen the preview fills the window: list, splitter and the preview's own header are hidden.
    set_display(&mut nodes, refs.left, flex_if(!layout.converter));
    set_display(
        &mut nodes,
        refs.panel,
        flex_if(layout.preview_open || layout.converter),
    );
    set_display(
        &mut nodes,
        refs.splitter,
        flex_if(layout.preview_open && !layout.converter),
    );
    set_display(
        &mut nodes,
        refs.options,
        flex_if(layout.preview_open && layout.options_open),
    );
    set_display(&mut nodes, refs.hint, flex_if(ps.scene.is_none()));

    set_display(
        &mut nodes,
        refs.drop_overlay,
        flex_if(layout.drop_hover && !layout.converter),
    );
    set_display(
        &mut nodes,
        refs.conv_drop_overlay,
        flex_if(layout.drop_hover && layout.converter),
    );

    // The list keeps its width when the window is resized (the preview takes the difference). With the preview
    // closed the list takes everything. The preview always keeps at least 260 px.
    let max_list = (ww - 6.0 - 260.0).max(260.0);
    let list_w = layout.list_w.clamp(260.0, max_list);
    if let Ok(mut n) = nodes.get_mut(refs.left) {
        if layout.preview_open {
            if n.width != Val::Px(list_w) {
                n.width = Val::Px(list_w);
            }
            if n.flex_grow != 0.0 {
                n.flex_grow = 0.0;
            }
        } else {
            // Not Auto: an auto width is the content's width, and a wrapping grid would then lay all tiles in one
            // endless row. 100% of the body keeps the width limited to the window so the tiles wrap.
            if n.width != Val::Percent(100.0) {
                n.width = Val::Percent(100.0);
            }
            if n.flex_grow != 0.0 {
                n.flex_grow = 0.0;
            }
        }
    }
}

/// Dragging a file over the window lights up the whole list area; dropping it switches the right side to the
/// converter. The window system does not say where a file was released, so a drop anywhere in the window counts
/// (the list area is the visible target). This works even after the small drop zone has been closed: closing it
/// only hides the hint.
pub fn drop_events(
    mut events: EventReader<FileDragAndDrop>,
    mut layout: ResMut<Layout>,
    mut conv: ResMut<ConvertState>,
    mut dialog: ResMut<crate::dialog::ReplaceDialog>,
    lib: Res<Library>,
    mut import: ResMut<crate::import::ImportJob>,
) {
    let _bench = crate::benchmark::sys("sys.ui.drop_events");
    let mut to_import: Vec<std::path::PathBuf> = Vec::new();
    for e in events.read() {
        match e {
            FileDragAndDrop::HoveredFile { .. } => {
                if !layout.drop_hover {
                    layout.drop_hover = true;
                }
                if layout.notice.is_some() {
                    layout.notice = None;
                }
            }
            FileDragAndDrop::HoveredFileCanceled { .. } => {
                if layout.drop_hover {
                    layout.drop_hover = false;
                }
            }
            FileDragAndDrop::DroppedFile { path_buf, .. } => {
                layout.drop_hover = false;
                if layout.converter
                    && conv.input.is_some()
                    && path_buf
                        .extension()
                        .is_some_and(|e| e.eq_ignore_ascii_case("png"))
                {
                    continue; // a PNG dropped in the editor sets the model's icon (conv_ui::icon_input_system)
                }
                let is_glb = path_buf
                    .extension()
                    .map(|e| e.eq_ignore_ascii_case("glb"))
                    .unwrap_or(false);
                if is_glb {
                    layout.notice = None;
                    if layout.converter && conv.input.is_some() && conv.props.is_none() {
                        // Already editing a model: ask before replacing it (dialog.rs) unless the user turned the
                        // question off. Either way the dropped file replaces the CURRENT model.
                        if dialog.dont_ask {
                            conv.replace_input(path_buf.clone());
                        } else {
                            dialog.pending = Some(path_buf.clone());
                        }
                    } else {
                        // Convert right away; the converter screen replaces the list until "Back".
                        layout.converter = true;
                        layout.preview_open = true;
                        conv.begin(path_buf.clone());
                    }
                } else if crate::import::is_archive(path_buf) {
                    // a model archive (.zip, .7z, .rar): copied into the printer folder (import.rs)
                    to_import.push(path_buf.clone());
                } else {
                    let name = path_buf
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    layout.notice = Some(format!(
                        "Only .glb files (to convert) and .zip, .7z or .rar files (to copy) are accepted ({name} was ignored)"
                    ));
                }
            }
        }
    }
    if !to_import.is_empty() {
        if layout.converter {
            layout.notice = Some(
                "Drop archives on the model list, not while editing a model"
                    .to_string(),
            );
        } else if import.busy() {
            layout.notice = Some("Still copying the previous drop".to_string());
        } else {
            layout.notice = Some("Copying...".to_string());
            crate::import::start(&mut import, lib.root.clone(), to_import);
        }
    }
}

pub fn tile_styles(
    lib: Res<Library>,
    mut q: Query<(&Tile, &Interaction, &mut BorderColor)>,
    changed: Query<(), (With<Tile>, Changed<Interaction>)>,
) {
    let _bench = crate::benchmark::sys("sys.ui.tile_styles");
    if !lib.is_changed() && changed.is_empty() {
        return;
    }
    for (t, i, mut b) in &mut q {
        let c = if lib.selection.contains(&t.0) || lib.selected == Some(t.0) {
            ACCENT
        } else if *i != Interaction::None {
            EDGE_HOVER
        } else {
            EDGE
        };
        if b.0 != c {
            b.0 = c;
        }
    }
}

pub fn button_colors(
    mut q: Query<
        (&Interaction, &mut BackgroundColor, Has<ItemButton>),
        (
            Changed<Interaction>,
            With<Button>,
            Without<Tile>,
            Without<BlenderButton>,
        ),
    >,
) {
    let _bench = crate::benchmark::sys("sys.ui.button_colors");
    for (i, mut bg, item) in &mut q {
        bg.0 = match (*i, item) {
            (Interaction::Pressed, _) => BTN_PRESS,
            (Interaction::Hovered, false) => BTN_HOVER,
            (Interaction::Hovered, true) => ITEM_HOVER,
            (Interaction::None, false) => BTN,
            (Interaction::None, true) => ITEM,
        };
    }
}

/// Mouse wheel: scrolls the icon list (also when over its scrollbar) or zooms the preview.
pub fn wheel_system(
    mut wheel: EventReader<MouseWheel>,
    hover: Res<HoverMap>,
    refs: Res<UiRefs>,
    cref: Res<ConvUiRefs>,
    mut lists: Query<(&mut ScrollPosition, &Node, &ComputedNode)>,
    surfaces: Query<(), With<PreviewSurface>>,
    overlays: Query<(), With<crate::conv_ui::RowOverlay>>,
    mut orbit: ResMut<Orbit>,
) {
    let _bench = crate::benchmark::sys("sys.ui.wheel_system");
    for ev in wheel.read() {
        let (px, notches) = match ev.unit {
            MouseScrollUnit::Line => (ev.y * 56.0, ev.y),
            MouseScrollUnit::Pixel => (ev.y, ev.y / 40.0),
        };
        for (_pointer, map) in hover.iter() {
            for entity in map.keys() {
                if surfaces.contains(*entity) {
                    orbit.dist = (orbit.dist * (1.0 - 0.1 * notches).clamp(0.5, 1.5)).max(0.02);
                }
                // The scrollbar track and thumb are not inside the list, so redirect them to it.
                let target = if *entity == refs.track || *entity == refs.thumb {
                    refs.grid
                } else if *entity == cref.sb_track
                    || *entity == cref.sb_thumb
                    || overlays.contains(*entity)
                {
                    cref.scroll // (a grayed-out row's overlay blocks the pointer, so it forwards the wheel too)
                } else if *entity == cref.info_sb_track || *entity == cref.info_sb_thumb {
                    cref.info_scroll
                } else {
                    *entity
                };
                if let Ok((mut sp, node, c)) = lists.get_mut(target) {
                    if node.overflow.y == OverflowAxis::Scroll {
                        let max = (c.content_size.y - c.size.y).max(0.0) * c.inverse_scale_factor;
                        sp.offset_y = (sp.offset_y - px).clamp(0.0, max);
                    } else if node.overflow.x == OverflowAxis::Scroll {
                        // (the model name) the wheel scrolls it sideways
                        let max = (c.content_size.x - c.size.x).max(0.0) * c.inverse_scale_factor;
                        sp.offset_x = (sp.offset_x - px).clamp(0.0, max);
                    }
                }
            }
        }
    }
}

/// Size and place the scrollbar thumb from the list's scroll position; hide it when nothing overflows.
pub fn scrollbar_system(
    refs: Res<UiRefs>,
    grids: Query<(&ScrollPosition, &ComputedNode)>,
    computed: Query<&ComputedNode>,
    mut nodes: Query<&mut Node>,
) {
    let _bench = crate::benchmark::sys("sys.ui.scrollbar_system");
    let (Ok((sp, g)), Ok(tr)) = (grids.get(refs.grid), computed.get(refs.track)) else {
        return;
    };
    let (visible, content, track_h) = scroll_metrics(g, tr);
    let max = (content - visible).max(0.0);
    let Ok(mut n) = nodes.get_mut(refs.thumb) else {
        return;
    };
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

pub fn status_text(
    lib: Res<Library>,
    pipe: Res<IconPipe>,
    layout: Res<Layout>,
    conv: Res<ConvertState>,
    refs: Res<UiRefs>,
    mut texts: Query<&mut Text, Without<OptLabel>>,
) {
    let _bench = crate::benchmark::sys("sys.ui.status_text");
    if !lib.is_changed() && !pipe.is_changed() && !layout.is_changed() && !conv.is_changed() {
        return;
    }
    let busy = pipe.done < pipe.total;
    // The folder's path is not shown (it often contains the user's name and is long): only whether it is usable.
    let tag = if !lib.root.is_dir() {
        "FOLDER NOT FOUND   |   "
    } else {
        ""
    };
    let regen = pipe.regen_remaining();
    let s = if let Some(n) = &layout.notice {
        n.clone()
    } else if regen > 0 {
        // Icon regeneration in progress (list toolbar or the editor); gone as soon as the last icon is saved.
        if regen == 1 {
            "Regenerating the icon...".to_string()
        } else {
            format!("Regenerating icons...   |   {regen} left")
        }
    } else if layout.converter {
        conv.summary()
    } else {
        format!(
            "{}{} models{}",
            tag,
            lib.entries.len(),
            if busy {
                format!("   |   icons {}/{}", pipe.done, pipe.total)
            } else {
                String::new()
            }
        )
    };
    if let Ok(mut t) = texts.get_mut(refs.status) {
        if t.0 != s {
            t.0 = s;
        }
    }
}
