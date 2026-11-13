//! The "which texture is the diffuse?" dialog of the import (`import.rs`).
//!
//! A model whose folder (or archive) name matches no `diffuse_<name>.png` is not left out silently: the import worker
//! thread asks, and waits, while this dialog offers
//! * one button per other `diffuse_*.png` file of the folder, if there are any: the import adds a copy of the chosen one
//!   named `diffuse_<model>.png`;
//! * **Import anyway**: the model is copied as it is, without a diffuse of its name;
//! * **Skip this model** (or Esc): the model is left out.
//! The overlay covers the whole window, so nothing underneath can be clicked meanwhile.

use bevy::prelude::*;

use crate::import::{Choice, ImportJob};
use crate::ui::{button, child, txt, ACCENT, DIM, EDGE_HOVER, PANEL, TEXT};

#[derive(Resource)]
pub struct PickRefs {
    overlay: Entity,
    title: Entity,
    body: Entity,
    list: Entity,
}

pub fn build(commands: &mut Commands, root: Entity) -> PickRefs {
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
            GlobalZIndex(210),
        ),
    );
    let panel = child(
        commands,
        overlay,
        (
            Node {
                width: Val::Px(460.0),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(12.0),
                padding: UiRect::all(Val::Px(20.0)),
                border: UiRect::all(Val::Px(1.0)),
                ..default()
            },
            BackgroundColor(PANEL),
            BorderColor(EDGE_HOVER),
            BorderRadius::all(Val::Px(8.0)),
        ),
    );
    let title = child(commands, panel, txt("", 18.0, TEXT));
    let body = child(commands, panel, txt("", 13.0, Color::srgb(0.75, 0.77, 0.82)));
    let list = child(
        commands,
        panel,
        Node {
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(6.0),
            ..default()
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
    let anyway = child(commands, row, button(Val::Px(130.0), Val::Px(30.0)));
    child(commands, anyway, txt("Import anyway", 14.0, TEXT));
    commands
        .entity(anyway)
        .observe(|t: Trigger<Pointer<Click>>, mut job: ResMut<ImportJob>| {
            if t.event().button == PointerButton::Primary {
                job.answer(Choice::Anyway);
            }
        });
    let skip = child(commands, row, button(Val::Px(150.0), Val::Px(30.0)));
    child(commands, skip, txt("Skip this model", 14.0, DIM));
    commands
        .entity(skip)
        .observe(|t: Trigger<Pointer<Click>>, mut job: ResMut<ImportJob>| {
            if t.event().button == PointerButton::Primary {
                job.answer(Choice::Skip);
            }
        });
    PickRefs {
        overlay,
        title,
        body,
        list,
    }
}

/// Shows the question while the import waits for it, one button per candidate texture; Esc skips the model.
#[allow(clippy::too_many_arguments)]
pub fn pick_system(
    mut commands: Commands,
    mut job: ResMut<ImportJob>,
    refs: Res<PickRefs>,
    keys: Res<ButtonInput<KeyCode>>,
    mut nodes: Query<&mut Node>,
    mut texts: Query<&mut Text>,
    mut shown: Local<u64>,
    mut buttons: Local<Vec<Entity>>,
) {
    let _bench = crate::benchmark::sys("sys.importpick.pick_system");
    if job.ask.is_some() && keys.just_pressed(KeyCode::Escape) {
        job.answer(Choice::Skip);
    }
    let want = if job.ask.is_some() {
        Display::Flex
    } else {
        Display::None
    };
    if let Ok(mut n) = nodes.get_mut(refs.overlay) {
        if n.display != want {
            n.display = want;
        }
    }
    let Some(q) = &job.ask else { return };
    if *shown == job.asked {
        return;
    }
    // A new question: its texts and one button per candidate.
    *shown = job.asked;
    let (model, archive, candidates) = (q.model.clone(), q.archive.clone(), q.candidates.clone());
    if let Ok(mut t) = texts.get_mut(refs.title) {
        t.0 = if candidates.is_empty() {
            format!("\"{model}\" has no diffuse texture")
        } else {
            format!("Which texture is the diffuse of \"{model}\"?")
        };
    }
    if let Ok(mut t) = texts.get_mut(refs.body) {
        t.0 = if candidates.is_empty() {
            format!(
                "\"{archive}\" has no diffuse_{model}.png, and no other diffuse texture. The game finds a model's texture by its name, so it would arrive without one. Import it anyway?"
            )
        } else {
            format!(
                "\"{archive}\" has no diffuse_{model}.png, but it has these other diffuse textures. The one you pick is added to the model as diffuse_{model}.png (the original file stays). Or import the model without a diffuse of its name."
            )
        };
    }
    for e in buttons.drain(..) {
        commands.entity(e).despawn();
    }
    for (i, name) in candidates.iter().enumerate() {
        let b = child(&mut commands, refs.list, button(Val::Percent(100.0), Val::Px(30.0)));
        child(&mut commands, b, txt(name, 14.0, ACCENT));
        commands
            .entity(b)
            .observe(move |t: Trigger<Pointer<Click>>, mut job: ResMut<ImportJob>| {
                if t.event().button == PointerButton::Primary {
                    job.answer(Choice::Pick(i));
                }
            });
        buttons.push(b);
    }
}
