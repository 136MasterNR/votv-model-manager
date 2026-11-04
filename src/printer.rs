//! The size limits of the in-game 3D printers, and the check the game makes before it prints a model.
//!
//! Source: the reverse engineer's notes (the `votv-printer-thumbnails` skill, `references/print-size-check.md`).
//! In short, the game takes the mesh's bounding box (vertices only, after the importer's transform, WITHOUT the
//! `properties.cfg` scale) and compares its HALF extents with the printer's, axis by axis; one axis over the limit and the
//! model is refused ("Print error"). The printed mesh is then placed centred on the plate (XY) with its lowest point on it,
//! which is what the preview's printer volumes show.
//!
//! * large printer: half extents (100, 100, 150) cm, i.e. a 200 x 200 x 300 cm volume
//! * desktop printer: half extents (22.5, 22.5, 30) cm, i.e. a 45 x 45 x 60 cm volume
//!
//! Not verified (see the skill's open questions): whether the limit is inclusive (`<=` is assumed), and which OBJ axis the
//! game's importer takes as "up" (the app's Z-up option decides, as everywhere else in the preview). To calibrate, select
//! any model in the game: its hint prints "SMALL BOX", the half extents this module works with.

/// One printer: its name and the half extents of the box a model must fit in, in cm (x, y, z = up).
#[derive(Clone, Copy)]
pub struct Printer {
    pub name: &'static str,
    pub half: [f32; 3],
}

pub const LARGE: Printer = Printer {
    name: "Large printer",
    half: [100.0, 100.0, 150.0],
};
pub const DESKTOP: Printer = Printer {
    name: "Desktop printer",
    half: [22.5, 22.5, 30.0],
};
/// Largest first.
pub const PRINTERS: [Printer; 2] = [LARGE, DESKTOP];

/// Slack for float noise in the bounding box (cm): a model that is exactly at the limit still fits.
const EPS: f32 = 1.0e-3;

/// The model's half extents as the check sees them: (horizontal, horizontal, up). `z_up` says which OBJ axis is up
/// (the app's Z-up option): Z when it is on, Y when it is off.
pub fn model_half(half: [f32; 3], z_up: bool) -> [f32; 3] {
    if z_up {
        half
    } else {
        [half[0], half[2], half[1]]
    }
}

/// The result of checking one model against one printer.
pub struct Fit {
    pub fits: bool,
    /// The largest uniform scale (1.0 = the model as it is now) at which the model still fits: below 1.0 when it is
    /// too big, above 1.0 when there is room to spare.
    pub max_scale: f32,
}

pub fn check(model: [f32; 3], p: &Printer) -> Fit {
    let fits = model.iter().zip(p.half).all(|(m, l)| *m <= l + EPS);
    let max_scale = model
        .iter()
        .zip(p.half)
        .map(|(m, l)| l / m.max(1.0e-4))
        .fold(f32::MAX, f32::min);
    Fit { fits, max_scale }
}

/// "126 x 56 x 179": the full size (twice the half extents) of `half`, in cm, without a unit. Only ASCII: the built-in UI
/// font has no multiplication sign.
fn size_text(half: [f32; 3]) -> String {
    let n = |h: f32| {
        let v = h * 2.0;
        if (v - v.round()).abs() < 0.05 {
            format!("{:.0}", v.round())
        } else {
            format!("{v:.1}")
        }
    };
    format!("{} x {} x {}", n(half[0]), n(half[1]), n(half[2]))
}

/// How a line of the printer report should look.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Level {
    Info,
    Good,
    Bad,
}

/// The lines of the report shown in the preview, for a model with half extents `model` (see `model_half`).
/// `all`: the whole report (the printer volumes are switched on); otherwise only the warning for a model that is too
/// big for every printer, so a hopeless model is flagged even with the volumes off.
pub fn report(model: [f32; 3], all: bool) -> Vec<(String, Level)> {
    let too_big = !check(model, &LARGE).fits;
    let mut out = Vec::new();
    if !all && !too_big {
        return out;
    }
    out.push((format!("Model: {} cm", size_text(model)), Level::Info));
    if all {
        for p in PRINTERS {
            let fit = check(model, &p);
            let head = format!("{} ({} cm)", p.name, size_text(p.half));
            out.push(if fit.fits {
                (format!("{head}: fits"), Level::Good)
            } else {
                // Rounded down, so the suggested percentage really fits.
                let pct = (fit.max_scale * 100.0).floor().max(1.0);
                (
                    format!("{head}: too big, shrink to {pct:.0}% or less"),
                    Level::Bad,
                )
            });
        }
    }
    if too_big {
        out.push((
            "WARNING: too big for every printer, the game will not print it".to_string(),
            Level::Bad,
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_limits_are_the_documented_ones() {
        assert_eq!(size_text(LARGE.half), "200 x 200 x 300");
        assert_eq!(size_text(DESKTOP.half), "45 x 45 x 60");
    }

    #[test]
    fn the_up_axis_follows_the_z_up_option() {
        assert_eq!(model_half([1.0, 2.0, 3.0], true), [1.0, 2.0, 3.0]);
        assert_eq!(model_half([1.0, 2.0, 3.0], false), [1.0, 3.0, 2.0]);
    }

    #[test]
    fn fit_is_checked_per_axis_and_inclusive() {
        // Exactly at the desktop limit fits; a hair over on ONE axis does not.
        assert!(check([22.5, 22.5, 30.0], &DESKTOP).fits);
        assert!(!check([22.6, 10.0, 10.0], &DESKTOP).fits);
        assert!(!check([10.0, 10.0, 30.5], &DESKTOP).fits);
        // Tall and thin: fits the large printer, not the desktop one.
        assert!(check([10.0, 10.0, 140.0], &LARGE).fits);
        assert!(!check([10.0, 10.0, 140.0], &DESKTOP).fits);
    }

    #[test]
    fn max_scale_is_the_tightest_axis() {
        // Twice the desktop printer's height: it must shrink to half.
        let f = check([10.0, 10.0, 60.0], &DESKTOP);
        assert!(!f.fits);
        assert!((f.max_scale - 0.5).abs() < 1.0e-6);
        // Half the limit: there is room to double it.
        let f = check([50.0, 50.0, 75.0], &LARGE);
        assert!(f.fits && (f.max_scale - 2.0).abs() < 1.0e-6);
    }

    #[test]
    fn the_report_warns_only_when_no_printer_can_take_the_model() {
        // Small model, volumes off: nothing to say.
        assert!(report([10.0, 10.0, 10.0], false).is_empty());
        // Fits the large printer only: no warning, the desktop line says so.
        let r = report([30.0, 30.0, 50.0], true);
        assert_eq!(r.len(), 3);
        assert_eq!(r[1].1, Level::Good);
        assert_eq!(r[2].1, Level::Bad);
        assert!(r.iter().all(|(t, _)| !t.contains("WARNING")));
        // Too big for everything: the warning shows even with the volumes off.
        let r = report([150.0, 10.0, 10.0], false);
        assert_eq!(r.len(), 2);
        assert!(r[1].0.starts_with("WARNING"));
        let r = report([150.0, 10.0, 10.0], true);
        assert_eq!(r.len(), 4);
        assert!(r[3].0.starts_with("WARNING"));
        // The suggested percentage really fits (rounded down).
        assert!(r[1].0.contains("shrink to 66%"));
    }
}
