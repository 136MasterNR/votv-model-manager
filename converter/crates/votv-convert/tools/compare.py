import sys, os, collections, numpy as np
from PIL import Image

def parse(path):
    v=[];vt=[];vn=[];faces=[];objs=[];cur=None
    for line in open(path):
        t=line.split()
        if not t: continue
        if t[0]=='v': v.append(tuple(map(float,t[1:4])))
        elif t[0]=='vt': vt.append(tuple(map(float,t[1:3])))
        elif t[0]=='vn': vn.append(tuple(map(float,t[1:4])))
        elif t[0]=='o': cur=t[1]; objs.append(cur)
        elif t[0]=='f':
            corners=[]
            for c in t[1:]:
                p=c.split('/'); corners.append((int(p[0])-1, int(p[1])-1 if len(p)>1 and p[1] else None, int(p[2])-1 if len(p)>2 else None))
            faces.append((cur,corners))
    return v,vt,vn,faces,objs

def r(x,d=3): return tuple(round(a,d)+0.0 for a in x)
def face_key(f,v,vt,vn,use_n=True,use_uv=True):
    cs=[]
    for (a,b,c) in f[1]:
        cs.append((r(v[a]), r(vt[b],4) if (use_uv and b is not None) else None, r(vn[c],2) if (use_n and c is not None) else None))
    # rotate to canonical start
    i=min(range(3), key=lambda k: str(cs[k])); return tuple(cs[i:]+cs[:i])

def compare(name):
    py=f"py_out/{name}"; rs=f"rs_out/{name}"; ok=True
    def bad(msg):
        nonlocal ok; ok=False; print("   DIFF:",msg)
    for f in ("properties.cfg", f"{name}.mtl"):
        if open(f"{py}/{f}").read()!=open(f"{rs}/{f}").read(): bad(f"{f} differs")
    P=parse(f"{py}/{name}.obj"); R=parse(f"{rs}/{name}.obj")
    pv=[f for f in P[3] if f[0] is None]; rv=[f for f in R[3] if f[0] is None]
    print(f"   visible faces py={len(pv)} rs={len(rv)}   objects py={P[4]} rs={R[4]}")
    cp=collections.Counter(face_key(f,P[0],P[1],P[2]) for f in pv); cr=collections.Counter(face_key(f,R[0],R[1],R[2]) for f in rv)
    if cp!=cr:
        only_p=sum((cp-cr).values()); only_r=sum((cr-cp).values()); bad(f"visible faces differ: {only_p} only in py, {only_r} only in rs")
        # looser: positions only
        cp2=collections.Counter(face_key(f,P[0],P[1],P[2],False,False) for f in pv); cr2=collections.Counter(face_key(f,R[0],R[1],R[2],False,False) for f in rv)
        print("      geometry-only match:", cp2==cr2, "| uv-only py/rs diff", sum((cp-cr).values()))
    # collision
    cpc=[f for f in P[3] if f[0]]; crc=[f for f in R[3] if f[0]]
    print(f"   collision faces py={len(cpc)} rs={len(crc)}")
    # images
    for fn in sorted(os.listdir(py)):
        if fn.endswith(".png"):
            a=Image.open(f"{py}/{fn}"); b=Image.open(f"{rs}/{fn}")
            if a.size!=b.size or a.mode!=b.mode: bad(f"{fn} size/mode py={a.size},{a.mode} rs={b.size},{b.mode}"); continue
            d=np.abs(np.asarray(a,dtype=int)-np.asarray(b,dtype=int))
            print(f"   {fn}: {a.size} {a.mode} mean|d|={d.mean():.3f} max={d.max()} px>4={(d.max(-1)>4).mean()*100:.2f}%")
            if d.max()>8 and (d.max(-1)>8).mean()>0.02: bad(f"{fn} pixels differ")
    print("   =>","MATCH" if ok else "DIFFERENCES")
    return ok

res={}
for n in sys.argv[1:]:
    print("==",n); res[n]=compare(n)
