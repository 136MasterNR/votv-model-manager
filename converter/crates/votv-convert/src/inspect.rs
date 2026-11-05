//! A quick look into a `.glb` without converting it, so a GUI can show what is inside and
//! pre-select sensible options.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::atlas;
use crate::convert::{material_is_transparent, material_transparency, Transparency};
use crate::error::{ConvertError, Result};
use crate::report::{Ctx, DiagCode, Diagnostic, Hooks};
use crate::scene;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaterialInfo {
    pub name: String,
    /// Size of the base colour texture in pixels, if it has one.
    pub base_texture: Option<(u32, u32)>,
    /// Translucent (BLEND with see-through alpha): what "drop transparent materials" removes.
    pub transparent: bool,
    /// Alpha cutout (MASK): kept by the converter; VotV clips alpha.
    pub cutout: bool,
    pub has_emission: bool,
    pub has_normal_map: bool,
    pub has_pbr: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Inspection {
    pub visible_parts: usize,
    pub triangles: usize,
    pub vertices: usize,
    /// Names of the model's own `UCX_*` collision objects.
    pub collision_objects: Vec<String>,
    pub materials: Vec<MaterialInfo>,
    pub skins: usize,
    pub animations: usize,
    /// Bounding box size in the file's units (metres for glTF) as [x, y, z].
    pub size: [f64; 3],
    /// The same size multiplied by the default scale of 100 (i.e. centimetres in VotV).
    pub size_in_votv_cm: [f64; 3],
    /// Triangles whose UVs run past one texture repeat (these get cut into more faces).
    pub tiled_triangles: usize,
    pub any_transparent: bool,
    /// Things worth telling the user (e.g. "rigged model, consider humanoid").
    pub hints: Vec<Diagnostic>,
}

pub fn inspect(input: &Path) -> Result<Inspection> {
    let mut input = input.to_path_buf();
    if input.extension().is_none() {
        input.set_extension("glb");
    }
    if !input.exists() {
        return Err(ConvertError::InputNotFound(input));
    }
    let ext = input.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if ext != "glb" {
        return Err(ConvertError::NotGlb(format!(".{ext}")));
    }
    let hooks = Hooks::default();
    let mut ctx = Ctx::new(&hooks);
    let sc = scene::load(&input, false, crate::options::EmissiveMode::Auto, &mut ctx)?;

    let (mut mn, mut mx) = ([f64::MAX; 3], [f64::MIN; 3]);
    let (mut tris, mut verts, mut tiled) = (0usize, 0usize, 0usize);
    for p in &sc.parts {
        tris += p.tris.len();
        verts += p.pos.len();
        for v in &p.pos {
            for k in 0..3 {
                mn[k] = mn[k].min(v[k]);
                mx[k] = mx[k].max(v[k]);
            }
        }
        let textured = p.material.and_then(|m| sc.materials[m].base_tex).is_some();
        if let (true, Some(uv)) = (textured, &p.uv) {
            for t in &p.tris {
                let u = [uv[t[0] as usize], uv[t[1] as usize], uv[t[2] as usize]];
                let span = |a: usize| {
                    let lo = u.iter().map(|c| c[a]).fold(f64::MAX, f64::min);
                    let hi = u.iter().map(|c| c[a]).fold(f64::MIN, f64::max);
                    (lo + 1e-9).floor() != (hi - 1e-9).floor() && hi - lo > 1e-9
                };
                if span(0) || span(1) {
                    tiled += 1;
                }
            }
        }
    }
    if sc.parts.is_empty() {
        return Err(ConvertError::NoGeometry);
    }
    let size = [mx[0] - mn[0], mx[1] - mn[1], mx[2] - mn[2]];

    let materials: Vec<MaterialInfo> = sc
        .materials
        .iter()
        .map(|m| MaterialInfo {
            name: m.name.clone(),
            base_texture: m.base_tex.and_then(|i| sc.textures[i].as_ref()).map(|t| t.img.dimensions()),
            transparent: material_is_transparent(Some(m), &sc),
            cutout: material_transparency(Some(m), &sc) == Transparency::Cutout,
            has_emission: atlas::has_emission(m),
            has_normal_map: atlas::has_normal(m),
            has_pbr: atlas::has_roughness(m),
        })
        .collect();
    let any_transparent = materials.iter().any(|m| m.transparent);

    let hint = |code: DiagCode, m: String| Diagnostic { severity: crate::report::Severity::Info, code, message: m };
    let mut hints = Vec::new();
    for d in &ctx.diags {
        if matches!(d.code, DiagCode::EmissiveUsedAsColour | DiagCode::EmissiveDuplicateIgnored | DiagCode::EmissiveIgnored) {
            hints.push(d.clone());
        }
    }
    if sc.skin_count > 0 {
        hints.push(hint(DiagCode::Rigged, format!("This model has {} skeleton(s) and {} animation clip(s). If it is a character that appears in a T-pose, convert with humanoid mode.", sc.skin_count, sc.animation_count)));
    }
    if any_transparent {
        hints.push(hint(DiagCode::Transparent, "Some materials are translucent (alpha blended). VotV can only clip alpha, not blend, so glass-like parts will not look right. Consider dropping transparent materials.".into()));
    }
    if tiled > 0 {
        hints.push(hint(DiagCode::TiledUvs, format!("{tiled} triangles repeat a texture and will be cut into more faces.")));
    }
    let big = size.iter().cloned().fold(0.0, f64::max);
    if big > 0.0 && (big < 0.01 || big > 50.0) {
        hints.push(hint(DiagCode::OddSize, format!("The model is {big:.3} units across; with the default scale of 100 that is {:.1} cm in VotV. If that looks wrong, adjust the scale.", big * 100.0)));
    }

    Ok(Inspection {
        visible_parts: sc.parts.len(),
        triangles: tris,
        vertices: verts,
        collision_objects: sc.collision_parts.iter().map(|p| p.name.clone()).collect(),
        materials,
        skins: sc.skin_count,
        animations: sc.animation_count,
        size,
        size_in_votv_cm: [size[0] * 100.0, size[1] * 100.0, size[2] * 100.0],
        tiled_triangles: tiled,
        any_transparent,
        hints,
    })
}
