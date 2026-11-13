//! Properties-only editor: change the `properties.cfg` values of a model that has no saved .glb (so the converter
//! cannot open it), without the converter, Blender or any other tool.
//!
//! The orange **Properties** button sits where **Edit** sits for an editable model (the preview's header). It opens the
//! editor screen with the usual header (icon, **Capture as icon**, the name with its rename pencil, the short list of
//! the properties in effect) and only the property sections (DURABILITY, MATERIAL, LIGHT, TEXTURE FILTERS); the
//! conversion sections and the logs are hidden (`conv_ui_system`). The preview keeps showing the model already loaded
//! for the list. The model's folder counts as "saved" (`ConvertState::saved`), which is all the icon box and Capture as
//! icon need; a rename goes through `listmenu::rename_model` (see `renamed`) instead of a conversion.
//!
//! Like the converter editor this saves by itself: a change waits for the same short delay as the converter's
//! (`SLIDER_DELAY` for sliders, none for toggles and choices), then `properties.cfg` is written, and the preview is
//! reloaded (texture filters are baked into the loaded images). The lamp marker follows the sliders at once
//! (`preview::props_live_cfg_system`).
//!
//! The file is written from the converter's own `PropertiesCfg` (the same type the editor uses), so keys this program
//! does not know are kept (`PropertiesCfg::extra`), but the first save normalises the file: key order, formatting and
//! comments are not preserved.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use bevy::window::RequestRedraw;
use votv_convert::model_core::PropertiesCfg;

use crate::converter::{ConvertState, Status};
use crate::Layout;

/// The model the Properties button would open: the selected one, when it has no saved .glb.
#[derive(Resource, Default)]
pub struct PropsTarget(pub Option<PathBuf>);

/// True when the folder has an emissive map (`emissive_*.png`), the case where `emissive_strength` is meaningful.
pub fn has_emissive_map(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok()).any(|e| {
                let n = e.file_name().to_string_lossy().to_lowercase();
                n.starts_with("emissive_") && n.ends_with(".png")
            })
        })
        .unwrap_or(false)
}

/// Validate and write `cfg` as `dir/properties.cfg` (through a temporary file, so a crash cannot leave half a file).
pub fn write_cfg(dir: &Path, cfg: &PropertiesCfg, with_emissive: bool) -> Result<(), String> {
    cfg.validate()?;
    let target = dir.join("properties.cfg");
    let tmp = dir.join("properties.cfg.tmp");
    std::fs::write(&tmp, cfg.to_cfg_string(with_emissive)).map_err(|e| format!("Could not write properties.cfg: {e}"))?;
    std::fs::rename(&tmp, &target).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("Could not replace properties.cfg: {e}")
    })
}

/// A rename in properties-only mode succeeded: the folder is now `new_dir`. Everything that holds the old path follows,
/// the preview reloads from the new folder, and the list is rescanned (selecting the new name) when the editor is left.
pub fn renamed(st: &mut ConvertState, lib: &mut crate::Library, new_dir: PathBuf, name: String) {
    if let Some(e) = lib.selected.and_then(|i| lib.entries.get_mut(i)) {
        e.dir = new_dir.clone();
        e.name = name.clone();
    }
    st.props = Some(new_dir.clone());
    st.input = Some(new_dir.clone());
    st.saved = Some(new_dir);
    st.opts.output_name = Some(name);
    st.saved_any = true;
    st.props_gen += 1;
}

/// Counts down the save delay in properties-only mode and saves.
pub fn props_system(
    mut st: ResMut<ConvertState>,
    layout: Res<Layout>,
    time: Res<Time<Real>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut redraw: EventWriter<RequestRedraw>,
) {
    let _bench = crate::benchmark::sys("sys.propsedit.props_system");
    if !layout.converter || st.props.is_none() {
        return;
    }
    // Safety net, as in the converter: a slider's release that was lost must not hold the save forever.
    if st.hold > 0 && !mouse.pressed(MouseButton::Left) {
        st.hold = 0;
        if st.pending.is_some() {
            st.pending = Some(crate::converter::SLIDER_DELAY);
        }
    }
    let Some(left) = st.pending else { return };
    redraw.write(RequestRedraw); // the window only redraws on demand; keep counting
    if st.hold > 0 {
        return;
    }
    let left = left - time.delta_secs();
    if left > 0.0 {
        st.pending = Some(left);
        return;
    }
    st.pending = None;
    let Some(dir) = st.props.clone() else { return };
    let _w = crate::benchmark::work("props.save");
    match write_cfg(&dir, &st.opts.properties, st.props_emissive) {
        Ok(()) => {
            st.status = Status::Done;
            st.props_gen += 1; // the preview reloads
        }
        Err(e) => {
            println!("[props] {e}");
            st.status = Status::Failed(e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("votv-props-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_saved_file_reads_back_the_same_and_keeps_unknown_keys() {
        let dir = temp("roundtrip");
        let mut cfg = PropertiesCfg::parse("physical_material=3\nfilter_diffuse_=1\nsome_new_key=hello\n");
        cfg.physical_material = 8;
        cfg.is_lamp = true;
        write_cfg(&dir, &cfg, false).unwrap();
        let back = PropertiesCfg::parse(&std::fs::read_to_string(dir.join("properties.cfg")).unwrap());
        assert_eq!(back.physical_material, 8);
        assert!(back.is_lamp);
        assert_eq!(back.filter_diffuse_.0, 1);
        assert_eq!(back.extra.get("some_new_key").map(|s| s.as_str()), Some("hello"));
        assert!(!dir.join("properties.cfg.tmp").exists(), "the temporary file is renamed away");
    }

    #[test]
    fn emissive_strength_is_written_only_when_asked() {
        let dir = temp("emissive");
        let cfg = PropertiesCfg::default();
        write_cfg(&dir, &cfg, false).unwrap();
        assert!(!std::fs::read_to_string(dir.join("properties.cfg")).unwrap().contains("emissive_strength"));
        write_cfg(&dir, &cfg, true).unwrap();
        assert!(std::fs::read_to_string(dir.join("properties.cfg")).unwrap().contains("emissive_strength"));
    }

    #[test]
    fn an_emissive_map_is_found_by_its_name() {
        let dir = temp("map");
        assert!(!has_emissive_map(&dir));
        std::fs::write(dir.join("Emissive_thing.PNG"), b"x").unwrap();
        assert!(has_emissive_map(&dir));
    }
}
