"""Find the OBJ -> Unreal axis mapping by comparing a software render with a real game icon.
Usage: GLB=chicken.glb python fit_axes.py samples/game-generated-icon.png
Needs numpy and pillow. Prints the 8 best of the 48 signed axis permutations by silhouette IoU.
Only valid for skinned/humanoid-style models that the converter poses like geo.py does; for other models adapt geo.py."""
import sys, itertools, os
sys.path.insert(0, os.path.dirname(__file__) or '.')
import numpy as np
from PIL import Image
from rend import render
game = np.array(Image.open(sys.argv[1]).convert('RGB')).astype(int)
mask = game.sum(-1) > 25
ph, yw = np.radians(30), np.radians(45)
f_ue = np.array([np.cos(ph) * np.cos(yw), np.cos(ph) * np.sin(yw), np.sin(ph)])
res = []
for perm in itertools.permutations(range(3)):
    for sg in itertools.product([1, -1], repeat=3):
        M = np.zeros((3, 3))
        for i in range(3):
            M[i, perm[i]] = sg[i]          # ue_i = sg_i * obj_perm[i]
        d, up = M.T @ f_ue, M.T @ np.array([0, 0, 1.0])
        _, al = render(d, [], up=up, ss=2)
        m = al > 0.5
        res.append(((m & mask).sum() / max((m | mask).sum(), 1), perm, sg, round(np.linalg.det(M))))
for r in sorted(res, reverse=True)[:8]:
    print('IoU %.3f perm %s signs %s det %d' % r)
