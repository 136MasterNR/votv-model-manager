//! Swatches, atlas packing and the matching emissive / normal / pbr atlases.

use image::imageops::{self, FilterType};
use image::{Rgba, RgbaImage};

use crate::geom::round_py;
use crate::scene::{Material, Tex};

pub const PAD: u32 = 4;

/// Identity of one atlas slot. Two parts with equal keys share a tile.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum SwatchKey {
    /// A part with no material at all.
    Default,
    Material(MatKey),
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct MatKey {
    name: String,
    base: Option<u64>,
    factor: Option<[u8; 4]>,
    emissive: Option<u64>,
    emissive_factor: Option<[u64; 3]>,
    normal: Option<u64>,
    mr: Option<u64>,
    rough: Option<u64>,
    metal: Option<u64>,
    alpha_mode: Option<String>,
}

pub fn material_key(mat: Option<&Material>, textures: &[Option<Tex>]) -> SwatchKey {
    let Some(m) = mat else { return SwatchKey::Default };
    let h = |i: Option<usize>| i.and_then(|i| textures[i].as_ref()).map(|t| t.hash);
    SwatchKey::Material(MatKey {
        name: m.name.clone(),
        base: h(m.base_tex),
        factor: m.base_factor,
        emissive: h(m.emissive_tex),
        emissive_factor: m.emissive_factor.map(|f| [f[0].to_bits(), f[1].to_bits(), f[2].to_bits()]),
        normal: h(m.normal_tex),
        mr: h(m.mr_tex),
        rough: m.roughness.map(f64::to_bits),
        metal: m.metallic.map(f64::to_bits),
        alpha_mode: m.alpha_mode.clone(),
    })
}

fn tex<'a>(textures: &'a [Option<Tex>], i: Option<usize>) -> Option<&'a Tex> {
    i.and_then(|i| textures.get(i)).and_then(|t| t.as_ref())
}

fn solid(w: u32, h: u32, c: [u8; 4]) -> RgbaImage {
    RgbaImage::from_pixel(w, h, Rgba(c))
}

/// Linear (0-1) to an 8-bit sRGB value.
pub fn srgb_encode(lin: f64) -> u8 {
    let c = lin.clamp(0.0, 1.0);
    let v = if c <= 0.003_130_8 { 12.92 * c } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
    round_py(v * 255.0).clamp(0.0, 255.0) as u8
}

/// 8-bit sRGB to linear (0-1).
pub fn srgb_decode(v: u8) -> f64 {
    let c = v as f64 / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// Multiply an sRGB texture by a linear colour factor the way a renderer does (decode, multiply, re-encode); alpha is linear.
pub fn scale_srgb(img: &mut RgbaImage, factor: [f64; 3], alpha: f64) {
    let mut lut = [[0u8; 256]; 3];
    for ch in 0..3 {
        for v in 0..256 {
            lut[ch][v] = srgb_encode(srgb_decode(v as u8) * factor[ch]);
        }
    }
    for p in img.pixels_mut() {
        for ch in 0..3 {
            p[ch] = lut[ch][p[ch] as usize];
        }
        p[3] = round_py(p[3] as f64 * alpha).clamp(0.0, 255.0) as u8;
    }
}

/// Resize with premultiplied alpha, like Pillow does for RGBA (no dark fringes at transparent edges).
pub fn resize_rgba(img: &RgbaImage, w: u32, h: u32) -> RgbaImage {
    if img.dimensions() == (w, h) {
        return img.clone();
    }
    let mut pm = img.clone();
    for p in pm.pixels_mut() {
        let a = p[3] as u32;
        if a != 255 {
            for c in 0..3 {
                p[c] = ((p[c] as u32 * a + 127) / 255) as u8;
            }
        }
    }
    let mut out = imageops::resize(&pm, w, h, FilterType::Lanczos3);
    for p in out.pixels_mut() {
        let a = p[3] as u32;
        if a != 255 && a != 0 {
            for c in 0..3 {
                p[c] = ((p[c] as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    out
}

/// Pillow's `Image.thumbnail` size rule (keeps aspect, never enlarges).
pub fn thumbnail_size(w: u32, h: u32, max: u32) -> (u32, u32) {
    if max >= w && max >= h {
        return (w, h);
    }
    let aspect = w as f64 / h as f64;
    let (mut x, mut y) = (max as f64, max as f64);
    let pick = |v: f64, err: &dyn Fn(f64) -> f64| -> f64 {
        let (lo, hi) = (v.floor(), v.ceil());
        let r = if err(lo) <= err(hi) { lo } else { hi };
        r.max(1.0)
    };
    if x / y >= aspect {
        x = pick(y * aspect, &|n| (aspect - n / y).abs());
    } else {
        let xx = x;
        y = pick(xx / aspect, &|n| if n == 0.0 { 0.0 } else { (aspect - xx / n).abs() });
    }
    (x as u32, y as u32)
}

/// (swatch, has_real_uv). `apply_factor` multiplies a base texture by `baseColorFactor`.
pub fn material_swatch(mat: Option<&Material>, textures: &[Option<Tex>], apply_factor: bool) -> (RgbaImage, bool) {
    if let Some(m) = mat {
        let keeps_alpha = matches!(m.alpha_mode.as_deref(), Some("MASK") | Some("BLEND"));
        if let Some(t) = tex(textures, m.base_tex) {
            let mut img = t.img.clone();
            if let Some(f) = m.base_linear {
                // glTF: colour = texture x factor (linear). The *alpha* factor is part of the material's opacity, so for BLEND/MASK materials it is
                // always baked into the tile: VotV clips alpha (about 1/3), so a 24%-opaque clear-coat shell must disappear instead of becoming a solid skin.
                let colour = apply_factor && f[..3].iter().any(|c| (*c - 1.0).abs() > 1e-4);
                let alpha = keeps_alpha && (f[3] - 1.0).abs() > 1e-4;
                if colour || alpha {
                    let rgb = if colour { [f[0], f[1], f[2]] } else { [1.0, 1.0, 1.0] };
                    scale_srgb(&mut img, rgb, if alpha { f[3] } else { 1.0 });
                }
            }
            // OPAQUE materials ignore alpha everywhere except VotV, which would clip stray alpha into holes
            if t.had_alpha && !keeps_alpha {
                for p in img.pixels_mut() {
                    p[3] = 255;
                }
            }
            return (img, true);
        }
        if let Some(l) = m.base_linear {
            // the factor is linear; the atlas is sRGB (a naive x255 made dark paints nearly black)
            let a = if keeps_alpha { round_py(l[3] * 255.0).clamp(0.0, 255.0) as u8 } else { 255 };
            return (solid(64, 64, [srgb_encode(l[0]), srgb_encode(l[1]), srgb_encode(l[2]), a]), false);
        }
    }
    (solid(64, 64, [200, 200, 200, 255]), false)
}

pub fn pad_edge(img: &RgbaImage, pad: u32) -> RgbaImage {
    let (w, h) = img.dimensions();
    let mut out = RgbaImage::new(w + 2 * pad, h + 2 * pad);
    for y in 0..h + 2 * pad {
        let sy = y.saturating_sub(pad).min(h - 1);
        for x in 0..w + 2 * pad {
            let sx = x.saturating_sub(pad).min(w - 1);
            out.put_pixel(x, y, *img.get_pixel(sx, sy));
        }
    }
    out
}

#[derive(Clone, Debug)]
pub struct Placement {
    /// Tile origin inside the atlas (image coordinates, y down), excluding padding.
    pub x: u32,
    pub y_top: u32,
    pub w: u32,
    pub h: u32,
    pub atlas_w: u32,
    pub atlas_h: u32,
}

/// Shelf packing, identical to the Python version (tiles sorted by height, stable).
pub fn pack(swatches: &[RgbaImage]) -> (RgbaImage, Vec<Placement>) {
    let mut order: Vec<usize> = (0..swatches.len()).collect();
    order.sort_by(|&a, &b| swatches[b].height().cmp(&swatches[a].height()));
    let padded: Vec<RgbaImage> = order.iter().map(|&i| pad_edge(&swatches[i], PAD)).collect();

    let total_area: u64 = padded.iter().map(|p| p.width() as u64 * p.height() as u64).sum();
    let max_w = padded.iter().map(|p| p.width()).max().unwrap_or(1);
    let atlas_w = max_w.max(((total_area as f64).sqrt() as u32).max(1));

    let (mut xc, mut yc, mut shelf_h) = (0u32, 0u32, 0u32);
    let mut raw: Vec<(u32, u32)> = Vec::new();
    for p in &padded {
        let (pw, ph) = p.dimensions();
        if xc + pw > atlas_w && xc > 0 {
            xc = 0;
            yc += shelf_h;
            shelf_h = 0;
        }
        raw.push((xc, yc));
        xc += pw;
        shelf_h = shelf_h.max(ph);
    }
    let atlas_h = yc + shelf_h;

    let mut atlas = RgbaImage::from_pixel(atlas_w, atlas_h.max(1), Rgba([0, 0, 0, 0]));
    let mut placements = vec![None; swatches.len()];
    for (slot, &i) in order.iter().enumerate() {
        let (px, py) = raw[slot];
        imageops::replace(&mut atlas, &padded[slot], px as i64, py as i64);
        placements[i] = Some(Placement {
            x: px + PAD,
            y_top: py + PAD,
            w: swatches[i].width(),
            h: swatches[i].height(),
            atlas_w,
            atlas_h,
        });
    }
    (atlas, placements.into_iter().map(|p| p.unwrap()).collect())
}

// ---------- extra maps --------------------------------------------------------------------

pub fn has_emission(m: &Material) -> bool {
    m.emissive_tex.is_some() || m.emissive_factor.map(|f| f.iter().any(|c| *c >= 0.02)).unwrap_or(false)
}
pub fn has_normal(m: &Material) -> bool {
    m.normal_tex.is_some()
}
pub fn has_roughness(m: &Material) -> bool {
    m.mr_tex.is_some() || m.roughness.is_some()
}

fn emissive_tile(m: &Material, tx: &[Option<Tex>]) -> Option<RgbaImage> {
    if let Some(t) = tex(tx, m.emissive_tex) {
        let mut img = t.img.clone();
        // glTF: emission = texture x factor (an absent factor next to a texture is taken as 1)
        if let Some(f) = m.emissive_factor {
            if f.iter().any(|c| (*c - 1.0).abs() > 1e-3) {
                scale_srgb(&mut img, [f[0].clamp(0.0, 1.0), f[1].clamp(0.0, 1.0), f[2].clamp(0.0, 1.0)], 1.0);
            }
        }
        return Some(img);
    }
    let f = m.emissive_factor?;
    if f.iter().any(|c| *c > 0.0) {
        return Some(solid(64, 64, [srgb_encode(f[0]), srgb_encode(f[1]), srgb_encode(f[2]), 255]));
    }
    None
}

fn normal_tile(m: &Material, tx: &[Option<Tex>], flip_green: bool) -> Option<RgbaImage> {
    let t = tex(tx, m.normal_tex)?;
    let mut img = t.img.clone();
    for p in img.pixels_mut() {
        if flip_green {
            p[1] = 255 - p[1];
        }
        p[3] = 255;
    }
    Some(img)
}

/// VotV layout: R = metallic, G = specular (constant 0.5), B = roughness.
/// glTF layout is G = roughness, B = metallic, so channels are re-shuffled. A material with only
/// factors contributes roughness only (metallic stays 0 on purpose, see lessons).
fn pbr_tile(m: &Material, tx: &[Option<Tex>], metallic_scale: f64) -> Option<RgbaImage> {
    let spec = 128u8;
    if let Some(t) = tex(tx, m.mr_tex) {
        let rf = m.roughness.map(|v| v.clamp(0.0, 1.0)).unwrap_or(1.0);
        let mf = m.metallic.map(|v| v.clamp(0.0, 1.0)).unwrap_or(1.0) * metallic_scale.clamp(0.0, 1.0);
        let mut img = t.img.clone();
        for p in img.pixels_mut() {
            let (g, b) = (p[1], p[2]);
            p[0] = round_py(b as f64 * mf) as u8;
            p[1] = spec;
            p[2] = round_py(g as f64 * rf) as u8;
            p[3] = 255;
        }
        return Some(img);
    }
    let rf = m.roughness?;
    let v = round_py(rf * 255.0).clamp(0.0, 255.0) as u8;
    Some(solid(64, 64, [0, spec, v, 255]))
}

pub enum MapKind {
    Emissive,
    Normal { flip_green: bool },
    /// `metallic_scale` multiplies every metallic value written (1 = as authored).
    Pbr { metallic_scale: f64 },
}

/// Build one extra atlas with the same layout as the diffuse atlas. Returns `None` when no
/// material has data for it. `mats[i]` is the material of slot `i` (`None` for flat/default slots).
pub fn build_map(
    kind: MapKind,
    mats: &[Option<&Material>],
    placements: &[Placement],
    textures: &[Option<Tex>],
) -> Option<RgbaImage> {
    let any = mats.iter().flatten().any(|m| match kind {
        MapKind::Emissive => has_emission(m),
        MapKind::Normal { .. } => has_normal(m),
        MapKind::Pbr { .. } => has_roughness(m),
    });
    if !any {
        return None;
    }
    let (aw, ah) = (placements[0].atlas_w, placements[0].atlas_h);
    let fill = match kind {
        MapKind::Emissive => [0, 0, 0, 255],
        MapKind::Normal { .. } => [128, 128, 255, 255],
        MapKind::Pbr { .. } => [0, 128, 128, 255],
    };
    let mut atlas = solid(aw, ah, fill);
    for (m, p) in mats.iter().zip(placements) {
        let Some(m) = m else { continue };
        let tile = match kind {
            MapKind::Emissive => emissive_tile(m, textures),
            MapKind::Normal { flip_green } => normal_tile(m, textures, flip_green),
            MapKind::Pbr { metallic_scale } => pbr_tile(m, textures, metallic_scale),
        };
        let Some(tile) = tile else { continue };
        let tile = if tile.dimensions() != (p.w, p.h) { resize_rgba(&tile, p.w, p.h) } else { tile };
        let padded = pad_edge(&tile, PAD);
        imageops::replace(&mut atlas, &padded, p.x as i64 - PAD as i64, p.y_top as i64 - PAD as i64);
    }
    Some(atlas)
}

pub fn shrink_to_fit(img: RgbaImage, max: u32) -> (RgbaImage, Option<((u32, u32), (u32, u32))>) {
    let (w, h) = img.dimensions();
    if w <= max && h <= max {
        return (img, None);
    }
    let ratio = (max as f64 / w as f64).min(max as f64 / h as f64);
    let nw = ((w as f64 * ratio) as u32).max(1);
    let nh = ((h as f64 * ratio) as u32).max(1);
    (resize_rgba(&img, nw, nh), Some(((w, h), (nw, nh))))
}
