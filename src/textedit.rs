//! Inline text fields, with the behaviour of an HTML `<input type="text">`.
//!
//! Used by: the model name in the properties panel (rename the model being edited), the value of a slider (type an exact
//! number) and the name on a list tile ("Rename" in the context menu). Bevy has no text input widget, so this module
//! implements one: a caret you can move, a selection, editing keys, clipboard and mouse placement.
//!
//! Mouse: a press focuses the field and puts the caret under the pointer; drag selects; double click selects all;
//! shift+click extends the selection; a click outside applies the edit.
//! Keys: Left/Right (Ctrl = by word, Shift = extend the selection), Home/End, Backspace/Delete (Ctrl = by word),
//! Ctrl+A / C / X / V (clipboard works on Windows), Enter or Tab applies, Esc cancels.
//!
//! Drawing: the text is the field's own `Text` node; the caret and the selection are two small overlay nodes that are
//! re-parented into the active field and positioned from the glyph advance. That works because Bevy's default font is
//! monospaced (advance = 0.6 em). When the text is longer than the box it scrolls so the caret stays visible.
//!
//! Applying: Rename sets `opts.output_name` and regenerates (the model keeps one folder; the old folder is removed
//! by `converter_system`); a slider value is any number (not limited to the slider's range; rounded for whole-number sliders) and regenerates;
//! a tile name renames the model folder and its files (`listmenu::rename_model`).

use bevy::{
    input::{
        keyboard::{Key, KeyboardInput},
        ButtonState,
    },
    prelude::*,
    window::{PrimaryWindow, RequestRedraw, SystemCursorIcon},
};

use crate::conv_ui::{model_name, ParamKind, ParamRows, Row};
use crate::converter::ConvertState;
use crate::listmenu::rename_model;
use crate::ui::{child, set_cursor, short, ACCENT, EDGE, EDGE_HOVER};
use crate::{Layout, Library};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Field {
    /// The model name in the properties panel.
    Rename,
    /// A slider value; the index is the row in the parameter table.
    Slider(usize),
    /// The name on a list tile; the index is the entry in the library.
    Tile(usize),
}

/// Where a field's text and box are, and the numbers needed to place the caret (logical px; offsets are from the box's
/// padding edge).
#[derive(Clone, Copy)]
pub struct Geom {
    pub text: Entity,
    pub boxe: Entity,
    pub size: f32,
    pub x0: f32,
    /// Room kept free at the right edge (the title's pencil icon), so the caret never scrolls under it.
    pub right: f32,
    pub top: f32,
    pub h: f32,
}

impl Geom {
    pub fn rename(text: Entity, boxe: Entity) -> Self {
        Self {
            text,
            boxe,
            size: 16.0,
            x0: 8.0,
            // padding 8 + pencil 14 + gap 6 (the name's scroll area ends before the pencil)
            right: 8.0 + 26.0 + 6.0,
            top: 4.0,
            h: 19.0,
        }
    }
    pub fn slider(text: Entity, boxe: Entity) -> Self {
        Self {
            text,
            boxe,
            size: 13.0,
            x0: 6.0,
            right: 6.0,
            top: 1.0,
            h: 16.0,
        }
    }
    pub fn tile(text: Entity, boxe: Entity) -> Self {
        Self {
            text,
            boxe,
            size: 12.0,
            x0: 3.0,
            right: 3.0,
            top: 2.0,
            h: 15.0,
        }
    }
    fn adv(&self) -> f32 {
        self.size * 0.6 // Fira Mono, Bevy's default font
    }
}

const BORDER: f32 = 1.0;

#[derive(Resource, Default)]
pub struct Editing {
    pub field: Option<Field>,
    chars: Vec<char>,
    pub caret: usize,
    /// The other end of the selection (None = nothing selected).
    pub anchor: Option<usize>,
    pub widget: Option<Entity>,
    geom: Option<Geom>,
    /// Horizontal scroll of the text inside the box (px).
    scroll: f32,
    /// Where a mouse drag started (character index).
    drag_from: Option<usize>,
    last_click: Option<(Entity, f64)>,
    /// Time of the last input; the caret blinks from there.
    blink: f64,
}

impl Editing {
    pub fn active(&self) -> bool {
        self.field.is_some()
    }
    pub fn is(&self, f: Field) -> bool {
        self.field == Some(f)
    }
    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }
    fn numeric(&self) -> bool {
        matches!(self.field, Some(Field::Slider(_)))
    }
    fn limit(&self) -> usize {
        if self.numeric() {
            14
        } else {
            48
        }
    }
    pub fn start(&mut self, field: Field, text: &str, geom: Geom, select_all: bool, now: f64) {
        self.field = Some(field);
        self.chars = text.chars().collect();
        self.caret = self.chars.len();
        self.anchor = if select_all && !self.chars.is_empty() {
            Some(0)
        } else {
            None
        };
        self.widget = Some(geom.boxe);
        self.geom = Some(geom);
        self.scroll = 0.0;
        self.drag_from = None;
        self.blink = now;
    }
    fn stop(&mut self) {
        self.field = None;
        self.widget = None;
        self.geom = None;
        self.chars.clear();
        self.anchor = None;
        self.drag_from = None;
    }
    /// The selected range (low, high), if any.
    fn sel(&self) -> Option<(usize, usize)> {
        match self.anchor {
            Some(a) if a != self.caret => Some((a.min(self.caret), a.max(self.caret))),
            _ => None,
        }
    }
    fn selected_text(&self) -> String {
        self.sel()
            .map(|(a, b)| self.chars[a..b].iter().collect())
            .unwrap_or_default()
    }
    fn delete_selection(&mut self) -> bool {
        if let Some((a, b)) = self.sel() {
            self.chars.drain(a..b);
            self.caret = a;
            self.anchor = None;
            true
        } else {
            false
        }
    }
    fn insert(&mut self, s: &str) {
        let numeric = self.numeric();
        let keep: Vec<char> = s
            .chars()
            .filter(|c| !c.is_control())
            .map(|c| if numeric && c == ',' { '.' } else { c })
            .filter(|c| !numeric || c.is_ascii_digit() || *c == '.' || *c == '-')
            .collect();
        if keep.is_empty() {
            return;
        }
        self.delete_selection();
        for c in keep {
            if self.chars.len() >= self.limit() {
                break;
            }
            self.chars.insert(self.caret, c);
            self.caret += 1;
        }
        self.anchor = None;
    }
    fn move_to(&mut self, to: usize, extend: bool) {
        let to = to.min(self.chars.len());
        if extend {
            if self.anchor.is_none() {
                self.anchor = Some(self.caret);
            }
            self.caret = to;
            if self.anchor == Some(self.caret) {
                self.anchor = None;
            }
        } else {
            self.anchor = None;
            self.caret = to;
        }
    }
    fn word_left(&self) -> usize {
        let mut i = self.caret;
        while i > 0 && self.chars[i - 1] == ' ' {
            i -= 1;
        }
        while i > 0 && self.chars[i - 1] != ' ' {
            i -= 1;
        }
        i
    }
    fn word_right(&self) -> usize {
        let n = self.chars.len();
        let mut i = self.caret;
        while i < n && self.chars[i] != ' ' {
            i += 1;
        }
        while i < n && self.chars[i] == ' ' {
            i += 1;
        }
        i
    }
    /// The character index under window x (logical px).
    fn index_at(
        &self,
        boxes: &Query<(&GlobalTransform, &ComputedNode)>,
        widget: Entity,
        x: f32,
    ) -> usize {
        let (Some(g), Ok((gt, c))) = (self.geom, boxes.get(widget)) else {
            return self.chars.len();
        };
        let left = (gt.translation().x - c.size.x * 0.5) * c.inverse_scale_factor;
        let px = x - left - BORDER - g.x0 + self.scroll;
        ((px / g.adv()).round().max(0.0) as usize).min(self.chars.len())
    }
}

pub fn shift_down(keys: &ButtonInput<KeyCode>) -> bool {
    keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight)
}

/// A mouse press inside the active field: place the caret (double click selects all, shift extends the selection).
pub fn press(
    ed: &mut Editing,
    boxes: &Query<(&GlobalTransform, &ComputedNode)>,
    widget: Entity,
    x: f32,
    shift: bool,
    now: f64,
) {
    let idx = ed.index_at(boxes, widget, x);
    let double = ed
        .last_click
        .map(|(w, t)| w == widget && now - t < 0.35)
        .unwrap_or(false);
    ed.last_click = Some((widget, now));
    ed.blink = now;
    if double {
        ed.anchor = Some(0);
        ed.caret = ed.chars.len();
        ed.drag_from = None;
    } else if shift {
        ed.move_to(idx, true);
    } else {
        ed.caret = idx;
        ed.anchor = None;
        ed.drag_from = Some(idx);
    }
}

/// Dragging inside the active field selects.
pub fn field_drag(
    t: Trigger<Pointer<Drag>>,
    boxes: Query<(&GlobalTransform, &ComputedNode)>,
    mut ed: ResMut<Editing>,
) {
    if ed.widget != Some(t.target()) {
        return;
    }
    let Some(from) = ed.drag_from else { return };
    let idx = ed.index_at(&boxes, t.target(), t.event().pointer_location.position.x);
    ed.anchor = Some(from);
    ed.caret = idx;
    if ed.anchor == Some(ed.caret) {
        ed.anchor = None;
    }
}

/// Hover over a field: pointer cursor and a lighter border (not while the field is being edited).
pub fn field_over(
    t: Trigger<Pointer<Over>>,
    mut commands: Commands,
    win: Query<Entity, With<PrimaryWindow>>,
    ed: Res<Editing>,
    mut borders: Query<&mut BorderColor>,
) {
    set_cursor(&mut commands, &win, SystemCursorIcon::Pointer);
    if ed.widget != Some(t.target()) {
        if let Ok(mut b) = borders.get_mut(t.target()) {
            b.0 = EDGE_HOVER;
        }
    }
}

pub fn field_out(
    t: Trigger<Pointer<Out>>,
    mut commands: Commands,
    win: Query<Entity, With<PrimaryWindow>>,
    ed: Res<Editing>,
    mut borders: Query<&mut BorderColor>,
) {
    set_cursor(&mut commands, &win, SystemCursorIcon::Default);
    if ed.widget != Some(t.target()) {
        if let Ok(mut b) = borders.get_mut(t.target()) {
            b.0 = EDGE;
        }
    }
}

// ---- clipboard (Windows; elsewhere nothing happens) -----------------------------------------------------

#[cfg(windows)]
fn clipboard_get() -> Option<String> {
    use std::ffi::c_void;
    #[link(name = "user32")]
    extern "system" {
        fn OpenClipboard(owner: *mut c_void) -> i32;
        fn CloseClipboard() -> i32;
        fn GetClipboardData(format: u32) -> *mut c_void;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GlobalLock(mem: *mut c_void) -> *mut c_void;
        fn GlobalUnlock(mem: *mut c_void) -> i32;
    }
    const CF_UNICODETEXT: u32 = 13;
    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return None;
        }
        let mut out = None;
        let h = GetClipboardData(CF_UNICODETEXT);
        if !h.is_null() {
            let p = GlobalLock(h) as *const u16;
            if !p.is_null() {
                let mut n = 0;
                while *p.add(n) != 0 && n < 4096 {
                    n += 1;
                }
                out = Some(String::from_utf16_lossy(std::slice::from_raw_parts(p, n)));
                GlobalUnlock(h);
            }
        }
        CloseClipboard();
        out
    }
}

#[cfg(windows)]
fn clipboard_set(text: &str) {
    use std::ffi::c_void;
    #[link(name = "user32")]
    extern "system" {
        fn OpenClipboard(owner: *mut c_void) -> i32;
        fn CloseClipboard() -> i32;
        fn EmptyClipboard() -> i32;
        fn SetClipboardData(format: u32, mem: *mut c_void) -> *mut c_void;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GlobalAlloc(flags: u32, bytes: usize) -> *mut c_void;
        fn GlobalLock(mem: *mut c_void) -> *mut c_void;
        fn GlobalUnlock(mem: *mut c_void) -> i32;
    }
    const CF_UNICODETEXT: u32 = 13;
    const GMEM_MOVEABLE: u32 = 2;
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let h = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2);
        if h.is_null() {
            return;
        }
        let p = GlobalLock(h) as *mut u16;
        if p.is_null() {
            return;
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr(), p, wide.len());
        GlobalUnlock(h);
        if OpenClipboard(std::ptr::null_mut()) != 0 {
            EmptyClipboard();
            // On success the system owns the memory.
            SetClipboardData(CF_UNICODETEXT, h);
            CloseClipboard();
        }
    }
}

#[cfg(not(windows))]
fn clipboard_get() -> Option<String> {
    None
}
#[cfg(not(windows))]
fn clipboard_set(_: &str) {}

// ---- applying -------------------------------------------------------------------------------------------

fn commit(
    ed: &mut Editing,
    st: &mut ConvertState,
    rows: &ParamRows,
    lib: &mut Library,
    layout: &mut Layout,
) {
    let Some(field) = ed.field else { return };
    let text = ed.text().trim().to_string();
    match field {
        Field::Rename => {
            if !text.is_empty() && text != model_name(st) {
                st.opts.output_name = Some(text);
                st.changed(0.0);
            }
        }
        Field::Slider(i) => {
            if let Some(Row::Param(def)) = rows.0.get(i) {
                if let ParamKind::Slider {
                    decimals, get, set, ..
                } = &def.kind
                {
                    if let Ok(mut v) = text.replace(',', ".").parse::<f64>() {
                        if v.is_finite() {
                            // No limits: any number can be typed (the slider only draws the range). Whole-number sliders
                            // still round. The converter reports a value it cannot use as a failed generation.
                            if *decimals == 0 {
                                v = v.round();
                            }
                            if v == 0.0 {
                                v = 0.0; // "-0", "-0.0" or a rounded -0.3 must not be stored as a negative zero
                            }
                            if (v - get(&st.opts)).abs() > 1.0e-9 {
                                set(&mut st.opts, v);
                                st.changed(0.0);
                            }
                        }
                    }
                }
            }
        }
        Field::Tile(i) => {
            let name = votv_convert::sanitize_name(&text);
            if let Some(e) = lib.entries.get(i) {
                if !name.is_empty() && name != e.name {
                    match rename_model(&e.dir, &name) {
                        Ok(_) => {
                            layout.notice = None;
                            lib.dirty = true; // rescan: the model is listed under its new name
                        }
                        Err(m) => layout.notice = Some(m),
                    }
                }
            }
        }
    }
    ed.stop();
}

/// Keys and clicks for the active field.
#[allow(clippy::too_many_arguments)]
pub fn edit_input_system(
    mut events: EventReader<KeyboardInput>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    ints: Query<&Interaction>,
    rows: Res<ParamRows>,
    time: Res<Time<Real>>,
    mut ed: ResMut<Editing>,
    mut st: ResMut<ConvertState>,
    mut lib: ResMut<Library>,
    mut layout: ResMut<Layout>,
) {
    if !ed.active() {
        return;
    }
    let now = time.elapsed_secs_f64();
    if mouse.just_pressed(MouseButton::Left) {
        let over = ed
            .widget
            .and_then(|w| ints.get(w).ok())
            .map(|i| *i != Interaction::None)
            .unwrap_or(false);
        if !over {
            commit(&mut ed, &mut st, &rows, &mut lib, &mut layout);
            return;
        }
    }
    let ctrl = keys.pressed(KeyCode::ControlLeft)
        || keys.pressed(KeyCode::ControlRight)
        || keys.pressed(KeyCode::SuperLeft);
    let shift = shift_down(&keys);
    for ev in events.read() {
        if ev.state != ButtonState::Pressed || !ed.active() {
            continue;
        }
        ed.blink = now;
        match &ev.logical_key {
            Key::Character(c) => {
                if ctrl {
                    match c.to_lowercase().as_str() {
                        "a" => {
                            ed.anchor = Some(0);
                            ed.caret = ed.chars.len();
                        }
                        "c" => {
                            let s = ed.selected_text();
                            if !s.is_empty() {
                                clipboard_set(&s);
                            }
                        }
                        "x" => {
                            let s = ed.selected_text();
                            if !s.is_empty() {
                                clipboard_set(&s);
                                ed.delete_selection();
                            }
                        }
                        "v" => {
                            if let Some(s) = clipboard_get() {
                                ed.insert(&s);
                            }
                        }
                        _ => {}
                    }
                } else {
                    ed.insert(c);
                }
            }
            Key::Space => {
                if !ctrl && !ed.numeric() {
                    ed.insert(" ");
                }
            }
            Key::Backspace => {
                if !ed.delete_selection() {
                    let from = if ctrl {
                        ed.word_left()
                    } else {
                        ed.caret.saturating_sub(1)
                    };
                    let to = ed.caret;
                    ed.chars.drain(from..to);
                    ed.caret = from;
                }
            }
            Key::Delete => {
                if !ed.delete_selection() {
                    let to = if ctrl {
                        ed.word_right()
                    } else {
                        (ed.caret + 1).min(ed.chars.len())
                    };
                    let from = ed.caret;
                    ed.chars.drain(from..to);
                }
            }
            Key::ArrowLeft => {
                if !shift && ed.sel().is_some() {
                    let (a, _) = ed.sel().unwrap();
                    ed.move_to(a, false);
                } else {
                    let to = if ctrl {
                        ed.word_left()
                    } else {
                        ed.caret.saturating_sub(1)
                    };
                    ed.move_to(to, shift);
                }
            }
            Key::ArrowRight => {
                if !shift && ed.sel().is_some() {
                    let (_, b) = ed.sel().unwrap();
                    ed.move_to(b, false);
                } else {
                    let to = if ctrl { ed.word_right() } else { ed.caret + 1 };
                    ed.move_to(to, shift);
                }
            }
            Key::Home => ed.move_to(0, shift),
            Key::End => {
                let n = ed.chars.len();
                ed.move_to(n, shift);
            }
            Key::Enter | Key::Tab => {
                commit(&mut ed, &mut st, &rows, &mut lib, &mut layout);
                return;
            }
            Key::Escape => {
                ed.stop();
                return;
            }
            _ => {}
        }
    }
}

// ---- drawing --------------------------------------------------------------------------------------------

/// The caret and the selection highlight; re-parented into whichever field is being edited.
///
/// They are CHILDREN of the active field, so they are despawned together with it (a list rescan removes the tiles, the
/// editor rebuilds its panel, ...). `edit_visuals_system` therefore checks that they still exist before using them and
/// spawns new ones when they do not (using a dead entity in `add_children` panics).
#[derive(Resource)]
pub struct EditOverlay {
    sel: Entity,
    caret: Entity,
}

fn overlay_node(commands: &mut Commands, color: Color) -> Entity {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                display: Display::None,
                ..default()
            },
            BackgroundColor(color),
            Pickable::IGNORE,
        ))
        .id()
}
const SEL_COLOR: Color = Color::srgba(0.30, 0.58, 0.98, 0.40);
const CARET_COLOR: Color = Color::srgb(0.85, 0.90, 1.0);

pub fn build_overlay(commands: &mut Commands, root: Entity) -> EditOverlay {
    let node = Node {
        position_type: PositionType::Absolute,
        display: Display::None,
        ..default()
    };
    let sel = child(
        commands,
        root,
        (node.clone(), BackgroundColor(SEL_COLOR), Pickable::IGNORE),
    );
    let caret = child(
        commands,
        root,
        (node, BackgroundColor(CARET_COLOR), Pickable::IGNORE),
    );
    EditOverlay { sel, caret }
}

/// Text, caret, selection, scrolling and borders of the active field; restores the normal look when it ends.
#[allow(clippy::too_many_arguments)]
pub fn edit_visuals_system(
    mut commands: Commands,
    mut ed: ResMut<Editing>,
    time: Res<Time<Real>>,
    st: Res<ConvertState>,
    rows: Res<ParamRows>,
    lib: Res<Library>,
    mut overlay: ResMut<EditOverlay>,
    computed: Query<&ComputedNode>,
    mut texts: Query<&mut Text>,
    mut nodes: Query<&mut Node>,
    mut layouts: Query<&mut TextLayout>,
    mut borders: Query<&mut BorderColor>,
    mut bgs: Query<&mut BackgroundColor>,
    mut redraw: EventWriter<RequestRedraw>,
    mut last: Local<Option<(Field, Geom)>>,
) {
    let set_text = |texts: &mut Query<&mut Text>, e: Entity, s: String| {
        if let Ok(mut t) = texts.get_mut(e) {
            if t.0 != s {
                t.0 = s;
            }
        }
    };
    // The previous field ended (or another one started): put its normal look back.
    if let Some((pf, pg)) = *last {
        if Some(pf) != ed.field {
            let (text, border) = match pf {
                Field::Rename => (model_name(&st), Color::NONE), // the name is plain text again
                Field::Slider(i) => {
                    let s = match rows.0.get(i) {
                        Some(Row::Param(def)) => match &def.kind {
                            ParamKind::Slider {
                                decimals,
                                unit,
                                get,
                                ..
                            } => format!("{:.*}{}", *decimals, get(&st.opts), unit),
                            _ => String::new(),
                        },
                        _ => String::new(),
                    };
                    (s, EDGE)
                }
                Field::Tile(i) => (
                    lib.entries
                        .get(i)
                        .map(|e| short(&e.name, 17))
                        .unwrap_or_default(),
                    Color::NONE,
                ),
            };
            set_text(&mut texts, pg.text, text);
            if let Ok(mut b) = borders.get_mut(pg.boxe) {
                b.0 = border;
            }
            if let Ok(mut n) = nodes.get_mut(pg.text) {
                n.margin.left = Val::Px(0.0);
            }
            match pf {
                Field::Slider(_) => {
                    if let Ok(mut n) = nodes.get_mut(pg.boxe) {
                        n.justify_content = JustifyContent::FlexEnd; // values are right aligned again
                    }
                }
                Field::Tile(_) => {
                    if let Ok(mut l) = layouts.get_mut(pg.text) {
                        l.justify = JustifyText::Center;
                    }
                    if let Ok(mut b) = bgs.get_mut(pg.boxe) {
                        b.0 = Color::NONE;
                    }
                }
                Field::Rename => {} // the name is one line (it scrolls sideways) in both states
            }
            for e in [overlay.sel, overlay.caret] {
                if let Ok(mut n) = nodes.get_mut(e) {
                    n.display = Display::None;
                }
            }
            *last = None;
        }
    }
    let (Some(field), Some(g)) = (ed.field, ed.geom) else {
        return;
    };

    // The field itself is gone (its tile was removed by a rescan, the panel was rebuilt, ...): drop the edit.
    if nodes.get(g.boxe).is_err() {
        ed.stop();
        *last = None;
        return;
    }

    // A field just started: move the overlays into it and switch it to the editing look.
    if last.is_none() {
        // The overlays die with the field they were in last: make new ones when that happened.
        if nodes.get(overlay.sel).is_err() {
            overlay.sel = overlay_node(&mut commands, SEL_COLOR);
        }
        if nodes.get(overlay.caret).is_err() {
            overlay.caret = overlay_node(&mut commands, CARET_COLOR);
        }
        commands
            .entity(g.boxe)
            .add_children(&[overlay.sel, overlay.caret]);
        match field {
            Field::Slider(_) => {
                if let Ok(mut n) = nodes.get_mut(g.boxe) {
                    n.justify_content = JustifyContent::FlexStart; // text from the left while editing
                }
            }
            Field::Tile(_) => {
                if let Ok(mut l) = layouts.get_mut(g.text) {
                    l.justify = JustifyText::Left;
                }
                if let Ok(mut b) = bgs.get_mut(g.boxe) {
                    b.0 = Color::srgb(0.07, 0.08, 0.10);
                }
            }
            Field::Rename => {} // the name is already one line; the scrolling below moves its text
        }
        *last = Some((field, g));
    }
    redraw.write(RequestRedraw); // the caret blinks

    set_text(&mut texts, g.text, ed.text());
    if let Ok(mut b) = borders.get_mut(g.boxe) {
        b.0 = ACCENT;
    }
    // Scroll so the caret stays inside the box.
    let adv = g.adv();
    let box_w = computed
        .get(g.boxe)
        .map(|c| c.size.x * c.inverse_scale_factor)
        .unwrap_or(0.0);
    let visible = (box_w - 2.0 * BORDER - g.x0 - g.right).max(adv * 2.0);
    let caret_px = ed.caret as f32 * adv;
    let scroll = (caret_px - (visible - adv)).max(0.0);
    if (ed.scroll - scroll).abs() > 0.01 {
        ed.scroll = scroll;
    }
    if let Ok(mut n) = nodes.get_mut(g.text) {
        let m = Val::Px(-scroll);
        if n.margin.left != m {
            n.margin.left = m;
        }
    }
    // Selection highlight and caret.
    let sel = ed.sel();
    let blink_on = (((time.elapsed_secs_f64() - ed.blink) * 2.0) as u64).is_multiple_of(2);
    if let Ok(mut n) = nodes.get_mut(overlay.sel) {
        match sel {
            Some((a, b)) => {
                n.display = Display::Flex;
                n.left = Val::Px(g.x0 + a as f32 * adv - scroll);
                n.top = Val::Px(g.top);
                n.width = Val::Px((b - a) as f32 * adv);
                n.height = Val::Px(g.h);
            }
            None => n.display = Display::None,
        }
    }
    if let Ok(mut n) = nodes.get_mut(overlay.caret) {
        if sel.is_none() && blink_on {
            n.display = Display::Flex;
            n.left = Val::Px(g.x0 + caret_px - scroll - 0.5);
            n.top = Val::Px(g.top);
            n.width = Val::Px(1.5);
            n.height = Val::Px(g.h);
        } else {
            n.display = Display::None;
        }
    }
}
