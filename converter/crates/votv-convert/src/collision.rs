//! Collision: generated convex hulls, hull trimming, the fallback box and the `show_collision` overlay.
//!
//! Everything returned from here is a plain indexed triangle mesh; the OBJ writer turns it into
//! `o UCX_*` objects.

use std::collections::{HashMap, HashSet};

use parry3d::math::Vector;
use parry3d::transformation::vhacd::{VHACDParameters, VHACD};
use parry3d::transformation::try_convex_hull;

use crate::geom::*;
use crate::options::{CollisionQuality, ConvertOptions};
use crate::scene::Part;

#[derive(Clone, Debug)]
pub struct Mesh {
    pub verts: Vec<V3>,
    pub faces: Vec<[u32; 3]>,
}

fn to_vec(p: V3) -> Vector {
    Vector::new(p[0] as f32, p[1] as f32, p[2] as f32)
}

fn from_vec(v: Vector) -> V3 {
    [v.x as f64, v.y as f64, v.z as f64]
}

fn signed_volume(verts: &[V3], faces: &[[u32; 3]]) -> f64 {
    faces
        .iter()
        .map(|f| dot(verts[f[0] as usize], cross(verts[f[1] as usize], verts[f[2] as usize])))
        .sum::<f64>()
        / 6.0
}

fn hull_of(points: &[V3]) -> Option<Mesh> {
    let pts: Vec<Vector> = points.iter().map(|p| to_vec(*p)).collect();
    let (v, f) = std::panic::catch_unwind(|| try_convex_hull(&pts)).ok()?.ok()?;
    if f.is_empty() {
        return None;
    }
    let verts: Vec<V3> = v.into_iter().map(from_vec).collect();
    let mut faces = f;
    if signed_volume(&verts, &faces) < 0.0 {
        faces.iter_mut().for_each(|t| t.swap(1, 2)); // outward, counter-clockwise
    }
    Some(Mesh { verts, faces })
}

/// Reduce a convex hull to at most `max_faces` faces by dropping the least important vertices
/// and re-hulling the rest. The result is a hull of a subset of the points, so it stays convex
/// (quadric decimation, used by the Python version, can leave a hull slightly non-convex).
pub fn simplify_hull(mut mesh: Mesh, max_faces: usize) -> Mesh {
    let mut guard = 0;
    while mesh.faces.len() > max_faces && mesh.verts.len() > 4 && guard < 200 {
        guard += 1;
        let n = mesh.verts.len();
        let mut incident: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (fi, f) in mesh.faces.iter().enumerate() {
            for &v in f {
                incident[v as usize].push(fi);
            }
        }
        // cost of removing a vertex ~ volume of the cap it forms over its neighbour ring
        let mut cost: Vec<(f64, usize)> = (0..n)
            .map(|v| {
                let mut ring: HashSet<u32> = HashSet::new();
                for &fi in &incident[v] {
                    ring.extend(mesh.faces[fi].iter().copied().filter(|&x| x as usize != v));
                }
                let c = ring.iter().fold([0.0; 3], |a, &r| add(a, mesh.verts[r as usize]));
                let c = mul(c, 1.0 / ring.len().max(1) as f64);
                let vol: f64 = incident[v]
                    .iter()
                    .map(|&fi| {
                        let f = mesh.faces[fi];
                        let o: Vec<V3> = f.iter().filter(|&&x| x as usize != v).map(|&x| mesh.verts[x as usize]).collect();
                        let pv = mesh.verts[v];
                        dot(sub(o[0], pv), cross(sub(o[1], pv), sub(c, pv))).abs() / 6.0
                    })
                    .sum();
                (vol, v)
            })
            .collect();
        cost.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let excess = mesh.faces.len() - max_faces;
        let want = (excess / 6).max(1);
        let mut blocked: HashSet<usize> = HashSet::new();
        let mut remove: HashSet<usize> = HashSet::new();
        for (_, v) in cost {
            if remove.len() >= want {
                break;
            }
            if blocked.contains(&v) {
                continue;
            }
            remove.insert(v);
            for &fi in &incident[v] {
                blocked.extend(mesh.faces[fi].iter().map(|&x| x as usize));
            }
        }
        let keep: Vec<V3> = (0..n).filter(|v| !remove.contains(v)).map(|v| mesh.verts[v]).collect();
        match hull_of(&keep) {
            Some(h) => mesh = h,
            None => break,
        }
    }
    mesh
}

pub struct Generated {
    pub hulls: Vec<Mesh>,
    /// Set when decomposition failed and a single hull was used instead.
    pub decomposition_error: Option<String>,
    pub hulls_over_cap: usize,
}

/// Hulls straight from decomposition (or the single hull), before the face cap is applied. This is the expensive part
/// and does not depend on scale, offsets or the cap, so it is what gets cached.
pub struct RawHulls {
    pub hulls: Vec<Mesh>,
    pub decomposition_error: Option<String>,
}

/// Hulls of the visible geometry in source units (metres), not yet scaled and not yet trimmed.
pub fn generate_raw(parts: &[Part], opts: &ConvertOptions) -> Result<RawHulls, String> {
    let mut pts: Vec<Vector> = Vec::new();
    let mut idx: Vec<[u32; 3]> = Vec::new();
    for p in parts {
        let base = pts.len() as u32;
        pts.extend(p.pos.iter().map(|v| to_vec(*v)));
        idx.extend(p.tris.iter().map(|t| [t[0] + base, t[1] + base, t[2] + base]));
    }
    let all: Vec<V3> = pts.iter().map(|v| from_vec(*v)).collect();

    let mut decomposition_error = None;
    let mut meshes: Option<Vec<Mesh>> = None;
    if opts.collision_quality == CollisionQuality::High {
        let params = VHACDParameters {
            resolution: opts.vhacd_resolution,
            max_convex_hulls: opts.max_decomp_hulls,
            ..Default::default()
        };
        let r = std::panic::catch_unwind(|| {
            let d = VHACD::decompose(&params, &pts, &idx, true);
            d.compute_exact_convex_hulls(&pts, &idx)
        });
        match r {
            Ok(hs) => {
                let v: Vec<Mesh> = hs
                    .into_iter()
                    .filter_map(|(v, f)| {
                        let verts: Vec<V3> = v.into_iter().map(from_vec).collect();
                        // re-hull for clean, outward, deduplicated output
                        hull_of(&verts).or_else(|| if f.is_empty() { None } else { Some(Mesh { verts, faces: f }) })
                    })
                    .collect();
                if v.is_empty() {
                    decomposition_error = Some("convex decomposition returned no hulls".to_string());
                } else {
                    meshes = Some(v);
                }
            }
            Err(_) => decomposition_error = Some("convex decomposition panicked on this geometry".to_string()),
        }
    }
    let meshes = match meshes {
        Some(m) => m,
        None => vec![hull_of(&all).ok_or_else(|| "could not build a convex hull (flat or empty geometry)".to_string())?],
    };
    Ok(RawHulls { hulls: meshes, decomposition_error })
}

/// Apply the per-hull face cap (cheap, so it is redone on every run).
pub fn trim(raw: &RawHulls, cap: usize) -> Generated {
    let mut meshes = raw.hulls.clone();
    let mut over = 0;
    for m in meshes.iter_mut() {
        if m.faces.len() > cap {
            over += 1;
            *m = simplify_hull(m.clone(), cap);
        }
    }
    Generated { hulls: meshes, decomposition_error: raw.decomposition_error.clone(), hulls_over_cap: over }
}

/// Axis-aligned box as an outward, counter-clockwise triangle mesh.
pub fn box_mesh(min: V3, max: V3) -> Mesh {
    let v = |x: usize, y: usize, z: usize| [if x == 0 { min[0] } else { max[0] }, if y == 0 { min[1] } else { max[1] }, if z == 0 { min[2] } else { max[2] }];
    let verts = vec![v(0, 0, 0), v(1, 0, 0), v(1, 1, 0), v(0, 1, 0), v(0, 0, 1), v(1, 0, 1), v(1, 1, 1), v(0, 1, 1)];
    let faces = vec![
        [0, 3, 2], [0, 2, 1], // -z
        [4, 5, 6], [4, 6, 7], // +z
        [0, 4, 7], [0, 7, 3], // -x
        [1, 2, 6], [1, 6, 5], // +x
        [0, 1, 5], [0, 5, 4], // -y
        [3, 7, 6], [3, 6, 2], // +y
    ];
    Mesh { verts, faces }
}

/// Small box at the bottom centre of the model, already in output units (cm).
pub fn fallback_box(parts: &[Part], size_cm: f64, scale: f64, off_x: f64, off_y: f64) -> Option<Mesh> {
    let mut mn = [f64::MAX; 3];
    let mut mx = [f64::MIN; 3];
    let mut any = false;
    for p in parts {
        for v in &p.pos {
            any = true;
            for k in 0..3 {
                mn[k] = mn[k].min(v[k] * scale);
                mx[k] = mx[k].max(v[k] * scale);
            }
        }
    }
    if !any {
        return None;
    }
    let (cx, cz, bottom) = ((mn[0] + mx[0]) / 2.0 + off_x, (mn[2] + mx[2]) / 2.0, mn[1] + off_y);
    let h = size_cm / 2.0;
    Some(box_mesh([cx - h, bottom, cz - h], [cx + h, bottom + size_cm, cz + h]))
}

/// Push one collision shape outward by about `d` (same units as the vertices). Vertex normals are
/// built from the shape's distinct face planes (so a triangulated flat face counts once), flipped
/// when the winding points inward, with a mitre factor (capped at 1.5, so sharp corners do not shoot out) so a box grows by about `d` on every side.
pub fn inflate(mesh: &Mesh, d: f64) -> Vec<V3> {
    let (v, f) = (&mesh.verts, &mesh.faces);
    let mut fn_: Vec<V3> = f.iter().map(|t| cross(sub(v[t[1] as usize], v[t[0] as usize]), sub(v[t[2] as usize], v[t[0] as usize]))).collect();
    if signed_volume(v, f) < 0.0 {
        fn_.iter_mut().for_each(|n| *n = mul(*n, -1.0));
    }
    let fu: Vec<V3> = fn_.iter().map(|n| normalize(*n)).collect();

    let mut seen: HashSet<(u32, [i32; 3])> = HashSet::new();
    let mut vn = vec![[0.0f64; 3]; v.len()];
    let mut uniq: Vec<(usize, V3)> = Vec::new();
    for (fi, t) in f.iter().enumerate() {
        let key3 = [(fu[fi][0] * 1000.0).round() as i32, (fu[fi][1] * 1000.0).round() as i32, (fu[fi][2] * 1000.0).round() as i32];
        for &vi in t {
            if seen.insert((vi, key3)) {
                vn[vi as usize] = add(vn[vi as usize], fu[fi]);
                uniq.push((vi as usize, fu[fi]));
            }
        }
    }
    let vn: Vec<V3> = vn.into_iter().map(normalize).collect();
    let mut dots = vec![0.0f64; v.len()];
    let mut cnt = vec![0.0f64; v.len()];
    for (vi, n) in &uniq {
        dots[*vi] += dot(vn[*vi], *n);
        cnt[*vi] += 1.0;
    }
    (0..v.len())
        .map(|i| {
            let mean = if cnt[i] > 0.0 { dots[i] / cnt[i] } else { 1.0 };
            let mitre = (1.0 / mean.max(1e-3)).clamp(1.0, 1.5);
            add(v[i], mul(vn[i], d * mitre))
        })
        .collect()
}

/// De-duplicate custom `UCX_*` names the way the OBJ needs them: `name`, `name_01`, `name_02`...
pub fn unique_names(names: impl Iterator<Item = String>) -> Vec<String> {
    let mut used: HashMap<String, usize> = HashMap::new();
    names
        .map(|base| {
            let n = used.entry(base.clone()).or_insert(0);
            let out = if *n == 0 { base.clone() } else { format!("{base}_{:02}", *n) };
            *n += 1;
            out
        })
        .collect()
}

pub use votv_model_core::ucx_object_name as sanitize_obj_object_name;
