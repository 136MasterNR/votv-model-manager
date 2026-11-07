import sys, numpy as np, trimesh, collections
def load(path):
    v=[];obj=None;blocks=collections.OrderedDict();vis=[]
    faces_vis=[]
    for line in open(path):
        t=line.split()
        if not t: continue
        if t[0]=='v': v.append(list(map(float,t[1:4])))
        elif t[0]=='o': obj=t[1]; blocks[obj]=[]
        elif t[0]=='f':
            idx=[int(c.split('/')[0])-1 for c in t[1:4]]
            (blocks[obj] if obj else faces_vis).append(idx)
    v=np.array(v)
    model=trimesh.Trimesh(v,np.array(faces_vis),process=False)
    hulls=[trimesh.Trimesh(v,np.array(f),process=False) for f in blocks.values()]
    return model,hulls
def within(h,pts,tol):
    n=h.face_normals; d=np.einsum('ij,ij->i',n,h.triangles[:,0])
    return ((pts@n.T - d[None,:])<=tol).all(1)
def report(name, root):
    model,hulls=load(f"{root}/{name}/{name}.obj")
    # only referenced vertices matter; use convex hull volume of each block
    hv=[]; 
    for h in hulls:
        u=trimesh.Trimesh(h.vertices[np.unique(h.faces)], process=False)
        try: hv.append(u.convex_hull)
        except Exception: pass
    pts,_=trimesh.sample.sample_surface(model,4000)
    inside=np.zeros(len(pts),bool)
    for h in hv:
        inside |= within(h,pts,0.5)
    tot=sum(h.volume for h in hv)
    try: mvol=model.convex_hull.volume
    except: mvol=float('nan')
    return len(hv), sum(len(h.faces) for h in hv), inside.mean(), tot, hv
for n in sys.argv[1:]:
    for root in ("py_out","rs_out"):
        k,f,cov,tot,hv=report(n,root)
        extra=""
        if n=="torus":
            c=np.array([[0,0,0.0]]); hole=any(within(h,c,0.0)[0] for h in hv)
            true_vol=2*np.pi**2*100*30**2   # 2π² R r²  (cm)
            extra=f" hole_filled={hole} hullvol/true={tot/true_vol:.2f}"
        print(f"{n:7s} {root}: hulls={k:3d} faces={f:5d} surface_covered={cov*100:5.1f}%{extra}")
