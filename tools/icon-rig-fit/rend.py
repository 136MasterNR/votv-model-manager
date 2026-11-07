import numpy as np, itertools
from geo import geometry
P,N,UV,T,TEX,CUT=geometry()
TEXA=np.array(TEX).astype(np.float32)/255.0
lo,hi=P.min(0),P.max(0); C=(lo+hi)/2; R=np.linalg.norm(hi-lo)/2
def s2l(c): return np.where(c<=0.04045,c/12.92,((c+0.055)/1.055)**2.4)
def l2s(c): c=np.clip(c,0,1); return np.where(c<=0.0031308,c*12.92,1.055*c**(1/2.4)-0.055)
def render(view_dir, lights, up=np.array([0,1.0,0]), size=128, ss=3, fov=10.0, bg=0.0, cull=True, ambient=0.0, Pm=None, Nm=None):
    """view_dir: unit vector from centre to camera (OBJ space). lights: list of (travel_dir, intensity_factor)."""
    Pm=P if Pm is None else Pm; Nm=N if Nm is None else Nm
    n=size*ss
    cam=C+view_dir*12*R
    f=-view_dir; up=np.asarray(up,float)
    if abs(f@up)>0.99: up=np.array([0,0,1.0])
    r=np.cross(f,up); r/=np.linalg.norm(r); u=np.cross(r,f)
    X=(Pm-cam)@np.c_[r,u,f]            # x right, y up, z depth
    t=np.tan(np.radians(fov)/2)
    sx=(X[:,0]/X[:,2])/t; sy=(X[:,1]/X[:,2])/t     # -1..1
    px=(sx*0.5+0.5)*n; py=(0.5-sy*0.5)*n
    img=np.full((n,n,3),bg,np.float32); alpha=np.zeros((n,n),bool); zb=np.full((n,n),1e9)
    # vertex lighting (lambert) -> linear radiance factor
    shade=np.zeros(len(Pm))+ambient
    for tr,k in lights: shade+=k*np.maximum(Nm@(-tr),0)
    for tri in T:
        a,b,c=tri
        x0,y0,x1,y1,x2,y2=px[a],py[a],px[b],py[b],px[c],py[c]
        area=(x1-x0)*(y2-y0)-(x2-x0)*(y1-y0)
        if abs(area)<1e-9: continue
        if cull and area>0: continue          # screen y points down: CCW in view space is negative area here
        xmin=int(max(np.floor(min(x0,x1,x2)),0)); xmax=int(min(np.ceil(max(x0,x1,x2)),n-1))
        ymin=int(max(np.floor(min(y0,y1,y2)),0)); ymax=int(min(np.ceil(max(y0,y1,y2)),n-1))
        if xmax<xmin or ymax<ymin: continue
        gx,gy=np.meshgrid(np.arange(xmin,xmax+1)+0.5,np.arange(ymin,ymax+1)+0.5)
        w0=((x1-gx)*(y2-gy)-(x2-gx)*(y1-gy))/area
        w1=((x2-gx)*(y0-gy)-(x0-gx)*(y2-gy))/area
        w2=1-w0-w1
        m=(w0>=0)&(w1>=0)&(w2>=0)
        if not m.any(): continue
        z=w0*X[a,2]+w1*X[b,2]+w2*X[c,2]
        uu=w0*UV[a,0]+w1*UV[b,0]+w2*UV[c,0]; vv=w0*UV[a,1]+w1*UV[b,1]+w2*UV[c,1]
        ti=np.clip((uu%1.0*TEXA.shape[1]).astype(int),0,TEXA.shape[1]-1); tj=np.clip((vv%1.0*TEXA.shape[0]).astype(int),0,TEXA.shape[0]-1)
        tx=TEXA[tj,ti]
        m&=tx[...,3]>=CUT
        m&=z<zb[ymin:ymax+1,xmin:xmax+1]
        if not m.any(): continue
        sh=w0*shade[a]+w1*shade[b]+w2*shade[c]
        col=s2l(tx[...,:3])*sh[...,None]
        sl=(slice(ymin,ymax+1),slice(xmin,xmax+1))
        zb[sl]=np.where(m,z,zb[sl]); img[sl]=np.where(m[...,None],col,img[sl]); alpha[sl]|=m
    # downsample (average in linear), encode
    img=img.reshape(size,ss,size,ss,3).mean((1,3)); al=alpha.reshape(size,ss,size,ss).mean((1,3))
    return img,al
def to_srgb8(img_lin, bg_lin=0.0, al=None):
    return (l2s(img_lin)*255+0.5).astype(np.uint8)
