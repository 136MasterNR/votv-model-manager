"""Builds small GLBs that each exercise one converter feature."""
import json, struct, numpy as np, trimesh
from PIL import Image
from trimesh.visual.material import PBRMaterial
from trimesh.visual import TextureVisuals
import os
out = os.path.dirname(os.path.abspath(__file__)); os.makedirs(out, exist_ok=True)

def grad(w, h, rgba=False):
    x = np.linspace(0, 255, w, dtype=np.uint8)[None, :].repeat(h, 0)
    y = np.linspace(0, 255, h, dtype=np.uint8)[:, None].repeat(w, 1)
    a = np.stack([x, y, (255 - x // 2).astype(np.uint8)] + ([np.full((h, w), 255, np.uint8)] if rgba else []), -1)
    return Image.fromarray(a)

def checker(n=64, cells=8):
    a = np.zeros((n, n, 3), np.uint8)
    for i in range(n):
        for j in range(n):
            a[i, j] = (230, 60, 60) if ((i * cells // n) + (j * cells // n)) % 2 else (40, 40, 200)
    return Image.fromarray(a)

def box_with(mat, uv_scale=1.0, extents=(1, 1, 1)):
    m = trimesh.creation.box(extents=extents)
    uv = m.vertices[:, [0, 2]] + 0.5
    m.visual = TextureVisuals(uv=uv * uv_scale, material=mat)
    return m

# 1. two materials: textured+normal+emissive+pbr, and flat colour
sc = trimesh.Scene()
tex = grad(128, 64)
nrm = Image.fromarray(np.tile(np.array([140, 100, 250], np.uint8), (64, 64, 1)))
em = Image.fromarray(np.tile(np.array([255, 120, 0], np.uint8), (32, 32, 1)))
mr = Image.fromarray(np.stack([np.zeros((32, 32), np.uint8), np.full((32, 32), 90, np.uint8), np.full((32, 32), 200, np.uint8)], -1))
m1 = box_with(PBRMaterial(name="skin", baseColorTexture=tex, normalTexture=nrm, emissiveTexture=em, metallicRoughnessTexture=mr,
                          roughnessFactor=1.0, metallicFactor=1.0, emissiveFactor=[1, 1, 1]))
m2 = trimesh.creation.icosphere(subdivisions=2, radius=0.4)
m2.apply_translation([1.5, 0, 0])
m2.visual = TextureVisuals(uv=np.zeros((len(m2.vertices), 2)), material=PBRMaterial(name="flat", baseColorFactor=[0.5, 0.2, 0.8, 1.0], roughnessFactor=0.3, metallicFactor=0.0))
sc.add_geometry(m1, node_name="body"); sc.add_geometry(m2, node_name="ball")
sc.export(f"{out}/basic.glb")

# 2. tiled UVs (plane repeating a checker 3.5 x 2 times, plus a hugely tiled one)
m = trimesh.creation.box(extents=(4, 0.1, 2))
uv = m.vertices[:, [0, 2]] / np.array([4, 2]) + 0.5
m.visual = TextureVisuals(uv=uv * np.array([3.5, 2.0]), material=PBRMaterial(name="tile", baseColorTexture=checker(), roughnessFactor=0.8, metallicFactor=0))
trimesh.Scene(m).export(f"{out}/tiled.glb")

# 3. custom UCX collision
sc = trimesh.Scene()
sc.add_geometry(box_with(PBRMaterial(name="m", baseColorFactor=[0.9, 0.9, 0.1, 1], roughnessFactor=0.5, metallicFactor=0)), node_name="crate")
c = trimesh.creation.box(extents=(0.8, 0.8, 0.8)); sc.add_geometry(c, node_name="UCX_crate_01")
c2 = trimesh.creation.box(extents=(0.3, 0.3, 0.3)); c2.apply_translation([0, 0.7, 0]); sc.add_geometry(c2, node_name="UCX_crate_02")
sc.export(f"{out}/ucx.glb")

# 4. concave (torus + L) for VHACD
t = trimesh.creation.torus(major_radius=1.0, minor_radius=0.3, major_sections=32, minor_sections=16)
t.visual = TextureVisuals(uv=np.zeros((len(t.vertices), 2)), material=PBRMaterial(name="ring", baseColorFactor=[0.2, 0.7, 0.3, 1], roughnessFactor=0.5, metallicFactor=0))
trimesh.Scene(t).export(f"{out}/torus.glb")

# 5. transparent (BLEND) + opaque
sc = trimesh.Scene()
sc.add_geometry(box_with(PBRMaterial(name="solid", baseColorFactor=[0.3, 0.3, 0.3, 1], roughnessFactor=0.5, metallicFactor=0)), node_name="a")
g = trimesh.creation.box(extents=(0.5, 0.5, 0.5)); g.apply_translation([0, 1, 0])
g.visual = TextureVisuals(uv=np.zeros((8, 2)), material=PBRMaterial(name="glass", baseColorFactor=[0.6, 0.8, 1, 0.3], alphaMode="BLEND", roughnessFactor=0.1, metallicFactor=0))
sc.add_geometry(g, node_name="b"); sc.export(f"{out}/glass.glb")

# 6. no material, no explicit normals (mesh from plain Trimesh) + mirrored node + duplicate part
sc = trimesh.Scene()
m = trimesh.creation.icosphere(subdivisions=1)
sc.add_geometry(m, node_name="plain")
mir = trimesh.creation.cone(radius=0.4, height=1.0)
T = np.eye(4); T[0, 0] = -1; T[1, 3] = 2
sc.add_geometry(mir, node_name="mirrored", transform=T)
d1 = trimesh.creation.box(extents=(0.2, 0.2, 0.2)); d1.apply_translation([3, 0, 0])
sc.add_geometry(d1, node_name="dupA"); sc.add_geometry(d1.copy(), node_name="dupB", geom_name="dupB_geom")
sc.export(f"{out}/misc.glb")

# 7. hand-built rigged mesh: 2 joints, a 4-vertex-high column bent by the rest pose
def build_skinned():
    pos = []; joints = []; weights = []; idx = []
    for i in range(4):
        y = i * 0.5
        for (x, z) in [(-0.1, -0.1), (0.1, -0.1), (0.1, 0.1), (-0.1, 0.1)]:
            pos.append([x, y, z]); w1 = i / 3.0
            joints.append([0, 1, 0, 0]); weights.append([1 - w1, w1, 0, 0])
    for i in range(3):
        for k in range(4):
            a = i * 4 + k; b = i * 4 + (k + 1) % 4
            idx += [a, b, a + 4, b, b + 4, a + 4]
    pos = np.array(pos, np.float32); joints = np.array(joints, np.uint16); weights = np.array(weights, np.float32)
    idx = np.array(idx, np.uint16)
    ibm0 = np.eye(4, dtype=np.float32)
    ibm1 = np.eye(4, dtype=np.float32); ibm1[1, 3] = -1.0   # joint 1 sits at y=1 in bind pose
    ibm = np.stack([ibm0.T, ibm1.T]).astype(np.float32)      # column-major
    blobs = []; views = []; accs = []
    def add(arr, target, ctype, typ, mn=None, mx=None):
        data = arr.tobytes(); off = sum(len(b) for b in blobs)
        pad = (4 - len(data) % 4) % 4; blobs.append(data + b"\0" * pad)
        views.append({"buffer": 0, "byteOffset": off, "byteLength": len(data), **({"target": target} if target else {})})
        a = {"bufferView": len(views) - 1, "componentType": ctype, "count": len(arr), "type": typ}
        if mn is not None: a["min"], a["max"] = mn, mx
        accs.append(a); return len(accs) - 1
    ap = add(pos, 34962, 5126, "VEC3", pos.min(0).tolist(), pos.max(0).tolist())
    aj = add(joints, 34962, 5123, "VEC4"); aw = add(weights, 34962, 5126, "VEC4")
    ai = add(idx, 34963, 5123, "SCALAR"); ab = add(ibm.reshape(2, 16), None, 5126, "MAT4")
    bin_ = b"".join(blobs)
    g = {"asset": {"version": "2.0"}, "scene": 0, "scenes": [{"nodes": [0, 3]}],
         # joint 1 is rotated 90deg about Z in its rest pose => "bent" character that the bind pose hides
         "nodes": [{"name": "root", "children": [1]},
                   {"name": "j0", "children": [2]},
                   {"name": "j1", "translation": [0, 1, 0], "rotation": [0, 0, 0.70710678, 0.70710678]},
                   {"name": "skinmesh", "mesh": 0, "skin": 0}],
         "skins": [{"joints": [1, 2], "inverseBindMatrices": ab, "skeleton": 0}],
         "meshes": [{"name": "m", "primitives": [{"attributes": {"POSITION": ap, "JOINTS_0": aj, "WEIGHTS_0": aw}, "indices": ai, "material": 0}]}],
         "materials": [{"name": "body", "pbrMetallicRoughness": {"baseColorFactor": [0.8, 0.5, 0.4, 1], "metallicFactor": 0, "roughnessFactor": 0.6}}],
         "accessors": accs, "bufferViews": views, "buffers": [{"byteLength": len(bin_)}]}
    js = json.dumps(g).encode(); js += b" " * ((4 - len(js) % 4) % 4)
    total = 12 + 8 + len(js) + 8 + len(bin_)
    open(f"{out}/rigged.glb", "wb").write(b"glTF" + struct.pack("<II", 2, total) + struct.pack("<I", len(js)) + b"JSON" + js + struct.pack("<I", len(bin_)) + b"BIN\0" + bin_)
build_skinned()
# 8/9. specular-glossiness only materials (Sketchfab-era exports) and a file requiring an unknown extension
def rewrite_json(path, fn):
    d = open(path, "rb").read()
    jl = struct.unpack_from("<I", d, 12)[0]
    j = json.loads(d[20:20 + jl]); rest = d[20 + jl:]
    fn(j)
    js = json.dumps(j).encode(); js += b" " * ((4 - len(js) % 4) % 4)
    total = 12 + 8 + len(js) + len(rest)
    open(path, "wb").write(b"glTF" + struct.pack("<II", 2, total) + struct.pack("<I", len(js)) + b"JSON" + js + rest)

def to_specgloss(j):
    for m in j["materials"]:
        pbr = m.pop("pbrMetallicRoughness", {})
        m["extensions"] = {"KHR_materials_pbrSpecularGlossiness": {
            "diffuseFactor": pbr.get("baseColorFactor", [1, 1, 1, 1]),
            **({"diffuseTexture": pbr["baseColorTexture"]} if "baseColorTexture" in pbr else {}),
            "glossinessFactor": 0.4, "specularFactor": [0, 0, 0]}}
    j["extensionsUsed"] = ["KHR_materials_pbrSpecularGlossiness"]
    j["extensionsRequired"] = ["KHR_materials_pbrSpecularGlossiness"]

def require_unknown(j):
    j["extensionsUsed"] = ["VENDOR_made_up"]; j["extensionsRequired"] = ["VENDOR_made_up"]

sc = trimesh.Scene(); sc.add_geometry(box_with(PBRMaterial(name="sg", baseColorTexture=grad(128, 64), roughnessFactor=0.5, metallicFactor=0)), node_name="box")
sc.export(f"{out}/specgloss.glb"); rewrite_json(f"{out}/specgloss.glb", to_specgloss)
sc.export(f"{out}/required_unknown.glb"); rewrite_json(f"{out}/required_unknown.glb", require_unknown)
print("added", [f for f in os.listdir(out) if f in ("specgloss.glb", "required_unknown.glb")])


# 10-12. alpha handling: cutout (MASK, like the chicken), OPAQUE with stray texture alpha, BLEND that is really opaque
def rgba_tex(w=64, h=64):
    a = np.zeros((h, w, 4), np.uint8); a[..., :3] = (200, 120, 40); a[..., 3] = 255
    a[:, : w // 2, 3] = 0           # the left half is fully transparent
    return Image.fromarray(a)

for name, kw, tex in [
    ("cutout", dict(alphaMode="MASK", alphaCutoff=0.5), rgba_tex()),
    ("opaque_alpha", dict(), rgba_tex()),
    ("blend_opaque", dict(alphaMode="BLEND"), grad(64, 64)),
]:
    trimesh.Scene(box_with(PBRMaterial(name=name, baseColorTexture=tex, roughnessFactor=0.5, metallicFactor=0, **kw))).export(f"{out}/{name}.glb")
print("added alpha fixtures")


# 13-19. emission and alpha cases taken from real toon-shader exports (Miyu, Higokumaru)
def albedo_like(n=64):
    a = np.zeros((n, n, 3), np.uint8); a[...] = (230, 180, 160); a[::8, :, :] = (250, 120, 120)
    return Image.fromarray(a)

def rgba_alpha(w=64, h=64, clear_rows=0, semi_rows=0, alpha=255):
    a = np.zeros((h, w, 4), np.uint8); a[..., :3] = (60, 50, 90); a[..., 3] = alpha
    a[:clear_rows, :, 3] = 0; a[clear_rows:clear_rows + semi_rows, :, 3] = 128
    return Image.fromarray(a)

def pbr(name, **kw):
    return PBRMaterial(name=name, roughnessFactor=0.5, metallicFactor=0, **kw)

def export(name, mat):
    trimesh.Scene(box_with(mat)).export(f"{out}/{name}.glb")

# colour only in the emissive texture, near-black base factor (Miyu's face, all of Higokumaru)
export("selflit", pbr("selflit", baseColorFactor=[0.03, 0.03, 0.03, 1.0], emissiveTexture=albedo_like(), emissiveFactor=[0.5, 0.5, 0.5]))
# emissive texture is a copy of the base texture (Miyu's body, cloth, hair)
t = grad(64, 64)
export("emissive_dup", pbr("dup", baseColorTexture=t, emissiveTexture=t.copy(), emissiveFactor=[0.5, 0.5, 0.5]))
# real glow: a dark mask with a bright spot over a different base texture
mask = np.zeros((64, 64, 3), np.uint8); mask[10:20, 10:20] = (255, 255, 200)
export("glow", pbr("glow", baseColorTexture=grad(64, 64), emissiveTexture=Image.fromarray(mask), emissiveFactor=[1, 1, 1]))
# emissive texture that is entirely black
export("black_emissive", pbr("blackem", baseColorTexture=grad(64, 64), emissiveTexture=Image.fromarray(np.zeros((64, 64, 3), np.uint8)), emissiveFactor=[1, 1, 1]))
# BLEND hair: factor alpha 0.99, a few transparent rows and soft edge rows (should be kept as a cutout)
export("blend_hair", pbr("hair", baseColorTexture=rgba_alpha(clear_rows=3, semi_rows=2), baseColorFactor=[1, 1, 1, 0.99], alphaMode="BLEND"))
# BLEND with a uniformly see-through texture (real glass: dropped)
export("blend_glass_tex", pbr("glasstex", baseColorTexture=rgba_alpha(alpha=100), alphaMode="BLEND"))
# an opaque dark outline shell next to the body
sc = trimesh.Scene()
sc.add_geometry(box_with(pbr("Hero_Body", baseColorTexture=grad(64, 64))), node_name="body")
o = trimesh.creation.box(extents=(1.05, 1.05, 1.05))
o.visual = TextureVisuals(uv=np.zeros((8, 2)), material=pbr("Hero_Outline", baseColorFactor=[0.06, 0.06, 0.06, 1.0]))
sc.add_geometry(o, node_name="outline_shell"); sc.export(f"{out}/outline.glb")
print("added emission/alpha fixtures")


# 20-21. a car-paint like material: dark flat colour (linear factor, no texture) and a fully metallic metallicRoughness texture
mr = np.zeros((32, 32, 3), np.uint8); mr[..., 1] = 200; mr[..., 2] = 255      # G roughness 200, B metallic 255
export("flat_paint", pbr("flatpaint", baseColorFactor=[0.0959, 0.0889, 0.0862, 1.0]))
trimesh.Scene(box_with(PBRMaterial(name="metalpaint", baseColorFactor=[0.0959, 0.0889, 0.0862, 1.0], metallicRoughnessTexture=Image.fromarray(mr)))).export(f"{out}/metal_paint.glb")
print("added paint fixtures")


# 22-23. layered clear-coat shell (BLEND, factor alpha 0.24, opaque texture) over a flat paint; ground shadow (white texture x near-black factor)
sc = trimesh.Scene()
sc.add_geometry(box_with(pbr("paint2", baseColorFactor=[0.8, 0.1, 0.1, 1.0])), node_name="body")
shell = box_with(pbr("coat2", baseColorTexture=grad(64, 64), baseColorFactor=[0.68, 0.68, 0.68, 0.24], alphaMode="BLEND"))
shell.apply_scale(1.01)
sc.add_geometry(shell, node_name="coat"); sc.export(f"{out}/coat_shell.glb")
sh = np.full((64, 64, 4), 255, np.uint8); sh[:40, :, 3] = 0       # white, 62% transparent
export("ground_shadow", pbr("shadow", baseColorTexture=Image.fromarray(sh), baseColorFactor=[0.0007, 0.0007, 0.0007, 1.0], alphaMode="BLEND"))
print("added coat/shadow fixtures")
