//! Tiled-texture handling. The atlas holds one copy of each texture, so a triangle whose UVs span
//! several repeats is cut along the integer UV lines; each piece then lies inside one repeat and
//! maps cleanly onto the tile (tiling is rebuilt as geometry).

use crate::geom::*;

pub const MAX_UV_CELLS_PER_FACE: i64 = 64;

type Corner = (V3, V3, [f64; 2]);

fn lerp(a: &Corner, b: &Corner, t: f64) -> Corner {
    (
        add(a.0, mul(sub(b.0, a.0), t)),
        add(a.1, mul(sub(b.1, a.1), t)),
        [a.2[0] + (b.2[0] - a.2[0]) * t, a.2[1] + (b.2[1] - a.2[1]) * t],
    )
}

/// Split a convex polygon at uv[axis] == c. Points exactly on the line go to both sides.
fn clip_poly(poly: &[Corner], axis: usize, c: f64) -> (Vec<Corner>, Vec<Corner>) {
    let (mut below, mut above) = (Vec::new(), Vec::new());
    let n = poly.len();
    for k in 0..n {
        let a = &poly[k];
        let b = &poly[(k + 1) % n];
        let da = a.2[axis] - c;
        let db = b.2[axis] - c;
        if da <= 0.0 {
            below.push(*a);
        }
        if da >= 0.0 {
            above.push(*a);
        }
        if (da < 0.0 && db > 0.0) || (db < 0.0 && da > 0.0) {
            let m = lerp(a, b, da / (da - db));
            below.push(m);
            above.push(m);
        }
    }
    (below, above)
}

fn split_tri(pos: [V3; 3], nrm: [V3; 3], uv: [[f64; 2]; 3]) -> Vec<Vec<Corner>> {
    let mut pieces: Vec<Vec<Corner>> = vec![(0..3).map(|k| (pos[k], nrm[k], uv[k])).collect()];
    for axis in 0..2 {
        let mut out = Vec::new();
        for pc in pieces {
            let lo = pc.iter().map(|v| v.2[axis]).fold(f64::MAX, f64::min);
            let hi = pc.iter().map(|v| v.2[axis]).fold(f64::MIN, f64::max);
            let lo = (lo + 1e-9).floor() as i64;
            let hi = (hi - 1e-9).floor() as i64;
            let mut cur = pc;
            for c in (lo + 1)..=hi {
                let (below, above) = clip_poly(&cur, axis, c as f64);
                if below.len() >= 3 {
                    out.push(below);
                }
                cur = above;
                if cur.len() < 3 {
                    break;
                }
            }
            if cur.len() >= 3 {
                out.push(cur);
            }
        }
        pieces = out;
    }
    pieces
}

/// One output triangle: vertex indices into `PartGeom::verts` and, for textured parts,
/// tile-local UVs in 0..1 per corner.
pub struct FaceRec {
    pub idx: [usize; 3],
    pub uv: Option<[[f64; 2]; 3]>,
}

pub struct PartGeom {
    /// Original vertices followed by the vertices created by cutting.
    pub verts: Vec<V3>,
    pub nrms: Vec<V3>,
    pub faces: Vec<FaceRec>,
    pub split: usize,
    pub unsplit: usize,
}

/// `verts` are already scaled. `uvs` is `Some` only for parts with a real base-colour texture.
pub fn build_part(verts: &[V3], nrms: &[V3], tris: &[[u32; 3]], uvs: Option<&[[f64; 2]]>) -> PartGeom {
    let mut g = PartGeom { verts: verts.to_vec(), nrms: nrms.to_vec(), faces: Vec::new(), split: 0, unsplit: 0 };
    let Some(uv) = uvs else {
        for t in tris {
            g.faces.push(FaceRec { idx: [t[0] as usize, t[1] as usize, t[2] as usize], uv: None });
        }
        return g;
    };
    for t in tris {
        let ti = [t[0] as usize, t[1] as usize, t[2] as usize];
        let fu = [uv[ti[0]], uv[ti[1]], uv[ti[2]]];
        let mn = [fu.iter().map(|u| u[0]).fold(f64::MAX, f64::min), fu.iter().map(|u| u[1]).fold(f64::MAX, f64::min)];
        let mx = [fu.iter().map(|u| u[0]).fold(f64::MIN, f64::max), fu.iter().map(|u| u[1]).fold(f64::MIN, f64::max)];
        let cell = [(mn[0] + 1e-9).floor(), (mn[1] + 1e-9).floor()];
        if mx[0] <= cell[0] + 1.0 + 1e-9 && mx[1] <= cell[1] + 1.0 + 1e-9 {
            let loc = [0, 1, 2].map(|k| [fu[k][0] - cell[0], fu[k][1] - cell[1]]);
            g.faces.push(FaceRec { idx: ti, uv: Some(loc) });
            continue;
        }
        let nx = (mx[0] - 1e-9).floor() - (mn[0] + 1e-9).floor() + 1.0;
        let ny = (mx[1] - 1e-9).floor() - (mn[1] + 1e-9).floor() + 1.0;
        if (nx * ny) as i64 > MAX_UV_CELLS_PER_FACE {
            // absurdly dense tiling: don't explode the face count, squeeze this one face into a tile
            g.unsplit += 1;
            let loc = [0, 1, 2].map(|k| [(fu[k][0] - cell[0]).clamp(0.0, 1.0), (fu[k][1] - cell[1]).clamp(0.0, 1.0)]);
            g.faces.push(FaceRec { idx: ti, uv: Some(loc) });
            continue;
        }
        g.split += 1;
        let pieces = split_tri([verts[ti[0]], verts[ti[1]], verts[ti[2]]], [nrms[ti[0]], nrms[ti[1]], nrms[ti[2]]], fu);
        for pc in pieces {
            let cen = [
                pc.iter().map(|c| c.2[0]).sum::<f64>() / pc.len() as f64,
                pc.iter().map(|c| c.2[1]).sum::<f64>() / pc.len() as f64,
            ];
            let pcell = [cen[0].floor(), cen[1].floor()];
            let mut idxs: Vec<(usize, [f64; 2])> = Vec::new();
            for (p, n, u) in &pc {
                g.verts.push(*p);
                g.nrms.push(normalize(*n));
                idxs.push((g.verts.len() - 1, [(u[0] - pcell[0]).clamp(0.0, 1.0), (u[1] - pcell[1]).clamp(0.0, 1.0)]));
            }
            for k in 1..idxs.len() - 1 {
                let (a, b, c) = (idxs[0], idxs[k], idxs[k + 1]);
                let area = ((b.1[0] - a.1[0]) * (c.1[1] - a.1[1]) - (c.1[0] - a.1[0]) * (b.1[1] - a.1[1])).abs();
                if area < 1e-10 {
                    continue;
                }
                g.faces.push(FaceRec { idx: [a.0, b.0, c.0], uv: Some([a.1, b.1, c.1]) });
            }
        }
    }
    g
}
