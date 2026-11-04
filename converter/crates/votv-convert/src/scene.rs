//! Reading a .glb into plain data: parts (world-space triangle meshes), materials and textures.
//!
//! The behaviour deliberately mirrors what the Python version got from trimesh:
//! * the default scene only, every mesh primitive becomes one part with its node transform baked in;
//! * material factors are `None` when the file does not state them (this matters: a missing
//!   `roughnessFactor` means "no pbr data", an explicit 1.0 means "roughness 255");
//! * the base colour factor is converted to 0-255 integers;
//! * only UV set 0 is used;
//! * a node named `UCX_*` (or, without a node name, a mesh named so) is collision, not geometry.

use std::collections::HashMap;
use std::path::Path;

use image::RgbaImage;

use crate::error::{ConvertError, Result};
use crate::geom::*;
use crate::report::{Ctx, DiagCode};

pub struct Tex {
    pub img: RgbaImage,
    /// The source image had an alpha channel (PIL mode "RGBA").
    pub had_alpha: bool,
    pub hash: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Material {
    pub name: String,
    pub base_tex: Option<usize>,
    /// 0-255 like trimesh; `None` when the file does not state it.
    pub base_factor: Option<[u8; 4]>,
    pub alpha_mode: Option<String>,
    pub emissive_tex: Option<usize>,
    pub emissive_factor: Option<[f64; 3]>,
    pub normal_tex: Option<usize>,
    pub mr_tex: Option<usize>,
    pub roughness: Option<f64>,
    pub metallic: Option<f64>,
}

#[derive(Clone)]
pub struct Part {
    pub name: String,
    pub material: Option<usize>,
    pub pos: Vec<V3>,
    pub nrm: Vec<V3>,
    pub uv: Option<Vec<[f64; 2]>>,
    pub tris: Vec<[u32; 3]>,
}

pub struct Scene {
    pub parts: Vec<Part>,
    pub collision_parts: Vec<Part>,
    pub materials: Vec<Material>,
    pub textures: Vec<Option<Tex>>,
    pub skin_count: usize,
    pub animation_count: usize,
    /// Number of skinned primitives that were repositioned (humanoid mode only).
    pub skinned_repositioned: usize,
    pub skinned_primitives: usize,
}

pub use votv_model_core::is_ucx_name;

fn fnv(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for chunk in bytes.chunks(8) {
        let mut v = 0u64;
        for (i, b) in chunk.iter().enumerate() {
            v |= (*b as u64) << (8 * i);
        }
        h ^= v;
        h = h.wrapping_mul(0x100000001b3);
        h ^= h >> 29;
    }
    h
}

fn convert_image(d: &gltf::image::Data) -> Option<Tex> {
    use gltf::image::Format::*;
    let (w, h) = (d.width, d.height);
    let n = (w as usize) * (h as usize);
    let px = &d.pixels;
    let hi = |i: usize| px[i * 2 + 1]; // 16-bit little endian: take the high byte
    let mut out = Vec::with_capacity(n * 4);
    let mut had_alpha = false;
    match d.format {
        R8 => (0..n).for_each(|i| out.extend_from_slice(&[px[i], px[i], px[i], 255])),
        R8G8 => (0..n)
            .for_each(|i| out.extend_from_slice(&[px[i * 2], px[i * 2], px[i * 2], px[i * 2 + 1]])),
        R8G8B8 => (0..n)
            .for_each(|i| out.extend_from_slice(&[px[i * 3], px[i * 3 + 1], px[i * 3 + 2], 255])),
        R8G8B8A8 => {
            out.extend_from_slice(px);
            had_alpha = true;
        }
        R16 => (0..n).for_each(|i| out.extend_from_slice(&[hi(i), hi(i), hi(i), 255])),
        R16G16 => (0..n)
            .for_each(|i| out.extend_from_slice(&[hi(i * 2), hi(i * 2), hi(i * 2), hi(i * 2 + 1)])),
        R16G16B16 => (0..n)
            .for_each(|i| out.extend_from_slice(&[hi(i * 3), hi(i * 3 + 1), hi(i * 3 + 2), 255])),
        R16G16B16A16 => {
            (0..n).for_each(|i| {
                out.extend_from_slice(&[hi(i * 4), hi(i * 4 + 1), hi(i * 4 + 2), hi(i * 4 + 3)])
            });
            had_alpha = true;
        }
        _ => return None, // float formats: not used by glTF 2.0 core
    }
    let hash = fnv(&out) ^ ((w as u64) << 32 | h as u64);
    RgbaImage::from_raw(w, h, out).map(|img| Tex {
        img,
        had_alpha,
        hash,
    })
}

fn json_f64(v: &serde_json::Value) -> Option<f64> {
    v.as_f64()
}

fn read_materials(
    doc: &gltf::Document,
    raw: &serde_json::Value,
    textures: &[Option<Tex>],
    ctx: &mut Ctx,
) -> Vec<Material> {
    let raw_mats = raw.get("materials").and_then(|m| m.as_array());
    let mut out = Vec::new();
    let mut spec_gloss = 0usize;
    for (i, m) in doc.materials().enumerate() {
        let rm = raw_mats.and_then(|a| a.get(i));
        let pbr_raw = rm.and_then(|x| x.get("pbrMetallicRoughness"));
        let pbr = m.pbr_metallic_roughness();
        let name = m
            .name()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "material".to_string());

        let tex_of = |t: Option<gltf::texture::Info>,
                      label: &str,
                      ctx: &mut Ctx|
         -> Option<usize> {
            let t = t?;
            if t.tex_coord() != 0 {
                ctx.warn(DiagCode::UvSetIgnored, format!(
                    "material {name:?}: its {label} texture uses UV set {}; only UV set 0 is supported, so it may look wrong.",
                    t.tex_coord()));
            }
            if t.texture_transform().is_some() {
                ctx.warn(DiagCode::TextureTransformIgnored, format!(
                    "material {name:?}: its {label} texture uses KHR_texture_transform, which is not applied."));
            }
            let idx = t.texture().source().index();
            textures.get(idx).and_then(|x| x.as_ref()).map(|_| idx)
        };

        let mut base_tex = tex_of(pbr.base_color_texture(), "base colour", ctx);
        let mr_tex = tex_of(pbr.metallic_roughness_texture(), "metallic-roughness", ctx);
        let emissive_tex = tex_of(m.emissive_texture(), "emissive", ctx);
        let normal_tex = m.normal_texture().and_then(|n| {
            if n.tex_coord() != 0 {
                ctx.warn(DiagCode::UvSetIgnored, format!("material {name:?}: its normal texture uses UV set {}; only UV set 0 is supported.", n.tex_coord()));
            }
            let idx = n.texture().source().index();
            textures.get(idx).and_then(|x| x.as_ref()).map(|_| idx)
        });

        let base_factor = pbr_raw
            .and_then(|p| p.get("baseColorFactor"))
            .and_then(|v| v.as_array())
            .filter(|a| a.len() == 4)
            .map(|a| {
                let mut f = [0u8; 4];
                for k in 0..4 {
                    f[k] = (json_f64(&a[k]).unwrap_or(1.0) * 255.0)
                        .round()
                        .clamp(0.0, 255.0) as u8;
                }
                f
            });
        // Older KHR_materials_pbrSpecularGlossiness materials: use the diffuse texture / colour as base colour (what trimesh
        // did). Specular and glossiness are not converted.
        let mut base_factor = base_factor;
        if let Some(sg) = rm
            .and_then(|x| x.get("extensions"))
            .and_then(|e| e.get("KHR_materials_pbrSpecularGlossiness"))
        {
            spec_gloss += 1;
            if base_tex.is_none() {
                if let Some(ti) = sg.get("diffuseTexture") {
                    if ti.get("texCoord").and_then(|v| v.as_u64()).unwrap_or(0) != 0 {
                        ctx.warn(DiagCode::UvSetIgnored, format!("material {name:?}: its diffuse texture uses a UV set other than 0; only UV set 0 is supported."));
                    }
                    base_tex = ti
                        .get("index")
                        .and_then(|v| v.as_u64())
                        .and_then(|i| doc.textures().nth(i as usize))
                        .map(|t| t.source().index())
                        .filter(|&i| textures.get(i).map(|x| x.is_some()).unwrap_or(false));
                }
            }
            if base_factor.is_none() {
                base_factor = sg
                    .get("diffuseFactor")
                    .and_then(|v| v.as_array())
                    .filter(|a| a.len() == 4)
                    .map(|a| {
                        let mut f = [0u8; 4];
                        for k in 0..4 {
                            f[k] = (json_f64(&a[k]).unwrap_or(1.0) * 255.0)
                                .round()
                                .clamp(0.0, 255.0) as u8;
                        }
                        f
                    });
            }
        }
        let roughness = pbr_raw
            .and_then(|p| p.get("roughnessFactor"))
            .and_then(json_f64);
        let metallic = pbr_raw
            .and_then(|p| p.get("metallicFactor"))
            .and_then(json_f64);
        let emissive_factor = rm
            .and_then(|x| x.get("emissiveFactor"))
            .and_then(|v| v.as_array())
            .filter(|a| a.len() == 3)
            .map(|a| {
                [
                    json_f64(&a[0]).unwrap_or(0.0),
                    json_f64(&a[1]).unwrap_or(0.0),
                    json_f64(&a[2]).unwrap_or(0.0),
                ]
            });
        let alpha_mode = rm
            .and_then(|x| x.get("alphaMode"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        out.push(Material {
            name,
            base_tex,
            base_factor,
            alpha_mode,
            emissive_tex,
            emissive_factor,
            normal_tex,
            mr_tex,
            roughness,
            metallic,
        });
    }
    if spec_gloss > 0 {
        ctx.info(DiagCode::SpecGlossConverted, format!("{spec_gloss} material(s) use the old specular-glossiness format: their diffuse texture/colour is used as the base colour; specular and glossiness are ignored."));
    }
    out
}

/// Smooth, angle-weighted vertex normals (what trimesh produces when a file has none), computed on
/// final positions. `weld` maps each vertex to a group id so vertices at the same place share a normal.
fn computed_normals(pos: &[V3], tris: &[[u32; 3]], weld: &[usize]) -> Vec<V3> {
    let ngroups = weld.iter().copied().max().map(|m| m + 1).unwrap_or(0);
    let mut acc = vec![[0.0f64; 3]; ngroups];
    for t in tris {
        let p = [pos[t[0] as usize], pos[t[1] as usize], pos[t[2] as usize]];
        let n = normalize(cross(sub(p[1], p[0]), sub(p[2], p[0])));
        if len(n) < 0.5 {
            continue;
        }
        for k in 0..3 {
            let a = normalize(sub(p[(k + 1) % 3], p[k]));
            let b = normalize(sub(p[(k + 2) % 3], p[k]));
            let ang = dot(a, b).clamp(-1.0, 1.0).acos();
            let g = weld[t[k] as usize];
            acc[g] = add(acc[g], mul(n, ang));
        }
    }
    (0..pos.len())
        .map(|i| {
            let n = normalize(acc[weld[i]]);
            if len(n) < 0.5 {
                [0.0, 0.0, 1.0]
            } else {
                n
            }
        })
        .collect()
}

fn triangulate(mode: gltf::mesh::Mode, idx: &[u32]) -> Vec<[u32; 3]> {
    use gltf::mesh::Mode::*;
    let mut t = Vec::new();
    match mode {
        Triangles => idx.chunks_exact(3).for_each(|c| t.push([c[0], c[1], c[2]])),
        TriangleStrip => {
            for i in 0..idx.len().saturating_sub(2) {
                if i % 2 == 0 {
                    t.push([idx[i], idx[i + 1], idx[i + 2]])
                } else {
                    t.push([idx[i + 1], idx[i], idx[i + 2]])
                }
            }
        }
        TriangleFan => {
            for i in 1..idx.len().saturating_sub(1) {
                t.push([idx[0], idx[i], idx[i + 1]]);
            }
        }
        _ => {}
    }
    t
}

struct Walk<'a> {
    doc: &'a gltf::Document,
    buffers: &'a [gltf::buffer::Data],
    world: HashMap<usize, M4>,
}

fn compute_world(doc: &gltf::Document) -> (HashMap<usize, M4>, Vec<usize>) {
    let mut world = HashMap::new();
    let roots: Vec<gltf::Node> = match doc.default_scene().or_else(|| doc.scenes().next()) {
        Some(s) => s.nodes().collect(),
        None => {
            let children: std::collections::HashSet<usize> = doc
                .nodes()
                .flat_map(|n| n.children().map(|c| c.index()).collect::<Vec<_>>())
                .collect();
            doc.nodes()
                .filter(|n| !children.contains(&n.index()))
                .collect()
        }
    };
    // Visit order matches trimesh's glTF loader (a LIFO stack: last root first, then its children
    // last-first). It decides atlas tile order and which duplicate is kept, so staying identical
    // keeps the output comparable with the Python version.
    let mut order = Vec::new();
    let mut stack: Vec<(M4, gltf::Node)> = roots.into_iter().map(|r| (IDENTITY, r)).collect();
    let mut seen = std::collections::HashSet::new();
    while let Some((parent, n)) = stack.pop() {
        if !seen.insert(n.index()) {
            continue;
        }
        let m = mat_mul(&parent, &from_gltf(n.transform().matrix()));
        world.insert(n.index(), m);
        order.push(n.index());
        for c in n.children() {
            stack.push((m, c));
        }
    }
    (world, order)
}

/// Joint matrices (joint world * inverse bind) per skin, in file order.
fn skin_matrices(w: &Walk) -> Vec<Vec<M4>> {
    w.doc
        .skins()
        .map(|skin| {
            let reader = skin.reader(|b| Some(&w.buffers[b.index()]));
            let ibms: Vec<M4> = reader
                .read_inverse_bind_matrices()
                .map(|it| it.map(from_gltf).collect())
                .unwrap_or_default();
            skin.joints()
                .enumerate()
                .map(|(i, j)| {
                    let jw = w.world.get(&j.index()).copied().unwrap_or(IDENTITY);
                    mat_mul(&jw, ibms.get(i).unwrap_or(&IDENTITY))
                })
                .collect()
        })
        .collect()
}

/// Rebuild the GLB without `extensionsRequired` (see `load`). Returns the new bytes and the names that were required.
fn strip_required_extensions(
    glb: &gltf::binary::Glb,
    raw: &serde_json::Value,
) -> Result<(Vec<u8>, Vec<String>)> {
    let mut v = raw.clone();
    let names: Vec<String> = v
        .get("extensionsRequired")
        .and_then(|a| a.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    if let Some(o) = v.as_object_mut() {
        o.remove("extensionsRequired");
    }
    let json = serde_json::to_vec(&v).map_err(|e| ConvertError::Unreadable(e.to_string()))?;
    let g = gltf::binary::Glb {
        header: gltf::binary::Header {
            magic: *b"glTF",
            version: 2,
            length: 0,
        },
        json: std::borrow::Cow::Owned(json),
        bin: glb.bin.clone(),
    };
    let bytes = g
        .to_vec()
        .map_err(|e| ConvertError::Unreadable(e.to_string()))?;
    Ok((bytes, names))
}

pub fn load(path: &Path, humanoid: bool, ctx: &mut Ctx) -> Result<Scene> {
    let bytes = std::fs::read(path)?;
    let glb = gltf::binary::Glb::from_slice(&bytes)
        .map_err(|e| ConvertError::Unreadable(format!("bad .glb header: {e}")))?;
    let raw: serde_json::Value = serde_json::from_slice(&glb.json)
        .map_err(|e| ConvertError::Unreadable(format!("bad JSON chunk: {e}")))?;
    let (doc, buffers, images) = match gltf::import_slice(&bytes) {
        Ok(x) => x,
        Err(e) if e.to_string().contains("Unsupported extension") => {
            // The file *requires* an extension the gltf crate does not know. The converter only reads standard
            // geometry, skins and textures, so treat those requirements as optional and try again.
            let (fixed, names) = strip_required_extensions(&glb, &raw)?;
            ctx.warn(DiagCode::UnsupportedExtensionIgnored, format!(
                "this file requires the glTF extension(s) {}, which this converter does not understand. They were ignored; materials may look different from the original.",
                names.join(", ")));
            gltf::import_slice(&fixed).map_err(|e| ConvertError::Unreadable(e.to_string()))?
        }
        Err(e) => return Err(ConvertError::Unreadable(e.to_string())),
    };

    let textures: Vec<Option<Tex>> = images.iter().map(convert_image).collect();
    ctx.check_cancel()?;
    let materials = read_materials(&doc, &raw, &textures, ctx);

    let (world, order) = compute_world(&doc);
    let walk = Walk {
        doc: &doc,
        buffers: &buffers,
        world,
    };
    let skins = if humanoid {
        skin_matrices(&walk)
    } else {
        Vec::new()
    };

    let mut parts = Vec::new();
    let mut collision_parts = Vec::new();
    let mut skinned_repositioned = 0usize;
    let mut skinned_primitives = 0usize;

    for ni in order {
        let node = doc.nodes().nth(ni).unwrap();
        let Some(mesh) = node.mesh() else { continue };
        let m = walk.world[&ni];
        let display = node
            .name()
            .filter(|s| !s.is_empty())
            .or_else(|| mesh.name())
            .unwrap_or("")
            .to_string();
        let flip = det3(&m) < 0.0;
        let nmat = normal_matrix(&m);

        for prim in mesh.primitives() {
            use gltf::mesh::Mode::*;
            if !matches!(prim.mode(), Triangles | TriangleStrip | TriangleFan) {
                continue;
            }
            let reader = prim.reader(|b| Some(&buffers[b.index()]));
            let Some(local_pos) = reader
                .read_positions()
                .map(|p| p.collect::<Vec<[f32; 3]>>())
            else {
                continue;
            };
            let idx: Vec<u32> = match reader.read_indices() {
                Some(i) => i.into_u32().collect(),
                None => (0..local_pos.len() as u32).collect(),
            };
            let mut tris = triangulate(prim.mode(), &idx);
            let nv = local_pos.len() as u32;
            tris.retain(|t| t.iter().all(|&i| i < nv));
            if tris.is_empty() {
                continue;
            }
            let file_normals: Option<Vec<[f32; 3]>> = reader.read_normals().map(|n| n.collect());
            let uv: Option<Vec<[f64; 2]>> = reader.read_tex_coords(0).map(|t| {
                t.into_f32()
                    .map(|u| [u[0] as f64, 1.0 - u[1] as f64])
                    .collect()
            }); // glTF V runs down, OBJ V runs up (trimesh flips too)
            let uv = uv.filter(|u| u.len() == local_pos.len());

            let skinned_data = if humanoid && node.skin().is_some() {
                match (reader.read_joints(0), reader.read_weights(0)) {
                    (Some(j), Some(w)) => Some((
                        j.into_u16().collect::<Vec<_>>(),
                        w.into_f32().collect::<Vec<_>>(),
                    )),
                    _ => None,
                }
            } else {
                None
            };
            if humanoid && skinned_data.is_some() {
                skinned_primitives += 1;
            }

            let (pos, mut nrm): (Vec<V3>, Option<Vec<V3>>) =
                if let (Some((joints, weights)), Some(skin)) = (skinned_data.as_ref(), node.skin())
                {
                    skinned_repositioned += 1;
                    let jm = &skins[skin.index()];
                    let mut pos = Vec::with_capacity(local_pos.len());
                    let mut nrm = file_normals
                        .as_ref()
                        .map(|_| Vec::with_capacity(local_pos.len()));
                    for i in 0..local_pos.len() {
                        let p = [
                            local_pos[i][0] as f64,
                            local_pos[i][1] as f64,
                            local_pos[i][2] as f64,
                        ];
                        let wsum: f64 = weights[i].iter().map(|w| *w as f64).sum::<f64>();
                        let wsum = if wsum == 0.0 { 1.0 } else { wsum };
                        let (mut acc, mut nacc) = ([0.0; 3], [0.0; 3]);
                        for k in 0..4 {
                            let w = weights[i][k] as f64 / wsum;
                            if w == 0.0 {
                                continue;
                            }
                            let jmat = jm.get(joints[i][k] as usize).unwrap_or(&IDENTITY);
                            acc = add(acc, mul(transform_point(jmat, p), w));
                            if let Some(fnorm) = &file_normals {
                                let n =
                                    [fnorm[i][0] as f64, fnorm[i][1] as f64, fnorm[i][2] as f64];
                                nacc = add(nacc, mul(transform_dir(jmat, n), w));
                            }
                        }
                        pos.push(acc);
                        if let Some(v) = nrm.as_mut() {
                            v.push(normalize(nacc));
                        }
                    }
                    (pos, nrm)
                } else {
                    let pos = local_pos
                        .iter()
                        .map(|p| transform_point(&m, [p[0] as f64, p[1] as f64, p[2] as f64]))
                        .collect();
                    let nrm = file_normals.as_ref().map(|ns| {
                        ns.iter()
                            .map(|n| {
                                normalize(mul3(&nmat, [n[0] as f64, n[1] as f64, n[2] as f64]))
                            })
                            .collect()
                    });
                    (pos, nrm)
                };

            if flip && skinned_data.is_none() {
                for t in tris.iter_mut() {
                    t.swap(1, 2);
                }
            }
            if nrm.is_none() {
                let mut ids: HashMap<[u32; 3], usize> = HashMap::new();
                let weld: Vec<usize> = local_pos
                    .iter()
                    .map(|p| {
                        let k = [p[0].to_bits(), p[1].to_bits(), p[2].to_bits()];
                        let n = ids.len();
                        *ids.entry(k).or_insert(n)
                    })
                    .collect();
                nrm = Some(computed_normals(&pos, &tris, &weld));
            }

            let part = Part {
                name: display.clone(),
                material: prim.material().index(),
                pos,
                nrm: nrm.unwrap(),
                uv,
                tris,
            };
            if is_ucx_name(&display) {
                collision_parts.push(part);
            } else {
                parts.push(part);
            }
        }
    }

    Ok(Scene {
        parts,
        collision_parts,
        materials,
        textures,
        skin_count: doc.skins().count(),
        animation_count: doc.animations().count(),
        skinned_repositioned,
        skinned_primitives,
    })
}
