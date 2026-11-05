<h2>
  <img src="assets/icon.png" width="32" />
  <strong>VotV Model Manager</strong>
</h2>

An app for managing your custom **3D printer models** in [Voices of the Void](https://mrdrnose.itch.io/votv).

Browse your models, check them in a 3D viewer, and import .glb files into the app to convert it into a model the game can print. GUI-only.

## What you can do

- **See your models at a glance.** Every model gets an icon, drawn similarly the game draws its own.
- **Preview in 3D.** Orbit, pan and zoom to check a model before you play.
- **Convert a any model to the format VotV accepts.** The manager builds the model, textures and collision for you and saves it into your printer folder.
- **Reset any setting.** A parameter that differs from its default shows a small circular-arrow button at the end of its row; one click puts the default back.
- **Tweak and see the result right away.** Change scale, collision, health, lamp settings, texture filters and more. The preview updates as you go.
- **Edit later.** Models you made with the manager can be reopened with all their settings and in blender.
- **Open in Blender** and send changes back. Save in Blender and the manager picks up the change on its own.
- **Manage your library.** Search, rename, disable or enable, delete, regenerate or add custom icons, and export a model as an uneditable `.zip` to share.

## Install

1. Open the **Releases** page of this repository (link on the right-hand side of the GitHub page).
2. Download the latest `votv-model-manager` file for your OS.
3. Put the executable anywhere you like. No installer, it's portable.

## Getting started

1. **Start the app.** It points to your VotV printer folder automatically:
   `%LocalAppData%\VotV\Assets\meshes\printer`
   If your models are somewhere else, use the gear menu at the top and choose **Change folder...**.
2. **Click a model** to see it in the 3D preview.
3. **Add a new model:** drag a `.glb` file onto the window (or click the "Drop a .glb here" tile). The editor opens and converts it straight away.
4. **Adjust the settings** in the panel on the right. Every change regenerates the model. The model is saved into your library immediately on any update or change.

## Controls in the preview

| Action | How |
|---|---|
| Rotate | Left mouse button, drag |
| Move the view | Right mouse button, drag |
| Zoom | Mouse wheel |
| More view options | The eye button at the bottom right of the preview (the options open above it): game renderer look / view, static light, **anti-aliasing (M)**, hulls, **printer limits (P)**, **lamp position (I)** and more |
| See where the lamp is | Open the **LIGHT** section of the properties (a light bulb icon on its header says the marker is on; click it to switch it off), or use **Lamp position (I)** in the view options. A cross with a small diamond in the lamp's colour is drawn at the lamp's position, with a thin line from the model's origin (the small grey cross) that the offset is measured from. It follows the Offset sliders at once and shows through the model. Nothing is drawn for a model that is not a lamp |
| Check the model against the game's printers | **Printer limits (P)** in the view options, or just open the **MODEL** section of the properties (a *fullscreen* icon on its header says the volumes are on; click it to switch them off). The preview draws the large (200 x 200 x 300 cm) and the desktop (45 x 45 x 60 cm) printer volume around the model, blue when it fits and red when it does not, with a short report at the top left. A model that is too big for every printer gets a red warning even with the volumes off |

## The model name in the editor

The model name at the top of the properties panel is plain text. The **pencil button** next to it starts the rename: the name
becomes a text field with all of its text selected (Enter confirms, Escape cancels). A name that does not fit scrolls slowly
and smoothly to its end while the pointer is over it and glides back when the pointer leaves; that never takes a click.

## Selecting models and the toolbar

Click a model to select it, **Ctrl + click** to add or remove one, **Shift + click** to select everything from the last
clicked model to this one (only the models the search leaves visible). The toolbar above the list works on **all** selected
models; hover a button to see what it does. The preview shows the last clicked one. **Export** (a `.zip` you can share safely: others cannot use it to edit your model) is the button at the top of the preview, not in this toolbar.

| Button | What it does | Selection |
|---|---|---|
| **Open location** | Opens the model's folder in Explorer | one model |
| **Rename** | Renames the model and its files (the name on the tile becomes a text field) | one model |
| **Regenerate icon** | Makes fresh icons, one after the other | any number |
| **Lock model** (yellow) | Deletes the saved `.glb` so the models can no longer be edited. Shows a warning and asks for a second click | editable models |
| **Disable / Enable** | Hides the models from the game without deleting them (it enables when only disabled models are selected) | any number |
| **Delete** (red) | Removes the models for good; asks for a second click | any number |

Buttons that do not apply to the selection are dimmed. A right click on a tile just selects it (there is no context menu).

## Good to know

- Only **.glb** files can be converted. Other file types are ignored.
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

The build number in the title bar is the GitHub Actions run number (`GITHUB_RUN_NUMBER`, passed in by `build.rs`); a local build has none, so nothing is shown.

Icons come from Google's Material Icons font plus a tiny Material Symbols subset (both Apache-2.0); see `docs/icon-fonts.md`.

More technical notes are in `docs/` and `converter/README.md`.

## Disclure

This project was vibe coded with Claude Sonnet 5.5 (Free Tier).

The app direction and design were not generated by AI.

Not a single penny was given to the evil AI corporations in the process of building this.


## Not affiliated

This is a fan-made tool. It is not made by or affiliated with the developer of Voices of the Void.
