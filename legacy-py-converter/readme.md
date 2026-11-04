This is a python script that converts .glb files to the format votv accepts

## Setup
- Install [Python 3.9+](https://www.python.org)
- Press `Y` when asked to install modules

## Convert
1. Get your glb model (sketchfab, blender, etc)
2. Download the python script
3. Put the model and python script in the same folder
4. Run `py printer_convert.py yourmodel` inside that folder
5. Drop the new folder it created into `%LocalAppData%\VotV\Assets\meshes\printer`

## Parameters
`input_name`
`output_name`
`--humanoid` apply skeleton
`--scale x` size multiplier in cm (100)
`--edible` make edible
`--no-pbr` skip pbr
`--no-emissive` skip glow
`--no-transparent` skip translucent parts
`--max-tile-size x` per-texture size cap (1024)
`--max-atlas-size x` final atlas size cap (4096)
`--coll-quality low/high` collision detail level
`--force-generate-coll` skip model's custom collisions
`--no-coll` skip collision generation except fallback
`--max-hull-faces x` face cap per collision hull (200)
`--max-decomp-hulls x` max collision pieces (64)
`--fallback-coll` add a bottom center collision box
`--fallback-coll-size x` size of collision box (33)
`--coll-offset-x/y x` offset all collisions
`--show-coll` show collisions
`--physical-material x` material FX

use `--help` for details

## FAQ
**Model T-poses**  Apply skeleton with `--humanoid`
**Printer says "Print error"** Print is huge: use `--scale` to downscale, print, regenerate the model without scale, save, reload
**Print is tiny or gone**  Use `--scale x` to make larger (x = number)
**Print is too fragile**  Edit `health=-1` in propertifes.cfg
**Print is too reflective** Use `--no-pbr`
**Print is jittery or dropping FPS**  The model likely needs manual fine tuning & use of script parameters
**Want custom collisions**  Create `UCX_*` objects in blender