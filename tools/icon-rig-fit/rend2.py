import numpy as np
from rend import P,N,UV,T,TEXA,CUT,C,R,s2l,l2s
def render_pp(view_dir, lights, up, rough=0.5, spec=1.0, size=128, ss=2, fov=10.0, f0=0.04):
    """per-pixel shading: lights=[(travel_dir, intensity)], diffuse = albedo*cos*I*s ; GGX specular (physical F0) * spec"""
    n=size*ss; cam=C+view_dir*12*R; f=-view_dir; up=np.asarray(up,float)
    r=np.cross(f,up); r/=np.linalg.norm(r); u=np.cross(r,f)
    X=(P-cam)@np.c_[r,u,f]; t=np.tan(np.radians(fov)/2)
    px=((X[:,0]/X[:,2])/t*0.5+0.5)*n; py=(0.5-(X[:,1]/X[:,2])/t*0.5)*n
    dif=np.zeros((n,n,3),np.float32); spc=np.zeros((n,n),np.float32); alpha=np.zeros((n,n),bool); zb=np.full((n,n),1e9)
    a2=rough**4  # alpha = roughness^2
    for tri in T:
        a,b,c=tri
        x0,y0,x1,y1,x2,y2=px[a],py[a],px[b],py[b],px[c],py[c]
        area=(x1-x0)*(y2-y0)-(x2-x0)*(y1-y0)
        if abs(area)<1e-9 or area>0: continue
        xmin=int(max(np.floor(min(x0,x1,x2)),0)); xmax=int(min(np.ceil(max(x0,x1,x2)),n-1)); ymin=int(max(np.floor(min(y0,y1,y2)),0)); ymax=int(min(np.ceil(max(y0,y1,y2)),n-1))
        if xmax<xmin or ymax<ymin: continue
        gx,gy=np.meshgrid(np.arange(xmin,xmax+1)+0.5,np.arange(ymin,ymax+1)+0.5)
        w0=((x1-gx)*(y2-gy)-(x2-gx)*(y1-gy))/area; w1=((x2-gx)*(y0-gy)-(x0-gx)*(y2-gy))/area; w2=1-w0-w1
        m=(w0>=0)&(w1>=0)&(w2>=0)
        if not m.any(): continue
        z=w0*X[a,2]+w1*X[b,2]+w2*X[c,2]
        uu=w0*UV[a,0]+w1*UV[b,0]+w2*UV[c,0]; vv=w0*UV[a,1]+w1*UV[b,1]+w2*UV[c,1]
        ti=np.clip((uu%1.0*TEXA.shape[1]).astype(int),0,TEXA.shape[1]-1); tj=np.clip((vv%1.0*TEXA.shape[0]).astype(int),0,TEXA.shape[0]-1)
        tx=TEXA[tj,ti]; m&=tx[...,3]>=CUT; sl=(slice(ymin,ymax+1),slice(xmin,xmax+1)); m&=z<zb[sl]
        if not m.any(): continue
        nn=w0[...,None]*N[a]+w1[...,None]*N[b]+w2[...,None]*N[c]; nn/=np.linalg.norm(nn,axis=-1,keepdims=True)+1e-9
        pw=w0[...,None]*P[a]+w1[...,None]*P[b]+w2[...,None]*P[c]; v=cam-pw; v/=np.linalg.norm(v,axis=-1,keepdims=True)
        d=np.zeros(nn.shape[:2]); s=np.zeros(nn.shape[:2])
        for tr,k in lights:
            l=-tr; nl=np.clip(nn@l,0,1); h=v+l; h/=np.linalg.norm(h,axis=-1,keepdims=True)+1e-9
            nh=np.clip((nn*h).sum(-1),0,1); nv=np.clip((nn*v).sum(-1),1e-3,1); vh=np.clip((v*h).sum(-1),0,1)
            D=a2/(np.pi*((nh*nh)*(a2-1)+1)**2+1e-9)
            k_=(rough**2)/2; G=(nl/(nl*(1-k_)+k_+1e-9))*(nv/(nv*(1-k_)+k_+1e-9))
            F=f0+(1-f0)*(1-vh)**5
            fs=D*G*F/(4*nl*nv+1e-6)   # = D*Vis*F with Vis folded into G
            d+=k*nl; s+=k*np.pi*fs*nl*spec  # light scale: diffuse albedo*nl*k ; spec = pi*fs*nl*k (k includes 1/pi*pi)
        col=s2l(tx[...,:3])*d[...,None]+s[...,None]
        zb[sl]=np.where(m,z,zb[sl]); dif[sl]=np.where(m[...,None],col,dif[sl]); alpha[sl]|=m
    img=dif.reshape(size,ss,size,ss,3).mean((1,3)); al=alpha.reshape(size,ss,size,ss).mean((1,3))
    return img,al
