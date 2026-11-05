//! A quick software-rendered preview (`icon.png`): orthographic three-quarter view, z-buffer, simple lighting,
//! texture sampled from the diffuse atlas. Not meant to match the game's own icon, only to give a manager
//! something to show.

use image::{Rgba, RgbaImage};

use crate::geom::*;

pub struct IconTri {
    pub p: [V3; 3],
    /// Atlas UV in OBJ convention (v up), 0..1.
    pub uv: [[f64; 2]; 3],
}

pub fn render(tris: &[IconTri], atlas: &RgbaImage, size: u32) -> RgbaImage {
    let ss = 2u32; // supersample, then reduce
    let n = (size * ss) as usize;
    let (yaw, pitch) = (35f64.to_radians(), 28f64.to_radians());
    let (cy, sy, cp, sp) = (yaw.cos(), yaw.sin(), pitch.cos(), pitch.sin());
    let view = |p: V3| -> V3 {
        let x1 = p[0] * cy + p[2] * sy;
        let z1 = -p[0] * sy + p[2] * cy;
        [x1, p[1] * cp - z1 * sp, p[1] * sp + z1 * cp]
    };
    let vt: Vec<[V3; 3]> = tris.iter().map(|t| [view(t.p[0]), view(t.p[1]), view(t.p[2])]).collect();

    let (mut mn, mut mx) = ([f64::MAX; 2], [f64::MIN; 2]);
    for t in &vt {
        for v in t {
            for k in 0..2 {
                mn[k] = mn[k].min(v[k]);
                mx[k] = mx[k].max(v[k]);
            }
        }
    }
    if tris.is_empty() {
        return RgbaImage::new(size, size);
    }
    let span = (mx[0] - mn[0]).max(mx[1] - mn[1]).max(1e-9);
    let scale = n as f64 * 0.88 / span;
    let off = [
        n as f64 / 2.0 - (mn[0] + mx[0]) / 2.0 * scale,
        n as f64 / 2.0 + (mn[1] + mx[1]) / 2.0 * scale, // screen y points down
    ];
    let to_screen = |v: V3| [v[0] * scale + off[0], off[1] - v[1] * scale, v[2]];

    let mut img = RgbaImage::new(n as u32, n as u32);
    let mut depth = vec![f64::MIN; n * n];
    let light = normalize([0.35, 0.65, 0.7]);
    let (aw, ah) = atlas.dimensions();

    for (t, v) in tris.iter().zip(&vt) {
        let s = [to_screen(v[0]), to_screen(v[1]), to_screen(v[2])];
        let mut nrm = normalize(cross(sub(v[1], v[0]), sub(v[2], v[0])));
        if nrm[2] < 0.0 {
            nrm = mul(nrm, -1.0); // two-sided
        }
        let shade = 0.42 + 0.58 * dot(nrm, light).max(0.0);
        let area = (s[1][0] - s[0][0]) * (s[2][1] - s[0][1]) - (s[2][0] - s[0][0]) * (s[1][1] - s[0][1]);
        if area.abs() < 1e-12 {
            continue;
        }
        let x0 = s.iter().map(|p| p[0]).fold(f64::MAX, f64::min).floor().max(0.0) as usize;
        let x1 = (s.iter().map(|p| p[0]).fold(f64::MIN, f64::max).ceil() as usize).min(n - 1);
        let y0 = s.iter().map(|p| p[1]).fold(f64::MAX, f64::min).floor().max(0.0) as usize;
        let y1 = (s.iter().map(|p| p[1]).fold(f64::MIN, f64::max).ceil() as usize).min(n - 1);
        for y in y0..=y1 {
            for x in x0..=x1 {
                let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                let w0 = ((s[1][0] - px) * (s[2][1] - py) - (s[2][0] - px) * (s[1][1] - py)) / area;
                let w1 = ((s[2][0] - px) * (s[0][1] - py) - (s[0][0] - px) * (s[2][1] - py)) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 < -1e-9 || w1 < -1e-9 || w2 < -1e-9 {
                    continue;
                }
                let z = w0 * s[0][2] + w1 * s[1][2] + w2 * s[2][2];
                if z <= depth[y * n + x] {
                    continue;
                }
                let u = w0 * t.uv[0][0] + w1 * t.uv[1][0] + w2 * t.uv[2][0];
                let vv = w0 * t.uv[0][1] + w1 * t.uv[1][1] + w2 * t.uv[2][1];
                let tx = ((u * aw as f64) as i64).clamp(0, aw as i64 - 1) as u32;
                let ty = (((1.0 - vv) * ah as f64) as i64).clamp(0, ah as i64 - 1) as u32;
                let c = atlas.get_pixel(tx, ty);
                depth[y * n + x] = z;
                let f = |c: u8| ((c as f64 * shade).round().min(255.0)) as u8;
                img.put_pixel(x as u32, y as u32, Rgba([f(c[0]), f(c[1]), f(c[2]), 255]));
            }
        }
    }
    // premultiplied reduce so the silhouette edge stays clean
    crate::atlas::resize_rgba(&img, size, size)
}
