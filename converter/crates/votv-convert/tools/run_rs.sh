#!/bin/bash
RS=/home/claude/work/votv-convert/target/release/votv-convert
rm -rf rs_out; mkdir rs_out
for m in "$@"; do timeout 200 $RS glb/$m.glb rs_out/$m </dev/null > rs_$m.log 2>&1 || { echo "RS FAIL $m"; tail -5 rs_$m.log; }; done
