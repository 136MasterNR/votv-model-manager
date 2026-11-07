#!/bin/bash
PY=/home/claude/skill/votv-models/scripts/printer_convert.py; RS=/home/claude/work/votv-convert/target/release/votv-convert
rm -rf pb rb; mkdir pb rb
s=$(date +%s.%N); timeout 250 python3 $PY glb/big.glb pb/big --no-coll </dev/null >pb.log 2>&1; e=$(date +%s.%N); echo "python no-coll: $(echo "$e - $s" | bc) s"
s=$(date +%s.%N); timeout 250 $RS glb/big.glb rb/big --no-coll </dev/null >rb.log 2>&1; e=$(date +%s.%N); echo "rust   no-coll: $(echo "$e - $s" | bc) s"
s=$(date +%s.%N); timeout 250 $RS glb/big.glb rb/big2 </dev/null >rb2.log 2>&1; e=$(date +%s.%N); echo "rust   default collision: $(echo "$e - $s" | bc) s"; grep -iE "hull|collision" rb2.log | cut -c1-170
ls -l pb/big rb/big | awk '{print $5, $9}'
