//! Extras of the list screen: the search bar under the list, the "Disabled" section header, and the right-click
//! context menu of a model tile.
//!
//! Search: a small text field (Bevy has no text input widget, so keys are read from `KeyboardInput`). Click it to
//! type; Escape clears and leaves it, Enter leaves it, a click elsewhere leaves it. Typing is ignored by the preview's
//! key shortcuts while the field has focus (`Search::focused`). Every word must appear in the model's folder name.
//!
//! List sections (always present, in this order): EDITABLE (models with a saved .glb; the first tile is the fake
//! "drop a .glb / click to browse" tile) and MODELS. A section with nothing to list shows a message row.
//! Disabled models are not a section: they stay where they belong and are greyed out with a "block" icon.
//!
//! Context menu (right click on a tile):
//!   Export          (editable models only) saves the model folder as a .zip (save dialog), without the .glb and votv_convert.json
//!   Open location   opens the model folder in the file explorer
//!   Regenerate icon renders a new icon.png for the model (overwrites it) and refreshes the tile
//!   Lock model      deletes the .glb saved in the folder (greyed out when there is none); the model leaves EDITABLE
//!   Disable         renames `<x>.obj` to `<x>.obj.disabled`, so the game skips the model; the tile is greyed out.
//!                   For a disabled model this entry reads "Enable" and renames the file back.
//!   Delete          deletes the model folder for good (asks to click a second time)

use std::path::Path;

use bevy::{
    input::{
        keyboard::{Key, KeyboardInput},
        mouse::MouseWheel,
        ButtonState,
    },
    prelude::*,
    text::LineBreak,
    window::PrimaryWindow,
};

use crate::converter::find_glb;
use crate::export::{self, ExportJob};
use crate::icon_font::{g, icon, Glyph, IconFont};
use crate::icons::{self, IconPipe};
use crate::textedit::{Editing, Field, Geom};
use crate::ui::{
    child, txt, Tile, UiRefs, ACCENT, BTN, DIM, EDGE, EDGE_HOVER, EXPORT_COL, TEXT, TILE,
};
use crate::{Layout, Library};

// ---- search ------------------------------------------------------------------------------------------

#[derive(Resource, Default)]
pub struct Search {
    pub text: String,
    pub focused: bool,
}

/// Names of the three list sections, indexed by `Entry::section()`.
pub const SECTION_NAMES: [&str; 2] = ["EDITABLE", "MODELS"];

/// How each section of the list is sorted: EDITABLE by last modified, MODELS by last added (the defaults). Each section
/// header has a button that cycles its mode; a change rescans the list (`ui::spawn_section`).
#[derive(Resource)]
pub struct SortState {
    pub modes: [crate::model::SortMode; 2],
}

impl Default for SortState {
    fn default() -> Self {
        SortState {
            modes: [
                crate::model::SortMode::LastModified,
                crate::model::SortMode::LastAdded,
            ],
        }
    }
}

#[derive(Component)]
pub struct SectionHeader;

/// The fake first tile of EDITABLE (drop a .glb here / click to browse).
#[derive(Component)]
pub struct DropTile;

/// The row shown while a section has no visible tile; `text` is the Text entity inside it.
#[derive(Component)]
pub struct SectionEmpty {
    pub rank: u8,
    pub text: Entity,
}

#[derive(Resource)]
pub struct SearchRefs {
    pub bar: Entity,
    pub text: Entity,
    pub clear: Entity,
}

/// Every word of `query` must occur in the name (case-insensitive; `_` counts as a space).
pub fn name_matches(name: &str, query: &str) -> bool {
    let lower = name.to_lowercase();
    let spaced = lower.replace('_', " ");
    query
        .to_lowercase()
        .split_whitespace()
        .all(|w| lower.contains(w) || spaced.contains(w))
}

/// The compact search bar that sits under the list.
pub fn build_search(commands: &mut Commands, parent: Entity, font: &IconFont) -> SearchRefs {
    let bar = child(
        commands,
        parent,
        (
            // Interaction without `Button`: the generic button colouring must not tint a text field.
            Interaction::default(),
            Node {
                height: Val::Px(38.0),
                flex_shrink: 0.0,
                border: UiRect::top(Val::Px(2.0)),
                align_items: AlignItems::Center,
                column_gap: Val::Px(8.0),
                padding: UiRect::horizontal(Val::Px(12.0)),
                ..default()
            },
            BackgroundColor(TILE),
            BorderColor(EDGE),
        ),
    );
    commands
        .entity(bar)
        .observe(|_: Trigger<Pointer<Click>>, mut s: ResMut<Search>| {
            if !s.focused {
                s.focused = true;
            }
        });
    child(
        commands,
        bar,
        (icon(font, g::SEARCH, 18.0, DIM), Pickable::IGNORE),
    );
    let area = child(
        commands,
        bar,
        (
            Node {
                flex_grow: 1.0,
                min_width: Val::Px(0.0),
                overflow: Overflow::clip(),
                ..default()
            },
            Pickable::IGNORE,
        ),
    );
    let text = child(
        commands,
        area,
        (
            txt("Search models...", 14.0, DIM),
            TextLayout {
                linebreak: LineBreak::NoWrap,
                ..default()
            },
            Pickable::IGNORE,
        ),
    );
    let clear = child(
        commands,
        bar,
        (
            Button,
            Node {
                width: Val::Px(22.0),
                height: Val::Px(20.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                flex_shrink: 0.0,
                display: Display::None,
                ..default()
            },
            BackgroundColor(BTN),
            BorderRadius::all(Val::Px(4.0)),
        ),
    );
    child(
        commands,
        clear,
        (icon(font, g::CLOSE, 14.0, TEXT), Pickable::IGNORE),
    );
    commands
        .entity(clear)
        .observe(|_: Trigger<Pointer<Click>>, mut s: ResMut<Search>| {
            s.text.clear();
            s.focused = true;
        });
    SearchRefs { bar, text, clear }
}

/// Typing into the search field.
pub fn search_input_system(
    mut events: EventReader<KeyboardInput>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    refs: Res<SearchRefs>,
    ints: Query<&Interaction>,
    mut s: ResMut<Search>,
) {
    if mouse.just_pressed(MouseButton::Left) && s.focused {
        let over = |e: Entity| {
            ints.get(e)
                .map(|i| *i != Interaction::None)
                .unwrap_or(false)
        };
        if !over(refs.bar) && !over(refs.clear) {
            s.focused = false;
        }
    }
    let ctrl = keys.pressed(KeyCode::ControlLeft)
        || keys.pressed(KeyCode::ControlRight)
        || keys.pressed(KeyCode::SuperLeft);
    for ev in events.read() {
        if ev.state != ButtonState::Pressed || !s.focused {
            continue;
        }
        match &ev.logical_key {
            Key::Character(c) if !ctrl => {
                for ch in c.chars().filter(|ch| !ch.is_control()) {
                    if s.text.chars().count() < 40 {
                        s.text.push(ch);
                    }
                }
            }
            Key::Space => {
                if s.text.chars().count() < 40 {
                    s.text.push(' ');
                }
            }
            Key::Backspace => {
                s.text.pop();
            }
            Key::Escape => {
                s.text.clear();
                s.focused = false;
            }
            Key::Enter => s.focused = false,
            _ => {}
        }
    }
}

/// Text, caret, border and clear button of the search bar.
pub fn search_visuals_system(
    s: Res<Search>,
    refs: Res<SearchRefs>,
    mut nodes: Query<&mut Node>,
    mut texts: Query<&mut Text>,
    mut colors: Query<&mut TextColor>,
    mut borders: Query<&mut BorderColor>,
) {
    if !s.is_changed() {
        return;
    }
    let (shown, color) = if s.text.is_empty() && !s.focused {
        ("Search models...".to_string(), DIM)
    } else {
        (
            format!("{}{}", s.text, if s.focused { "|" } else { "" }),
            TEXT,
        )
    };
    if let Ok(mut t) = texts.get_mut(refs.text) {
        if t.0 != shown {
            t.0 = shown;
        }
    }
    if let Ok(mut c) = colors.get_mut(refs.text) {
        if c.0 != color {
            c.0 = color;
        }
    }
    if let Ok(mut b) = borders.get_mut(refs.bar) {
        let c = if s.focused { ACCENT } else { EDGE };
        if b.0 != c {
            b.0 = c;
        }
    }
    if let Ok(mut n) = nodes.get_mut(refs.clear) {
        let d = if s.text.is_empty() {
            Display::None
        } else {
            Display::Flex
        };
        if n.display != d {
            n.display = d;
        }
    }
}

/// Hide the tiles that do not match the query and show the empty-section rows where nothing is left.
#[allow(clippy::too_many_arguments)]
pub fn filter_system(
    search: Res<Search>,
    lib: Res<Library>,
    refs: Res<UiRefs>,
    mut tiles: Query<(&Tile, &mut Node)>,
    mut empties: Query<(&SectionEmpty, &mut Node), Without<Tile>>,
    added: Query<(), Or<(Added<Tile>, Added<SectionEmpty>)>>,
    mut scrolls: Query<&mut ScrollPosition>,
    mut texts: Query<&mut Text>,
    mut last: Local<String>,
) {
    if !search.is_changed() && !lib.is_changed() && added.is_empty() {
        return;
    }
    let query = search.text.clone();
    let mut visible = [0usize; 2];
    let mut total = [0usize; 2];
    for e in &lib.entries {
        total[e.section() as usize] += 1;
    }
    for (t, mut n) in &mut tiles {
        let Some(e) = lib.entries.get(t.0) else {
            continue;
        };
        let show = name_matches(&e.name, &query);
        let d = if show { Display::Flex } else { Display::None };
        if n.display != d {
            n.display = d;
        }
        if show {
            visible[e.section() as usize] += 1;
        }
    }
    for (se, mut n) in &mut empties {
        let r = se.rank as usize;
        let d = if visible[r] == 0 {
            Display::Flex
        } else {
            Display::None
        };
        if n.display != d {
            n.display = d;
        }
        if visible[r] == 0 {
            let msg = if total[r] > 0 {
                "No matches".to_string()
            } else if total.iter().sum::<usize>() > 0 {
                "No other models".to_string() // everything in the folder is in the other section
            } else if !lib.root.is_dir() {
                format!(
                    "Folder not found: {}   (use \"Change folder...\")",
                    lib.root.display()
                )
            } else {
                format!("No models found in {}", lib.root.display())
            };
            if let Ok(mut t) = texts.get_mut(se.text) {
                if t.0 != msg {
                    t.0 = msg;
                }
            }
        }
    }
    if *last != query {
        *last = query;
        if let Ok(mut sp) = scrolls.get_mut(refs.grid) {
            sp.offset_y = 0.0; // the filtered list is shorter: start at the top
        }
    }
}

// ---- context menu ------------------------------------------------------------------------------------

const MENU_W: f32 = 200.0;
const MENU_H: f32 = 7.0 * 28.0 + 6.0 * 2.0 + 8.0 + 2.0 + 74.0; // 7 entries; 74 = room for the Lock warning
const DANGER: Color = Color::srgb(0.95, 0.45, 0.45);
const WARN: Color = Color::srgb(0.96, 0.80, 0.25); // Lock model (yellow)
const DISABLE_COL: Color = Color::srgb(0.98, 0.60, 0.30); // Disable (orange); "Enable" uses EXPORT_COL (green)
const WARN_DIM: Color = Color::srgb(0.50, 0.43, 0.20); // Lock model when there is no .glb to delete
const LOCK_WARNING: &str = "Locking deletes the saved .glb. The model stays in the game but can no longer be edited here. Click again to lock.";

#[derive(Resource, Default)]
pub struct CtxMenu {
    pub open: bool,
    fresh: bool, // just opened: place it
    pub target: usize,
    pos: Vec2,
    generation: u64,
    has_glb: bool,
    disabled: bool,
    confirm: bool,      // "Delete" was clicked once
    confirm_lock: bool, // "Lock model" was clicked once (the warning is showing)
}

impl CtxMenu {
    pub fn open_for(&mut self, i: usize, lib: &Library, pos: Vec2) {
        let Some(e) = lib.entries.get(i) else { return };
        *self = CtxMenu {
            open: true,
            fresh: true,
            target: i,
            pos,
            generation: lib.generation,
            has_glb: find_glb(&e.dir).is_some(),
            disabled: e.disabled,
            confirm: false,
            confirm_lock: false,
        };
    }
    pub fn close(&mut self) {
        if self.open {
            self.open = false;
            self.confirm = false;
            self.confirm_lock = false;
        }
    }
}

#[derive(Resource)]
pub struct CtxRefs {
    menu: Entity,
    open: Entity,
    export: Entity,
    rename: Entity,
    regen: Entity,
    lock: Entity,
    toggle: Entity,
    perm: Entity,
    lock_text: Entity,
    lock_warn: Entity, // the warning box under Lock model
    toggle_text: Entity,
    perm_text: Entity,
    // icons that change with the state
    lock_icon: Entity,
    toggle_icon: Entity,
    perm_icon: Entity,
}

pub(crate) fn primary(t: &Trigger<Pointer<Click>>) -> bool {
    t.event().button == PointerButton::Primary
}

/// Build the (hidden) menu under `root` so it draws above everything else.
pub fn build_menu(commands: &mut Commands, root: Entity, font: &IconFont) -> CtxRefs {
    let menu = child(
        commands,
        root,
        (
            Interaction::default(),
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                width: Val::Px(MENU_W),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(2.0),
                padding: UiRect::all(Val::Px(4.0)),
                border: UiRect::all(Val::Px(1.0)),
                display: Display::None,
                ..default()
            },
            BackgroundColor(Color::srgb(0.12, 0.13, 0.16)),
            BorderColor(EDGE_HOVER),
            BorderRadius::all(Val::Px(6.0)),
            GlobalZIndex(150),
        ),
    );
    // One row: icon column, label.
    let item = |commands: &mut Commands, glyph: Glyph, label: &str| -> (Entity, Entity, Entity) {
        let b = child(
            commands,
            menu,
            (
                Button,
                Node {
                    height: Val::Px(28.0),
                    align_items: AlignItems::Center,
                    column_gap: Val::Px(8.0),
                    padding: UiRect::horizontal(Val::Px(8.0)),
                    ..default()
                },
                BackgroundColor(BTN),
                BorderRadius::all(Val::Px(4.0)),
            ),
        );
        let i = child(
            commands,
            b,
            (
                icon(font, glyph, 18.0, TEXT),
                Node {
                    width: Val::Px(20.0),
                    ..default()
                },
                Pickable::IGNORE,
            ),
        );
        let t = child(
            commands,
            b,
            (
                txt(label, 13.0, TEXT),
                TextLayout {
                    linebreak: LineBreak::NoWrap,
                    ..default()
                },
                Pickable::IGNORE,
            ),
        );
        (b, t, i)
    };
    let (export, export_text, export_icon) = item(commands, g::FILE_UPLOAD, "Export");
    commands.entity(export_text).insert(TextColor(EXPORT_COL));
    commands.entity(export_icon).insert(TextColor(EXPORT_COL));
    let (open, _, _) = item(commands, g::FOLDER_OPEN, "Open location");
    let (rename, _, _) = item(commands, g::EDIT, "Rename");
    let (regen, _, _) = item(commands, g::REFRESH, "Regenerate icon");
    let (lock, lock_text, lock_icon) = item(commands, g::LOCK, "Lock model");
    // Warning shown after the first click on Lock model (hidden until then).
    let lock_warn = child(
        commands,
        menu,
        (
            Node {
                display: Display::None,
                padding: UiRect::axes(Val::Px(8.0), Val::Px(4.0)),
                ..default()
            },
            Pickable::IGNORE,
        ),
    );
    child(
        commands,
        lock_warn,
        (txt(LOCK_WARNING, 12.0, WARN), Pickable::IGNORE),
    );
    let (toggle, toggle_text, toggle_icon) = item(commands, g::BLOCK, "Disable");
    let (perm, perm_text, perm_icon) = item(commands, g::DELETE, "Delete");

    commands.entity(open).observe(
        |t: Trigger<Pointer<Click>>, mut ctx: ResMut<CtxMenu>, lib: Res<Library>| {
            if !primary(&t) {
                return;
            }
            if let Some(e) = lib.entries.get(ctx.target) {
                open_location(&e.dir);
            }
            ctx.close();
        },
    );
    commands.entity(export).observe(
        |t: Trigger<Pointer<Click>>,
         mut ctx: ResMut<CtxMenu>,
         lib: Res<Library>,
         mut job: ResMut<ExportJob>| {
            if !primary(&t) {
                return;
            }
            if !ctx.has_glb {
                return; // hidden for models that cannot be edited
            }
            if let Some(e) = lib.entries.get(ctx.target) {
                export::start(&mut job, e.dir.clone());
            }
            ctx.close();
        },
    );
    // Rename: the name on the tile becomes a text field with its text selected.
    commands.entity(rename).observe(
        |t: Trigger<Pointer<Click>>,
         mut ctx: ResMut<CtxMenu>,
         lib: Res<Library>,
         mut ed: ResMut<Editing>,
         time: Res<Time<Real>>| {
            if !primary(&t) {
                return;
            }
            if let (Some(e), Some(tile)) = (lib.entries.get(ctx.target), lib.tiles.get(ctx.target))
            {
                ed.start(
                    Field::Tile(ctx.target),
                    &e.name,
                    Geom::tile(tile.name_text, tile.name_box),
                    true,
                    time.elapsed_secs_f64(),
                );
            }
            ctx.close();
        },
    );
    commands.entity(regen).observe(
        |t: Trigger<Pointer<Click>>,
         mut ctx: ResMut<CtxMenu>,
         lib: Res<Library>,
         mut pipe: ResMut<IconPipe>| {
            if !primary(&t) {
                return;
            }
            if let Some(e) = lib.entries.get(ctx.target) {
                icons::request_regen(
                    &mut pipe,
                    e.dir.clone(),
                    Some(ctx.target),
                    lib.generation,
                    None,
                ); // the list's icon: the game's own view
            }
            ctx.close();
        },
    );
    commands.entity(lock).observe(
        |t: Trigger<Pointer<Click>>,
         mut ctx: ResMut<CtxMenu>,
         mut lib: ResMut<Library>,
         mut layout: ResMut<Layout>| {
            if !primary(&t) || !ctx.has_glb {
                return; // greyed out
            }
            if !ctx.confirm_lock {
                ctx.confirm_lock = true; // shows the warning; a second click locks
                ctx.confirm = false;
                return;
            }
            if let Some(e) = lib.entries.get(ctx.target) {
                match remove_glbs(&e.dir) {
                    Ok(_) => {
                        layout.notice = None;
                        lib.dirty = true; // rescan: the model leaves EDITABLE and loses its Edit button
                    }
                    Err(er) => layout.notice = Some(format!("Could not delete the .glb: {er}")),
                }
            }
            ctx.close();
        },
    );
    commands.entity(toggle).observe(
        |t: Trigger<Pointer<Click>>,
         mut ctx: ResMut<CtxMenu>,
         mut lib: ResMut<Library>,
         mut layout: ResMut<Layout>| {
            if !primary(&t) {
                return;
            }
            if let Some(e) = lib.entries.get(ctx.target) {
                match set_enabled(&e.dir, e.disabled) {
                    Ok(()) => {
                        layout.notice = None;
                        lib.dirty = true; // rescan: the model moves to / from the Disabled section
                    }
                    Err(er) => layout.notice = Some(er),
                }
            }
            ctx.close();
        },
    );
    commands.entity(perm).observe(
        |t: Trigger<Pointer<Click>>,
         mut ctx: ResMut<CtxMenu>,
         mut lib: ResMut<Library>,
         mut layout: ResMut<Layout>| {
            if !primary(&t) {
                return;
            }
            if !ctx.confirm {
                ctx.confirm = true; // asks for a second click
                ctx.confirm_lock = false;
                return;
            }
            if let Some(e) = lib.entries.get(ctx.target) {
                // Only ever delete a model folder directly inside the library folder.
                if e.dir.parent() == Some(lib.root.as_path()) && e.dir != lib.root {
                    match std::fs::remove_dir_all(&e.dir) {
                        Ok(()) => {
                            layout.notice = None;
                            lib.dirty = true;
                        }
                        Err(er) => {
                            layout.notice = Some(format!("Could not delete the folder: {er}"))
                        }
                    }
                }
            }
            ctx.close();
        },
    );

    CtxRefs {
        menu,
        open,
        export,
        rename,
        regen,
        lock,
        toggle,
        perm,
        lock_text,
        lock_warn,
        toggle_text,
        perm_text,
        lock_icon,
        toggle_icon,
        perm_icon,
    }
}

/// Opens / places / closes the menu and keeps its entries up to date.
#[allow(clippy::too_many_arguments)]
pub fn ctx_menu_system(
    mut ctx: ResMut<CtxMenu>,
    lib: Res<Library>,
    refs: Res<CtxRefs>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut wheel: EventReader<MouseWheel>,
    ints: Query<&Interaction>,
    mut nodes: Query<&mut Node>,
    mut texts: Query<&mut Text>,
    mut colors: Query<&mut TextColor>,
    font: Res<IconFont>,
) {
    let scrolled = wheel.read().count() > 0;
    if ctx.open {
        let over = [
            refs.menu,
            refs.open,
            refs.export,
            refs.rename,
            refs.regen,
            refs.lock,
            refs.toggle,
            refs.perm,
        ]
        .iter()
        .any(|e| {
            ints.get(*e)
                .map(|i| *i != Interaction::None)
                .unwrap_or(false)
        });
        let pressed =
            mouse.just_pressed(MouseButton::Left) || mouse.just_pressed(MouseButton::Right);
        if keys.just_pressed(KeyCode::Escape)
            || scrolled
            || (pressed && !over)
            || ctx.generation != lib.generation
        {
            ctx.close();
        }
    }
    let set_display = |nodes: &mut Query<&mut Node>, e: Entity, d: Display| {
        if let Ok(mut n) = nodes.get_mut(e) {
            if n.display != d {
                n.display = d;
            }
        }
    };
    if !ctx.open {
        set_display(&mut nodes, refs.menu, Display::None);
        return;
    }
    if ctx.fresh {
        ctx.fresh = false;
        let (ww, wh) = windows
            .single()
            .map(|w| (w.width(), w.height()))
            .unwrap_or((1200.0, 760.0));
        let (x, y) = (
            ctx.pos.x.min(ww - MENU_W - 4.0).max(0.0),
            ctx.pos.y.min(wh - MENU_H - 4.0).max(0.0),
        );
        if let Ok(mut n) = nodes.get_mut(refs.menu) {
            n.left = Val::Px(x);
            n.top = Val::Px(y);
        }
    }
    set_display(&mut nodes, refs.menu, Display::Flex);
    // Export is only offered for editable models (a saved .glb in the folder), like the Edit button.
    set_display(
        &mut nodes,
        refs.export,
        if ctx.has_glb {
            Display::Flex
        } else {
            Display::None
        },
    );

    let mut set = |e: Entity, s: &str, c: Color| {
        if let Ok(mut t) = texts.get_mut(e) {
            if t.0 != s {
                t.0 = s.to_string();
            }
        }
        if let Ok(mut col) = colors.get_mut(e) {
            if col.0 != c {
                col.0 = c;
            }
        }
    };
    let lock_col = if ctx.has_glb { WARN } else { WARN_DIM };
    set(
        refs.lock_text,
        if ctx.confirm_lock {
            "Confirm lock"
        } else {
            "Lock model"
        },
        lock_col,
    );
    set_display(
        &mut nodes,
        refs.lock_warn,
        if ctx.confirm_lock {
            Display::Flex
        } else {
            Display::None
        },
    );
    let toggle_col = if ctx.disabled {
        EXPORT_COL
    } else {
        DISABLE_COL
    };
    set(
        refs.toggle_text,
        if ctx.disabled { "Enable" } else { "Disable" },
        toggle_col,
    );
    set(
        refs.perm_text,
        if ctx.confirm {
            "Click to confirm"
        } else {
            "Delete"
        },
        DANGER,
    );
    set(refs.lock_icon, &font.text(g::LOCK), lock_col);
    set(
        refs.toggle_icon,
        &font.text(if ctx.disabled {
            g::CHECK_CIRCLE
        } else {
            g::BLOCK
        }),
        toggle_col,
    );
    set(
        refs.perm_icon,
        &font.text(if ctx.confirm {
            g::DELETE_FOREVER
        } else {
            g::DELETE
        }),
        DANGER,
    );
}

// ---- app settings menu (title bar drop-down) ----------------------------------------------------------

#[derive(Resource, Default)]
pub struct SettingsMenu {
    pub open: bool,
}

#[derive(Resource)]
pub struct SettingsRefs {
    pub button: Entity,
    pub menu: Entity,
    pub folder: Entity,
    pub refresh: Entity,
    pub blender: Entity,
}

/// Click on the "Settings" button.
pub fn toggle_settings(t: Trigger<Pointer<Click>>, mut m: ResMut<SettingsMenu>) {
    if primary(&t) {
        m.open = !m.open;
    }
}

/// The (hidden) drop-down under the title bar, right-aligned. The caller attaches the actions to `folder` / `refresh`.
pub fn build_settings_menu(
    commands: &mut Commands,
    root: Entity,
    button: Entity,
    font: &IconFont,
) -> SettingsRefs {
    let menu = child(
        commands,
        root,
        (
            Interaction::default(),
            Node {
                position_type: PositionType::Absolute,
                right: Val::Px(10.0),
                top: Val::Px(40.0),
                width: Val::Px(262.0),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(2.0),
                padding: UiRect::all(Val::Px(4.0)),
                border: UiRect::all(Val::Px(1.0)),
                display: Display::None,
                ..default()
            },
            BackgroundColor(Color::srgb(0.12, 0.13, 0.16)),
            BorderColor(EDGE_HOVER),
            BorderRadius::all(Val::Px(6.0)),
            GlobalZIndex(150),
        ),
    );
    let mut item = |glyph: Glyph, label: &str| -> (Entity, Entity) {
        let b = child(
            commands,
            menu,
            (
                Button,
                Node {
                    height: Val::Px(28.0),
                    align_items: AlignItems::Center,
                    column_gap: Val::Px(8.0),
                    padding: UiRect::horizontal(Val::Px(8.0)),
                    ..default()
                },
                BackgroundColor(BTN),
                BorderRadius::all(Val::Px(4.0)),
            ),
        );
        let ic = child(
            commands,
            b,
            (
                icon(font, glyph, 18.0, TEXT),
                Node {
                    width: Val::Px(20.0),
                    ..default()
                },
                Pickable::IGNORE,
            ),
        );
        child(
            commands,
            b,
            (
                txt(label, 13.0, TEXT),
                TextLayout {
                    linebreak: LineBreak::NoWrap,
                    ..default()
                },
                Pickable::IGNORE,
            ),
        );
        // Every action closes the menu.
        commands
            .entity(b)
            .observe(|_: Trigger<Pointer<Click>>, mut m: ResMut<SettingsMenu>| m.open = false);
        (b, ic)
    };
    let (folder, _) = item(g::FOLDER_OPEN, "Change folder...");
    let (refresh, _) = item(g::REFRESH, "Refresh");
    let (blender, _) = item(g::LAUNCH, "Blender path...");
    SettingsRefs {
        button,
        menu,
        folder,
        refresh,
        blender,
    }
}

/// Shows / hides the menu; it closes on Escape, on a click outside it, and on the converter screen.
pub fn settings_menu_system(
    mut m: ResMut<SettingsMenu>,
    refs: Res<SettingsRefs>,
    layout: Res<Layout>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    ints: Query<&Interaction>,
    mut nodes: Query<&mut Node>,
) {
    if m.open {
        let over = [
            refs.button,
            refs.menu,
            refs.folder,
            refs.refresh,
            refs.blender,
        ]
        .iter()
        .any(|e| {
            ints.get(*e)
                .map(|i| *i != Interaction::None)
                .unwrap_or(false)
        });
        let pressed =
            mouse.just_pressed(MouseButton::Left) || mouse.just_pressed(MouseButton::Right);
        if keys.just_pressed(KeyCode::Escape) || (pressed && !over) || layout.converter {
            m.open = false;
        }
    }
    if let Ok(mut n) = nodes.get_mut(refs.menu) {
        let d = if m.open { Display::Flex } else { Display::None };
        if n.display != d {
            n.display = d;
        }
    }
}

// ---- renaming a model folder ---------------------------------------------------------------------------

fn copy_dir_all(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let (p, t) = (e.path(), to.join(e.file_name()));
        if p.is_dir() {
            copy_dir_all(&p, &t)?;
        } else {
            std::fs::copy(&p, &t)?;
        }
    }
    Ok(())
}

/// Files whose names never contain the model's name.
const FIXED_FILES: [&str; 3] = ["icon.png", "properties.cfg", "votv_convert.json"];

/// The new name of a file of the model `old` renamed to `new`: `<old>.obj`, `<old>.mtl`, `<old>.glb`, `<old>_...` and the
/// textures the converter names `diffuse_<old>.png`, `normal_<old>.png`, `pbr_<old>.png`, `emissive_<old>.png` (any
/// `<word>_<old>.<ext>`). Other files keep their names. Comparison ignores the case.
fn renamed_file(fname: &str, old: &str, new: &str) -> String {
    if FIXED_FILES.iter().any(|f| f.eq_ignore_ascii_case(fname)) {
        return fname.to_string();
    }
    if let Some(head) = fname.get(..old.len()) {
        if head.eq_ignore_ascii_case(old) {
            return format!("{new}{}", &fname[old.len()..]);
        }
    }
    // `<word>_<old>` followed by the extension (or the end)
    let stem_end = fname.find('.').unwrap_or(fname.len());
    let stem = &fname[..stem_end];
    if stem.len() > old.len() + 1 {
        let cut = stem.len() - old.len();
        if let (Some(tail), Some(sep)) = (stem.get(cut..), stem.get(cut - 1..cut)) {
            if sep == "_" && tail.eq_ignore_ascii_case(old) {
                return format!("{}{new}{}", &stem[..cut], &fname[stem_end..]);
            }
        }
    }
    fname.to_string()
}

/// Rewrites the places of an .obj / .mtl that carry the model's name, and nothing else (the old code replaced every
/// occurrence of the text, which wrecks a model with a short name): file references (`mtllib`, `map_*`, `bump`) get the
/// name replaced inside the file name; names of materials, objects and groups (`usemtl`, `newmtl`, `o`, `g`) change when
/// they are exactly the old name.
fn rewrite_refs(text: &str, old: &str, new: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let body = line.trim_end_matches(['\r', '\n']);
        let eol = &line[body.len()..];
        let Some(ws) = body.find(char::is_whitespace) else {
            out.push_str(line);
            continue;
        };
        let rest_at = ws + (body.len() - ws - body[ws..].trim_start().len());
        let (head, rest) = body.split_at(rest_at);
        let key = body[..ws].to_ascii_lowercase();
        out.push_str(head);
        if key == "mtllib"
            || key.starts_with("map_")
            || key == "bump"
            || key == "disp"
            || key == "decal"
        {
            out.push_str(&rest.replace(old, new));
        } else if matches!(key.as_str(), "usemtl" | "newmtl" | "o" | "g") && rest.trim() == old {
            out.push_str(new);
        } else {
            out.push_str(rest);
        }
        out.push_str(eol);
    }
    out
}

/// Brings `votv_convert.json` (written next to the model by the converter) in line with the new name: `name`,
/// `options.output_name` when it was the old name, and `source` when it pointed into the old folder. Best effort.
fn update_sidecar(new_dir: &Path, old_dir: &Path, old: &str, new: &str) {
    let path = new_dir.join("votv_convert.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return;
    };
    if v.get("name").and_then(|n| n.as_str()) == Some(old) {
        v["name"] = serde_json::Value::String(new.to_string());
    }
    if v.pointer("/options/output_name").and_then(|n| n.as_str()) == Some(old) {
        v["options"]["output_name"] = serde_json::Value::String(new.to_string());
    }
    if let Some(src) = v
        .get("source")
        .and_then(|s| s.as_str())
        .map(std::path::PathBuf::from)
    {
        let same = |a: &Path, b: &Path| {
            a.to_string_lossy()
                .eq_ignore_ascii_case(&b.to_string_lossy())
        };
        if let (Some(parent), Some(file)) = (src.parent(), src.file_name()) {
            if same(parent, old_dir) {
                let moved = new_dir.join(renamed_file(&file.to_string_lossy(), old, new));
                v["source"] = serde_json::Value::String(moved.to_string_lossy().into_owned());
            }
        }
    }
    if let Ok(out) = serde_json::to_string_pretty(&v) {
        let _ = std::fs::write(&path, out);
    }
}

/// Rename a model: the folder and every file that carries its name (`<old>.obj`, `<old>.mtl`, `<old>.glb`, the textures
/// `diffuse_<old>.png` / `normal_` / `pbr_` / `emissive_`, see `renamed_file`); inside the .obj / .mtl the references to
/// those names (`rewrite_refs`) and in `votv_convert.json` the name and source (`update_sidecar`) are updated too. The model is copied under the new name first and the old folder removed only afterwards, so a failure
/// leaves the old model untouched. Returns the new folder.
pub fn rename_model(dir: &Path, new_name: &str) -> Result<std::path::PathBuf, String> {
    let old = dir
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("The model folder has no name.")?
        .to_string();
    let parent = dir
        .parent()
        .ok_or("The model folder has no parent folder.")?;
    if new_name.is_empty() || new_name == old {
        return Err("Nothing to rename.".to_string());
    }
    let target = parent.join(new_name);
    if target.exists() {
        return Err(format!("A model named \"{new_name}\" already exists."));
    }
    let copy_all = || -> std::io::Result<()> {
        std::fs::create_dir(&target)?;
        for e in std::fs::read_dir(dir)? {
            let e = e?;
            let (p, fname) = (e.path(), e.file_name().to_string_lossy().into_owned());
            let lower = fname.to_lowercase();
            let new_fname = renamed_file(&fname, &old, new_name);
            if p.is_dir() {
                copy_dir_all(&p, &target.join(&new_fname))?;
            } else if lower.ends_with(".obj")
                || lower.ends_with(".mtl")
                || lower.ends_with(".obj.disabled")
            {
                match std::fs::read_to_string(&p) {
                    Ok(text) => std::fs::write(
                        target.join(&new_fname),
                        rewrite_refs(&text, &old, new_name),
                    )?,
                    Err(_) => {
                        std::fs::copy(&p, target.join(&new_fname))?; // not UTF-8: copy unchanged
                    }
                }
            } else {
                std::fs::copy(&p, target.join(&new_fname))?;
            }
        }
        Ok(())
    };
    match copy_all() {
        Ok(()) => {
            update_sidecar(&target, dir, &old, new_name);
            std::fs::remove_dir_all(dir).map_err(|e| format!("The model was copied to \"{new_name}\" but the old folder could not be removed: {e}"))?;
            Ok(target)
        }
        Err(e) => {
            let _ = std::fs::remove_dir_all(&target);
            Err(format!(
                "Could not rename the model: {e} (is it open in another program?)"
            ))
        }
    }
}

// ---- file operations -----------------------------------------------------------------------------------

fn open_location(dir: &Path) {
    #[cfg(target_os = "windows")]
    let (prog, arg) = ("explorer", dir.to_string_lossy().replace('/', "\\"));
    #[cfg(target_os = "macos")]
    let (prog, arg) = ("open", dir.to_string_lossy().into_owned());
    #[cfg(all(unix, not(target_os = "macos")))]
    let (prog, arg) = ("xdg-open", dir.to_string_lossy().into_owned());
    // explorer.exe reports failure exit codes even when it worked, so the result is not checked.
    let _ = std::process::Command::new(prog).arg(arg).spawn();
}

/// Delete every .glb in a model folder. Returns how many were removed.
fn remove_glbs(dir: &Path) -> std::io::Result<usize> {
    let mut n = 0;
    for e in std::fs::read_dir(dir)?.flatten() {
        let p = e.path();
        if p.is_file()
            && p.extension()
                .map(|x| x.eq_ignore_ascii_case("glb"))
                .unwrap_or(false)
        {
            std::fs::remove_file(&p)?;
            n += 1;
        }
    }
    Ok(n)
}

/// Disable (`enable == false`: `<x>.obj` -> `<x>.obj.disabled`) or restore (the reverse) a model folder.
fn set_enabled(dir: &Path, enable: bool) -> Result<(), String> {
    let rd = std::fs::read_dir(dir).map_err(|e| format!("Cannot read {}: {e}", dir.display()))?;
    let mut renamed = 0;
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        let lower = name.to_lowercase();
        let new_name = if enable {
            lower
                .ends_with(".obj.disabled")
                .then(|| name[..name.len() - crate::model::DISABLED_SUFFIX.len()].to_string())
        } else {
            (p.is_file() && lower.ends_with(".obj"))
                .then(|| format!("{name}{}", crate::model::DISABLED_SUFFIX))
        };
        if let Some(n) = new_name {
            std::fs::rename(&p, dir.join(&n)).map_err(|er| {
                format!("Could not rename {name}: {er} (is the model open in another program?)")
            })?;
            renamed += 1;
        }
    }
    if renamed == 0 {
        return Err("Nothing to rename in that folder.".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_matches_all_words() {
        assert!(name_matches(
            "horseless_headless_horsemann",
            "headless horse"
        ));
        assert!(name_matches("Chair", "CHA"));
        assert!(name_matches("anything", ""));
        assert!(!name_matches("chair", "table"));
        assert!(name_matches("old_chair", "old_chair"));
    }

    #[test]
    fn disable_and_restore_rename_the_obj() {
        let dir = std::env::temp_dir().join(format!("votv-mm-disable-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("m.obj"), "o a").unwrap();
        std::fs::write(dir.join("m.mtl"), "").unwrap();
        set_enabled(&dir, false).unwrap();
        assert!(
            dir.join("m.obj.disabled").is_file()
                && !dir.join("m.obj").exists()
                && dir.join("m.mtl").is_file()
        );
        set_enabled(&dir, true).unwrap();
        assert!(dir.join("m.obj").is_file() && !dir.join("m.obj.disabled").exists());
        assert!(set_enabled(&dir, true).is_err()); // nothing left to restore
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod rename_tests {
    use super::*;

    #[test]
    fn files_carrying_the_name_are_renamed() {
        assert_eq!(renamed_file("old.obj", "old", "new"), "new.obj");
        assert_eq!(renamed_file("old.glb", "old", "new"), "new.glb");
        assert_eq!(
            renamed_file("diffuse_old.png", "old", "new"),
            "diffuse_new.png"
        );
        assert_eq!(
            renamed_file("Normal_OLD.png", "old", "new"),
            "Normal_new.png"
        );
        assert_eq!(renamed_file("pbr_old.png", "old", "new"), "pbr_new.png");
        assert_eq!(
            renamed_file("emissive_old.png", "old", "new"),
            "emissive_new.png"
        );
        assert_eq!(renamed_file("icon.png", "old", "new"), "icon.png");
        assert_eq!(renamed_file("icon.png", "icon", "new"), "icon.png");
        assert_eq!(
            renamed_file("properties.cfg", "old", "new"),
            "properties.cfg"
        );
        assert_eq!(
            renamed_file("diffuse_golden.png", "old", "new"),
            "diffuse_golden.png"
        ); // not a whole name
    }

    #[test]
    fn only_name_carrying_lines_are_rewritten() {
        let mtl = "newmtl old\r\nKd 1 1 1\r\nmap_Kd diffuse_old.png\r\nmap_Bump normal_old.png\r\n";
        let out = rewrite_refs(mtl, "old", "new");
        assert_eq!(
            out,
            "newmtl new\r\nKd 1 1 1\r\nmap_Kd diffuse_new.png\r\nmap_Bump normal_new.png\r\n"
        );
        // a one letter name must not touch anything else
        let obj = "mtllib a.mtl\nusemtl a\no Ucx_a_1\nv 1 2 3\nvn 0 0 1\nf 1 2 3\n";
        assert_eq!(
            rewrite_refs(obj, "a", "chair"),
            "mtllib chair.mtl\nusemtl chair\no Ucx_a_1\nv 1 2 3\nvn 0 0 1\nf 1 2 3\n"
        );
    }

    #[test]
    fn rename_model_updates_every_file() {
        let root = std::env::temp_dir().join(format!("votv_rename_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("old");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("old.obj"), "mtllib old.mtl\nusemtl old\nv 1 2 3\n").unwrap();
        std::fs::write(
            dir.join("old.mtl"),
            "newmtl old\nmap_Kd diffuse_old.png\nmap_Bump normal_old.png\n",
        )
        .unwrap();
        for f in ["diffuse_old.png", "normal_old.png", "icon.png", "old.glb"] {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        std::fs::write(dir.join("properties.cfg"), "a=1\n").unwrap();
        let side = serde_json::json!({ "name": "old", "source": dir.join("old.glb").to_string_lossy(), "options": { "output_name": "old" } });
        std::fs::write(dir.join("votv_convert.json"), side.to_string()).unwrap();

        let new_dir = rename_model(&dir, "new").unwrap();
        assert!(!dir.exists());
        for f in [
            "new.obj",
            "new.mtl",
            "diffuse_new.png",
            "normal_new.png",
            "icon.png",
            "new.glb",
            "properties.cfg",
            "votv_convert.json",
        ] {
            assert!(new_dir.join(f).is_file(), "missing {f}");
        }
        let mtl = std::fs::read_to_string(new_dir.join("new.mtl")).unwrap();
        assert!(mtl.contains("map_Kd diffuse_new.png") && mtl.contains("newmtl new"));
        let v: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(new_dir.join("votv_convert.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(v["name"], "new");
        assert_eq!(v["options"]["output_name"], "new");
        assert!(v["source"].as_str().unwrap().ends_with("new.glb"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
