#!/bin/bash
# name | model | flags
cases=(
"showcoll|ucx|--show-coll"
"offsets|ucx|--coll-offset-x 3.5 --coll-offset-y -2 --scale 50"
"nocoll_fb|basic|--no-coll --fallback-coll --fallback-coll-size 20"
"notrans|glass|--no-transparent --no-coll"
"humanoid|rigged|--humanoid --no-coll"
"plainrig|rigged|--no-coll"
"edible|ucx|--edible --physical-material 3 --emissive-strength 4.5"
"tilesz|basic|--max-tile-size 32 --max-atlas-size 100 --no-coll"
"noflip|basic|--no-flip-normal-green --no-emissive --no-coll"
"forcegen|ucx|--force-generate-coll --coll-quality low"
)
PY=/home/claude/skill/votv-models/scripts/printer_convert.py; RS=/home/claude/work/votv-convert/target/release/votv-convert
rm -rf py_o rs_o; mkdir py_o rs_o
for c in "${cases[@]}"; do IFS='|' read -r n m f <<< "$c"
  python3 $PY glb/$m.glb py_o/$n $f </dev/null > py_o_$n.log 2>&1 || echo "PY FAIL $n"
  $RS glb/$m.glb rs_o/$n $f </dev/null > rs_o_$n.log 2>&1 || echo "RS FAIL $n"
done
