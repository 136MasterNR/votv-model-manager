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

/// Copy `tile` into `dst` with its top-left padding corner at (`ox`, `oy`) and `pad` pixels of replicated edge on every side.
/// Same result as the old `pad_edge` followed by `imageops::replace`, but written row by row (memcpy for the middle of each row) with no
/// intermediate image. Anything outside `dst` is clipped, like `replace` does.
pub fn blit_padded(dst: &mut RgbaImage, tile: &RgbaImage, ox: u32, oy: u32, pad: u32) {
    let (w, h) = tile.dimensions();
    if w == 0 || h == 0 {
        return;
    }
    let (dw, dh) = dst.dimensions();
    let src: &[u8] = tile.as_raw();
    let raw: &mut [u8] = &mut *dst;
    let (wu, dwu) = (w as usize, dw as usize);
    for y in 0..h + 2 * pad {
        let dy = oy + y;
        if dy >= dh {
            break;
        }
        let sy = y.saturating_sub(pad).min(h - 1) as usize;
        let srow = &src[sy * wu * 4..(sy + 1) * wu * 4];
        let drow = &mut raw[dy as usize * dwu * 4..(dy as usize + 1) * dwu * 4];
        for x in 0..pad {
            let dx = ox + x;
            if dx < dw {
                drow[dx as usize * 4..dx as usize * 4 + 4].copy_from_slice(&srow[..4]);
            }
            let dx = ox + pad + w + x;
            if dx < dw {
                drow[dx as usize * 4..dx as usize * 4 + 4].copy_from_slice(&srow[(wu - 1) * 4..]);
            }
        }
        let mx = ox + pad;
        if mx < dw {
            let n = wu.min((dw - mx) as usize);
            drow[mx as usize * 4..(mx as usize + n) * 4].copy_from_slice(&srow[..n * 4]);
        }
    }
}

#[cfg(test)]
pub fn pad_edge(img: &RgbaImage, pad: u32) -> RgbaImage {
    let (w, h) = img.dimensions();
    let mut out = RgbaImage::new(w + 2 * pad, h + 2 * pad);
    blit_padded(&mut out, img, 0, 0, pad);
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
    // size of each tile with its padding; the tiles are written straight into the atlas below
    let dims: Vec<(u32, u32)> = order.iter().map(|&i| (swatches[i].width() + 2 * PAD, swatches[i].height() + 2 * PAD)).collect();

    let total_area: u64 = dims.iter().map(|d| d.0 as u64 * d.1 as u64).sum();
    let max_w = dims.iter().map(|d| d.0).max().unwrap_or(1);
    let atlas_w = max_w.max(((total_area as f64).sqrt() as u32).max(1));

    let (mut xc, mut yc, mut shelf_h) = (0u32, 0u32, 0u32);
    let mut raw: Vec<(u32, u32)> = Vec::new();
    for &(pw, ph) in &dims {
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
        blit_padded(&mut atlas, &swatches[i], px, py, PAD);
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
fn pbr_tile(m: &Material, tx: &[Option<Tex>], metallic_scale: f64, metallic_max: f64) -> Option<RgbaImage> {
    let spec = 128u8;
    if let Some(t) = tex(tx, m.mr_tex) {
        let rf = m.roughness.map(|v| v.clamp(0.0, 1.0)).unwrap_or(1.0);
        let mf = m.metallic.map(|v| v.clamp(0.0, 1.0)).unwrap_or(1.0) * metallic_scale.clamp(0.0, 1.0);
        let cap = round_py(metallic_max.clamp(0.0, 1.0) * 255.0) as u8;
        let mut img = t.img.clone();
        for p in img.pixels_mut() {
            let (g, b) = (p[1], p[2]);
            p[0] = (round_py(b as f64 * mf).min(255.0) as u8).min(cap);
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

#[derive(Clone, Copy, Debug)]
pub enum MapKind {
    Emissive,
    Normal { flip_green: bool },
    /// `metallic_scale` multiplies every metallic value written (1 = as authored).
    Pbr { metallic_scale: f64, metallic_max: f64 },
}

/// Does any material have data for this kind of extra map? (When none does, the map is not written at all.)
pub fn map_needed(kind: MapKind, mats: &[Option<&Material>]) -> bool {
    mats.iter().flatten().any(|m| match kind {
        MapKind::Emissive => has_emission(m),
        MapKind::Normal { .. } => has_normal(m),
        MapKind::Pbr { .. } => has_roughness(m),
    })
}

/// The tile of one slot for an extra map, already at the slot's size in the atlas. `None` when the material has nothing for this map
/// (the slot then keeps the map's neutral fill). Slots are independent, so the callers compute them in parallel.
pub fn map_tile(kind: MapKind, m: &Material, p: &Placement, textures: &[Option<Tex>]) -> Option<RgbaImage> {
    let tile = match kind {
        MapKind::Emissive => emissive_tile(m, textures),
        MapKind::Normal { flip_green } => normal_tile(m, textures, flip_green),
        MapKind::Pbr { metallic_scale, metallic_max } => pbr_tile(m, textures, metallic_scale, metallic_max),
    }?;
    Some(if tile.dimensions() != (p.w, p.h) { resize_rgba(&tile, p.w, p.h) } else { tile })
}

/// Lay the finished tiles of one extra map out in an atlas with the same layout as the diffuse atlas (`tiles[i]` belongs to `placements[i]`).
pub fn assemble_map(kind: MapKind, placements: &[Placement], tiles: &[Option<RgbaImage>]) -> RgbaImage {
    let (aw, ah) = (placements[0].atlas_w, placements[0].atlas_h);
    let fill = match kind {
        MapKind::Emissive => [0, 0, 0, 255],
        MapKind::Normal { .. } => [128, 128, 255, 255],
        MapKind::Pbr { .. } => [0, 128, 128, 255],
    };
    let mut atlas = solid(aw, ah, fill);
    for (tile, p) in tiles.iter().zip(placements) {
        if let Some(tile) = tile {
            blit_padded(&mut atlas, tile, p.x.saturating_sub(PAD), p.y_top.saturating_sub(PAD), PAD);
        }
    }
    atlas
}

/// Build one extra atlas with the same layout as the diffuse atlas, one slot after the other. Returns `None` when no
/// material has data for it. `mats[i]` is the material of slot `i` (`None` for flat/default slots). Built from
/// [`map_needed`], [`map_tile`] and [`assemble_map`].
pub fn build_map(kind: MapKind, mats: &[Option<&Material>], placements: &[Placement], textures: &[Option<Tex>]) -> Option<RgbaImage> {
    if !map_needed(kind, mats) {
        return None;
    }
    let tiles: Vec<Option<RgbaImage>> = mats.iter().zip(placements).map(|(m, p)| m.and_then(|m| map_tile(kind, m, p, textures))).collect();
    Some(assemble_map(kind, placements, &tiles))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The original per-pixel implementation, kept here as the oracle for `blit_padded`.
    fn pad_edge_slow(img: &RgbaImage, pad: u32) -> RgbaImage {
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

    fn noise(w: u32, h: u32, seed: u32) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| {
            let v = x.wrapping_mul(2654435761).wrapping_add(y.wrapping_mul(40503)).wrapping_add(seed.wrapping_mul(97));
            Rgba([v as u8, (v >> 8) as u8, (v >> 16) as u8, (v >> 24) as u8])
        })
    }

    #[test]
    fn blit_padded_equals_pad_edge_then_replace() {
        for &(w, h) in &[(1u32, 1u32), (1, 5), (7, 1), (3, 4), (16, 9), (64, 64)] {
            let tile = noise(w, h, w * 31 + h);
            assert_eq!(pad_edge(&tile, PAD), pad_edge_slow(&tile, PAD), "{w}x{h}");
            // inside a bigger atlas, at an offset, over existing content
            let (aw, ah) = (w + 2 * PAD + 5, h + 2 * PAD + 3);
            let mut a = noise(aw, ah, 1);
            let mut b = a.clone();
            blit_padded(&mut a, &tile, 3, 2, PAD);
            imageops::replace(&mut b, &pad_edge_slow(&tile, PAD), 3, 2);
            assert_eq!(a, b, "offset {w}x{h}");
        }
    }

    #[test]
    fn blit_padded_clips_like_replace() {
        let tile = noise(10, 6, 7);
        let mut a = noise(12, 9, 2);
        let mut b = a.clone();
        blit_padded(&mut a, &tile, 6, 4, PAD); // sticks out to the right and bottom
        imageops::replace(&mut b, &pad_edge_slow(&tile, PAD), 6, 4);
        assert_eq!(a, b);
    }

    #[test]
    fn pack_places_every_tile_with_its_edge_padding() {
        let sw = vec![noise(8, 8, 1), noise(5, 12, 2), noise(30, 3, 3)];
        let (atlas, pl) = pack(&sw);
        for (img, p) in sw.iter().zip(&pl) {
            for y in 0..img.height() {
                for x in 0..img.width() {
                    assert_eq!(atlas.get_pixel(p.x + x, p.y_top + y), img.get_pixel(x, y));
                }
            }
            // the padding repeats the edge pixel
            assert_eq!(atlas.get_pixel(p.x - 1, p.y_top), img.get_pixel(0, 0));
            assert_eq!(atlas.get_pixel(p.x + img.width(), p.y_top + img.height() - 1), img.get_pixel(img.width() - 1, img.height() - 1));
        }
    }
}
