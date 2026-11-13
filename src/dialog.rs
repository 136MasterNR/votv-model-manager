//! The "replace the current model?" dialog. Dropping a .glb on the converter screen while a model is being edited does
//! not swap it right away: the dropped path waits in `ReplaceDialog::pending` until the user picks Replace (or Enter)
//! or Cancel (or Esc). The overlay covers the whole window, so nothing underneath can be clicked meanwhile.

use std::path::PathBuf;

use bevy::prelude::*;

use crate::converter::ConvertState;
use crate::icon_font::{g, icon, IconFont};
use crate::ui::{button, child, txt, ACCENT, BTN, DIM, EDGE_HOVER, PANEL, TEXT};
use crate::Layout;

/// Marker file in the settings folder: it exists when the user chose "Don't show this again".
fn marker_path() -> PathBuf {
    crate::settings_dir().join("no_replace_prompt")
}

#[derive(Resource, Default)]
pub struct ReplaceDialog {
    pub pending: Option<PathBuf>,
    /// Replace without asking (the "Don't show this again" box of the dialog; kept as a marker file in the settings folder).
    pub dont_ask: bool,
    /// The "Don't show this again" box of the open dialog.
    pub remember: bool,
}

impl ReplaceDialog {
    pub fn load() -> Self {
        Self {
            dont_ask: marker_path().exists(),
            ..default()
        }
    }
    /// Change the setting and keep it for the next start.
    pub fn set_dont_ask(&mut self, on: bool) {
        self.dont_ask = on;
        let p = marker_path();
        if on {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(&p, b"1");
        } else {
            let _ = std::fs::remove_file(&p);
        }
    }
}

#[derive(Resource)]
pub struct DialogRefs {
    overlay: Entity,
    body: Entity,
    check: Entity,
}

/// Replace: swap the input (keeps parameters and camera), or start a session if the editor was left meanwhile.
fn accept(dlg: &mut ReplaceDialog, layout: &mut Layout, conv: &mut ConvertState) {
    if dlg.remember {
        dlg.set_dont_ask(true);
    }
    dlg.remember = false;
    if let Some(path) = dlg.pending.take() {
        if layout.converter && conv.input.is_some() {
            conv.replace_input(path);
        } else {
            layout.converter = true;
            layout.preview_open = true;
            conv.begin(path);
        }
    }
}

pub fn build_dialog(commands: &mut Commands, root: Entity, font: &IconFont) -> DialogRefs {
    let overlay = child(
        commands,
        root,
        (
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                right: Val::Px(0.0),
                top: Val::Px(0.0),
                bottom: Val::Px(0.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                display: Display::None,
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.55)),
            GlobalZIndex(200),
        ),
    );
    let panel = child(
        commands,
        overlay,
        (
            Node {
                width: Val::Px(420.0),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(14.0),
                padding: UiRect::all(Val::Px(20.0)),
                border: UiRect::all(Val::Px(1.0)),
                ..default()
            },
            BackgroundColor(PANEL),
            BorderColor(EDGE_HOVER),
            BorderRadius::all(Val::Px(8.0)),
        ),
    );
    child(
        commands,
        panel,
        txt("Replace the current model?", 18.0, TEXT),
    );
    let body = child(
        commands,
        panel,
        txt("", 13.0, Color::srgb(0.75, 0.77, 0.82)),
    );
    // "Don't show this again" check box.
    let remember = child(
        commands,
        panel,
        (
            Button,
            Node {
                height: Val::Px(26.0),
                align_items: AlignItems::Center,
                column_gap: Val::Px(8.0),
                padding: UiRect::horizontal(Val::Px(4.0)),
                ..default()
            },
            BackgroundColor(BTN),
            BorderRadius::all(Val::Px(4.0)),
        ),
    );
    let boxe = child(
        commands,
        remember,
        (
            Node {
                width: Val::Px(18.0),
                height: Val::Px(18.0),
                border: UiRect::all(Val::Px(1.0)),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(Color::srgb(0.04, 0.05, 0.07)),
            BorderColor(EDGE_HOVER),
            BorderRadius::all(Val::Px(3.0)),
            Pickable::IGNORE,
        ),
    );
    let check = child(
        commands,
        boxe,
        (icon(font, g::CHECK, 16.0, Color::NONE), Pickable::IGNORE),
    );
    child(
        commands,
        remember,
        (txt("Don't show this again", 13.0, DIM), Pickable::IGNORE),
    );
    commands.entity(remember).observe(
        |t: Trigger<Pointer<Click>>, mut dlg: ResMut<ReplaceDialog>| {
            if t.event().button == PointerButton::Primary {
                dlg.remember = !dlg.remember;
            }
        },
    );
    let row = child(
        commands,
        panel,
        Node {
            justify_content: JustifyContent::FlexEnd,
            column_gap: Val::Px(10.0),
            margin: UiRect::top(Val::Px(6.0)),
            ..default()
        },
    );
    let cancel = child(commands, row, button(Val::Px(96.0), Val::Px(30.0)));
    child(commands, cancel, txt("Cancel", 14.0, TEXT));
    let replace = child(commands, row, button(Val::Px(96.0), Val::Px(30.0)));
    child(commands, replace, txt("Replace", 14.0, ACCENT));

    commands.entity(cancel).observe(
        |_: Trigger<Pointer<Click>>, mut dlg: ResMut<ReplaceDialog>| {
            dlg.pending = None;
            dlg.remember = false;
        },
    );
    commands.entity(replace).observe(
        |_: Trigger<Pointer<Click>>,
         mut dlg: ResMut<ReplaceDialog>,
         mut layout: ResMut<Layout>,
         mut conv: ResMut<ConvertState>| {
            accept(&mut dlg, &mut layout, &mut conv);
        },
    );
    DialogRefs {
        overlay,
        body,
        check,
    }
}

/// Shows the dialog while a path is pending; Enter replaces, Esc cancels.
#[allow(clippy::too_many_arguments)]
pub fn dialog_system(
    mut dlg: ResMut<ReplaceDialog>,
    refs: Res<DialogRefs>,
    keys: Res<ButtonInput<KeyCode>>,
    mut layout: ResMut<Layout>,
    mut conv: ResMut<ConvertState>,
    mut nodes: Query<&mut Node>,
    mut texts: Query<&mut Text>,
    mut text_colors: Query<&mut TextColor>,
) {
    let _bench = crate::benchmark::sys("sys.dialog.dialog_system");
    if dlg.pending.is_some() {
        if keys.just_pressed(KeyCode::Escape) {
            dlg.pending = None;
            dlg.remember = false;
        } else if keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::NumpadEnter) {
            accept(&mut dlg, &mut layout, &mut conv);
        }
    }
    let want = if dlg.pending.is_some() {
        Display::Flex
    } else {
        Display::None
    };
    if let Ok(mut n) = nodes.get_mut(refs.overlay) {
        if n.display != want {
            n.display = want;
        }
    }
    if let Some(p) = &dlg.pending {
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let open = conv.current_model_name().unwrap_or_default();
        let msg = format!(
            "\"{name}\" will replace the model you are editing (\"{open}\"). The model keeps its name and folder; your current parameters are kept."
        );
        if let Ok(mut t) = texts.get_mut(refs.body) {
            if t.0 != msg {
                t.0 = msg;
            }
        }
    }
    if let Ok(mut c) = text_colors.get_mut(refs.check) {
        let want = if dlg.remember { ACCENT } else { Color::NONE };
        if c.0 != want {
            c.0 = want;
        }
    }
}
