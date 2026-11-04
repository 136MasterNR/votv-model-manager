//! Icons from the Material Icons font (`assets/fonts/MaterialIcons-Regular.ttf`, Apache-2.0, from Google Fonts / the
//! material-design-icons repository). Material Symbols uses the same code points for these icons.
//!
//! The font file is compiled into the executable (`include_bytes!`), so a standalone release exe needs no `assets`
//! folder. It is registered as a font asset at startup (`IconFont::new`).
//!
//! An icon is a one-character `Text` set in the icon font (`icon()`); it is coloured with `TextColor` like any text, so
//! hover / danger / disabled states are plain colour changes. Bevy does not fall back to another font per glyph, so the
//! icon always has its own `TextFont`.
//!
//! If the font cannot be parsed the icons are left EMPTY on purpose (so the problem is obvious), except the close
//! button, which keeps a plain "X" so the window can still be closed.

use bevy::{prelude::*, text::LineBreak};

/// The icon font, embedded in the executable.
pub const ICON_FONT_BYTES: &[u8] = include_bytes!("../assets/fonts/MaterialIcons-Regular.ttf");

#[derive(Clone, Copy)]
pub struct Glyph {
    pub ch: char,
    /// Shown when the font is unusable. Empty for every icon except the close button.
    pub fallback: &'static str,
}

/// Code points of the icons in use (Material Icons names in the comments).
pub mod g {
    use super::Glyph;
    const fn gl(ch: char) -> Glyph {
        Glyph { ch, fallback: "" }
    }
    pub const CLOSE: Glyph = Glyph {
        ch: '\u{e5cd}',
        fallback: "X",
    }; // close; the only icon with a plain-text stand-in
    pub const EXPAND_MORE: Glyph = gl('\u{e5cf}'); // expand_more
    pub const EXPAND_LESS: Glyph = gl('\u{e5ce}'); // expand_less
    pub const CHEVRON_RIGHT: Glyph = gl('\u{e5cc}'); // chevron_right
    pub const CHECK: Glyph = gl('\u{e5ca}'); // check
    pub const CHECK_CIRCLE: Glyph = gl('\u{e86c}'); // check_circle
    pub const FOLDER_OPEN: Glyph = gl('\u{e2c8}'); // folder_open
    pub const REFRESH: Glyph = gl('\u{e5d5}'); // refresh
    pub const LOCK: Glyph = gl('\u{e897}'); // lock
    pub const BLOCK: Glyph = gl('\u{e14b}'); // block
    pub const DELETE: Glyph = gl('\u{e872}'); // delete
    pub const DELETE_FOREVER: Glyph = gl('\u{e92b}'); // delete_forever
    pub const FILE_UPLOAD: Glyph = gl('\u{e2c6}'); // file_upload (arrow out of a tray): export
    pub const LAUNCH: Glyph = gl('\u{e895}'); // launch (arrow out of a box): opens another program
    pub const EDIT: Glyph = gl('\u{e3c9}'); // edit
    pub const SEARCH: Glyph = gl('\u{e8b6}'); // search
    pub const ADD: Glyph = gl('\u{e145}'); // add
    pub const SETTINGS: Glyph = gl('\u{e8b8}'); // settings (gear)
    pub const DESCRIPTION: Glyph = gl('\u{e873}'); // description (document with lines)
    pub const MORE_TIME: Glyph = gl('\u{ea5d}'); // more_time (clock with a plus): sort by last added
    pub const UPDATE: Glyph = gl('\u{e923}'); // update (clock with arrow): sort by last modified
    pub const SORT_BY_ALPHA: Glyph = gl('\u{e053}'); // sort_by_alpha: sort by name
    pub const VISIBILITY: Glyph = gl('\u{e8f4}'); // visibility (eye): preview options
}

#[derive(Clone, Copy, PartialEq)]
pub enum IconState {
    Ready,
    /// The embedded font could not be parsed (should never happen): icons are empty, the X shows "X".
    Missing,
}

#[derive(Resource)]
pub struct IconFont {
    pub handle: Handle<Font>,
    pub state: IconState,
}

impl IconFont {
    /// Registers the embedded font with Bevy.
    pub fn new(fonts: &mut Assets<Font>) -> Self {
        match Font::try_from_bytes(ICON_FONT_BYTES.to_vec()) {
            Ok(font) => Self {
                handle: fonts.add(font),
                state: IconState::Ready,
            },
            Err(_) => {
                eprintln!(
                    "[icons] the embedded icon font could not be loaded; icons will be empty"
                );
                Self {
                    handle: Handle::default(),
                    state: IconState::Missing,
                }
            }
        }
    }
    /// The text to put in an icon's `Text` right now.
    pub fn text(&self, g: Glyph) -> String {
        if self.state == IconState::Missing {
            g.fallback.to_string()
        } else {
            g.ch.to_string()
        }
    }
    fn font(&self) -> Handle<Font> {
        if self.state == IconState::Missing {
            Handle::default()
        } else {
            self.handle.clone()
        }
    }
}

/// A one-glyph text in the icon font.
pub fn icon(font: &IconFont, g: Glyph, size: f32, color: Color) -> impl Bundle {
    (
        Text::new(font.text(g)),
        TextFont {
            font: font.font(),
            font_size: size,
            ..default()
        },
        TextColor(color),
        TextLayout {
            linebreak: LineBreak::NoWrap,
            ..default()
        },
    )
}
