# The printer size check and the "Print error" message

Decoded from `objects/3dPrinterAnim` (UAssetGUI bytecode, `ExecuteUbergraph_3dPrinterAnim`), plus `ui_printer` and
`3dPrinter_desktop`. Full pseudo-code: `bytecode/3dPrinterAnim.dump.txt`, `bytecode/ui_printer.ubergraph-and-gen.dump.txt`.
Labels: [V] decoded, [I] inferred, [?] unknown. Statement numbers are indexes in those dumps.

## Short answer [V]

The size check is **not** done when you press Print. It runs when a model is **selected** (`selectMesh`):

```
sumOfVerts = all vertices of all geometry sections
box        = vecBoundBox(sumOfVerts).box                      (half-extents)
fits       = canBoxFitInBox(box_b.BoxExtent, box)             (statement 95)
if !fits: hint "Error: mesh is too big for printer. Mesh bounding box should be less than ({box_b.BoxExtent})"
          Array_Clear(sumOfVerts)                              (statements 114-125)
```

Later, **Print** calls `print()`, which sees a loaded mesh (`multisectionMeshData.geometry[0].vertices[0]` exists) but an **empty
`sumOfVerts`**, and shows the generic `addHint("Print error", 2)` (statements 17-18 -> 66). Print therefore reports one
message for every selection failure, and the reason hints were only shown earlier, at selection time.

There are **two printers** that share this code (`3dPrinterAnim` is the base class and is itself the large printer; `3dPrinter_desktop` derives from it as the small one). Each has its own `box_b`:

| | Large printer (`3dPrinterAnim`, base class) | Small printer (`3dPrinter_desktop`) |
|---|---|---|
| `box_b.BoxExtent` (half-extents) | **(100, 100, 150)** [V, base-class default; confirmed live by the in-game hint `BIG BOX: X=100.000 Y=100.000 Z=150.000` [M]] | **(22.5, 22.5, 30)** [V, class default] |
| `box_b` relative location | (0, 0, 150) | (0, 0, 30) |
| Full size if half-extents [I] | **200 x 200 x 300** | **45 x 45 x 60** |
| `box_s` default | (100, 100, 150) | (22.5, 22.5, 30) |
| `maxHeight` | 300 | 60 |
| `q_rad`, `q_lin` | 30, 75 | 60, 200 |
| `speed`, `name` | not overridden in the base class | 100, `printer_d` |
| Meshes | `meshes/printer/*` (presumably) [I] | `meshes/desktopPrinter/*` |

Which printer a player uses decides which limit applies. The hint shown on selection prints that printer's limit as `BIG BOX`.

## Event entry points [V]

Recorded entry points in the ubergraph (the stub functions call `ExecuteUbergraph_3dPrinterAnim(N)`):

| Event | Entry | Lands on |
|---|---|---|
| `print` | 8909 | statement 233 `goto @15` -> statement 2 (see below) |
| `selectMesh(meshName)` | 9377 | statement 244 |
| `finish` | 9680 | statement 259 |
| `ReceiveTick` | 5670 | tick animation |
| `ReceiveBeginPlay` | 5605 | |

Offsets in the dump are computed sizes; the cooked graph has unsized nodes (`EX_ComputedJump`, `EX_SkipOffsetConst`), so recorded jump
targets are larger than the dump's `@N` by 9 near the start (13 by the end). Example: `if !(IsValidIndex(sumOfVerts,0)) goto @3003` hits
statement 66 (`@2994` in the dump). Statement order is exact.

## `selectMesh(meshName)` [V]

1. `if !(active) goto` continue; `meshName = <arg>; active = false; canPrint = false` (statements 244-254).
2. `multisectionMeshData = gamemode.loadPrinterMesh(meshName, skipMessages = false, importConfig = false)` (255-256). Note `importConfig = false`.
3. `Array_Clear(sumOfVerts)` (257), then loop over `multisectionMeshData.geometry` (73-90): first section assigns `sumOfVerts = vertices`, later sections `Array_Append`. **All sections' visible vertices; the collision hulls (`coll`) are not part of `geometry`** [I].
4. `if !(loadPrinterMesh succ)` -> `addHint("Error importing the model", 2)` (91 -> 129).
5. `if !(IsValidIndex(sumOfVerts, 0))` -> `addHint("Bounding box error code 1", 2)` (92-93 -> 127).
6. `vecBoundBox(sumOfVerts) -> min, max, box, origin, boundingCenter` (94).
7. `fits = canBoxFitInBox(box_b.BoxExtent, box)` (95); `displayBox(box, box_b.BoxExtent)` (96, unconditional).
8. **Fits** (98-113): hint `"Mesh data successfully imported"` (type 0); `mesh.SetRelativeLocation(-boundingCenter + (0, 0, box.Z) + (0, 0, 0.01))`;
   `offset = -boundingCenter + (0, 0, box.Z)`; `gamemode.printer.info(sumOfVerts)`; then `gamemode.printer.gatherConfigCurrentName(multisectionMeshData)`.
9. **Does not fit** (97 -> 114-126): format `"Error: mesh is too big for printer. Mesh bounding box should be less than ({a})"` with `a = Conv_VectorToString(box_b.BoxExtent)`,
   `addHint(text, 1)`, then **`Array_Clear(sumOfVerts)`**. Flow continues to the same tail as success (`gatherConfigCurrentName`), so the UI still shows the model's config.

### `displayBox(s, b)` [V]

Parameter order is `(s, b)`; it is called with `s = mesh box`, `b = box_b.BoxExtent`. It draws a green debug box of extent `s` and a red one of extent `b`
at `box_b`'s location (5 s, `DrawDebugBox`, normally compiled out of shipping builds [I]), shows hint `"SMALL BOX: {s}\r\nBIG BOX: {b}"` (type 0), then
`box_b.SetBoxExtent(b)` (no change) and `box_s.SetBoxExtent(s)`. **`box_s` is the visual box of the selected mesh; `box_b` is the printer's limit box.**
There is no limit drift between selections.

So on every selection the game itself tells you the mesh half-extents (`SMALL BOX`) and the limit (`BIG BOX`) in a hint, if hints are visible.

## `print()` [V]

Statements 2-18 then 66:

1. `lib.isCustom_func(self) -> isCustom`; if false, return silently (meaning of "custom" [?]).
2. If `active` (already printing), return.
3. `multisectionMeshData.geometry[0]` and its `vertices[0]` must exist, otherwise return **silently** (no mesh selected).
4. `height = 0; L_height = -0.1; mode = false; active = false; canPrint = false; mesh.ClearMeshSection(0)`.
5. **`if !(IsValidIndex(sumOfVerts, 0))` -> `lib.addHint("Print error", 2, false)` and stop.**
6. Otherwise: clear `dynmats`, build one procedural section + dynamic material per geometry entry (`tex/pbr/normal/emissive/offset/inprint_color`),
   `makePath()`, and run the print animation (`active`, `mode`, `height`, `finish` etc.).

`ui_printer`: `Button_print` -> `printer.print()` directly with no UI-side check. `Button_printCheat` -> `printer.finish()` (jumps to the finished state, `mode = true`,
`height = 300`). Neither shows messages of its own.

## Everything that produces "Print error" [V logic, I for user-facing causes]

A mesh must be loaded (geometry[0].vertices[0]) and `sumOfVerts` empty at Print time. `sumOfVerts` is empty after any of these at selection:

- bounding box too big (`canBoxFitInBox` false) [V];
- `loadPrinterMesh` failed (`succ` false): hint "Error importing the model" [V];
- no vertices summed: hint "Bounding box error code 1" [V];
- or a previous selection failed and the player pressed Print anyway.

Pressing Print with no mesh selected, while printing, or when `isCustom_func` is false does **not** show "Print error"; it does nothing.

## Limits [V unless marked]

| Value | Where | Number |
|---|---|---|
| `box_b.BoxExtent` | desktop printer: `3dPrinter_desktop` default; large printer: `3dPrinterAnim` default | (22.5, 22.5, 30) desktop; **(100, 100, 150)** large |
| `box_b.RelativeLocation` | same | (0, 0, 30) -> box spans z 0..60 above the plate |
| `box_s.BoxExtent` | same, replaced at run time by the mesh box | (22.5, 22.5, 30) default |
| `maxHeight` | `3dPrinter_desktop` / `3dPrinterAnim` defaults | 60.0 desktop, 300.0 large (animation clamp: `FClamp(height, 0, maxHeight)`) |
| `q_rad`, `q_lin` | same | 60, 200 desktop; 30, 75 large (path resolution in `makePath`: loop `<= q_rad`, `<= q_lin`) |

**Half vs full size [I, strong].** The mesh is placed at `-center + (0, 0, box.Z)`, so `box.Z` is the **half**-height (bottom of the mesh lands at z about 0.01),
and `box_b` spans z 0..2*Z above the plate (`RelativeLocation.Z` equals `BoxExtent.Z` on both printers: 30/30 and 150/150). Both are half-extents, so a mesh fits when each axis is within **45 x 45 x 60 cm** (small printer) or **200 x 200 x 300 cm** (large printer) in full size.
The hint prints the half-extents, which is why the message looks like it asks for a smaller size than the printer can hold.

The check uses the **visible geometry vertices after the importer's transform**, **before** any config (`importConfig = false`) [I], so a `scale` in `properties.cfg`
does not rescue a too-big mesh at the check, although it may still change the printed object later [?].

## Why the player sees only "Print error" [I]

The specific reason is shown once at selection time (`addHint` type 1/2 plus a type-0 "SMALL BOX / BIG BOX" hint, all in one frame), while the **Print** click shows the generic type-2 hint.
How `lib.addHint` displays and stacks hints (and whether several hints in one frame hide each other) is in `main/lib`, which has not been exported [?].

## Not decoded [?]

- `bpCodeLib::canBoxFitInBox` and `vecBoundBox` live in the native module **`/Script/VotV`** (C++ in the exe, not in the pak). Whether the comparison is `<` or `<=`, per axis, or allows axis swapping is not visible.
- Units: the importer's scale from OBJ units to game units (Real Time Import plugin).
- What `lib.isCustom_func` tests; `lib.addHint` types (0 info, 1 warning, 2 error is the natural reading).
- Whether there are more printer variants: the pak list shows only `3dPrinterAnim` (large) and `3dPrinter_desktop` (small).

## Verifying and tooling

- `python scripts/print_size_check.py model.obj --scale S` applies the rule above (half-extents versus (22.5, 22.5, 30), UCX hulls skipped), prints margins and the uniform scale that would fit.
- Calibrate units and axes once: select any model and read the hint `SMALL BOX: (X= Y= Z=)`; it must equal the script's half-extents.
- Boundary test: print cubes of 44.9 / 45.0 / 45.1 cm to learn strict versus inclusive.
- To raise a limit (mod): the small printer's `box_b` default lives in the data-only asset `3dPrinter_desktop` (about 12 KB), the large printer's in `3dPrinterAnim`; the plate meshes and `maxHeight` would need to follow [I].

## Status of the limit (what is and is not settled)

Settled [V]: `box_b.BoxExtent` defaults (22.5, 22.5, 30) for the small printer (`3dPrinter_desktop`) and (100, 100, 150) for the large printer (`3dPrinterAnim`, confirmed live by the `BIG BOX` hint); the single check `canBoxFitInBox(box_b.BoxExtent, vecBoundBox(sumOfVerts).box)` in `selectMesh`; failure clears `sumOfVerts`, so `print()` later shows only "Print error"; `maxHeight` 60 only clamps the print animation; `box_b` is read in three places in `3dPrinterAnim`.
Not settled [?]: the native comparison rule (`<` vs `<=`, per axis, any swapping); that both boxes are half-extents (strongly implied by the mesh placement, not confirmed); the importer's OBJ-unit to cm scale; whether anything outside `3dPrinterAnim` changes `box_b` at runtime (the `3dPrinter_desktop` bytecode was not exported); how config scale interacts with the check.
Direct reads: the selection hint `SMALL BOX` / `BIG BOX` shows the mesh half-sizes and the live limit; `scripts/ue4ss_dump_icon_pipeline.lua` now also prints the printer's `box_b`, `box_s`, `maxHeight`, `q_rad`, `q_lin` and actor scale; cubes of 44.9 / 45.0 / 45.1 cm settle `<` vs `<=`.

## Measured sample [M, from a player screenshot of the selection hint, large printer]

`SMALL BOX: X=63.080 Y=28.082 Z=89.415` / `BIG BOX: X=100.000 Y=100.000 Z=150.000`. `BIG BOX` equals the base-class `box_b` default exactly, so this was the large printer. The mesh half-extents (126 x 56 x 179 cm full) fit with margins of (36.9, 71.9, 60.6). This also confirms the hint format and that `SMALL BOX` is the mesh box and `BIG BOX` is the printer's live limit box. The model's file dimensions were not given, so the import scale is still unmeasured [?].
Because this info hint (blue "i") is visible to players while the too-big warning (hint type 1) is reportedly not, **compare `SMALL BOX` against `BIG BOX` on each axis yourself**: if the mesh is within the box on all three axes and Print still says "Print error", the cause is not this size comparison (see the other triggers above), or the native comparison is stricter than a per-axis half-extent test.
