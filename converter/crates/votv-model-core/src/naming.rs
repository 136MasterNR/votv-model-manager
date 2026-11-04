//! File and object naming rules.
//!
//! A model folder is `<name>/` containing `<name>.obj`, `<name>.mtl`, `<kind>_<material>.png` textures and
//! `properties.cfg`. Names are ASCII (the manager's UI font is ASCII only), lower case, `[a-z0-9_]`, no spaces.
//! The converter names its single material after the model, so `diffuse_<name>.png` matches `usemtl <name>`.

/// Prefix of collision objects inside the OBJ.
pub const UCX_PREFIX: &str = "UCX_";

/// Folder / file-stem / material rule: lower case, `[a-z0-9_]`, runs of other characters (including every
/// non-ASCII character) collapse to one `_`, leading/trailing `_` trimmed, never empty (`"model"`).
pub fn sanitize_name(name: &str) -> String {
    let mut s = String::new();
    let mut last_us = false;
    for ch in name.trim().to_lowercase().chars() {
        let c = if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            ch
        } else {
            '_'
        };
        if c == '_' {
            if !last_us {
                s.push('_');
            }
            last_us = true;
        } else {
            s.push(c);
            last_us = false;
        }
    }
    let s = s.trim_matches('_').to_string();
    if s.is_empty() {
        "model".to_string()
    } else {
        s
    }
}

/// True when an object/node name denotes collision (`UCX_` prefix, any case, surrounding spaces ignored).
pub fn is_ucx_name(name: &str) -> bool {
    name.trim().to_ascii_uppercase().starts_with(UCX_PREFIX)
}

/// OBJ-safe ASCII object name for a collision object: other characters become `_` and the prefix is forced to
/// upper-case `UCX_` (the game and the Model Manager compare it case-sensitively, so `ucx_box` would not count as a hull).
/// Falls back to `UCX_collision`.
pub fn ucx_object_name(name: &str) -> String {
    let n = ucx_object_name_raw(name);
    if n.len() >= UCX_PREFIX.len() && n[..UCX_PREFIX.len()].eq_ignore_ascii_case(UCX_PREFIX) {
        format!("{UCX_PREFIX}{}", &n[UCX_PREFIX.len()..])
    } else {
        n
    }
}

fn ucx_object_name_raw(name: &str) -> String {
    let mut s = String::new();
    let mut last_us = false;
    for ch in name.trim().chars() {
        let c = if ch.is_ascii_alphanumeric() { ch } else { '_' };
        if c == '_' {
            if !last_us {
                s.push('_');
            }
            last_us = true;
        } else {
            s.push(c);
            last_us = false;
        }
    }
    let s = s.trim_matches('_').to_string();
    if s.is_empty() {
        "UCX_collision".to_string()
    } else {
        s
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextureKind {
    Diffuse,
    Pbr,
    Normal,
    Emissive,
}

impl TextureKind {
    pub const ALL: [TextureKind; 4] = [Self::Diffuse, Self::Pbr, Self::Normal, Self::Emissive];
    pub fn prefix(self) -> &'static str {
        match self {
            Self::Diffuse => "diffuse",
            Self::Pbr => "pbr",
            Self::Normal => "normal",
            Self::Emissive => "emissive",
        }
    }
}

/// `diffuse_<material>.png` and friends.
pub fn texture_file_name(kind: TextureKind, material: &str) -> String {
    format!("{}_{}.png", kind.prefix(), material)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names() {
        assert_eq!(sanitize_name("  My Cool-Model (v2)! "), "my_cool_model_v2");
        assert_eq!(sanitize_name("Модель"), "model");
        assert_eq!(sanitize_name("***"), "model");
        assert!(is_ucx_name(" ucx_Box_01"));
        assert!(!is_ucx_name("box"));
        assert_eq!(ucx_object_name("UCX_my box"), "UCX_my_box");
        assert_eq!(ucx_object_name("ucx_Box"), "UCX_Box");
        assert_eq!(ucx_object_name(" Ucx_a-b "), "UCX_a_b");
        assert_eq!(
            texture_file_name(TextureKind::Normal, "chair"),
            "normal_chair.png"
        );
    }
}
