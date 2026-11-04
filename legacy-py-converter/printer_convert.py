"""
usage:
py printer_convert.py your_model_name
"""

import os
import re
import sys
import json
import math
import struct
import shutil
import hashlib
import argparse

if os.name == "nt":
    os.system("")

_COLOR_OK = sys.stdout.isatty()


class _C:
    RESET = "\033[0m" if _COLOR_OK else ""
    BOLD = "\033[1m" if _COLOR_OK else ""
    RED = "\033[91m" if _COLOR_OK else ""
    GREEN = "\033[92m" if _COLOR_OK else ""
    YELLOW = "\033[93m" if _COLOR_OK else ""
    CYAN = "\033[96m" if _COLOR_OK else ""


def info(msg):
    print(f"{_C.CYAN}{msg}{_C.RESET}")


def success(msg):
    print(f"{_C.GREEN}[success] {msg}{_C.RESET}")


def warn(msg):
    print(f"{_C.YELLOW}[warning] {msg}{_C.RESET}")


_BONE_WARNED = False


def bone_warn(msg):
    global _BONE_WARNED
    _BONE_WARNED = True
    print(f"{_C.RED}{_C.BOLD}[warning]{_C.RESET}{_C.RED} {msg}{_C.RESET}")


def error(msg):
    print(f"{_C.RED}{_C.BOLD}[error]{_C.RESET}{_C.RED} {msg}{_C.RESET}")


def _wait_before_exit(force=False):
    if not force:
        return
    try:
        input("\nPress Enter to close")
    except EOFError:
        pass


_REQUIRED = {
    "trimesh": "trimesh",
    "numpy": "numpy",
    "PIL": "Pillow",
    "vhacdx": "vhacdx",
}
_OPTIONAL = {
    "fast_simplification": "fast_simplification",
    "scipy": "scipy",
}
_OPTIONAL_NOTES = {
    "fast_simplification": "collision hulls just won't be trimmed down to --max-hull-faces",
    "scipy": "models that come without stored normals can take far longer to process "
    "(measured: 39 seconds instead of 0.1 for an 82,000-face mesh)",
}

_IMPORT_ERRORS = {}
_INSTALL_RESULTS = {}


def _find_missing(table):
    missing = []
    for import_name, pip_name in table.items():
        try:
            __import__(import_name)
            _IMPORT_ERRORS.pop(pip_name, None)
        except ImportError as e:
            _IMPORT_ERRORS[pip_name] = str(e)
            missing.append(pip_name)
    return missing


def _pip_available():
    try:
        import importlib.util

        return importlib.util.find_spec("pip") is not None
    except Exception:
        return False


def _try_bootstrap_pip():
    import importlib.util

    if importlib.util.find_spec("ensurepip") is None:
        return False

    info(f"\npip isn't available for this Python install ({sys.executable}).")
    print("Attempting to bootstrap it using Python's built-in ensurepip module...\n")
    try:
        import subprocess

        result = subprocess.run(
            [sys.executable, "-m", "ensurepip", "--upgrade"],
            check=False,
        )
    except Exception as e:
        error(f"ensurepip failed to run: {e}")
        return False

    if result.returncode != 0:
        return False

    import importlib

    importlib.invalidate_caches()
    ok = _pip_available()
    if ok:
        success("pip was installed successfully.\n")
    return ok


def _run_pip(args, capture=True):
    import subprocess

    return subprocess.run(
        [sys.executable, "-m", "pip", *args],
        capture_output=capture,
        text=True,
        check=False,
    )


def _pip_version_line():
    try:
        r = _run_pip(["--version"])
    except Exception:
        return None
    if r.returncode != 0:
        return None
    return r.stdout.strip() or None


def _ensure_pip_ready():
    info("\nChecking pip...")
    line = _pip_version_line()
    if line is None:
        if not _try_bootstrap_pip():
            return False
        line = _pip_version_line()
        if line is None:
            return False
    print(f"  Registered: {line}")

    m = re.search(r"\(python (\d+)\.(\d+)\)", line)
    if m and (int(m.group(1)), int(m.group(2))) != tuple(sys.version_info[:2]):
        warn(
            f"This pip reports Python {m.group(1)}.{m.group(2)}, but this script is running on "
            f"Python {sys.version_info[0]}.{sys.version_info[1]}."
        )

    try:
        r = _run_pip(["install", "--upgrade", "pip"])
    except Exception as e:
        warn(f"Couldn't check for a newer pip ({e}). Continuing with the current one.")
        return True
    if r.returncode != 0:
        warn("Couldn't update pip. Continuing with the current one.")
        tail = (r.stderr or r.stdout or "").strip().splitlines()
        if tail:
            print(f"  {tail[-1]}")
        return True
    after = _pip_version_line() or line
    if after != line:
        success(f"  pip updated: {after}")
    else:
        print("  pip is up to date.")
    return True


def _refresh_imports():
    import importlib
    import site

    importlib.invalidate_caches()
    try:
        user_site = site.getusersitepackages()
        if os.path.isdir(user_site) and user_site not in sys.path:
            sys.path.append(user_site)
    except Exception:
        pass


_NETWORK_HINTS = (
    "Retrying",
    "NewConnectionError",
    "Failed to establish",
    "Temporary failure in name resolution",
    "getaddrinfo failed",
    "ProxyError",
    "SSLError",
    "Connection refused",
    "timed out",
    "ConnectionError",
)


def _safe_print(text):
    enc = getattr(sys.stdout, "encoding", None) or "utf-8"
    print(text.encode(enc, "replace").decode(enc))


def _pip_install_one(pip_name, extra_args=()):
    import subprocess

    cmd = [
        sys.executable,
        "-m",
        "pip",
        "install",
        "--only-binary=:all:",
        *extra_args,
        pip_name,
    ]
    lines = []
    try:
        proc = subprocess.Popen(
            cmd,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            errors="replace",
            bufsize=1,
        )
        for line in proc.stdout:
            lines.append(line)
            _safe_print("    " + line.rstrip())
        proc.wait()
    except Exception as e:
        error(f"Failed to run pip: {e}")
        return False, "error"
    text = "".join(lines)
    if proc.returncode == 0:
        return True, "ok"
    if any(h in text for h in _NETWORK_HINTS):
        return False, "network"
    if (
        "No matching distribution found" in text
        or "Could not find a version that satisfies" in text
    ):
        return False, "no_wheel"
    return False, "failed"


def _failure_reason(reason):
    py = f"Python {sys.version_info[0]}.{sys.version_info[1]} ({struct.calcsize('P') * 8}-bit)"
    if reason == "no_wheel":
        return f"pip found no prebuilt version of it for {py} on this system"
    if reason == "network":
        return "pip couldn't reach the package index (check your internet connection or proxy)"
    return "pip reported an error (see its output above)"


def _try_cache_cleanup_reinstall(broken):
    info(
        f"\npip says these are installed, but Python can't load them: {', '.join(broken)}"
    )
    for p in broken:
        detail = (_IMPORT_ERRORS.get(p) or "").strip().splitlines()
        if detail:
            _safe_print(f"  - {p}: {detail[-1]}")
    print(
        "\nA damaged install is the usual cause. The script can clear pip's cache and reinstall them from scratch.\n"
    )
    try:
        answer = input("Clean the cache and reinstall now? [Y/n] ").strip().lower()
    except EOFError:
        answer = "y"
    if answer not in ("", "y", "yes"):
        return False

    try:
        r = _run_pip(["cache", "purge"], capture=False)
        if r.returncode != 0:
            warn("Couldn't clear pip's cache. Reinstalling without it anyway.")
    except Exception as e:
        warn(f"Couldn't clear pip's cache ({e}). Reinstalling without it anyway.")

    import_name_of = {
        **{v: k for k, v in _REQUIRED.items()},
        **{v: k for k, v in _OPTIONAL.items()},
    }
    for pkg in sorted(broken, key=lambda p: p != "numpy"):
        _refresh_imports()
        try:
            __import__(import_name_of[pkg])
            success(f"{pkg} works again.")
            continue
        except ImportError:
            pass
        info(f"\nReinstalling {pkg}...")
        ok, reason = _pip_install_one(pkg, ("--no-cache-dir", "--force-reinstall"))
        if ok:
            success(f"{pkg} reinstalled.")
        else:
            error(f"{pkg} could not be reinstalled: {_failure_reason(reason)}")
    return True


def _try_auto_install(missing_required, missing_optional):
    if not _pip_available():
        if not _try_bootstrap_pip():
            return "no_pip"

    labels = list(missing_required) + [f"{p} (optional)" for p in missing_optional]
    info(f"\nThe following libraries are missing: {', '.join(labels)}")
    print(
        f"Each one will be installed separately with: "
        f"{_C.CYAN}{sys.executable} -m pip install --only-binary=:all: <name>{_C.RESET}\n"
    )
    try:
        answer = input("Install them now? [Y/n] ").strip().lower()
    except EOFError:
        answer = "y"
    if answer not in ("", "y", "yes"):
        return "declined"

    if not _ensure_pip_ready():
        return "no_pip"

    for pkg in list(missing_required) + list(missing_optional):
        info(f"\nInstalling {pkg}...")
        ok, reason = _pip_install_one(pkg)
        _INSTALL_RESULTS[pkg] = reason
        if ok:
            success(f"{pkg} installed.")
        else:
            error(f"{pkg} could not be installed: {_failure_reason(reason)}")
    return "done"


_MISSING = _find_missing(_REQUIRED)
_MISSING_OPT = _find_missing(_OPTIONAL)
_install_outcome = None

if _MISSING or _MISSING_OPT:
    _install_outcome = _try_auto_install(_MISSING, _MISSING_OPT)
    if _install_outcome == "done":
        _refresh_imports()
        _MISSING = _find_missing(_REQUIRED)
        _MISSING_OPT = _find_missing(_OPTIONAL)
        _broken = [
            p for p in _MISSING + _MISSING_OPT if _INSTALL_RESULTS.get(p) == "ok"
        ]
        if _broken and _try_cache_cleanup_reinstall(_broken):
            _refresh_imports()
            _MISSING = _find_missing(_REQUIRED)
            _MISSING_OPT = _find_missing(_OPTIONAL)

if _MISSING:
    plural = "is" if len(_MISSING) == 1 else "are"
    if _install_outcome == "no_pip":
        error(
            f"\nThis script needs {', '.join(_MISSING)}, which {plural} not installed yet."
        )
        print(
            f"\n{_C.BOLD}pip itself doesn't seem to be available for this Python install "
            f"({sys.executable}).{_C.RESET}"
        )
        print(
            "Please install pip first (see https://pip.pypa.io/en/stable/installation/), "
            "then run this script again."
        )
        _wait_before_exit(force=True)
        sys.exit(1)
    elif _install_outcome == "declined":
        error(
            f"\nThis script needs {', '.join(_MISSING)}, which {plural} not installed yet."
        )
        print(f"\nYou can install them yourself by running:\n")
        for _p in _MISSING:
            print(
                f'    {_C.CYAN}"{sys.executable}" -m pip install --only-binary=:all: {_p}{_C.RESET}'
            )
        print()
    else:
        error(
            f"\nCouldn't set up {', '.join(_MISSING)}, which {plural} required to continue."
        )
        _any_no_wheel = False
        for _p in _MISSING:
            _r = _INSTALL_RESULTS.get(_p)
            if _r and _r != "ok":
                print(f"  - {_p}: {_failure_reason(_r)}")
                _any_no_wheel = _any_no_wheel or _r == "no_wheel"
            elif _p in _IMPORT_ERRORS:
                _detail = (_IMPORT_ERRORS[_p] or "").strip().splitlines()
                _safe_print(
                    f"  - {_p}: installed, but Python can't load it ({_detail[-1] if _detail else 'unknown error'})"
                )
        if _any_no_wheel:
            print(
                "\n64-bit Python 3.11 to 3.14 has prebuilt versions of everything this script needs."
            )
    print("Then run this script again.")
    _wait_before_exit(force=True)
    sys.exit(1)

for _p in _MISSING_OPT:
    _r = _INSTALL_RESULTS.get(_p)
    _why = _failure_reason(_r) if _r not in (None, "ok") else "it wasn't installed"
    warn(
        f"{_p} (optional) is not available ({_why}). Continuing without it: {_OPTIONAL_NOTES[_p]}."
    )

import trimesh
import numpy as np
from PIL import Image
import vhacdx

try:
    import fast_simplification

    _HAVE_FAST_SIMPLIFICATION = True
except ImportError:
    _HAVE_FAST_SIMPLIFICATION = False


def sanitize_name(name: str) -> str:
    name = name.strip().lower()
    name = re.sub(r"[^a-z0-9_]+", "_", name)
    name = re.sub(r"_+", "_", name)
    name = name.strip("_")
    return name or "model"


def _looks_like_previous_output(model_dir, out_name):
    try:
        existing = set(os.listdir(model_dir))
    except OSError:
        return False
    has_properties = "properties.cfg" in existing
    has_matching_obj_or_mtl = (
        f"{out_name}.obj" in existing or f"{out_name}.mtl" in existing
    )
    return has_properties and has_matching_obj_or_mtl


def _material_key(mat):
    if mat is None:
        return ("default", None, None)
    tex = getattr(mat, "baseColorTexture", None)
    factor = getattr(mat, "baseColorFactor", None)
    name = getattr(mat, "name", None) or "material"
    tex_hash = None
    if tex is not None:
        try:
            tex_hash = hashlib.md5(tex.tobytes()).hexdigest()
        except Exception:
            tex_hash = id(tex)
    factor_t = tuple(round(float(c), 4) for c in factor) if factor is not None else None
    return (name, tex_hash, factor_t)


def _material_is_transparent(mat, alpha_threshold=254):
    if mat is None:
        return False
    alpha_mode = getattr(mat, "alphaMode", None)
    if alpha_mode not in (None, "OPAQUE"):
        return True
    factor = getattr(mat, "baseColorFactor", None)
    if factor is not None and len(factor) > 3:
        try:
            if float(factor[3]) < alpha_threshold:
                return True
        except Exception:
            pass
    tex = getattr(mat, "baseColorTexture", None)
    if tex is not None and tex.mode == "RGBA":
        arr = np.array(tex)
        if arr.shape[-1] == 4 and arr[..., 3].min() < alpha_threshold:
            return True
    return False


def _material_has_emission(mat):
    if mat is None:
        return False
    if getattr(mat, "emissiveTexture", None) is not None:
        return True
    factor = getattr(mat, "emissiveFactor", None)
    if factor is not None:
        try:
            return any(float(c) > 0.0 for c in factor[:3])
        except Exception:
            return False
    return False


def _emissive_swatch(mat, flat_tile_size=64):
    if mat is None:
        return None
    tex = getattr(mat, "emissiveTexture", None)
    if tex is not None:
        return tex.convert("RGBA")
    factor = getattr(mat, "emissiveFactor", None)
    if factor is not None and any(float(c) > 0.0 for c in factor[:3]):
        r, g, b = [int(max(0, min(255, round(float(c) * 255)))) for c in factor[:3]]
        return Image.new("RGBA", (flat_tile_size, flat_tile_size), (r, g, b, 255))
    return None


def _material_has_normal(mat):
    if mat is None:
        return False
    return getattr(mat, "normalTexture", None) is not None


def _normal_swatch(mat, flip_green=True):
    if mat is None:
        return None
    tex = getattr(mat, "normalTexture", None)
    if tex is not None:
        rgb = tex.convert("RGB")
        if flip_green:
            r, g, b = rgb.split()
            rgb = Image.merge("RGB", (r, g.point(lambda x: 255 - x), b))
        return rgb
    return None


def _material_has_roughness(mat):
    if mat is None:
        return False
    if getattr(mat, "metallicRoughnessTexture", None) is not None:
        return True
    return getattr(mat, "roughnessFactor", None) is not None


def _pbr_swatch(mat, flat_tile_size=64):
    if mat is None:
        return None
    spec = 128
    tex = getattr(mat, "metallicRoughnessTexture", None)
    rf = getattr(mat, "roughnessFactor", None)
    if tex is not None:
        mf = getattr(mat, "metallicFactor", None)
        rf_v = 1.0 if rf is None else max(0.0, min(1.0, float(rf)))
        mf_v = 1.0 if mf is None else max(0.0, min(1.0, float(mf)))
        _, g, b = tex.convert("RGB").split()
        rough = g.point(lambda x: int(round(x * rf_v)))
        metal = b.point(lambda x: int(round(x * mf_v)))
        spec_ch = Image.new("L", metal.size, spec)
        return Image.merge("RGB", (metal, spec_ch, rough))
    if rf is not None:
        v = int(max(0, min(255, round(float(rf) * 255))))
        return Image.new("RGB", (flat_tile_size, flat_tile_size), (0, spec, v))
    return None


def _material_swatch(mat, flat_tile_size=64):
    if mat is not None:
        img = getattr(mat, "baseColorTexture", None)
        if img is not None:
            return img.convert("RGBA"), True
        factor = getattr(mat, "baseColorFactor", None)
        if factor is not None:
            r, g, b = [int(max(0, min(255, round(float(c))))) for c in factor[:3]]
            a = (
                int(max(0, min(255, round(float(factor[3])))))
                if len(factor) > 3
                else 255
            )
            return Image.new(
                "RGBA", (flat_tile_size, flat_tile_size), (r, g, b, a)
            ), False
    return Image.new(
        "RGBA", (flat_tile_size, flat_tile_size), (200, 200, 200, 255)
    ), False


def _pad_image_edge(img, pad):
    import numpy as np

    arr = np.array(img)
    padded = np.pad(arr, ((pad, pad), (pad, pad), (0, 0)), mode="edge")
    return Image.fromarray(padded)


def _pack_atlas(swatches, pad=4, verbose=True):
    items = list(swatches.items())
    items.sort(key=lambda kv: kv[1].height, reverse=True)

    padded_items = [(key, _pad_image_edge(img, pad)) for key, img in items]

    total_area = sum(im.width * im.height for _, im in padded_items)
    atlas_w = max(
        max(im.width for _, im in padded_items), int(math.sqrt(total_area)) or 1
    )

    x_cursor = 0
    y_cursor = 0
    shelf_h = 0
    placements = {}
    for (key, orig_img), (_, padded_img) in zip(items, padded_items):
        pw, ph = padded_img.size
        if x_cursor + pw > atlas_w and x_cursor > 0:
            x_cursor = 0
            y_cursor += shelf_h
            shelf_h = 0
        placements[key] = {
            "x": x_cursor + pad,
            "y_top": y_cursor + pad,
            "w": orig_img.width,
            "h": orig_img.height,
            "_paste_x": x_cursor,
            "_paste_y": y_cursor,
        }
        x_cursor += pw
        shelf_h = max(shelf_h, ph)
    atlas_h = y_cursor + shelf_h

    atlas_img = Image.new("RGBA", (atlas_w, atlas_h), (0, 0, 0, 0))
    for key, padded_img in padded_items:
        p = placements[key]
        atlas_img.paste(padded_img, (p["_paste_x"], p["_paste_y"]))

    for key in placements:
        placements[key]["atlas_w"] = atlas_w
        placements[key]["atlas_h"] = atlas_h

    if verbose:
        info(
            f"  Packed {len(items)} texture(s)/color(s) into a {atlas_w}x{atlas_h} atlas ({pad}px edge padding per tile)."
        )
    return atlas_img, placements


def _find_exact_duplicate_parts(parts, verbose=True):
    groups = {}
    for i, p in enumerate(parts):
        groups.setdefault((len(p.vertices), len(p.faces)), []).append(i)

    to_drop = set()
    for (_, _), idxs in groups.items():
        if len(idxs) < 2:
            continue
        for a in range(len(idxs)):
            i = idxs[a]
            if i in to_drop:
                continue
            for b in range(a + 1, len(idxs)):
                j = idxs[b]
                if j in to_drop:
                    continue
                pi, pj = parts[i], parts[j]
                if not np.allclose(pi.vertices, pj.vertices, atol=1e-5):
                    continue
                if not np.array_equal(pi.faces, pj.faces):
                    continue

                mat_i = getattr(pi.visual, "material", None)
                mat_j = getattr(pj.visual, "material", None)
                tex_i = getattr(mat_i, "baseColorTexture", None) is not None
                tex_j = getattr(mat_j, "baseColorTexture", None) is not None
                name_i = getattr(mat_i, "name", None) or f"part_{i}"
                name_j = getattr(mat_j, "name", None) or f"part_{j}"

                if tex_i and not tex_j:
                    drop, keep_name, drop_name = j, name_i, name_j
                elif tex_j and not tex_i:
                    drop, keep_name, drop_name = i, name_j, name_i
                else:
                    drop, keep_name, drop_name = j, name_i, name_j

                to_drop.add(drop)
                if verbose:
                    info(
                        f"  [dedup] parts {i} ({name_i!r}) and {j} ({name_j!r}) are geometrically identical, keeping {keep_name!r}, dropping {drop_name!r}."
                    )
    return to_drop


_GLTF_COMPONENT_DTYPES = {
    5120: np.int8,
    5121: np.uint8,
    5122: np.int16,
    5123: np.uint16,
    5125: np.uint32,
    5126: np.float32,
}
_GLTF_TYPE_COUNTS = {
    "SCALAR": 1,
    "VEC2": 2,
    "VEC3": 3,
    "VEC4": 4,
    "MAT2": 4,
    "MAT3": 9,
    "MAT4": 16,
}


def _load_glb_chunks(path):
    with open(path, "rb") as f:
        data = f.read()
    if len(data) < 12 or data[:4] != b"glTF":
        raise ValueError("not a valid binary .glb file (bad header)")
    length = struct.unpack_from("<I", data, 8)[0]
    offset = 12
    json_chunk = None
    bin_chunk = None
    while offset < length:
        chunk_len, chunk_type = struct.unpack_from("<II", data, offset)
        offset += 8
        chunk_data = data[offset : offset + chunk_len]
        offset += chunk_len
        if chunk_type == 0x4E4F534A:  # 'JSON'
            json_chunk = chunk_data
        elif chunk_type == 0x004E4942:  # 'BIN\0'
            bin_chunk = chunk_data
    if json_chunk is None:
        raise ValueError(".glb file has no JSON chunk")
    return json.loads(json_chunk.decode("utf-8")), bin_chunk


def _read_gltf_accessor(gltf, bin_data, accessor_index):
    if accessor_index is None:
        return None
    acc = gltf["accessors"][accessor_index]
    if "sparse" in acc:
        raise NotImplementedError("sparse accessors are not supported")
    count = acc["count"]
    dtype = _GLTF_COMPONENT_DTYPES[acc["componentType"]]
    ncomp = _GLTF_TYPE_COUNTS[acc["type"]]
    bv_index = acc.get("bufferView")
    if bv_index is None:
        return np.zeros((count,) if ncomp == 1 else (count, ncomp), dtype=np.float32)

    bv = gltf["bufferViews"][bv_index]
    if bv.get("buffer", 0) != 0:
        raise ValueError("only single-buffer (embedded) .glb files are supported")
    if bin_data is None:
        raise ValueError(".glb file has no binary chunk to read vertex data from")

    base_offset = bv.get("byteOffset", 0) + acc.get("byteOffset", 0)
    comp_size = np.dtype(dtype).itemsize
    elem_size = comp_size * ncomp
    stride = bv.get("byteStride") or elem_size

    if stride == elem_size:
        arr = (
            np.frombuffer(
                bin_data, dtype=dtype, count=count * ncomp, offset=base_offset
            )
            .reshape(count, ncomp)
            .copy()
        )
    else:
        arr = np.empty((count, ncomp), dtype=dtype)
        for i in range(count):
            row_off = base_offset + i * stride
            arr[i] = np.frombuffer(bin_data, dtype=dtype, count=ncomp, offset=row_off)

    if acc.get("normalized"):
        arr = arr.astype(np.float32) / np.iinfo(dtype).max

    if acc["type"] == "SCALAR":
        arr = arr.reshape(count)
    return arr


def _gltf_node_local_matrix(node):
    if "matrix" in node:
        return np.array(node["matrix"], dtype=np.float64).reshape(4, 4).T
    t = node.get("translation", [0.0, 0.0, 0.0])
    r = node.get("rotation", [0.0, 0.0, 0.0, 1.0])
    s = node.get("scale", [1.0, 1.0, 1.0])
    T = trimesh.transformations.translation_matrix(t)
    R = trimesh.transformations.quaternion_matrix([r[3], r[0], r[1], r[2]])
    S = np.diag([s[0], s[1], s[2], 1.0])
    return T @ R @ S


def _compute_gltf_world_transforms(gltf):
    nodes = gltf.get("nodes", [])
    world = {}

    def visit(idx, parent_matrix):
        if idx in world:
            return
        node = nodes[idx]
        m = parent_matrix @ _gltf_node_local_matrix(node)
        world[idx] = m
        for child in node.get("children", []):
            visit(child, m)

    scenes = gltf.get("scenes")
    if scenes:
        roots = scenes[gltf.get("scene", 0)].get("nodes", [])
    else:
        child_ids = set()
        for node in nodes:
            child_ids.update(node.get("children", []))
        roots = [i for i in range(len(nodes)) if i not in child_ids]

    for r in roots:
        visit(r, np.eye(4))

    return world


def _compute_gltf_skin_matrices(gltf, bin_data, world_transforms):
    skin_matrices = []
    for skin in gltf.get("skins", []):
        joints = skin["joints"]
        ibm_idx = skin.get("inverseBindMatrices")
        if ibm_idx is not None:
            ibm_flat = _read_gltf_accessor(gltf, bin_data, ibm_idx)
            ibms = [ibm_flat[i].reshape(4, 4).T for i in range(len(joints))]
        else:
            ibms = [np.eye(4) for _ in joints]
        mats = [world_transforms[joints[i]] @ ibms[i] for i in range(len(joints))]
        skin_matrices.append(mats)
    return skin_matrices


def _collect_humanoid_primitives(gltf, bin_data, world_transforms, skin_matrices):
    prims = []
    for node_idx, node in enumerate(gltf.get("nodes", [])):
        mesh_idx = node.get("mesh")
        if mesh_idx is None:
            continue
        skin_idx = node.get("skin")
        mesh = gltf["meshes"][mesh_idx]
        for prim in mesh.get("primitives", []):
            attrs = prim.get("attributes", {})
            if "POSITION" not in attrs:
                continue
            positions = _read_gltf_accessor(gltf, bin_data, attrs["POSITION"]).astype(
                np.float64
            )
            normals = None
            if "NORMAL" in attrs:
                normals = _read_gltf_accessor(gltf, bin_data, attrs["NORMAL"]).astype(
                    np.float64
                )

            skinned = (
                skin_idx is not None and "JOINTS_0" in attrs and "WEIGHTS_0" in attrs
            )

            if skinned:
                joints0 = _read_gltf_accessor(gltf, bin_data, attrs["JOINTS_0"]).astype(
                    np.int64
                )
                weights0 = _read_gltf_accessor(
                    gltf, bin_data, attrs["WEIGHTS_0"]
                ).astype(np.float64)
                wsum = weights0.sum(axis=1, keepdims=True)
                wsum[wsum == 0] = 1.0
                weights0 = weights0 / wsum

                mats_arr = np.stack(skin_matrices[skin_idx], axis=0)  # (J, 4, 4)
                influence_mats = mats_arr[joints0]  # (N, 4, 4, 4)
                blended = np.einsum("nk,nkij->nij", weights0, influence_mats)

                homog = np.concatenate(
                    [positions, np.ones((len(positions), 1))], axis=1
                )
                world_positions = np.einsum("nij,nj->ni", blended, homog)[:, :3]

                world_normals = None
                if normals is not None:
                    world_normals = np.einsum("nij,nj->ni", blended[:, :3, :3], normals)
                    lens = np.linalg.norm(world_normals, axis=1, keepdims=True)
                    lens[lens == 0] = 1
                    world_normals = world_normals / lens
            else:
                m = world_transforms.get(node_idx, np.eye(4))
                world_positions = trimesh.transformations.transform_points(positions, m)
                world_normals = None
                if normals is not None:
                    rot = m[:3, :3]
                    world_normals = normals @ rot.T
                    lens = np.linalg.norm(world_normals, axis=1, keepdims=True)
                    lens[lens == 0] = 1
                    world_normals = world_normals / lens

            prims.append(
                {
                    "local_positions": positions,
                    "world_positions": world_positions,
                    "world_normals": world_normals,
                    "skinned": skinned,
                }
            )
    return prims


def _match_humanoid_primitive(geom_vertices, humanoid_prims, used):
    for i, prim in enumerate(humanoid_prims):
        if i in used:
            continue
        lp = prim["local_positions"]
        if lp.shape != geom_vertices.shape:
            continue
        if np.allclose(lp, geom_vertices, atol=1e-4):
            used.add(i)
            return prim
    return None


def _is_ucx_name(name):
    return bool(name) and name.strip().upper().startswith("UCX_")


def _sanitize_obj_object_name(name):
    name = re.sub(r"\s+", "_", name.strip())
    name = re.sub(r"[^A-Za-z0-9_]+", "_", name)
    name = re.sub(r"_+", "_", name).strip("_")
    return name or "UCX_collision"


def _iter_scene_parts(scene, humanoid_prims=None, verbose=True):
    parts = []
    collision_parts = []
    used = set()
    skinned_count = 0
    for node_name in scene.graph.nodes_geometry:
        transform, geometry_name = scene.graph[node_name]
        geom = scene.geometry.get(geometry_name)
        if geom is None or not hasattr(geom, "faces") or len(geom.faces) == 0:
            continue

        prim = (
            _match_humanoid_primitive(geom.vertices, humanoid_prims, used)
            if humanoid_prims
            else None
        )

        if prim is not None and prim["skinned"]:
            verts = prim["world_positions"]
            skinned_count += 1
        else:
            verts = trimesh.transformations.transform_points(geom.vertices, transform)

        part = trimesh.Trimesh(
            vertices=verts, faces=geom.faces, process=False, validate=False
        )

        if (
            prim is not None
            and prim["skinned"]
            and prim["world_normals"] is not None
            and len(prim["world_normals"]) == len(verts)
        ):
            part.vertex_normals = prim["world_normals"]
        elif geom.vertex_normals is not None and len(geom.vertex_normals) == len(
            geom.vertices
        ):
            rot = transform[:3, :3]
            n = geom.vertex_normals @ rot.T
            lens = np.linalg.norm(n, axis=1, keepdims=True)
            lens[lens == 0] = 1
            part.vertex_normals = n / lens

        part.visual = geom.visual

        display_name = node_name or geometry_name or ""
        if _is_ucx_name(display_name):
            part.metadata["ucx_name"] = display_name
            collision_parts.append(part)
        else:
            parts.append(part)

    if humanoid_prims is not None and verbose:
        info(
            f"  [--humanoid] repositioned {skinned_count} skinned part(s) using skeleton bind data instead of node transforms."
        )

    return parts, collision_parts


_FALLBACK_BOX_SIZE_CM = 33.0


def _make_fallback_collision_box(parts, size_cm, scale, offset_x=0.0, offset_y=0.0):
    verts_list = [p.vertices for p in parts if len(p.vertices) > 0]
    if not verts_list:
        return None
    all_verts = np.concatenate(verts_list, axis=0) * scale
    mn = all_verts.min(axis=0)
    mx = all_verts.max(axis=0)
    center_x = (mn[0] + mx[0]) / 2.0
    center_z = (mn[2] + mx[2]) / 2.0
    bottom_y = mn[1]

    box = trimesh.creation.box(extents=[size_cm, size_cm, size_cm])
    box.apply_translation(
        [center_x + offset_x, bottom_y + size_cm / 2.0 + offset_y, center_z]
    )
    return box


def _clip_poly(poly, axis, c):
    below, above = [], []
    n = len(poly)
    for k in range(n):
        a = poly[k]
        b = poly[(k + 1) % n]
        da = a[2][axis] - c
        db = b[2][axis] - c
        if da <= 0:
            below.append(a)
        if da >= 0:
            above.append(a)
        if (da < 0 < db) or (db < 0 < da):
            t = da / (da - db)
            m = (
                a[0] + (b[0] - a[0]) * t,
                a[1] + (b[1] - a[1]) * t,
                a[2] + (b[2] - a[2]) * t,
            )
            below.append(m)
            above.append(m)
    return below, above


def _split_tri_on_uv_grid(pos, nrm, uv):
    pieces = [[(pos[k], nrm[k], uv[k]) for k in range(3)]]
    for axis in (0, 1):
        out = []
        for pc in pieces:
            vals = [v[2][axis] for v in pc]
            lo = math.floor(min(vals) + 1e-9)
            hi = math.floor(max(vals) - 1e-9)
            cur = pc
            for c in range(lo + 1, hi + 1):
                below, above = _clip_poly(cur, axis, c)
                if len(below) >= 3:
                    out.append(below)
                cur = above
                if len(cur) < 3:
                    break
            if len(cur) >= 3:
                out.append(cur)
        pieces = out
    return pieces


_MAX_UV_CELLS_PER_FACE = 64


_COLLISION_VIS_KEY = ("__votv_collision_outline__", None, None)


def _inflate_collision(V, F, block_v, block_f, d):
    out = V.copy()
    n_blocks = len(block_v)
    for k in range(n_blocks):
        v0_, v1_ = block_v[k], (block_v[k + 1] if k + 1 < n_blocks else len(V))
        f0_, f1_ = block_f[k], (block_f[k + 1] if k + 1 < n_blocks else len(F))
        if f1_ <= f0_ or v1_ <= v0_:
            continue
        Fb = F[f0_:f1_]
        tri = V[Fb]
        fn = np.cross(tri[:, 1] - tri[:, 0], tri[:, 2] - tri[:, 0])
        volume = np.einsum("ij,ij->i", tri[:, 0], np.cross(tri[:, 1], tri[:, 2])).sum()
        if volume < 0:
            fn = -fn
        fl = np.linalg.norm(fn, axis=1, keepdims=True)
        fu = fn / np.where(fl > 1e-12, fl, 1.0)
        vi = Fb.ravel()
        fur = np.repeat(fu, 3, axis=0)
        _, first = np.unique(
            np.column_stack([vi, np.round(fur, 3)]), axis=0, return_index=True
        )
        vi_u, fu_u = vi[first], fur[first]
        vn = np.zeros_like(V)
        np.add.at(vn, vi_u, fu_u)
        ln = np.linalg.norm(vn, axis=1, keepdims=True)
        vn = vn / np.where(ln > 1e-12, ln, 1.0)
        dots = np.zeros(len(V))
        cnt = np.zeros(len(V))
        np.add.at(dots, vi_u, np.einsum("ij,ij->i", vn[vi_u], fu_u))
        np.add.at(cnt, vi_u, 1.0)
        mean_dot = np.where(cnt > 0, dots / np.maximum(cnt, 1.0), 1.0)
        mitre = np.clip(1.0 / np.maximum(mean_dot, 1e-3), 1.0, 2.5)
        idx = np.arange(v0_, v1_)
        out[idx] = V[idx] + vn[idx] * (d * mitre[idx])[:, None]
    return out


def _add_collision_outline(obj_lines, start, v0, red_uv, push):
    coll = obj_lines[start:]
    vert_text, faces, block_v, block_f = [], [], [], []
    for line in coll:
        if line.startswith("o "):
            block_v.append(len(vert_text))
            block_f.append(len(faces))
        elif line.startswith("v "):
            vert_text.append(line)
        elif line.startswith("f "):
            faces.append([int(x) - 1 - v0 for x in line.split()[1:4]])
    if not vert_text or not faces:
        return obj_lines, 0
    if not block_v:
        block_v, block_f = [0], [0]

    V = np.array([[float(x) for x in t.split()[1:4]] for t in vert_text])
    F = np.asarray(faces, dtype=np.int64)
    if push > 0:
        V = _inflate_collision(V, F, block_v, block_f, push)
    tri = V[F]
    nrm = np.cross(tri[:, 1] - tri[:, 0], tri[:, 2] - tri[:, 0])
    ln = np.linalg.norm(nrm, axis=1, keepdims=True)
    nrm = np.where(
        ln > 1e-12, nrm / np.where(ln > 1e-12, ln, 1.0), np.array([0.0, 0.0, 1.0])
    )

    head = obj_lines[:start]
    vt_i = sum(1 for l in head if l.startswith("vt ")) + 1
    vn_base = sum(1 for l in head if l.startswith("vn ")) + 1

    out = [
        "# --show-coll: red copy of the collision shapes",
        f"vt {red_uv[0]:.6f} {red_uv[1]:.6f}",
    ]
    out += [f"v {x:.6f} {y:.6f} {z:.6f}" for x, y, z in V]
    for n in nrm:
        out.append(f"vn {n[0]:.6f} {n[1]:.6f} {n[2]:.6f}")
        out.append(f"vn {-n[0]:.6f} {-n[1]:.6f} {-n[2]:.6f}")
    for i, (a, b, c) in enumerate(F):
        a, b, c = a + v0 + 1, b + v0 + 1, c + v0 + 1
        nf, nb = vn_base + 2 * i, vn_base + 2 * i + 1
        out.append(f"f {a}/{vt_i}/{nf} {b}/{vt_i}/{nf} {c}/{vt_i}/{nf}")
        out.append(f"f {a}/{vt_i}/{nb} {c}/{vt_i}/{nb} {b}/{vt_i}/{nb}")

    n_new = len(vert_text)
    shifted = []
    for line in coll:
        if line.startswith("f "):
            p, q, r = (int(x) + n_new for x in line.split()[1:4])
            shifted.append(f"f {p} {q} {r}")
        else:
            shifted.append(line)
    return head + out + shifted, 2 * len(F)


def export_votv_model(
    input_path,
    out_name,
    out_dir=".",
    scale=100.0,
    max_tile_size=1024,
    max_atlas_size=4096,
    generate_collision=True,
    max_hull_faces=None,
    collision_quality="high",
    max_decomp_hulls=64,
    emissive_enabled=True,
    emissive_strength=10.0,
    edible=False,
    strip_transparent=False,
    humanoid=False,
    fallback_collision=False,
    fallback_collision_size=_FALLBACK_BOX_SIZE_CM,
    collision_offset_x=0.0,
    collision_offset_y=0.0,
    force_generate_collisions=False,
    flip_normal_green=True,
    physical_material=0,
    show_collisions=False,
    verbose=True,
):
    out_name = sanitize_name(out_name)
    if max_hull_faces is None:
        max_hull_faces = 200 if collision_quality == "high" else 80

    if os.path.splitext(input_path)[1] == "":
        input_path += ".glb"

    if not os.path.exists(input_path):
        raise FileNotFoundError(
            f"Could not find the file:\n    {input_path}\n\n"
            f"Double-check the spelling and the folder."
        )

    ext = os.path.splitext(input_path)[1].lower()
    if ext != ".glb":
        raise ValueError(
            f"This script only reads .glb 3D model files, but you gave it a {ext or '(no extension)'} file.\n\n"
            f'If you downloaded the model from Sketchfab or a similar, look for a ".glb" download option.\n'
            f'If you exported it through blender, export as "glTF 2.0 (.glb/.gltf)".'
        )

    try:
        scene = trimesh.load(input_path, force="scene", process=not humanoid)
    except Exception as e:
        raise ValueError(
            f"This file could not be read as a 3D model ({e}).\n\n"
            f"The file is likely damaged, incomplete, or not really a .glb file despite its name.\n\n"
            f"Try downloading/exporting it again."
        )

    humanoid_prims = None
    if humanoid:
        try:
            gltf_json, bin_data = _load_glb_chunks(input_path)
            world_transforms = _compute_gltf_world_transforms(gltf_json)
            skin_matrices = _compute_gltf_skin_matrices(
                gltf_json, bin_data, world_transforms
            )
            humanoid_prims = _collect_humanoid_primitives(
                gltf_json, bin_data, world_transforms, skin_matrices
            )
            if not any(p["skinned"] for p in humanoid_prims):
                warn(
                    "--humanoid was given, but this file doesn't seem to contain "
                    "any skinned mesh/skeleton data, continuing as a normal model."
                )
        except Exception as e:
            warn(
                f"--humanoid was given, but the skeleton data couldn't be read ({e}). Falling back to normal node-transform positioning, which may look distorted/exploded for a rigged character."
            )
            humanoid_prims = None
    else:
        try:
            gltf_json, _bin = _load_glb_chunks(input_path)
            if gltf_json.get("skins"):
                bone_warn(
                    "this model contains skeleton/bone data, but --humanoid wasn't used, if this is a humanoid model you might notice T-posing."
                )
        except Exception:
            pass

    parts, custom_collision_parts = _iter_scene_parts(
        scene, humanoid_prims=humanoid_prims, verbose=verbose
    )
    parts = [p for p in parts if p is not None and len(getattr(p, "faces", [])) > 0]
    custom_collision_parts = [
        p
        for p in custom_collision_parts
        if p is not None and len(getattr(p, "faces", [])) > 0
    ]
    if custom_collision_parts and verbose:
        names = ", ".join(
            sorted(
                {p.metadata.get("ucx_name", "UCX_?") for p in custom_collision_parts}
            )
        )
        info(
            f"  Found {len(custom_collision_parts)} custom collision mesh part(s) in the source file (named UCX_*): {names}.\n"
            f"These will be used as-is instead of auto-generating collision."
        )
    if custom_collision_parts and force_generate_collisions:
        if verbose:
            warn(
                "  --force-generate-coll was given, so the custom UCX_* mesh(es) found above will be ignored, and new collision will be generated from scratch instead."
            )
        custom_collision_parts = []
    if not parts:
        raise ValueError(
            "This file loaded, but it does not contain any visible 3D shapes (no faces/triangles were found).\n"
            "It may be an empty scene, a file that only holds an animation/skeleton, or a file that saved wrong.\n"
            "Try re-exporting or re-downloading it."
        )

    if strip_transparent:
        kept = []
        dropped = 0
        for part in parts:
            mat = getattr(part.visual, "material", None)
            if _material_is_transparent(mat):
                dropped += 1
                if verbose:
                    name = getattr(mat, "name", None) or "material"
                    info(
                        f"  [transparent] dropping part using material {name!r} (BLEND/MASK alphaMode or alpha < threshold), VotV only supports fully opaque geometry."
                    )
                continue
            kept.append(part)
        parts = kept
        if verbose and dropped:
            info(f"  Removed {dropped} transparent part(s).")
        if not parts:
            raise ValueError(
                "The --no-transparent option removed every single part of this model, because every material\n"
                "in it is non-opaque (glass, transparent plastic, etc). Try running again without --no-transparent."
            )

    dup_drop = _find_exact_duplicate_parts(parts, verbose=verbose)
    if dup_drop:
        parts = [p for i, p in enumerate(parts) if i not in dup_drop]

    model_dir = os.path.join(out_dir, out_name)
    preserved_properties = None
    try:
        if os.path.exists(model_dir):
            if not _looks_like_previous_output(model_dir, out_name):
                raise ValueError(
                    f'A folder called "{out_name}" already exists:\n    {model_dir}\n\n'
                    f"It wasn't created by this script, so the process was cancelled in case \n"
                    f"this was a mistake.\n\nRename, delete or move that folder, or run this again\n"
                    f"with a different output name."
                )
            cfg_old = os.path.join(model_dir, "properties.cfg")
            if os.path.isfile(cfg_old):
                with open(cfg_old, "rb") as fh:
                    preserved_properties = fh.read()
            shutil.rmtree(model_dir)
        os.makedirs(model_dir)
        if preserved_properties is not None:
            with open(os.path.join(model_dir, "properties.cfg"), "wb") as fh:
                fh.write(preserved_properties)
    except PermissionError:
        raise PermissionError(
            f"Your system won't let this script write to:\n    {model_dir}\n\n"
            f"This usually means a file inside that folder is open in another program (close it and\n"
            f"try again), or the folder is somewhere protected like Program Files.\n\n"
            f"Try running this from a normal folder such as your Desktop."
        )

    swatches = {}
    has_real_uv = {}
    mats_by_key = {}
    part_mat_keys = []
    clamped_uv_warning = False

    for part in parts:
        vis = part.visual
        mat = getattr(vis, "material", None)
        key = _material_key(mat)
        part_mat_keys.append(key)
        if key not in swatches:
            img, real_uv = _material_swatch(mat)
            if img.width > max_tile_size or img.height > max_tile_size:
                img = img.copy()
                img.thumbnail((max_tile_size, max_tile_size), Image.LANCZOS)
            swatches[key] = img
            has_real_uv[key] = real_uv
            mats_by_key[key] = mat

    if show_collisions:
        swatches[_COLLISION_VIS_KEY] = Image.new("RGBA", (32, 32), (255, 0, 0, 255))
        has_real_uv[_COLLISION_VIS_KEY] = False
        mats_by_key[_COLLISION_VIS_KEY] = None

    atlas_img, placements = _pack_atlas(swatches, verbose=verbose)

    red_uv = None
    if show_collisions:
        rp = placements[_COLLISION_VIS_KEY]
        red_uv = (
            (rp["x"] + 0.5 * rp["w"]) / rp["atlas_w"],
            ((rp["atlas_h"] - rp["y_top"] - rp["h"]) + 0.5 * rp["h"]) / rp["atlas_h"],
        )

    emissive_atlas_img = None
    if emissive_enabled and any(
        _material_has_emission(mats_by_key[k]) for k in swatches
    ):
        raw_w = max(p["atlas_w"] for p in placements.values())
        raw_h = max(p["atlas_h"] for p in placements.values())
        emissive_atlas_img = Image.new("RGBA", (raw_w, raw_h), (0, 0, 0, 255))
        pad = 4
        for key, p in placements.items():
            mat = mats_by_key[key]
            tile = _emissive_swatch(mat)
            if tile is None:
                continue
            if tile.size != (p["w"], p["h"]):
                tile = tile.resize((p["w"], p["h"]), Image.LANCZOS)
            padded = _pad_image_edge(tile, pad)
            emissive_atlas_img.paste(padded, (p["x"] - pad, p["y_top"] - pad))
        if verbose:
            emissive_count = sum(
                1 for k in swatches if _material_has_emission(mats_by_key[k])
            )
            info(
                f"  Built emissive atlas ({emissive_count}/{len(swatches)})."
            )

    normal_atlas_img = None
    if any(_material_has_normal(mats_by_key[k]) for k in swatches):
        raw_w = max(p["atlas_w"] for p in placements.values())
        raw_h = max(p["atlas_h"] for p in placements.values())
        normal_atlas_img = Image.new("RGB", (raw_w, raw_h), (128, 128, 255))
        pad = 4
        for key, p in placements.items():
            mat = mats_by_key[key]
            tile = _normal_swatch(mat, flip_green=flip_normal_green)
            if tile is None:
                continue
            if tile.size != (p["w"], p["h"]):
                tile = tile.resize((p["w"], p["h"]), Image.LANCZOS)
            padded = _pad_image_edge(tile.convert("RGBA"), pad).convert("RGB")
            normal_atlas_img.paste(padded, (p["x"] - pad, p["y_top"] - pad))
        if verbose:
            normal_count = sum(
                1 for k in swatches if _material_has_normal(mats_by_key[k])
            )
            info(
                f"  Built normal atlas ({normal_count}/{len(swatches)} materials have a normal map)."
            )

    pbr_atlas_img = None
    if any(_material_has_roughness(mats_by_key[k]) for k in swatches):
        raw_w = max(p["atlas_w"] for p in placements.values())
        raw_h = max(p["atlas_h"] for p in placements.values())
        pbr_atlas_img = Image.new("RGB", (raw_w, raw_h), (0, 128, 128))
        pad = 4
        for key, p in placements.items():
            mat = mats_by_key[key]
            tile = _pbr_swatch(mat)
            if tile is None:
                continue
            if tile.size != (p["w"], p["h"]):
                tile = tile.resize((p["w"], p["h"]), Image.LANCZOS)
            padded = _pad_image_edge(tile.convert("RGBA"), pad).convert("RGB")
            pbr_atlas_img.paste(padded, (p["x"] - pad, p["y_top"] - pad))
        if verbose:
            pbr_count = sum(
                1 for k in swatches if _material_has_roughness(mats_by_key[k])
            )
            info(
                f"  Built pbr atlas ({pbr_count}/{len(swatches)} materials have roughness data)."
            )

    if atlas_img.width > max_atlas_size or atlas_img.height > max_atlas_size:
        old_size = atlas_img.size
        ratio = min(max_atlas_size / atlas_img.width, max_atlas_size / atlas_img.height)
        new_size = (
            max(1, int(atlas_img.width * ratio)),
            max(1, int(atlas_img.height * ratio)),
        )
        atlas_img = atlas_img.resize(new_size, Image.LANCZOS)
        if verbose:
            warn(
                f"the diffuse atlas came out to {old_size[0]}x{old_size[1]}px, bigger than the {max_atlas_size}px limit, so it was shrunk to {new_size[0]}x{new_size[1]}px. Raise --max-atlas-size if the texture looks blurry."
            )

    if emissive_atlas_img is not None and (
        emissive_atlas_img.width > max_atlas_size
        or emissive_atlas_img.height > max_atlas_size
    ):
        ratio = min(
            max_atlas_size / emissive_atlas_img.width,
            max_atlas_size / emissive_atlas_img.height,
        )
        new_size = (
            max(1, int(emissive_atlas_img.width * ratio)),
            max(1, int(emissive_atlas_img.height * ratio)),
        )
        emissive_atlas_img = emissive_atlas_img.resize(new_size, Image.LANCZOS)

    if normal_atlas_img is not None and (
        normal_atlas_img.width > max_atlas_size
        or normal_atlas_img.height > max_atlas_size
    ):
        ratio = min(
            max_atlas_size / normal_atlas_img.width,
            max_atlas_size / normal_atlas_img.height,
        )
        new_size = (
            max(1, int(normal_atlas_img.width * ratio)),
            max(1, int(normal_atlas_img.height * ratio)),
        )
        normal_atlas_img = normal_atlas_img.resize(new_size, Image.LANCZOS)

    if pbr_atlas_img is not None and (
        pbr_atlas_img.width > max_atlas_size or pbr_atlas_img.height > max_atlas_size
    ):
        ratio = min(
            max_atlas_size / pbr_atlas_img.width, max_atlas_size / pbr_atlas_img.height
        )
        new_size = (
            max(1, int(pbr_atlas_img.width * ratio)),
            max(1, int(pbr_atlas_img.height * ratio)),
        )
        pbr_atlas_img = pbr_atlas_img.resize(new_size, Image.LANCZOS)

    mtl_name = "atlas_material"
    obj_lines = [f"mtllib {out_name}.mtl", f"usemtl {mtl_name}"]
    vert_offset = 0
    uv_offset = 0
    vt_cache = {}

    def _vt_index(new_u, new_v):
        nonlocal uv_offset
        key = f"{new_u:.6f} {new_v:.6f}"
        idx = vt_cache.get(key)
        if idx is None:
            obj_lines.append(f"vt {key}")
            uv_offset += 1
            idx = uv_offset
            vt_cache[key] = idx
        return idx

    total_verts = 0
    total_faces = 0
    tiled_split = 0
    tiled_unsplit = 0

    for i, (part, key) in enumerate(zip(parts, part_mat_keys)):
        verts = part.vertices * scale
        faces = part.faces
        p = placements[key]
        x_off, y_top, w, h, aw, ah = (
            p["x"],
            p["y_top"],
            p["w"],
            p["h"],
            p["atlas_w"],
            p["atlas_h"],
        )
        y_off_bottom = ah - y_top - h

        vis = part.visual
        uvs = None
        if (
            has_real_uv[key]
            and hasattr(vis, "uv")
            and vis.uv is not None
            and len(vis.uv) == len(verts)
        ):
            uvs = vis.uv

        obj_lines.append(f"# part_{i}")
        normals = np.asarray(part.vertex_normals, dtype=float)
        extra_pos, extra_nrm = [], []
        records = []  # ((i0,i1,i2), ((lu,lv),(lu,lv),(lu,lv)))
        n_base = len(verts)
        if uvs is not None:
            uv_arr = np.asarray(uvs, dtype=float)
            for f in faces:
                tri = [int(f[0]), int(f[1]), int(f[2])]
                fu = uv_arr[tri]
                mn, mx = fu.min(axis=0), fu.max(axis=0)
                cell = np.floor(mn + 1e-9)
                if np.all(mx <= cell + 1.0 + 1e-9):
                    loc = fu - cell
                    records.append(
                        (tuple(tri), tuple((float(a), float(b)) for a, b in loc))
                    )
                    continue
                n_cells = int(np.prod(np.floor(mx - 1e-9) - np.floor(mn + 1e-9) + 1))
                if n_cells > _MAX_UV_CELLS_PER_FACE:
                    tiled_unsplit += 1
                    loc = np.clip(fu - cell, 0.0, 1.0)
                    records.append(
                        (tuple(tri), tuple((float(a), float(b)) for a, b in loc))
                    )
                    continue
                tiled_split += 1
                pieces = _split_tri_on_uv_grid(verts[tri], normals[tri], fu)
                for pc in pieces:
                    cen = np.mean([v[2] for v in pc], axis=0)
                    pcell = np.floor(cen)
                    idxs = []
                    for pp, pn, pu in pc:
                        nn = np.linalg.norm(pn)
                        extra_pos.append(np.asarray(pp, dtype=float))
                        extra_nrm.append(
                            np.asarray(pn, dtype=float) / nn
                            if nn > 1e-12
                            else np.asarray(pn, dtype=float)
                        )
                        idxs.append(
                            (
                                n_base + len(extra_pos) - 1,
                                tuple(np.clip(np.asarray(pu) - pcell, 0.0, 1.0)),
                            )
                        )
                    for k in range(1, len(idxs) - 1):
                        a, b, c = idxs[0], idxs[k], idxs[k + 1]
                        area = abs(
                            (b[1][0] - a[1][0]) * (c[1][1] - a[1][1])
                            - (c[1][0] - a[1][0]) * (b[1][1] - a[1][1])
                        )
                        if area < 1e-10:
                            continue
                        records.append(((a[0], b[0], c[0]), (a[1], b[1], c[1])))
        else:
            for f in faces:
                records.append(((int(f[0]), int(f[1]), int(f[2])), None))

        all_verts = verts if not extra_pos else np.vstack([verts, np.array(extra_pos)])
        all_nrms = (
            normals if not extra_nrm else np.vstack([normals, np.array(extra_nrm)])
        )
        for v in all_verts:
            obj_lines.append(f"v {v[0]:.6f} {v[1]:.6f} {v[2]:.6f}")
        for n in all_nrms:
            obj_lines.append(f"vn {n[0]:.6f} {n[1]:.6f} {n[2]:.6f}")

        eu, ev = 0.5 / max(w, 1), 0.5 / max(h, 1)
        center_u = (x_off + 0.5 * w) / aw
        center_v = (y_off_bottom + 0.5 * h) / ah
        for idx3, corner_uv in records:
            vt_idx = []
            for k in range(3):
                if corner_uv is None:
                    vt_idx.append(_vt_index(center_u, center_v))
                else:
                    lu = eu + corner_uv[k][0] * (1.0 - 2 * eu)
                    lv = ev + corner_uv[k][1] * (1.0 - 2 * ev)
                    vt_idx.append(
                        _vt_index((x_off + lu * w) / aw, (y_off_bottom + lv * h) / ah)
                    )
            obj_lines.append(
                f"f {idx3[0] + 1 + vert_offset}/{vt_idx[0]}/{idx3[0] + 1 + vert_offset} "
                f"{idx3[1] + 1 + vert_offset}/{vt_idx[1]}/{idx3[1] + 1 + vert_offset} "
                f"{idx3[2] + 1 + vert_offset}/{vt_idx[2]}/{idx3[2] + 1 + vert_offset}"
            )

        verts = all_verts
        faces = records

        vert_offset += len(verts)
        total_verts += len(verts)
        total_faces += len(faces)

    diffuse_filename = f"diffuse_{out_name}.png"
    emissive_filename = (
        f"emissive_{out_name}.png" if emissive_atlas_img is not None else None
    )
    normal_filename = f"normal_{out_name}.png" if normal_atlas_img is not None else None
    pbr_filename = f"pbr_{out_name}.png" if pbr_atlas_img is not None else None
    mtl_lines = [
        f"newmtl {mtl_name}",
        "Kd 1.0000 1.0000 1.0000",
        "Ka 0.2 0.2 0.2",
        "Ks 0.1 0.1 0.1",
        "Ns 10.0",
        "d 1.0000",
        f"map_Kd {diffuse_filename}",
    ]
    if emissive_filename is not None:
        mtl_lines.append(f"map_Ke {emissive_filename}")
    if normal_filename is not None:
        mtl_lines.append(f"map_Bump {normal_filename}")
    mtl_lines.append("")

    hull_verts = hull_faces = 0
    collision_start = len(obj_lines)
    collision_v0 = vert_offset
    if custom_collision_parts:
        used_names = {}
        ucx_total_verts = ucx_total_faces = 0
        for cpart in custom_collision_parts:
            base_name = _sanitize_obj_object_name(
                cpart.metadata.get("ucx_name") or f"UCX_{out_name}"
            )
            n = used_names.get(base_name, 0)
            used_names[base_name] = n + 1
            ucx_name = base_name if n == 0 else f"{base_name}_{n:02d}"

            obj_lines.append(f"o {ucx_name}")
            cv = cpart.vertices * scale
            cv[:, 0] += collision_offset_x
            cv[:, 1] += collision_offset_y
            for v in cv:
                obj_lines.append(f"v {v[0]:.6f} {v[1]:.6f} {v[2]:.6f}")
            for f in cpart.faces:
                obj_lines.append(
                    f"f {f[0] + 1 + vert_offset} {f[1] + 1 + vert_offset} {f[2] + 1 + vert_offset}"
                )
            vert_offset += len(cpart.vertices)
            hull_verts, hull_faces = len(cpart.vertices), len(cpart.faces)
            ucx_total_verts += len(cpart.vertices)
            ucx_total_faces += len(cpart.faces)

        if verbose:
            success(
                f"  Used {len(custom_collision_parts)} custom collision meshes: {ucx_total_verts} verts, {ucx_total_faces} faces total."
            )
            if not generate_collision:
                info(
                    "  (--no-coll was given, but it only suppresses auto-generated collision, the custom UCX_ mesh(es) found in the source file are still used.)"
                )
    elif generate_collision:
        try:
            merged_for_hull = trimesh.util.concatenate(parts)

            hulls = None
            if collision_quality == "high":
                try:
                    hulls = merged_for_hull.convex_decomposition(
                        maxConvexHulls=max_decomp_hulls
                    )
                    if not hulls:
                        raise ValueError("convex_decomposition returned no hulls")
                    if verbose:
                        info(
                            f"  High-quality collision: decomposed into {len(hulls)} convex hull(s)."
                        )
                except Exception as e:
                    if verbose:
                        warn(
                            f"convex decomposition failed on this model's geometry ({e}), falling back to low quality collision"
                        )
                    hulls = None

            if hulls is None:
                hulls = [merged_for_hull.convex_hull]

            ucx_total_verts = ucx_total_faces = 0
            hulls_not_trimmed = 0
            for i, hull in enumerate(hulls):
                if len(hull.faces) > max_hull_faces and not _HAVE_FAST_SIMPLIFICATION:
                    hulls_not_trimmed += 1
                elif len(hull.faces) > max_hull_faces:
                    pre_faces = len(hull.faces)
                    try:
                        hull = hull.simplify_quadric_decimation(
                            face_count=max_hull_faces
                        )
                        if verbose:
                            info(
                                f"  Collision hull {i} had {pre_faces} faces, simplified to {len(hull.faces)}."
                            )
                    except Exception as e:
                        if verbose:
                            warn(
                                f"hull {i} had {pre_faces} faces and could not be simplified ({e}). Physics may be unstable."
                            )

                hull_verts = len(hull.vertices)
                hull_faces = len(hull.faces)
                ucx_total_verts += hull_verts
                ucx_total_faces += hull_faces
                ucx_name = (
                    f"UCX_{out_name}" if len(hulls) == 1 else f"UCX_{out_name}_{i:02d}"
                )
                obj_lines.append(f"o {ucx_name}")
                hv = hull.vertices * scale
                hv[:, 0] += collision_offset_x
                hv[:, 1] += collision_offset_y
                for v in hv:
                    obj_lines.append(f"v {v[0]:.6f} {v[1]:.6f} {v[2]:.6f}")
                for f in hull.faces:
                    obj_lines.append(
                        f"f {f[0] + 1 + vert_offset} {f[1] + 1 + vert_offset} {f[2] + 1 + vert_offset}"
                    )
                vert_offset += hull_verts

            if hulls_not_trimmed and verbose:
                warn(
                    f"{hulls_not_trimmed} collision hulls have more than {max_hull_faces} faces but were left as-is "
                    f"cuz the fast_simplification library isn't installed."
                )
            if verbose:
                info(
                    f"  Added {len(hulls)} UCX_ collision hull(s): {ucx_total_verts} verts, {ucx_total_faces} faces total."
                )
        except Exception as e:
            if verbose:
                warn(
                    f"could not generate a collision shape for this model ({e}). Added a fallback box collision at the base so it doesn't fall through the floor."
                )
            fallback_box = _make_fallback_collision_box(
                parts,
                fallback_collision_size,
                scale,
                offset_x=collision_offset_x,
                offset_y=collision_offset_y,
            )
            if fallback_box is not None:
                obj_lines.append(f"o UCX_{out_name}")
                for v in fallback_box.vertices:
                    obj_lines.append(f"v {v[0]:.6f} {v[1]:.6f} {v[2]:.6f}")
                for f in fallback_box.faces:
                    obj_lines.append(
                        f"f {f[0] + 1 + vert_offset} {f[1] + 1 + vert_offset} {f[2] + 1 + vert_offset}"
                    )
                vert_offset += len(fallback_box.vertices)
                hull_verts, hull_faces = (
                    len(fallback_box.vertices),
                    len(fallback_box.faces),
                )
    elif fallback_collision and not custom_collision_parts:
        fallback_box = _make_fallback_collision_box(
            parts,
            fallback_collision_size,
            scale,
            offset_x=collision_offset_x,
            offset_y=collision_offset_y,
        )
        if fallback_box is not None:
            obj_lines.append(f"o UCX_{out_name}")
            for v in fallback_box.vertices:
                obj_lines.append(f"v {v[0]:.6f} {v[1]:.6f} {v[2]:.6f}")
            for f in fallback_box.faces:
                obj_lines.append(
                    f"f {f[0] + 1 + vert_offset} {f[1] + 1 + vert_offset} {f[2] + 1 + vert_offset}"
                )
            vert_offset += len(fallback_box.vertices)
            hull_verts, hull_faces = len(fallback_box.vertices), len(fallback_box.faces)
            if verbose:
                info(
                    f"  --no-coll with --fallback-coll: added a {fallback_collision_size:.1f}cm fallback box"
                )

    outline_stats = None
    if show_collisions:
        allv = np.vstack([np.asarray(p.vertices) for p in parts]) * scale
        collision_push = max(
            0.5, 0.015 * float(np.linalg.norm(allv.max(axis=0) - allv.min(axis=0)))
        )
        obj_lines, outline_stats = _add_collision_outline(
            obj_lines, collision_start, collision_v0, red_uv, collision_push
        )
        if not outline_stats and verbose:
            warn("--show-coll: this model has no collision shapes to show")

    atlas_img.save(os.path.join(model_dir, diffuse_filename))
    if emissive_atlas_img is not None:
        emissive_atlas_img.save(os.path.join(model_dir, emissive_filename))
    if normal_atlas_img is not None:
        normal_atlas_img.save(os.path.join(model_dir, normal_filename))
    if pbr_atlas_img is not None:
        pbr_atlas_img.save(os.path.join(model_dir, pbr_filename))
    with open(os.path.join(model_dir, f"{out_name}.obj"), "w") as f:
        f.write("\n".join(obj_lines) + "\n")
    with open(os.path.join(model_dir, f"{out_name}.mtl"), "w") as f:
        f.write("\n".join(mtl_lines) + "\n")

    if preserved_properties is not None:
        if verbose:
            info("  Kept the existing properties")
            if edible or physical_material:
                warn(
                    "--edible / --physical-material were not applied, "
                    "cuz the model properties were preserved."
                )
    else:
        _write_properties_cfg(
            model_dir,
            emissive=emissive_atlas_img is not None,
            emissive_strength=emissive_strength,
            edible=edible,
            physical_material=physical_material,
        )

    if verbose:
        info(
            f"  Parts: {len(parts)}  Unique materials baked: {len(swatches) - (1 if show_collisions else 0)}  Verts: {total_verts}  Faces: {total_faces}"
        )
        if outline_stats:
            info(
                f"  --show-coll: added a red copy of the collision ({outline_stats} faces)"
            )
        if tiled_split:
            info(
                f"  Tiled textures: {tiled_split} faces were cut along texture-repeat lines to keep tiling intact."
            )
        if tiled_unsplit:
            warn(
                f"{tiled_unsplit} faces repeat their texture more than {_MAX_UV_CELLS_PER_FACE} times and were left squeezed into a single tile (expect stretch issues)."
            )
        success(f"Your VotV model is in: {model_dir}")
        for fn in sorted(os.listdir(model_dir)):
            size = os.path.getsize(os.path.join(model_dir, fn))
            print(f"  {fn}  ({size / 1024:.1f} KB)")

    return model_dir


def _write_properties_cfg(
    model_dir,
    filter_diffuse=0,
    filter_pbr=0,
    filter_normal=0,
    filter_emissive=0,
    physical_material=0,
    health=-1,
    impact_resistance=1,
    damage_resistance=1,
    emissive=False,
    emissive_strength=10.0,
    is_lamp=0,
    lamp_color=(1.0, 1.0, 1.0),
    lamp_offset=(0.0, 0.0, 0.0),
    lamp_intensity=5001,
    lamp_attenuation=2500,
    lamp_shadows=0,
    light_toggle=1,
    edible=False,
):
    cfg_path = os.path.join(model_dir, "properties.cfg")
    lr, lg, lb = lamp_color
    lx, ly, lz = lamp_offset
    if edible:
        physical_material = 8
    content = (
        f"filter_diffuse_={filter_diffuse}\n"
        f"filter_pbr_={filter_pbr}\n"
        f"filter_normal_={filter_normal}\n"
        f"filter_emissive_={filter_emissive}\n"
        f"physical_material={physical_material}\n"
        f"health={health}\n"
        f"impact_resistance={impact_resistance}\n"
        f"damage_resistance={damage_resistance}\n"
        f"is_lamp={is_lamp}\n"
        f"lamp_color=(R={lr},G={lg},B={lb})\n"
        f"lamp_offset=(X={lx},Y={ly},Z={lz})\n"
        f"lamp_intensity={lamp_intensity}\n"
        f"lamp_attenuation={lamp_attenuation}\n"
        f"lamp_shadows={lamp_shadows}\n"
        f"light_toggle={light_toggle}\n"
    )
    if emissive:
        content += f"emissive_strength={emissive_strength}\n"
    with open(cfg_path, "w") as f:
        f.write(content)


def main():
    if len(sys.argv) == 1:
        info("Steps to convert your model:")
        print("  1. Get your glb model.")
        print("  2. Put the model and python script in the same folder.")
        print(
            f"  3. Run {_C.BOLD}py printer_convert.py yourmodel{_C.RESET} inside that folder."
        )
        print(
            f"  4. Drop the new folder into {_C.YELLOW}%LocalAppData%\\VotV\\Assets\\meshes\\printer{_C.RESET}."
        )
        _wait_before_exit(force=True)
        sys.exit(0)

    p = argparse.ArgumentParser(
        description="Convert a GLB into a VotV 3D Printer style folder."
    )
    p.add_argument("input", help="model file, .glb assumed if no extension")
    p.add_argument(
        "output",
        nargs="?",
        default=None,
        help="output name or path, defaults to input filename in current dir",
    )
    p.add_argument(
        "--scale",
        type=float,
        default=100.0,
        help="scale multiplier, glTF is meters and VotV is cm so default 100 converts, use 1 if already cm",
    )
    p.add_argument(
        "--humanoid",
        action="store_true",
        help="apply skinned mesh placement using skeleton data",
    )
    p.add_argument(
        "--max-tile-size",
        type=int,
        default=1024,
        help="max texture size before atlas packing (px)",
    )
    p.add_argument(
        "--max-atlas-size",
        type=int,
        default=4096,
        help="max size of final diffuse atlas (px)",
    )
    p.add_argument(
        "--no-coll", action="store_true", help="skip auto collision generation"
    )
    p.add_argument(
        "--force-generate-coll",
        action="store_true",
        help="ignore UCX_* meshes in source, regen collision",
    )
    p.add_argument(
        "--fallback-coll",
        action="store_true",
        help="add a small box collider if model has none",
    )
    p.add_argument(
        "--fallback-coll-size",
        type=float,
        default=_FALLBACK_BOX_SIZE_CM,
        help=f"fallback box edge length in cm (default {_FALLBACK_BOX_SIZE_CM:.1f})",
    )
    p.add_argument(
        "--coll-offset-x",
        type=float,
        default=0.0,
        help="horizontal collision offset (cm)",
    )
    p.add_argument(
        "--coll-offset-y",
        type=float,
        default=0.0,
        help="vertical collision offset (cm)",
    )
    p.add_argument(
        "--max-hull-faces",
        type=int,
        default=None,
        help="max faces per hull, defaults to 200 (high) / 80 (low)",
    )
    p.add_argument(
        "--coll-quality",
        choices=["low", "high"],
        default="high",
        help="low = single hull, high = VHACD decomposition",
    )
    p.add_argument(
        "--max-decomp-hulls",
        type=int,
        default=64,
        help="max hulls from high-quality decomposition",
    )
    p.add_argument(
        "--no-emissive", action="store_true", help="don't bake an emissive atlas"
    )
    p.add_argument(
        "--physical-material",
        type=int,
        default=0,
        choices=range(16),
        metavar="0-15",
        help="impact/footstep sound material: 0 default, 1 wood, 2 metal, 3 concrete, 4 rubber, 5 paper, 6 hollow metal, 7 flesh, 8 plush, 9 metal2, 10 pinecone, 11 glass, 12 cardboard, 13 heavy metal, 14 no sound, 15 rubber2",
    )
    p.add_argument(
        "--no-flip-normal-green",
        action="store_true",
        help="don't invert the normal map green channel",
    )
    p.add_argument(
        "--edible",
        action="store_true",
        help="mark print as edible (physical_material=8)",
    )
    p.add_argument(
        "--show-coll",
        action="store_true",
        help="add a red copy of the collision pushed slightly outward",
    )
    p.add_argument("--pause", action="store_true", help="wait for Enter before exiting")
    p.add_argument(
        "--no-transparent",
        action="store_true",
        help="drop transparent/masked materials (VotV has no alpha blending)",
    )
    args = p.parse_args()

    if args.output is None:
        out_dir = "."
        out_name = os.path.splitext(os.path.basename(args.input))[0]
    else:
        out_dir, out_name = os.path.split(args.output)
        if out_dir == "":
            out_dir = "."
        if out_name == "":
            out_name = os.path.splitext(os.path.basename(args.input))[0]

    if args.max_hull_faces is None:
        max_hull_faces = 200 if args.coll_quality == "high" else 80
    else:
        max_hull_faces = args.max_hull_faces

    try:
        export_votv_model(
            args.input,
            out_name,
            out_dir=out_dir,
            scale=args.scale,
            max_tile_size=args.max_tile_size,
            max_atlas_size=args.max_atlas_size,
            generate_collision=not args.no_coll,
            max_hull_faces=max_hull_faces,
            collision_quality=args.coll_quality,
            max_decomp_hulls=args.max_decomp_hulls,
            emissive_enabled=not args.no_emissive,
            edible=args.edible,
            strip_transparent=args.no_transparent,
            humanoid=args.humanoid,
            fallback_collision=args.fallback_coll,
            fallback_collision_size=args.fallback_coll_size,
            collision_offset_x=args.coll_offset_x,
            collision_offset_y=args.coll_offset_y,
            force_generate_collisions=args.force_generate_coll,
            flip_normal_green=not args.no_flip_normal_green,
            physical_material=args.physical_material,
            show_collisions=args.show_coll,
        )
    except (FileNotFoundError, ValueError, PermissionError) as e:
        error(str(e))
        _wait_before_exit(force=True)
        sys.exit(1)
    except MemoryError:
        error(
            "Ran out of memory, the model is probably too large. Try closing other programs, or lower --max-tile-size and --max-atlas-size to use less memory."
        )
        _wait_before_exit(force=True)
        sys.exit(1)
    except Exception as e:
        error(
            f"Something unexpected went wrong: {e}\n\n"
            f"When reporting an issue, copy this whole message (including the text above) and send it\n"
            f"along with the model file you were trying to convert."
        )
        _wait_before_exit(force=True)
        sys.exit(1)

    _wait_before_exit(force=args.pause or _BONE_WARNED)


if __name__ == "__main__":
    main()
