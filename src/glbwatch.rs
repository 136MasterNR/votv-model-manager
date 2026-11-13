//! Watches the `.glb` open in the editor and regenerates the model when it changes on disk (typically after the user
//! exports it again from Blender, see `blender.rs`, or any other tool).
//!
//! * The change is recognised by the file's CONTENT (a 64-bit hash), not only its time stamp: saving the model rewrites
//!   the `.glb` into the model folder as an identical copy with a new time stamp, and that must not start another run.
//! * A changed file must stay unchanged for one more check (about a second) before it is read: exporters write in pieces.
//! * It then does exactly what dropping a new file on the editor does (`ConvertState::replace_input`): the parameters,
//!   the model's name and the camera stay, the conversion runs again. Nothing happens while a slider is being dragged.
//! * The window redraws on demand, so a system could sleep for a minute while Blender has the focus: while the editor is
//!   open the update interval is capped at one second (`pacing_system`).

use std::{
    path::PathBuf,
    time::{Duration, SystemTime},
};

use bevy::{
    prelude::*,
    winit::{UpdateMode, WinitSettings},
};

use crate::{converter::ConvertState, Layout};

/// Seconds between two looks at the file.
const POLL: f32 = 0.5;

type Stamp = (SystemTime, u64); // modified time, length

#[derive(Default)]
pub struct Watch {
    session: u64,
    path: Option<PathBuf>,
    stamp: Option<Stamp>,
    hash: Option<u64>,
    /// A changed stamp that has been seen `n` times in a row.
    candidate: Option<(Stamp, u32)>,
    since_poll: f32,
}

fn stamp_of(path: &std::path::Path) -> Option<Stamp> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.modified().ok()?, m.len()))
}

/// FNV-1a over the whole file.
fn hash_file(path: &std::path::Path) -> Option<u64> {
    let _bench = crate::benchmark::work("glbwatch.hash_file");
    let bytes = std::fs::read(path).ok()?;
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    Some(h)
}

pub fn watch_system(
    time: Res<Time>,
    layout: Res<Layout>,
    mut conv: ResMut<ConvertState>,
    mut w: Local<Watch>,
) {
    let _bench = crate::benchmark::sys("sys.glbwatch.watch_system");
    let input = conv.input.clone().filter(|p| layout.converter && is_glb(p));
    let Some(path) = input else {
        if w.path.is_some() {
            *w = Watch::default();
        }
        return;
    };
    w.since_poll += time.delta_secs();
    if w.path.as_ref() != Some(&path) || w.session != conv.session {
        // A new editor session (or another file): this is the baseline.
        *w = Watch {
            session: conv.session,
            stamp: stamp_of(&path),
            hash: hash_file(&path),
            path: Some(path),
            ..default()
        };
        return;
    }
    if w.since_poll < POLL {
        return;
    }
    w.since_poll = 0.0;
    let Some(cur) = stamp_of(&path) else { return }; // being replaced right now: look again next time
    if Some(cur) == w.stamp {
        w.candidate = None;
        return;
    }
    // The stamp changed: wait until it stops changing.
    match w.candidate {
        Some((c, n)) if c == cur => w.candidate = Some((c, n + 1)),
        _ => {
            w.candidate = Some((cur, 0));
            return;
        }
    }
    if w.candidate.is_none_or(|(_, n)| n < 1) || conv.hold > 0 {
        return;
    }
    w.candidate = None;
    w.stamp = Some(cur);
    let h = hash_file(&path);
    if h.is_none() || h == w.hash {
        return; // unreadable for now, or the same bytes (our own copy of the file): nothing to do
    }
    w.hash = h;
    println!("[convert] {} changed on disk: regenerating", path.display());
    conv.replace_input(path);
}

fn is_glb(p: &std::path::Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("glb"))
}

/// While the editor is open, wake the app at least once a second (it redraws on demand; with another window focused it
/// would otherwise sleep for a minute and the file would be noticed late). Restored when the editor closes.
pub fn pacing_system(
    layout: Res<Layout>,
    conv: Res<ConvertState>,
    mut winit: ResMut<WinitSettings>,
    mut saved: Local<Option<(Duration, Duration)>>,
) {
    let _bench = crate::benchmark::sys("sys.glbwatch.pacing_system");
    let watching = layout.converter && conv.input.is_some();
    let waits = |s: &WinitSettings| {
        let f = if let UpdateMode::Reactive { wait, .. } = &s.focused_mode {
            Some(*wait)
        } else {
            None
        };
        let u = if let UpdateMode::Reactive { wait, .. } = &s.unfocused_mode {
            Some(*wait)
        } else {
            None
        };
        (f, u)
    };
    if watching && saved.is_none() {
        if let (Some(f), Some(u)) = waits(&winit) {
            *saved = Some((f, u));
            let one = Duration::from_secs(1);
            if let UpdateMode::Reactive { wait, .. } = &mut winit.focused_mode {
                *wait = f.min(one);
            }
            if let UpdateMode::Reactive { wait, .. } = &mut winit.unfocused_mode {
                *wait = u.min(one);
            }
        }
    } else if !watching {
        if let Some((f, u)) = saved.take() {
            if let UpdateMode::Reactive { wait, .. } = &mut winit.focused_mode {
                *wait = f;
            }
            if let UpdateMode::Reactive { wait, .. } = &mut winit.unfocused_mode {
                *wait = u;
            }
        }
    }
}
