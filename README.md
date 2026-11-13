<h2>
  <img src="assets/icon.png" width="32" />
  <strong>VotV Model Manager</strong>
</h2>

An app for managing your custom **3D printer models** in [Voices of the Void](https://mrdrnose.itch.io/votv).

Browse your models, check them in a 3D viewer, and drag any `.glb` onto the window to turn it into a model the game can print.

![demo](readme/demo.webp)

## Contents

- [Download](#download)
- [Getting started](#getting-started)
- [Your library](#your-library)
- [The 3D preview](#the-3d-preview)
- [Converting and editing](#converting-and-editing)
- [Blender](#blender)
- [The app](#the-app)
- [Good to know](#good-to-know)
- [Something went wrong?](#something-went-wrong)
- [For developers](#for-developers)

## Download

1. Open the **Releases** page of this repository (link on the right-hand side of the GitHub page).
2. Download the latest executable.
3. Launch it. No installer, it's portable.

## Getting started

1. **Start the app.** It points to your VotV printer folder automatically:
   `%LocalAppData%\VotV\Assets\meshes\printer`
   If your models are somewhere else, use the gear menu at the top and choose **Change folder...**.
2. **Click a model** to see it in the 3D preview.
3. **Add a new model:** drag a `.glb` file onto the window (or click the "Drop a .glb here" tile). The editor opens and converts it straight away.
4. **Adjust the settings** in the panel on the right. Every change regenerates the model, and the model is saved into your library immediately.

## Your library

### See your models at a glance

Every model gets an icon, drawn the way the game draws its own. The list has two sections.
- **EDITABLE**:models made with the manager, which can be opened and edited again
- **MODELS**:everything else in your printer folder). Disabled models are dimmed.

![](readme/library-grid.png)

### Search

Type in the search box above the list to filter the models by name.

![](readme/search.gif)

### Sort each section

Each section header has its own sort button. Click it to cycle **Last Added**, **Last Modified** and **Alphabetic**.

![](readme/sort.gif)

### Select one or many

Click selects a model, **Ctrl + click** adds or removes one, **Shift + click** selects everything from the last clicked model to this one.

![](readme/select.gif)

### Rename

Renames the model and every file that carries its name, including the textures, and keeps the references inside the model files in step.

![](readme/rename.gif)

### Regenerate icons

Makes fresh icons for the selected models.

![](readme/regen-icons.gif)

### Disable and enable

Hides a model from the game without deleting it.

![](readme/disable.gif)

### Lock a model

Removes the saved `.glb` so a model can no longer be edited.

![](readme/lock.gif)

### Delete

Removes models for good. The red button asks for a second click first.

![](readme/delete.gif)

### Import ready-made models

Drop model folders or archives (`.zip`, `.7z` or `.rar`) onto the list, or click the first tile to browse for them. They are unpacked into your printer folder. Every folder inside that holds an `.obj` becomes a model; other files in the archive (sources, previews) are left out. A model is refused unless its name matches its diffuse texture: the model folder (or the archive, for an `.obj` at its root) must be called like the `diffuse_<name>.png` it contains, in any letter case. If the model folder holds other `diffuse_*.png` files, you are asked which one to use: the one you pick is added to the model as `diffuse_<name>.png` (the original file stays). You can also choose **Import anyway** to copy the model without a diffuse of its name (also when it has no diffuse texture at all). Only the models you skip are left out: they are listed in the status line, and the other models of the same archive are still imported. Nothing is ever overwritten: a name that is taken gets `_2`, `_3`, ... Password-protected archives and multi-part RAR sets are not supported.

![](readme/import.gif)

### Export a model to share

The **Export** button above the preview saves a model as a `.zip` that you can share. It does not contain the `.glb`, so others cannot edit your model with it.

Select several tiles (Ctrl + click, Shift + click for a range) and the button reads **Export Bundle**: it saves all the selected models into ONE `.zip`, each in its own folder, with the same rule (no editable `.glb`).

![](readme/export.gif)

## The 3D preview

### Look at it from every side

Rotate with the left mouse button, move the view with the right one, zoom with the wheel. **Reset camera** puts the view back.

| Action | How |
|---|---|
| Rotate | Left mouse button, drag |
| Move the view | Right mouse button, drag |
| Zoom | Mouse wheel |

![](readme/preview-camera.gif)

### View options

The eye button at the bottom right of the preview opens the view options. Most have a keybind, shown in brackets, that toggles them.

### See it like the game sees it

**Game renderer look (L)** shows the model with the game's lighting and shading, as in its icon. **Game renderer view (G)** uses the game's icon camera. Switch them off for a plain studio view. Both the preview and the generated icons get a soft overall brightness (a third of every surface's own colour), so parts the light does not reach are dark rather than black.

With **Game renderer look (L)** on, the preview is tuned to the game's WORLD at night (not its icon renderer): a filmic tone curve (`WORLD_TONEMAP` in `preview.rs`: no curve, as before, `WORLD_EV_OFFSET` = +0.2 stop (slightly darker), `WORLD_SATURATION` 1.0) for the game's contrast, a small ambient light (`gamerig::WORLD_AMBIENT_FRACTION`, 12 %) so shadows stay dark, and a roughness floor (`scene::WORLD_ROUGHNESS_MIN`, 0.7) that softens the highlights the game does not show (not on glowing glass or metal), and a faint light along the view direction (`gamerig::HEADLIGHT_INTENSITY`) that gives smooth surfaces such as the crystal their white glint. A lamp's coloured light is its physical strength (`scene::LAMP_GAIN`, 4 pi lumens per `lamp_intensity`). The pbr map is read per texel the way the game packs it (red metallic, green specular, blue roughness): a texel without specular that is not metal is matte (`model::pbr_texel`), so the crystal lamp's vines and pad are matte while its metallic crystal keeps its reflections. (An ORM detection exists, `model::PBR_ORM_AUTODETECT`, but it is off: it misread that crystal.) Generated icons keep their own look.

### Debug views

**View (C)** cycles between the textured model, the **section colours** (one colour per material section) and a **UV checker** pattern, to find stretched textures and mixed-up parts.

### Anti-aliasing

**Anti-aliasing (M)** smooths the jagged edges of the preview. It is on by default. The generated icons are anti-aliased too (4x MSAA, `ICON_MSAA` in `icons.rs`; set it to `Msaa::Off` for the game's hard edges); the editor's **Regenerate icon** follows the preview's option.

### Static light

By default the preview lights turn with the camera. **Static light (S)** keeps them where they are, so you can see how the light falls while you turn the model.

### When the model looks wrong

A few switches change how the preview treats the model, which helps when something looks wrong: **Flip UV V (U)**, **Flip winding (F)**, **Two-sided (B)**, **Z-up (Z)**, **Alpha (A)** (cycles through the transparency modes), **Tangents (T)** (the game's tangents or generated ones) and **Drop duplicate faces (D)**.

### Collision hulls

**Show hulls (H)** draws the collision shapes the game will use around the model. Opening the **COLLISION** section of the properties turns it on for you (an eye icon on the section header says so; click the icon to switch it off).

### Will it fit the printer?

**Printer limits (P)**, or just opening the **MODEL** section, draws the large (200 x 200 x 300 cm) and the desktop (45 x 45 x 60 cm) printer volume around the model: blue when it fits, red when it does not, with a short report at the top left. A model that is too big for every printer gets a red warning even with the volumes off.

![](readme/printer-limits.png)

### See where the lamp is

For lamp models, **Lamp position (I)** or the **LIGHT** section draws a marker in the lamp's colour at the lamp's position, with a thin line from the model's origin. It follows the Offset sliders at once and shows through the model.

## Converting and editing

### Drop a .glb to make a printer model

Drag a `.glb` onto the window (or click the **Drop a .glb here** tile). The editor opens and converts it straight away. The manager builds the model, textures and collision the game needs and saves everything into your printer folder.

![](readme/convert-drop.gif)

### The editor

The 3D preview on the left and the **properties** panel on the right: icon, name, an info box and the collapsible sections of settings.

### Every change shows up right away

Change any setting and the model is rebuilt; the preview updates as you go. The result is saved into your library on every change, so there is no save button to forget.

### Sliders, fields and dropdowns

Sliders can be dragged, clicked, or moved with the keyboard (arrows, PageUp / PageDown, Home / End). Click a number to type an exact value. Options that only matter when another one is on are dimmed until you switch that one on. Only one section is open at a time.

### Reset any setting

A setting that differs from its default shows a small circular-arrow button at the end of its row. One click puts the default back.

### MODEL

**Scale**, **Apply skeleton** and **Drop transparent parts**.

### TEXTURES

Texture size limits (**Max tile size**, **Max atlas size**), **Apply base colour factor**, **Flip normal green**, **Bake emissive map** and **Emissive strength**.

### COLLISION

Generate collision, ignore `UCX_` shapes, quality, VHACD resolution, maximum hulls, fallback box size and offsets, with **Show collision (debug)** to see the result.

### DURABILITY

**Indestructible**, **Health** (0 = automatic), **Impact resistance** and **Damage resistance**.

### MATERIAL

The **Physical material** of the model.

### LIGHT

Turn a model into a **Lamp** and set its intensity, radius, shadows, whether the player can switch it, its colour and its position offsets.

### TEXTURE FILTERS

Pick how the game filters each texture (**Diffuse**, **PBR**, **Normal**, **Emissive**): Nearest, Bilinear, Trilinear or Default.

### Info box

The box under the model name lists what will be in the game for the current settings: material, durability, lamp values and texture filters. It changes as you edit.

### Rename in the editor

The pencil button next to the model name starts the rename: the name becomes a text field with all its text selected. A name that does not fit scrolls to its end while the pointer is over it and glides back when it leaves. A name that another model already uses is refused (the status line says so); the other model is never replaced.

### Icon of the model

The icon box shows the model's icon. **Capture as icon** renders a new one. To use your own picture, drop a `.png` onto the editor.

### Replace the model

Drop another `.glb` while you are editing to run the same settings on a different model.

### Posing a character

The converter only sees the pose that is saved in the exported file. It ignores animations and shape keys.

1. Pose your character in **Pose Mode**.
2. Select the mesh and choose **Object > Apply > Visual Geometry to Mesh**.
3. Delete the armature.

### Logs

The logs button at the bottom left of the preview shows what the converter did: the input, any failure, every message of the last run and which stages were reused.

### Edit later

Select a model from the EDITABLE section and click **Edit** above the preview: it reopens with all the settings it was made with.

### Properties (models without a saved .glb)

A model in the MODELS section cannot be reopened in the converter, but its game properties can still be changed: select it and click the orange **Properties** button above the preview (where **Edit** appears for editable models). The editor opens with the usual header (icon, **Capture as icon**, the name with its rename pencil, and the short list of the properties in effect) and only the property sections (Durability, Material, Light, Texture filters). Nothing is converted and no other program is involved; changes are saved to the model's `properties.cfg` by themselves, a moment after you stop changing a value, and the preview follows. Unknown keys in the file are kept, but the first save rewrites the file in the converter's format (key order and comments are not preserved).

## Blender

### Open in Blender

In the editor, the blue and orange **Open in Blender** button opens the model in a clean Blender scene. It appears only when Blender is found.

![](readme/blender-open.gif)

### Send changes back

The manager adds an **Export to VotV Model Manager** button inside Blender (3D view header and File menu). Click it and the model is written back; the manager notices the change by itself and regenerates the model.

![](readme/blender-export-close-up.png)
![](readme/blender-back.gif)

### Blender path

The manager looks for Blender in the usual places. If yours lives somewhere else, pick it under the gear menu, **Blender path...**.

## The app

### Settings menu

The gear menu has **Change folder...** (choose a different printer folder), **Refresh** (rescan the folder) and **Blender path...**.

### When something is wrong

A model that cannot be loaded shows a `!` on its tile. Run the app from a terminal with `--diagnose "C:\path\to\ModelFolder"` to get a report to include when you ask for help.

### Build number

The small gray number next to the app name is the build number. Mention it when you report a problem.

![](readme/build-number.png)

## Good to know

- Only **.glb** files can be converted. Other file types are ignored.
- Dropping a model **folder** or a model archive (**.zip**, **.7z**, **.rar**) copies it into your library instead of converting it.
- The first time the 3D view opens there may be a short pause while it gets ready.
- Names with non-English letters may show up as boxes in the list.
- **Open in Blender** appears only when Blender is found. If it is not, set it under the gear menu, **Blender path...**.
- The printer limits come from the game's own size check, as found by reverse engineering (see `docs/printer-size-check.md`). Which model axis the game treats as "up" is not confirmed: if the report disagrees with the game, toggle **Z-up (Z)** and compare with the "SMALL BOX" values the game prints when you select the model.
- Settings are stored in `%LocalAppData%\VotV Model Manager`.

## Something went wrong?

- **A model shows a `!`** and will not load: open a terminal and run
  `votv-model-manager.exe --diagnose "C:\path\to\ModelFolder"`
  then include the report when you ask for help.
- **The list is empty:** the message in the list names the folder being used; pick another one under **Settings > Change folder...**. A model is a sub-folder that contains an `.obj` file.
- **Reporting a problem:** please mention the small gray number next to the app name in the title bar (for example `42`). It is the build number, the same as the release name `build.42` you downloaded from.

## For developers

Built with Rust and [Bevy](https://bevyengine.org/). To build it yourself:

```
cargo run --release
```

or for debugging:
```
cargo run --features bevy/dynamic_linking
```

### Benchmark mode

`votv-model-manager.exe --benchmark` (or `--benchmark="D:\some\folder"`) records how long every workload takes: folder scan, tile building, icon loading and rendering, model and texture loading, preview building, every converter stage, export, import, each per-frame system, frame times, CPU and memory. Use the app normally, close it, and send the run folder (default: `%LocalAppData%\VotV Model Manager\benchmark\run-<time>\`, its path is also in `benchmark\LAST_RUN.txt`; start with `summary.txt`). Without the flag nothing is recorded. The code is in `src/benchmark.rs`.

### Graphics backend

On Windows the app starts with Vulkan only, which launches faster and avoids a multi-second freeze the first time a model is shown with DirectX 12. If a start with Vulkan never gets as far as drawing its first frame (no working Vulkan driver), the next start switches to automatic selection and remembers it (the file `backend-auto` in `%LocalAppData%\VotV Model Manager\`; delete it to try Vulkan again). `--backend=vulkan`, `--backend=dx12` or `--backend=auto` overrides this.

The build number in the title bar is the GitHub Actions run number (`GITHUB_RUN_NUMBER`, passed in by `build.rs`); a local build has none, so nothing is shown.

## Disclosure

This project is being vibe coded with Claude Sonnet 5.5 (Free Tier).

This is high quality slop. The app UX and Design were not generated by AI.

Not a single penny was given to the evil AI corporations in the process of building this.


## Not affiliated

This is a fan-made tool. It is not made by or affiliated with the developer of Voices of the Void.
