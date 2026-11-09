//! Model files on disk: OBJ + MTL, PNG maps, properties.cfg. No rendering types here, so all of this
//! can run on a worker thread. How the game reads these is documented in `votv-printer-internals`.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use bevy::color::Color;
use bevy::math::Vec3;

pub struct RawSection {
    pub name: String,
    pub mat_name: String,
    pub kd: Option<String>,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    /// `indices` without the triangles dropped by the duplicate rule (None when nothing was dropped).
    pub dedup: Option<Vec<u32>>,
}

/// One `UCX_` object: the game turns each into a convex hull.
pub struct Hull {
    pub name: String,
    pub points: Vec<[f32; 3]>,
    pub edges: Vec<([f32; 3], [f32; 3])>,
}

pub struct ModelData {
    pub name: String,
    pub sections: Vec<RawSection>,
    pub hulls: Vec<Hull>,
    pub cfg: HashMap<String, String>,
    pub files: HashMap<String, PathBuf>, // lowercase file name -> path
    pub center: [f32; 3],
    pub radius: f32,
    /// Half the size of the visible geometry's bounding box along each OBJ axis, in cm (the printer check uses it).
    pub half: [f32; 3],
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Diffuse,
    Pbr,
    Normal,
    Emissive,
}
pub const KINDS: [Kind; 4] = [Kind::Diffuse, Kind::Pbr, Kind::Normal, Kind::Emissive];

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Diffuse => "diffuse",
            Kind::Pbr => "pbr",
            Kind::Normal => "normal",
            Kind::Emissive => "emissive",
        }
    }
    pub fn idx(self) -> usize {
        match self {
            Kind::Diffuse => 0,
            Kind::Pbr => 1,
            Kind::Normal => 2,
            Kind::Emissive => 3,
        }
    }
}

pub const PHYS_MATERIALS: [&str; 16] = [
    "plastic",
    "wood",
    "metal",
    "concrete",
    "rubber",
    "bag",
    "metal_hollow",
    "flesh",
    "food",
    "device",
    "dirt",
    "glass",
    "paper",
    "vehicle",
    "wheel",
    "ball",
];

// ---------------------------------------------------------------------------------------------
// Folder scanning
// ---------------------------------------------------------------------------------------------

pub struct Entry {
    pub name: String,
    pub dir: PathBuf,
    /// When the model folder was created (what "last added" sorts by).
    pub added: std::time::SystemTime,
    /// The newest change of any file in the folder except the icon (what "last modified" sorts by).
    pub modified: std::time::SystemTime,
    /// The `.obj` was renamed to `.obj.disabled` (the game skips it). Such models are listed in their own section.
    pub disabled: bool,
    /// The folder holds a saved `.glb`, so the converter can reopen it (Edit).
    pub editable: bool,
}

impl Entry {
    /// List section: 0 = EDITABLE, 1 = MODELS. (A disabled model stays in its section and is greyed out.)
    pub fn section(&self) -> u8 {
        if self.editable {
            0
        } else {
            1
        }
    }
}

/// How a section of the model list is ordered. A click on the sort button in the section header
/// (`ui::spawn_section`) switches between the section's date order and the alphabetic one: EDITABLE has Last Modified and
/// Alphabetic, MODELS has Last Added and Alphabetic.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SortMode {
    LastAdded,
    LastModified,
    NameAz,
}

impl SortMode {
    /// The mode a click switches to in section `rank` (0 = EDITABLE, 1 = MODELS).
    pub fn next(self, rank: u8) -> SortMode {
        match self {
            SortMode::NameAz if rank == 0 => SortMode::LastModified,
            SortMode::NameAz => SortMode::LastAdded,
            _ => SortMode::NameAz,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SortMode::LastAdded => "Last Added",
            SortMode::LastModified => "Last Modified",
            SortMode::NameAz => "Alphabetic",
        }
    }
}

/// Orders `entries` by list section, then inside each section by that section's mode (newest first for the dates).
///
/// The key (section, the section's date newest first, lowercase name) is computed once per entry by
/// `sort_by_cached_key`; the comparator it replaces built two lowercase `String`s on every comparison. The order is the
/// same, and the sort is stable, so entries with equal keys keep their input order as before. (`str::to_lowercase` is
/// kept on purpose: unlike a per-character lowercase it treats a final capital sigma the way the old comparator did.)
pub fn sort_entries(entries: &mut [Entry], modes: [SortMode; 2]) {
    entries.sort_by_cached_key(|e| {
        let when = match modes[e.section() as usize] {
            SortMode::LastAdded => e.added,
            SortMode::LastModified => e.modified,
            // The same for every entry of the section, so only the name decides.
            SortMode::NameAz => std::time::UNIX_EPOCH,
        };
        (e.section(), std::cmp::Reverse(when), e.name.to_lowercase())
    });
}

/// Name of the file a model is disabled with: `<x>.obj` -> `<x>.obj.disabled`.
pub const DISABLED_SUFFIX: &str = ".disabled";

/// `name` ends with `suffix`, ignoring ASCII case, without allocating (the old check lowercased a `String` per file).
/// Same answer as `name.to_string_lossy().to_lowercase().ends_with(suffix)` for the ASCII suffixes used here (`.glb`,
/// `.obj.disabled`): no non-ASCII character lowercases to one of their letters, and a lossy replacement character can
/// never be part of an ASCII suffix.
fn name_ends_with(name: &std::ffi::OsStr, suffix: &str) -> bool {
    let (n, s) = (name.as_encoded_bytes(), suffix.as_bytes());
    n.len() >= s.len() && n[n.len() - s.len()..].eq_ignore_ascii_case(s)
}

/// What one pass over a model folder found: the three flags `scan_models` needs and the newest change of any file in
/// it, not counting `icon.png` (the app rewrites that on its own). `newest` stays `UNIX_EPOCH` when no file had a time.
struct FolderScan {
    has_obj: bool,
    has_obj_disabled: bool,
    has_glb: bool,
    newest: std::time::SystemTime,
}

/// One `read_dir` of a model folder instead of three or four. `None` when the folder cannot be read: the old code then
/// found neither an `.obj` nor an `.obj.disabled` and skipped the folder, and so does the caller.
fn scan_folder(dir: &Path) -> Option<FolderScan> {
    let rd = fs::read_dir(dir).ok()?;
    let mut s = FolderScan {
        has_obj: false,
        has_obj_disabled: false,
        has_glb: false,
        newest: std::time::UNIX_EPOCH,
    };
    for f in rd.flatten() {
        let name = f.file_name();
        // The old test, on the same last path component: extension "obj" in any case. A directory called `x.obj`
        // counts too, and so does nothing without an extension (a file named just `.obj`).
        if !s.has_obj
            && Path::new(&name)
                .extension()
                .and_then(|x| x.to_str())
                .is_some_and(|x| x.eq_ignore_ascii_case("obj"))
        {
            s.has_obj = true;
        }
        if !s.has_obj_disabled && name_ends_with(&name, ".obj.disabled") {
            s.has_obj_disabled = true;
        }
        if !s.has_glb && name_ends_with(&name, ".glb") {
            s.has_glb = true;
        }
        if !name.eq_ignore_ascii_case("icon.png") {
            if let Some(t) = f.metadata().ok().and_then(|m| m.modified().ok()) {
                s.newest = s.newest.max(t);
            }
        }
    }
    Some(s)
}

/// Every sub-folder of `root` that contains an .obj file. Fast: no model is parsed.
pub fn scan_models(root: &Path) -> Vec<Entry> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(root) else {
        return out;
    };
    for e in rd.flatten() {
        // The folder test comes from the directory listing (no extra syscall). A symlink, a junction or an unknown
        // type asks the filesystem like the old `path().is_dir()` did, which follows links.
        let is_dir = match e.file_type() {
            Ok(t) if t.is_dir() => true,
            Ok(t) if t.is_file() => false,
            _ => e.path().is_dir(),
        };
        if !is_dir {
            continue;
        }
        let name = e.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') {
            continue; // hidden folders: the converter's temp folders
        }
        let p = e.path();
        let Some(scan) = scan_folder(&p) else {
            continue;
        };
        // An .obj makes a model; without one an `.obj.disabled` makes a disabled model; a `.glb` makes it editable.
        if !scan.has_obj && !scan.has_obj_disabled {
            continue;
        }
        // (added, modified): the folder's creation time, and the newest file change in it (`added` if no file had one).
        let added = fs::metadata(&p)
            .ok()
            .and_then(|m| m.created().ok().or_else(|| m.modified().ok()))
            .unwrap_or(std::time::UNIX_EPOCH);
        let modified = if scan.newest == std::time::UNIX_EPOCH {
            added
        } else {
            scan.newest
        };
        out.push(Entry {
            name: name.into_owned(),
            dir: p,
            added,
            modified,
            disabled: !scan.has_obj,
            editable: scan.has_glb,
        });
    }
    // Sorted by list section (editable, models), then by name; the key is computed once per entry.
    out.sort_by_cached_key(|e| (e.section(), e.name.to_lowercase()));
    out
}

// ---- reference implementation and equivalence tests (test builds only) ----

/// The pre-H1 `entry_times`, verbatim: part of the reference below, and no longer used by the program.
#[cfg(test)]
fn entry_times(dir: &Path) -> (std::time::SystemTime, std::time::SystemTime) {
    use std::time::UNIX_EPOCH;
    let meta = fs::metadata(dir).ok();
    let added = meta
        .as_ref()
        .and_then(|m| m.created().ok().or_else(|| m.modified().ok()))
        .unwrap_or(UNIX_EPOCH);
    let mut modified = UNIX_EPOCH;
    if let Ok(rd) = fs::read_dir(dir) {
        for f in rd.flatten() {
            if f.file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case("icon.png")
            {
                continue;
            }
            if let Some(t) = f.metadata().ok().and_then(|m| m.modified().ok()) {
                modified = modified.max(t);
            }
        }
    }
    if modified == UNIX_EPOCH {
        modified = added;
    }
    (added, modified)
}

/// The pre-H1 `has_file_ending`, verbatim: part of the reference below, and no longer used by the program.
#[cfg(test)]
fn has_file_ending(dir: &Path, ends: &str) -> bool {
    fs::read_dir(dir)
        .map(|d| {
            d.flatten().any(|f| {
                f.file_name()
                    .to_string_lossy()
                    .to_lowercase()
                    .ends_with(ends)
            })
        })
        .unwrap_or(false)
}

/// The pre-H1 `scan_models`, verbatim apart from its name: the reference of `scan_equivalence_tests`.
#[cfg(test)]
fn scan_models_reference(root: &Path) -> Vec<Entry> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(root) else {
        return out;
    };
    for e in rd.flatten() {
        let p = e.path();
        if !p.is_dir() {
            continue;
        }
        let has_obj = fs::read_dir(&p)
            .map(|d| {
                d.flatten().any(|f| {
                    f.path()
                        .extension()
                        .and_then(|x| x.to_str())
                        .map(|x| x.eq_ignore_ascii_case("obj"))
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false);
        let name = p
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        if name.starts_with('.') {
            continue; // hidden folders: the converter's temp folders
        }
        if has_obj {
            let editable = has_file_ending(&p, ".glb");
            let (added, modified) = entry_times(&p);
            out.push(Entry {
                name,
                dir: p,
                added,
                modified,
                disabled: false,
                editable,
            });
        } else if has_file_ending(&p, ".obj.disabled") {
            let editable = has_file_ending(&p, ".glb");
            let (added, modified) = entry_times(&p);
            out.push(Entry {
                name,
                dir: p,
                added,
                modified,
                disabled: true,
                editable,
            });
        }
    }
    // Sorted by list section (editable, models), then by name.
    out.sort_by_key(|e| (e.section(), e.name.to_lowercase()));
    out
}

#[cfg(test)]
mod scan_equivalence_tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    fn temp(name: &str) -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!(
            "votv_scan_{}_{}_{}",
            name,
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn touch(root: &Path, rel: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, b"x").unwrap();
    }

    /// Everything a scan returns, as comparable values (`Entry` itself has no `PartialEq`).
    type Row = (String, PathBuf, bool, bool, std::time::SystemTime, std::time::SystemTime);

    fn rows(v: &[Entry]) -> Vec<Row> {
        v.iter()
            .map(|e| (e.name.clone(), e.dir.clone(), e.disabled, e.editable, e.added, e.modified))
            .collect()
    }

    fn assert_same(root: &Path) {
        assert_eq!(rows(&scan_models(root)), rows(&scan_models_reference(root)));
    }

    #[test]
    fn a_mixed_tree_gives_the_same_entries_as_the_reference() {
        let root = temp("mixed");
        for f in [
            "plain/a.obj",
            "plain/a.mtl",
            "upper/A.OBJ",
            "mixed/x.Obj",
            "disabled/x.obj.disabled",
            "disabled_upper/X.OBJ.DISABLED",
            "both/a.obj",
            "both/a.obj.disabled",
            "glb/a.obj",
            "glb/a.GLB",
            "glb_only/a.glb",
            "disabled_glb/a.obj.disabled",
            "disabled_glb/b.Glb",
            "dotobj/.obj",
            "iconly/icon.png",
            ".hidden/a.obj",
            "loose.obj",
        ] {
            touch(&root, f);
        }
        fs::create_dir_all(root.join("empty")).unwrap();
        // A DIRECTORY called thing.obj counts as an .obj, as it always did.
        fs::create_dir_all(root.join("dirobj/thing.obj")).unwrap();
        assert_same(&root);
        // And the result is what the rules say: editable models first, then by lowercase name.
        let found: Vec<(String, bool, bool)> = scan_models(&root)
            .iter()
            .map(|e| (e.name.clone(), e.disabled, e.editable))
            .collect();
        let want = [
            ("disabled_glb", true, true),
            ("glb", false, true),
            ("both", false, false),
            ("dirobj", false, false),
            ("disabled", true, false),
            ("disabled_upper", true, false),
            ("mixed", false, false),
            ("plain", false, false),
            ("upper", false, false),
        ];
        let want: Vec<(String, bool, bool)> =
            want.iter().map(|(n, d, e)| (n.to_string(), *d, *e)).collect();
        assert_eq!(found, want);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn the_modified_time_ignores_icon_png() {
        let root = temp("modified");
        for f in ["m/a.obj", "m/b.mtl", "m/icon.png"] {
            touch(&root, f);
        }
        let set = |rel: &str, secs: u64| -> bool {
            fs::OpenOptions::new()
                .write(true)
                .open(root.join(rel))
                .and_then(|f| f.set_modified(UNIX_EPOCH + Duration::from_secs(secs)))
                .is_ok()
        };
        let ok = set("m/a.obj", 1_000) && set("m/b.mtl", 2_000) && set("m/icon.png", 9_000_000_000);
        assert_same(&root);
        if ok {
            let e = scan_models(&root);
            assert_eq!(e.len(), 1);
            // The newest file that is not the icon, not the icon's far-future time.
            assert_eq!(e[0].modified, UNIX_EPOCH + Duration::from_secs(2_000));
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_root_and_a_root_without_models_give_nothing() {
        let root = temp("none");
        assert!(scan_models(&root.join("does_not_exist")).is_empty());
        assert_same(&root.join("does_not_exist"));
        touch(&root, "only_a_file.txt");
        fs::create_dir_all(root.join("folder_without_models")).unwrap();
        assert!(scan_models(&root).is_empty());
        assert_same(&root);
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_model_folder_is_followed_and_a_dangling_link_is_skipped() {
        let root = temp("links");
        let real = temp("links_real");
        touch(&real, "m/a.obj");
        std::os::unix::fs::symlink(real.join("m"), root.join("linked")).unwrap();
        std::os::unix::fs::symlink(real.join("missing"), root.join("dangling")).unwrap();
        assert_same(&root);
        let names: Vec<String> = scan_models(&root).iter().map(|e| e.name.clone()).collect();
        assert_eq!(names, ["linked"]);
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&real);
    }

    #[cfg(unix)]
    #[test]
    fn a_file_name_that_is_not_utf8_is_handled_like_before() {
        use std::os::unix::ffi::OsStrExt;
        let root = temp("utf8");
        let m = root.join("m");
        fs::create_dir_all(&m).unwrap();
        fs::write(m.join(std::ffi::OsStr::from_bytes(b"a.o\xffj")), b"x").unwrap();
        fs::write(m.join("b.obj.DISABLED"), b"x").unwrap();
        assert_same(&root);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn name_ends_with_ignores_ascii_case_and_short_names() {
        let o = std::ffi::OsStr::new;
        assert!(name_ends_with(o("a.GLB"), ".glb"));
        assert!(name_ends_with(o(".glb"), ".glb"));
        assert!(!name_ends_with(o("glb"), ".glb"));
        assert!(!name_ends_with(o("a.glb.bak"), ".glb"));
        assert!(name_ends_with(o("x.OBJ.Disabled"), ".obj.disabled"));
    }

    // ---- sort_entries against the comparator it replaced ----

    fn at(secs: u64) -> std::time::SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    /// Entries with ties on purpose: names that differ only in case, a final capital sigma, equal times.
    fn sample() -> Vec<Entry> {
        [
            "beta",
            "Alpha",
            "alpha",
            "Zed",
            "\u{c4}PFEL",
            "\u{e4}pfel",
            // a word ending in a capital sigma next to its lowercase spelling (final sigma): the lowercase of the
            // whole string differs from the per-character lowercase, which is why `to_lowercase` stays
            "\u{39f}\u{394}\u{3a5}\u{3a3}\u{3a3}\u{395}\u{3a5}\u{3a3}",
            "\u{39f}\u{3b4}\u{3c5}\u{3c3}\u{3c3}\u{3b5}\u{3c5}\u{3c2}",
            "\u{391}\u{3a3}",
            "\u{3b1}\u{3c2}",
            "x1",
            "X1",
            "mm",
        ]
        .iter()
        .enumerate()
        .map(|(i, n)| Entry {
            name: n.to_string(),
            dir: PathBuf::from(format!("{i:02}")),
            added: at((i * 7 % 5) as u64),
            modified: at((i * 3 % 4) as u64),
            disabled: i % 4 == 0,
            editable: i % 3 == 0,
        })
        .collect()
    }

    /// The comparator `sort_entries` had before H1, verbatim.
    fn old_sort_entries(entries: &mut [Entry], modes: [SortMode; 2]) {
        entries.sort_by(|a, b| {
            let by_name = || a.name.to_lowercase().cmp(&b.name.to_lowercase());
            a.section()
                .cmp(&b.section())
                .then_with(|| match modes[a.section() as usize] {
                    SortMode::LastAdded => b.added.cmp(&a.added).then_with(by_name),
                    SortMode::LastModified => b.modified.cmp(&a.modified).then_with(by_name),
                    SortMode::NameAz => by_name(),
                })
        });
    }

    #[test]
    fn sort_entries_orders_like_the_comparator_it_replaced() {
        use super::SortMode::{LastAdded, LastModified, NameAz};
        let all = [LastAdded, LastModified, NameAz];
        for m0 in all {
            for m1 in all {
                let modes = [m0, m1];
                let mut new = sample();
                sort_entries(&mut new, modes);
                let mut old = sample();
                old_sort_entries(&mut old, modes);
                let order = |v: &[Entry]| v.iter().map(|e| e.dir.clone()).collect::<Vec<_>>();
                assert_eq!(order(&new), order(&old), "modes {modes:?}");
            }
        }
    }
}

/// True when `scan_models(root)` would find at least one model: a sub-folder of `root` (symlinks followed, as
/// `Path::is_dir` does) whose name does not start with a dot and that holds an entry with the extension `obj` (any case)
/// or an entry whose name ends with `.obj.disabled` (any case). It stops at the first one, so the start-up check of the
/// candidate folders costs one folder listing instead of a scan of the whole library. It deliberately repeats the tests
/// of `scan_models` instead of calling its helpers (another change may remove or rename them); the unit tests below keep
/// the two in step.
pub fn has_any_model(root: &Path) -> bool {
    let Ok(rd) = fs::read_dir(root) else {
        return false;
    };
    rd.flatten().any(|e| {
        let p = e.path();
        if !p.is_dir() || e.file_name().to_string_lossy().starts_with('.') {
            return false; // a loose file, or a hidden folder (the converter's temp folders)
        }
        let Ok(inner) = fs::read_dir(&p) else {
            return false;
        };
        inner.flatten().any(|f| {
            let is_obj = f
                .path()
                .extension()
                .and_then(|x| x.to_str())
                .is_some_and(|x| x.eq_ignore_ascii_case("obj"));
            is_obj
                || f.file_name()
                    .to_string_lossy()
                    .to_lowercase()
                    .ends_with(".obj.disabled")
        })
    })
}

#[cfg(test)]
mod has_any_model_tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A fresh temp folder that is removed again when it goes out of scope.
    struct Tree(PathBuf);

    impl Tree {
        fn new() -> Tree {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let dir = std::env::temp_dir().join(format!(
                "votv-has-any-model-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::SeqCst)
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Tree(dir)
        }

        /// Creates a file (and the folders above it).
        fn file(&self, rel: &str) -> &Tree {
            let p = self.0.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, b"x").unwrap();
            self
        }

        fn dir(&self, rel: &str) -> &Tree {
            fs::create_dir_all(self.0.join(rel)).unwrap();
            self
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// The contract: `has_any_model` says exactly what `scan_models` finds, and `expected` is what both must say.
    /// (`scan_models` is compared as it is when the test runs; its body may be rewritten, its output may not change.)
    fn check(root: &Path, expected: bool) {
        let scanned = !scan_models(root).is_empty();
        assert_eq!(scanned, expected, "scan_models gives a different answer for {}", root.display());
        assert_eq!(
            has_any_model(root),
            scanned,
            "has_any_model disagrees with scan_models for {}",
            root.display()
        );
    }

    #[test]
    fn an_empty_root_has_no_model() {
        let t = Tree::new();
        check(&t.0, false);
    }

    #[test]
    fn a_missing_root_has_no_model() {
        let t = Tree::new();
        check(&t.0.join("missing"), false);
    }

    #[test]
    fn a_file_instead_of_a_folder_has_no_model() {
        let t = Tree::new();
        t.file("not_a_folder.txt");
        check(&t.0.join("not_a_folder.txt"), false);
    }

    #[test]
    fn loose_files_in_the_root_do_not_count() {
        let t = Tree::new();
        t.file("loose.obj").file("readme.txt");
        check(&t.0, false);
    }

    #[test]
    fn a_hidden_folder_does_not_count() {
        let t = Tree::new();
        t.file(".converter_tmp/a.obj");
        check(&t.0, false);
    }

    #[test]
    fn an_obj_in_any_case_counts() {
        let t = Tree::new();
        t.file("chair/chair.OBJ");
        check(&t.0, true);
    }

    #[test]
    fn a_disabled_obj_in_any_case_counts() {
        let t = Tree::new();
        t.file("lamp/lamp.OBJ.Disabled");
        check(&t.0, true);
    }

    #[test]
    fn a_glb_an_icon_or_an_empty_folder_alone_do_not_count() {
        let t = Tree::new();
        t.file("a/m.glb").file("b/icon.png").dir("c");
        check(&t.0, false);
    }

    #[test]
    fn a_folder_called_x_obj_counts_like_a_file_called_x_obj() {
        // `scan_models` tests the extension of every entry, folders included; the quirk is kept on purpose.
        let t = Tree::new();
        t.dir("e/inner.obj");
        check(&t.0, true);
    }

    #[test]
    fn a_file_called_dot_obj_has_no_extension() {
        let t = Tree::new();
        t.file("f/.obj");
        check(&t.0, false);
    }

    #[test]
    fn a_model_after_folders_without_one_is_found() {
        let t = Tree::new();
        t.dir("a").file("b/readme.txt").file("z/z.obj");
        check(&t.0, true);
    }
}

// ---------------------------------------------------------------------------------------------
// Loading one model
// ---------------------------------------------------------------------------------------------

pub fn load_model(dir: &Path) -> Result<ModelData, String> {
    let dir = dir
        .canonicalize()
        .map_err(|e| format!("cannot open {}: {e}", dir.display()))?;
    let name = dir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("model")
        .to_string();

    let mut files = HashMap::new();
    for entry in fs::read_dir(&dir).map_err(|e| format!("cannot read folder: {e}"))? {
        let p = entry.map_err(|e| e.to_string())?.path();
        if let Some(n) = p.file_name().and_then(|s| s.to_str()) {
            files.insert(n.to_lowercase(), p.clone());
        }
    }
    let obj = files
        .get(&format!("{}.obj", name.to_lowercase()))
        .or_else(|| files.get(&format!("{}.obj.disabled", name.to_lowercase())))
        .cloned()
        .or_else(|| {
            files
                .iter()
                .filter(|(k, _)| k.ends_with(".obj"))
                .map(|(_, v)| v.clone())
                .min()
        })
        .or_else(|| {
            files
                .iter()
                .filter(|(k, _)| k.ends_with(".obj.disabled"))
                .map(|(_, v)| v.clone())
                .min()
        })
        .ok_or("no .obj file in the folder")?;

    let (models, mats) = load_obj_robust(&obj, &files)?;

    let mut sections = Vec::new();
    let mut hulls = Vec::new();
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);

    for m in models {
        let mesh = m.mesh;
        let pos: Vec<[f32; 3]> = mesh
            .positions
            .chunks_exact(3)
            .map(|c| [c[0], c[1], c[2]])
            .collect();
        // The game removes `UCX_` objects from the render list and keeps their points as hulls.
        if m.name.starts_with("UCX_") {
            let mut edges = Vec::new();
            for t in mesh.indices.chunks_exact(3) {
                for k in 0..3 {
                    edges.push((pos[t[k] as usize], pos[t[(k + 1) % 3] as usize]));
                }
            }
            hulls.push(Hull {
                name: m.name,
                points: pos,
                edges,
            });
            continue;
        }
        for p in &pos {
            for a in 0..3 {
                lo[a] = lo[a].min(p[a]);
                hi[a] = hi[a].max(p[a]);
            }
        }
        let uvs: Vec<[f32; 2]> = if mesh.texcoords.len() == pos.len() * 2 {
            mesh.texcoords
                .chunks_exact(2)
                .map(|c| [c[0], c[1]])
                .collect()
        } else {
            vec![[0.0, 0.0]; pos.len()]
        };
        let (positions, normals, uvs, indices) = if mesh.normals.len() == mesh.positions.len() {
            let n = mesh
                .normals
                .chunks_exact(3)
                .map(|c| [c[0], c[1], c[2]])
                .collect();
            (pos, n, uvs, mesh.indices)
        } else {
            flatten(&pos, &uvs, &mesh.indices) // no normals in the file: flat shading
        };
        let mat = mesh.material_id.and_then(|i| mats.get(i));
        sections.push(RawSection {
            name: m.name,
            mat_name: mat.map(|x| x.name.clone()).unwrap_or_default(),
            kd: mat.and_then(|x| x.diffuse_texture.clone()),
            positions,
            normals,
            uvs,
            indices,
            dedup: None,
        });
    }
    if sections.is_empty() {
        return Err("the OBJ has no renderable (non-UCX_) geometry".into());
    }

    let center = [
        (lo[0] + hi[0]) / 2.0,
        (lo[1] + hi[1]) / 2.0,
        (lo[2] + hi[2]) / 2.0,
    ];
    let radius = Vec3::from(hi).distance(Vec3::from(lo)) / 2.0;
    let half = [
        (hi[0] - lo[0]) / 2.0,
        (hi[1] - lo[1]) / 2.0,
        (hi[2] - lo[2]) / 2.0,
    ];

    let mut cfg = HashMap::new();
    if let Some(p) = files.get("properties.cfg") {
        if let Ok(text) = fs::read_to_string(p) {
            for line in text.lines() {
                if let Some((k, v)) = line.split_once('=') {
                    cfg.insert(k.trim().to_string(), v.trim().to_string());
                }
            }
        }
    }
    Ok(ModelData {
        name,
        sections,
        hulls,
        cfg,
        files,
        center,
        radius: radius.max(0.001),
        half,
    })
}

// ---------------------------------------------------------------------------------------------
// Tolerant OBJ/MTL reading
// ---------------------------------------------------------------------------------------------

fn read_text(p: &Path) -> Result<String, String> {
    let bytes = fs::read(p).map_err(|e| format!("cannot read {}: {e}", p.display()))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Rebuild an MTL that tobj cannot reject: keep only `newmtl` and `map_Kd` (all the manager uses),
/// and make sure every material the OBJ asks for with `usemtl` exists. Odd values in Ka/Kd/Ks/Ns/d/illum
/// lines, or a `usemtl` with no matching `newmtl`, are what usually cause "material parse error".
pub fn clean_mtl(obj_text: &str, mtl_texts: &[String]) -> String {
    let mut order: Vec<String> = Vec::new();
    let mut kd: HashMap<String, String> = HashMap::new();
    let mut cur: Option<String> = None;
    for text in mtl_texts {
        for line in text.lines() {
            let l = line.trim();
            let (key, rest) = l.split_once(char::is_whitespace).unwrap_or((l, ""));
            if key.eq_ignore_ascii_case("newmtl") {
                let name = rest.trim().to_string();
                if !name.is_empty() {
                    if !order.contains(&name) {
                        order.push(name.clone());
                    }
                    cur = Some(name);
                }
            } else if key.eq_ignore_ascii_case("map_Kd") {
                if let (Some(c), Some(p)) = (&cur, rest.split_whitespace().last()) {
                    kd.insert(c.clone(), p.to_string());
                }
            }
        }
    }
    for line in obj_text.lines() {
        if let Some(rest) = line.trim().strip_prefix("usemtl") {
            let name = rest.trim().to_string();
            if !name.is_empty() && !order.contains(&name) {
                order.push(name);
            }
        }
    }
    let mut out = String::new();
    for n in &order {
        out.push_str(&format!("newmtl {n}\n"));
        if let Some(p) = kd.get(n) {
            out.push_str(&format!("map_Kd {p}\n"));
        }
    }
    out
}

fn load_obj_robust(
    obj: &Path,
    files: &HashMap<String, PathBuf>,
) -> Result<(Vec<tobj::Model>, Vec<tobj::Material>), String> {
    let raw = read_text(obj)?;
    // A `usemtl` line with no name makes tobj fail with "material parse error". Some exporters write one for
    // faces without a material. Give those faces a named placeholder material instead.
    let mut text = String::with_capacity(raw.len() + 64);
    for line in raw.lines() {
        if line.trim() == "usemtl" {
            text.push_str("usemtl __none__\n");
        } else {
            text.push_str(line);
            text.push('\n');
        }
    }

    let mut mtl_texts = Vec::new();
    let mut has_lib = false;
    for line in text.lines() {
        if let Some(rest) = line.trim().strip_prefix("mtllib") {
            has_lib = true;
            if let Some(p) = files.get(&rest.trim().to_lowercase()) {
                if let Ok(t) = read_text(p) {
                    mtl_texts.push(t);
                }
            }
        }
    }
    if mtl_texts.is_empty() {
        // No (readable) mtllib: fall back to a .mtl with the same name as the OBJ.
        let stem = obj
            .file_stem()
            .and_then(|x| x.to_str())
            .unwrap_or("")
            .to_lowercase();
        let stem = stem.strip_suffix(".obj").unwrap_or(&stem).to_string(); // `x.obj.disabled` -> `x`
        if let Some(t) = files
            .get(&format!("{stem}.mtl"))
            .and_then(|p| read_text(p).ok())
        {
            mtl_texts.push(t);
        }
    }
    let mtl = clean_mtl(&text, &mtl_texts);
    if !has_lib {
        text.insert_str(0, "mtllib __auto.mtl\n");
    }

    // Geometry: our own parser first (see `parse_obj_geometry`). A model exported with separate v / vt / vn index lists
    // (Blender's usual output, e.g. Miku) came out of tobj without UVs and normals in the preview, which showed one texel
    // with flat per-triangle lighting (the cyan shimmer). tobj stays as the fallback for anything the parser refuses.
    if let Ok((own_mats, _)) = tobj::load_mtl_buf(&mut std::io::Cursor::new(mtl.clone().into_bytes())) {
        match parse_obj_geometry(&text, &own_mats) {
            Ok(models) => return Ok((models, own_mats)),
            Err(e) => eprintln!("[model] own OBJ parser refused the file ({e}); using tobj"),
        }
    }

    // tobj calls the loader once per `mtllib` line; give it the cleaned MTL once and nothing after.
    let first = std::cell::Cell::new(true);
    let opts = tobj::LoadOptions {
        triangulate: true,
        single_index: true,
        ..Default::default()
    };
    let (models, mats) =
        tobj::load_obj_buf(&mut std::io::Cursor::new(text.into_bytes()), &opts, |_| {
            let body = if first.replace(false) {
                mtl.clone()
            } else {
                String::new()
            };
            tobj::load_mtl_buf(&mut std::io::Cursor::new(body.into_bytes()))
        })
        .map_err(|e| format!("OBJ parse error ({e:?}): {e}"))?;
    Ok((models, mats.unwrap_or_default()))
}

/// One mesh being built: every distinct (position, uv, normal) index triple of a face corner becomes one vertex, so the
/// arrays are aligned whatever index lists the file uses.
#[derive(Default)]
struct MeshBuilder {
    map: HashMap<(usize, usize, usize), u32>,
    positions: Vec<f32>,
    normals: Vec<f32>,
    texcoords: Vec<f32>,
    indices: Vec<u32>,
    all_uv: bool,
    all_normal: bool,
}

impl MeshBuilder {
    fn new() -> Self {
        Self { all_uv: true, all_normal: true, ..Default::default() }
    }
}

/// `1`, `-1` (relative to the end of the list) -> zero-based index.
fn obj_index(s: &str, len: usize) -> Result<usize, String> {
    let i: i64 = s.parse().map_err(|_| format!("bad index '{s}'"))?;
    let z = if i > 0 { i - 1 } else if i < 0 { len as i64 + i } else { return Err("index 0".into()) };
    if z < 0 || z as usize >= len {
        return Err(format!("index {i} out of range ({len})"));
    }
    Ok(z as usize)
}

fn floats<'a>(it: impl Iterator<Item = &'a str>, n: usize) -> Result<Vec<f32>, String> {
    let mut v: Vec<f32> = Vec::with_capacity(n);
    for t in it.take(n) {
        v.push(t.parse().map_err(|_| format!("bad number '{t}'"))?);
    }
    Ok(v)
}

/// Read the geometry of an OBJ: one mesh per object / group and material (like tobj), with polygons fan-triangulated.
/// Position, UV and normal lists are indexed independently, as the format allows.
fn parse_obj_geometry(text: &str, mats: &[tobj::Material]) -> Result<Vec<tobj::Model>, String> {
    let (mut pos, mut tex, mut nor): (Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<[f32; 3]>) = (vec![], vec![], vec![]);
    let mut models: Vec<tobj::Model> = Vec::new();
    let mut name = String::from("unnamed");
    let mut mat: Option<usize> = None;
    let mut cur = MeshBuilder::new();

    fn flush(cur: &mut MeshBuilder, name: &str, mat: Option<usize>, models: &mut Vec<tobj::Model>) {
        let b = std::mem::replace(cur, MeshBuilder::new());
        if b.indices.is_empty() {
            return;
        }
        let mesh = tobj::Mesh {
            positions: b.positions,
            normals: if b.all_normal { b.normals } else { Vec::new() },
            texcoords: if b.all_uv { b.texcoords } else { Vec::new() },
            indices: b.indices,
            material_id: mat,
            ..Default::default()
        };
        models.push(tobj::Model::new(mesh, name.to_string()));
    }

    for (ln, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut it = line.split_whitespace();
        let key = it.next().unwrap_or("");
        let err = |m: String| format!("line {}: {m}", ln + 1);
        match key {
            "v" => {
                let f = floats(it, 3).map_err(err)?;
                if f.len() < 3 {
                    return Err(err("v needs 3 numbers".into()));
                }
                pos.push([f[0], f[1], f[2]]);
            }
            "vt" => {
                let f = floats(it, 2).map_err(err)?;
                if f.is_empty() {
                    return Err(err("vt needs a number".into()));
                }
                tex.push([f[0], f.get(1).copied().unwrap_or(0.0)]);
            }
            "vn" => {
                let f = floats(it, 3).map_err(err)?;
                if f.len() < 3 {
                    return Err(err("vn needs 3 numbers".into()));
                }
                nor.push([f[0], f[1], f[2]]);
            }
            "f" => {
                let mut corners: Vec<(usize, Option<usize>, Option<usize>)> = Vec::new();
                for c in it {
                    let mut parts = c.split('/');
                    let v = obj_index(parts.next().unwrap_or(""), pos.len()).map_err(err)?;
                    let t = match parts.next() {
                        Some(s) if !s.is_empty() => Some(obj_index(s, tex.len()).map_err(err)?),
                        _ => None,
                    };
                    let n = match parts.next() {
                        Some(s) if !s.is_empty() => Some(obj_index(s, nor.len()).map_err(err)?),
                        _ => None,
                    };
                    corners.push((v, t, n));
                }
                if corners.len() < 3 {
                    continue; // points and lines are not geometry we draw
                }
                // Resolve each corner to a vertex of the mesh being built.
                let mut ids = Vec::with_capacity(corners.len());
                for &(v, t, n) in &corners {
                    let key = (v, t.unwrap_or(usize::MAX), n.unwrap_or(usize::MAX));
                    let id = match cur.map.get(&key) {
                        Some(&id) => id,
                        None => {
                            let id = (cur.positions.len() / 3) as u32;
                            cur.positions.extend_from_slice(&pos[v]);
                            match t {
                                Some(t) => cur.texcoords.extend_from_slice(&tex[t]),
                                None => {
                                    cur.all_uv = false;
                                    cur.texcoords.extend_from_slice(&[0.0, 0.0]);
                                }
                            }
                            match n {
                                Some(n) => cur.normals.extend_from_slice(&nor[n]),
                                None => {
                                    cur.all_normal = false;
                                    cur.normals.extend_from_slice(&[0.0, 0.0, 0.0]);
                                }
                            }
                            cur.map.insert(key, id);
                            id
                        }
                    };
                    ids.push(id);
                }
                for k in 1..ids.len() - 1 {
                    cur.indices.extend_from_slice(&[ids[0], ids[k], ids[k + 1]]);
                }
            }
            "o" | "g" => {
                flush(&mut cur, &name, mat, &mut models);
                let rest = line[key.len()..].trim();
                name = if rest.is_empty() { "unnamed".to_string() } else { rest.to_string() };
            }
            "usemtl" => {
                flush(&mut cur, &name, mat, &mut models);
                let m = line[key.len()..].trim();
                mat = mats.iter().position(|x| x.name == m);
            }
            _ => {}
        }
    }
    flush(&mut cur, &name, mat, &mut models);
    if models.is_empty() {
        return Err("no faces".into());
    }
    Ok(models)
}

fn flatten(
    pos: &[[f32; 3]],
    uv: &[[f32; 2]],
    idx: &[u32],
) -> (Vec<[f32; 3]>, Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<u32>) {
    let (mut p, mut n, mut u, mut i) = (vec![], vec![], vec![], vec![]);
    for t in idx.chunks_exact(3) {
        let (a, b, c) = (
            Vec3::from(pos[t[0] as usize]),
            Vec3::from(pos[t[1] as usize]),
            Vec3::from(pos[t[2] as usize]),
        );
        let nn = (b - a).cross(c - a).normalize_or_zero().to_array();
        for k in 0..3 {
            p.push(pos[t[k] as usize]);
            n.push(nn);
            u.push(uv[t[k] as usize]);
            i.push(i.len() as u32);
        }
    }
    (p, n, u, i)
}

fn parse_struct(s: &str) -> HashMap<String, f32> {
    s.trim()
        .trim_matches(|c| c == '(' || c == ')')
        .split(',')
        .filter_map(|kv| {
            let (k, v) = kv.split_once('=')?;
            Some((k.trim().to_uppercase(), v.trim().parse().ok()?))
        })
        .collect()
}

impl ModelData {
    pub fn num(&self, key: &str, default: f32) -> f32 {
        self.cfg
            .get(key)
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    }
    pub fn vec3(&self, key: &str) -> Vec3 {
        let m = self
            .cfg
            .get(key)
            .map(|s| parse_struct(s))
            .unwrap_or_default();
        Vec3::new(
            *m.get("X").unwrap_or(&0.0),
            *m.get("Y").unwrap_or(&0.0),
            *m.get("Z").unwrap_or(&0.0),
        )
    }
    pub fn color(&self, key: &str) -> Color {
        let m = self
            .cfg
            .get(key)
            .map(|s| parse_struct(s))
            .unwrap_or_default();
        Color::linear_rgb(
            *m.get("R").unwrap_or(&1.0),
            *m.get("G").unwrap_or(&1.0),
            *m.get("B").unwrap_or(&1.0),
        )
    }
    /// `filter_<kind>_` is an engine TextureFilter value: 0 Nearest, 1 Bilinear, 2 Trilinear, 3 Default.
    pub fn filter(&self, kind: Kind) -> u8 {
        self.num(&format!("filter_{}_", kind.name()), 0.0) as u8
    }
    /// Filter for one section's texture. Multi-material cfgs use `filter_<kind>_<material>` (for example
    /// `filter_diffuse_body`); single-material ones use `filter_<kind>_`. Per-material wins, default is 0 (nearest).
    pub fn filter_for(&self, kind: Kind, s: &RawSection) -> u8 {
        self.cfg
            .get(&format!("filter_{}_{}", kind.name(), s.mat_name))
            .and_then(|v| v.parse::<f32>().ok())
            .map(|v| v as u8)
            .unwrap_or_else(|| self.filter(kind))
    }
    /// Texture lookup: `<kind>_<material>.png`, then `<kind>_<folder>.png`, then the name implied by map_Kd.
    pub fn find_tex(&self, kind: Kind, s: &RawSection) -> Option<PathBuf> {
        let mut cands = vec![];
        if !s.mat_name.is_empty() {
            cands.push(format!("{}_{}.png", kind.name(), s.mat_name));
        }
        cands.push(format!("{}_{}.png", kind.name(), self.name));
        if let Some(kd) = &s.kd {
            let stem = Path::new(kd)
                .file_stem()
                .and_then(|x| x.to_str())
                .unwrap_or("");
            if stem.to_lowercase().starts_with("diffuse_") {
                cands.push(format!("{}_{}.png", kind.name(), &stem[8..]));
            }
        }
        cands
            .iter()
            .find_map(|c| self.files.get(&c.to_lowercase()).cloned())
    }
}

// ---------------------------------------------------------------------------------------------
// Textures: decoded off the main thread, optionally downscaled (icons only need ~128 px)
// ---------------------------------------------------------------------------------------------

pub struct RawImage {
    pub w: u32,
    pub h: u32,
    pub data: Vec<u8>,
}

pub struct PoolImg {
    pub img: RawImage,
    pub kind: Kind,
    pub filter: u8,
}

/// A model plus its decoded textures. `tex[section][kind.idx()]` indexes into `pool`.
pub struct Loaded {
    pub data: ModelData,
    pub tex: Vec<[Option<usize>; 4]>,
    pub pool: Vec<PoolImg>,
    pub dup_removed: usize, // triangles dropped by the converter's duplicate rule
}

fn decode(path: &Path, kind: Kind, max_side: Option<u32>) -> Option<RawImage> {
    let mut img = image::open(path).ok()?.to_rgba8();
    if let Some(m) = max_side {
        let (w, h) = img.dimensions();
        let longest = w.max(h);
        if longest > m {
            let s = m as f32 / longest as f32;
            let (nw, nh) = (
                ((w as f32 * s).round() as u32).max(1),
                ((h as f32 * s).round() as u32).max(1),
            );
            img = image::imageops::thumbnail(&img, nw, nh);
        }
    }
    let (w, h) = img.dimensions();
    let mut data = img.into_raw();
    if kind == Kind::Pbr {
        // VotV: R metallic, G specular, B roughness. Bevy wants G roughness, B metallic. Bevy ignores R, so the game's
        // specular goes there (scene.rs averages it into the material's `reflectance`).
        for px in data.chunks_exact_mut(4) {
            let (metal, spec, rough) = (px[0], px[1], px[2]);
            px[0] = spec;
            px[1] = rough;
            px[2] = metal;
            px[3] = 255;
        }
    }
    Some(RawImage { w, h, data })
}

/// Parse the model and decode only the requested texture kinds. Textures shared between sections
/// (the usual single-atlas layout) are decoded once.
pub fn load_with_textures(
    dir: &Path,
    kinds: &[Kind],
    max_side: Option<u32>,
) -> Result<Loaded, String> {
    let mut data = load_model(dir)?;
    let mut pool: Vec<PoolImg> = Vec::new();
    let mut seen: HashMap<(PathBuf, Kind), usize> = HashMap::new();
    let mut tex = Vec::new();
    for s in &data.sections {
        let mut slots: [Option<usize>; 4] = [None; 4];
        for &k in kinds {
            let Some(p) = data.find_tex(k, s) else {
                continue;
            };
            let key = (p.clone(), k);
            let idx = if let Some(&i) = seen.get(&key) {
                Some(i)
            } else if let Some(img) = decode(&p, k, max_side) {
                pool.push(PoolImg {
                    img,
                    kind: k,
                    filter: data.filter_for(k, s),
                });
                seen.insert(key, pool.len() - 1);
                Some(pool.len() - 1)
            } else {
                None
            };
            slots[k.idx()] = idx;
        }
        tex.push(slots);
    }
    // Same policy as printer_convert.py: where geometry is duplicated, keep the copy that has a texture.
    let has_tex: Vec<bool> = tex
        .iter()
        .map(|t| t[Kind::Diffuse.idx()].is_some())
        .collect();
    let dup_removed = mark_duplicate_faces(&mut data, &has_tex);
    Ok(Loaded {
        data,
        tex,
        pool,
        dup_removed,
    })
}

/// The converter's duplicate rule (`_find_exact_duplicate_parts` in printer_convert.py), applied per triangle.
///
/// The converter drops a whole part when another part has identical vertices and faces, keeping the one with a
/// base-colour texture (or the earlier one when both or neither have one). Here a triangle counts as a duplicate when
/// another section has a triangle with the same three corners in the same winding order; the copy in a section
/// without a texture is dropped. Copies inside one section, and reversed-winding copies, are left alone.
/// Fills `RawSection::dedup` and returns how many triangles were dropped.
pub fn mark_duplicate_faces(data: &mut ModelData, has_tex: &[bool]) -> usize {
    let q = |p: [f32; 3]| {
        [
            (p[0] * 1.0e4).round() as i64,
            (p[1] * 1.0e4).round() as i64,
            (p[2] * 1.0e4).round() as i64,
        ]
    };
    let mut groups: HashMap<[[i64; 3]; 3], Vec<(usize, usize)>> = HashMap::new(); // corners -> (section, triangle)
    for (si, s) in data.sections.iter().enumerate() {
        for (ti, t) in s.indices.chunks_exact(3).enumerate() {
            let c = [
                q(s.positions[t[0] as usize]),
                q(s.positions[t[1] as usize]),
                q(s.positions[t[2] as usize]),
            ];
            let r = (0..3).min_by_key(|&i| c[i]).unwrap_or(0); // rotate so the smallest corner is first: keeps winding
            groups
                .entry([c[r], c[(r + 1) % 3], c[(r + 2) % 3]])
                .or_default()
                .push((si, ti));
        }
    }
    let mut drop: Vec<HashSet<usize>> = vec![HashSet::new(); data.sections.len()];
    let mut n = 0;
    for members in groups.values() {
        if members.len() < 2 {
            continue;
        }
        let keeper = members
            .iter()
            .find(|(si, _)| has_tex.get(*si).copied().unwrap_or(false))
            .unwrap_or(&members[0])
            .0;
        for &(si, ti) in members {
            if si != keeper {
                drop[si].insert(ti);
                n += 1;
            }
        }
    }
    for (si, set) in drop.iter().enumerate() {
        if set.is_empty() {
            continue;
        }
        let s = &mut data.sections[si];
        let mut out = Vec::with_capacity(s.indices.len());
        for (ti, t) in s.indices.chunks_exact(3).enumerate() {
            if !set.contains(&ti) {
                out.extend_from_slice(t);
            }
        }
        s.dedup = Some(out);
    }
    n
}

/// Triangles that sit exactly on top of an earlier triangle (same three corner positions, 0.001 unit grid).
/// Returns ((section that has it first, section holding the copy), how many). They fight for the same pixels,
/// so a copy with a different material shows as flickering stripes.
pub fn duplicate_faces(m: &ModelData) -> Vec<((usize, usize), usize)> {
    let q = |p: [f32; 3]| {
        [
            (p[0] * 1000.0).round() as i64,
            (p[1] * 1000.0).round() as i64,
            (p[2] * 1000.0).round() as i64,
        ]
    };
    let mut seen: HashMap<[[i64; 3]; 3], usize> = HashMap::new();
    let mut dup: HashMap<(usize, usize), usize> = HashMap::new();
    for (si, s) in m.sections.iter().enumerate() {
        for t in s.indices.chunks_exact(3) {
            let mut v = [
                q(s.positions[t[0] as usize]),
                q(s.positions[t[1] as usize]),
                q(s.positions[t[2] as usize]),
            ];
            v.sort();
            match seen.get(&v) {
                Some(&first) => *dup.entry((first, si)).or_default() += 1,
                None => {
                    seen.insert(v, si);
                }
            }
        }
    }
    let mut out: Vec<_> = dup.into_iter().collect();
    out.sort_by_key(|a| std::cmp::Reverse(a.1));
    out
}

/// Print what the loader found, without opening a window (`--check <folder>`).
pub fn check(m: &ModelData) {
    println!("model  : {}", m.name);
    println!(
        "sections: {}   hulls (UCX_): {}",
        m.sections.len(),
        m.hulls.len()
    );
    for (i, s) in m.sections.iter().enumerate() {
        println!(
            "  [{i}] {:<24} verts {:<6} tris {:<6} material '{}'",
            s.name,
            s.positions.len(),
            s.indices.len() / 3,
            s.mat_name
        );
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for p in &s.positions {
            for a in 0..3 {
                lo[a] = lo[a].min(p[a]);
                hi[a] = hi[a].max(p[a]);
            }
        }
        println!(
            "        bbox   x {:.1}..{:.1}   y {:.1}..{:.1}   z {:.1}..{:.1}",
            lo[0], hi[0], lo[1], hi[1], lo[2], hi[2]
        );
        if s.uvs.iter().all(|u| u[0] == 0.0 && u[1] == 0.0) {
            println!("        uv     NONE (every vertex has UV 0,0: no texture coordinates for this part)");
        } else {
            let (mut ulo, mut uhi) = ([f32::MAX; 2], [f32::MIN; 2]);
            let mut outside = 0usize;
            for u in &s.uvs {
                for a in 0..2 {
                    ulo[a] = ulo[a].min(u[a]);
                    uhi[a] = uhi[a].max(u[a]);
                }
                if u[0] < 0.0 || u[0] > 1.0 || u[1] < 0.0 || u[1] > 1.0 {
                    outside += 1;
                }
            }
            let mut degenerate = 0usize;
            for t in s.indices.chunks_exact(3) {
                let (a, b, c) = (
                    s.uvs[t[0] as usize],
                    s.uvs[t[1] as usize],
                    s.uvs[t[2] as usize],
                );
                let area =
                    ((b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1])).abs() * 0.5;
                if area < 1e-9 {
                    degenerate += 1;
                }
            }
            println!(
                "        uv     u {:.3}..{:.3}   v {:.3}..{:.3}   outside 0..1: {:.0}%   zero-area UV triangles: {} of {}",
                ulo[0], uhi[0], ulo[1], uhi[1],
                100.0 * outside as f32 / s.uvs.len().max(1) as f32,
                degenerate, s.indices.len() / 3
            );
        }
        for k in KINDS {
            match m.find_tex(k, s) {
                Some(p) => {
                    let d = image::image_dimensions(&p)
                        .map(|(w, h)| format!("{w}x{h}"))
                        .unwrap_or("unreadable".into());
                    println!(
                        "        {:<9} {} ({d}) filter={}",
                        k.name(),
                        p.file_name().unwrap().to_string_lossy(),
                        m.filter_for(k, s)
                    );
                }
                None => println!("        {:<9} (not found)", k.name()),
            }
        }
    }
    let dups = duplicate_faces(m);
    if !dups.is_empty() {
        println!("WARNING: duplicate faces (triangles drawn exactly on top of each other; they z-fight and flicker).");
        println!("         The manager preview drops the copy without a texture, like printer_convert.py does (D toggles).");
        for ((a, b), n) in &dups {
            println!(
                "  {n} triangles of section [{b}] ('{}') duplicate section [{a}] ('{}')",
                m.sections[*b].mat_name, m.sections[*a].mat_name
            );
        }
    }
    for h in &m.hulls {
        let uniq: HashSet<[u32; 3]> = h.points.iter().map(|p| p.map(f32::to_bits)).collect();
        println!("  hull {:<14} unique points {}", h.name, uniq.len());
    }
    let pm = m.num("physical_material", 0.0) as usize;
    println!(
        "physical_material {} ({})",
        pm,
        PHYS_MATERIALS.get(pm).unwrap_or(&"?")
    );
    println!("center {:?}  radius {:.1} cm", m.center, m.radius);
    if m.hulls.is_empty() {
        println!("note: no UCX_ hulls, so in game this model gets no health, damage data, edible or bites");
    }
}

#[cfg(test)]
mod obj_parser_tests {
    use super::*;

    /// Separate v / vt / vn lists (more normals than positions, like Blender exports): UVs and normals must come out
    /// aligned with the positions and keep their values.
    #[test]
    fn separate_index_lists_keep_uvs_and_normals() {
        let obj = "o A\nv 0 0 0\nv 1 0 0\nv 0 1 0\nvt 0.1 0.2\nvt 0.3 0.4\nvt 0.5 0.6\n\
                   vn 0 0 1\nvn 0 1 0\nvn 1 0 0\nvn 0 0 -1\nusemtl m\nf 1/1/1 2/2/2 3/3/3\nf 1/1/4 3/3/3 2/2/2\n";
        let models = parse_obj_geometry(obj, &[]).unwrap();
        assert_eq!(models.len(), 1);
        let m = &models[0].mesh;
        // 4 distinct corner triples: (1,1,1) (2,2,2) (3,3,3) (1,1,4)
        assert_eq!(m.positions.len(), 4 * 3);
        assert_eq!(m.normals.len(), m.positions.len());
        assert_eq!(m.texcoords.len(), 4 * 2);
        assert_eq!(m.indices.len(), 6);
        assert_eq!(&m.texcoords[0..2], &[0.1, 0.2]);
        assert_eq!(&m.texcoords[4..6], &[0.5, 0.6]);
    }

    #[test]
    fn quads_are_triangulated_and_negative_indices_work() {
        let obj = "v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nf -4 -3 -2 -1\n";
        let models = parse_obj_geometry(obj, &[]).unwrap();
        assert_eq!(models[0].mesh.indices.len(), 6);
        assert!(models[0].mesh.texcoords.is_empty() && models[0].mesh.normals.is_empty());
    }
}
