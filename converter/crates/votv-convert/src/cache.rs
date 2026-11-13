//! Optional cache that makes re-conversion of the same file fast when only some options change.
//!
//! The GUI keeps one [`ConvertCache`] for its session and hands a clone to every job via
//! [`Hooks::with_cache`](crate::Hooks::with_cache). Results are reused only when everything they depend on is identical, so
//! the output is byte-for-byte the same as without a cache. Cached stages and what invalidates each:
//!
//! | stage      | reused while these stay the same                                                                                   |
//! |------------|---------------------------------------------------------------------------------------------------------------------|
//! | `scene`    | the file (path, size, modified time), `humanoid` and `emissive_mode`: skips reading the .glb and decoding its textures              |
//! | `atlas`    | scene, `drop_transparent`, `max_tile_size`, `max_atlas_size`, `apply_base_color_factor`, `flip_normal_green`, `emissive`, `show_collision`, `metallic_scale`, `metallic_max`: skips baking/packing the atlases **and PNG compression** |
//! | `geometry` | atlas inputs plus `scale` and `write_icon`: skips UV cutting and OBJ text for the visible mesh                      |
//! | `collision`| scene, `drop_transparent`, `collision_quality`, `vhacd_resolution`, `max_decomp_hulls`: skips VHACD (the slowest step) |
//!
//! Changing only collision offsets, `show_collision_offset`, hull face cap, fallback box size, emissive strength or any
//! `properties` value reuses everything expensive. A finished VHACD run is stored even if the job was cancelled right after.
//!
//! Memory: one scene (decoded textures) plus a few atlas/geometry/hull entries are kept (least recently used dropped).
//! Call [`ConvertCache::clear`] to free it. The cache is `Send + Sync`; clones share the same storage.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use image::RgbaImage;

use crate::atlas::Placement;
use crate::collision::{self, Generated, RawHulls};
use crate::icon::IconTri;
use crate::options::{EmissiveMode, PngLevel};
use crate::report::Diagnostic;
use crate::scene::Scene;

const MAX_ATLAS: usize = 3;
const MAX_GEOMETRY: usize = 3;
const MAX_HULLS: usize = 8;

#[derive(Clone, Default)]
pub struct ConvertCache(Arc<Mutex<Inner>>);

#[derive(Default)]
struct Inner {
    next_id: u64,
    scene: Option<(SceneKey, Arc<SceneEntry>)>,
    atlas: Vec<(AtlasKey, Arc<AtlasEntry>)>,
    geometry: Vec<(GeoKey, Arc<GeoEntry>)>,
    hulls: Vec<(HullKey, Arc<HullEntry>)>,
}

#[derive(Clone, PartialEq)]
pub(crate) struct SceneKey {
    pub path: PathBuf,
    pub mtime: Option<SystemTime>,
    pub len: u64,
    pub humanoid: bool,
    pub emissive_mode: EmissiveMode,
}

pub(crate) struct SceneEntry {
    /// Unique per loaded scene; later stages key on it.
    pub id: u64,
    pub scene: Scene,
    /// Messages the loader produced, replayed on every reuse so reports look the same.
    pub diags: Vec<Diagnostic>,
}

#[derive(Clone, PartialEq)]
pub(crate) struct AtlasKey {
    pub scene_id: u64,
    pub drop_transparent: bool,
    pub max_tile_size: u32,
    pub max_atlas_size: u32,
    pub apply_factor: bool,
    pub flip_green: bool,
    pub emissive: bool,
    pub show_collision: bool,
    pub metallic_scale_bits: u64,
    pub metallic_max_bits: u64,
}

pub(crate) struct AtlasEntry {
    pub diffuse: RgbaImage,
    pub emissive: Option<RgbaImage>,
    pub normal: Option<RgbaImage>,
    pub pbr: Option<RgbaImage>,
    pub placements: Vec<Placement>,
    pub real_uv: Vec<bool>,
    pub part_slot: Vec<usize>,
    pub unique_materials: usize,
    pub red_slot: Option<usize>,
    pub diags: Vec<Diagnostic>,
    /// Compressed PNG bytes, filled on first use: [diffuse, emissive, normal, pbr].
    /// One lock per map so the maps can be compressed on different threads. Remembers the level the bytes were made with.
    pub png: [Mutex<Option<(PngLevel, Arc<Vec<u8>>)>>; 4],
}

#[derive(Clone, PartialEq)]
pub(crate) struct GeoKey {
    pub atlas: AtlasKey,
    pub scale_bits: u64,
    pub write_icon: bool,
}

pub(crate) struct GeoEntry {
    /// OBJ text of the visible mesh, without the `mtllib` / `usemtl` header (those depend on the output name).
    pub text: String,
    pub vt_count: usize,
    pub vert_offset: usize,
    pub total_verts: usize,
    pub total_faces: usize,
    pub tiled_split: usize,
    pub tiled_unsplit: usize,
    pub icon_tris: Vec<IconTri>,
}

#[derive(Clone, PartialEq)]
pub(crate) struct HullKey {
    pub scene_id: u64,
    pub drop_transparent: bool,
    /// 0 = single hull, 1 = decomposition.
    pub quality: u8,
    pub resolution: u32,
    pub max_hulls: u32,
}

pub(crate) struct HullEntry {
    pub raw: Result<RawHulls, String>,
    /// Microseconds the decomposition itself took (for `Report::timings`).
    pub generate_micros: u64,
    /// Trimmed hulls per face cap. The trim is deterministic and not free (re-hulling big hulls), so a repeat run with the same cap reuses it.
    trimmed: Mutex<Vec<(usize, Arc<Generated>)>>,
}

impl HullEntry {
    pub(crate) fn new(raw: Result<RawHulls, String>, generate_micros: u64) -> Self {
        HullEntry { raw, generate_micros, trimmed: Mutex::new(Vec::new()) }
    }

    /// The hulls with every hull cut down to at most `cap` faces. `None` when the decomposition produced no hulls at all (`raw` is an error).
    pub(crate) fn trimmed(&self, cap: usize) -> Option<Arc<Generated>> {
        let raw = self.raw.as_ref().ok()?;
        let mut memo = self.trimmed.lock().ok();
        if let Some(m) = memo.as_ref() {
            if let Some((_, g)) = m.iter().find(|(c, _)| *c == cap) {
                return Some(g.clone());
            }
        }
        let g = Arc::new(collision::trim(raw, cap));
        if let Some(m) = memo.as_mut() {
            m.insert(0, (cap, g.clone()));
            m.truncate(4);
        }
        Some(g)
    }
}

fn lru_get<K: PartialEq, V>(v: &mut Vec<(K, Arc<V>)>, k: &K) -> Option<Arc<V>> {
    let i = v.iter().position(|(kk, _)| kk == k)?;
    let e = v.remove(i);
    let a = e.1.clone();
    v.insert(0, e);
    Some(a)
}

fn lru_put<K: PartialEq, V>(v: &mut Vec<(K, Arc<V>)>, k: K, val: Arc<V>, max: usize) {
    if let Some(i) = v.iter().position(|(kk, _)| *kk == k) {
        v.remove(i);
    }
    v.insert(0, (k, val));
    v.truncate(max);
}

impl ConvertCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Drop everything (frees the decoded textures and cached atlases).
    pub fn clear(&self) {
        if let Ok(mut g) = self.0.lock() {
            let next = g.next_id;
            *g = Inner::default();
            g.next_id = next;
        }
    }

    pub(crate) fn scene(&self, k: &SceneKey) -> Option<Arc<SceneEntry>> {
        let g = self.0.lock().ok()?;
        g.scene.as_ref().filter(|(kk, _)| kk == k).map(|(_, e)| e.clone())
    }

    /// Store a freshly loaded scene. A different scene replaces the old one and everything derived from it.
    pub(crate) fn put_scene(&self, k: SceneKey, scene: Scene, diags: Vec<Diagnostic>) -> Arc<SceneEntry> {
        let Ok(mut g) = self.0.lock() else {
            return Arc::new(SceneEntry { id: 0, scene, diags });
        };
        g.next_id += 1;
        let e = Arc::new(SceneEntry { id: g.next_id, scene, diags });
        g.atlas.clear();
        g.geometry.clear();
        g.hulls.clear();
        g.scene = Some((k, e.clone()));
        e
    }

    pub(crate) fn atlas(&self, k: &AtlasKey) -> Option<Arc<AtlasEntry>> {
        lru_get(&mut self.0.lock().ok()?.atlas, k)
    }
    pub(crate) fn put_atlas(&self, k: AtlasKey, e: Arc<AtlasEntry>) {
        if let Ok(mut g) = self.0.lock() {
            lru_put(&mut g.atlas, k, e, MAX_ATLAS);
        }
    }
    pub(crate) fn geometry(&self, k: &GeoKey) -> Option<Arc<GeoEntry>> {
        lru_get(&mut self.0.lock().ok()?.geometry, k)
    }
    pub(crate) fn put_geometry(&self, k: GeoKey, e: Arc<GeoEntry>) {
        if let Ok(mut g) = self.0.lock() {
            lru_put(&mut g.geometry, k, e, MAX_GEOMETRY);
        }
    }
    pub(crate) fn hulls(&self, k: &HullKey) -> Option<Arc<HullEntry>> {
        lru_get(&mut self.0.lock().ok()?.hulls, k)
    }
    pub(crate) fn put_hulls(&self, k: HullKey, e: Arc<HullEntry>) {
        if let Ok(mut g) = self.0.lock() {
            lru_put(&mut g.hulls, k, e, MAX_HULLS);
        }
    }
}
