# icon-rig-fit

Scripts that were used to measure the game's icon rig against a real `icon.png` (2026-11-01, GUI agent).
They are not part of the app and are never built by cargo.

* `glb.py`, `geo.py`: read a `.glb` and pose it like the converter does in humanoid mode (skin matrices, `joint world * inverse bind`).
* `rend.py`: a small numpy software renderer (perspective, z-buffer, alpha mask, Lambert, culling) that mirrors the manager's
  icon render. Validated against the manager's own icon of the chicken: silhouette IoU 0.993.
* `fit_axes.py`: tries all 48 signed axis permutations as OBJ -> Unreal mapping and ranks them against a game icon.
* `samples/`: the game's icon of the chicken (`game-generated-icon.png`) and the manager's earlier, wrong one.
  The model (`chicken.glb`, 2.9 MB) is not included; ask the user for it.

Result (see `src/gamerig.rs`): Unreal (x, y, z) = OBJ (z, x, y); background black; sRGB output; key light ~1.07/pi per intensity unit.
