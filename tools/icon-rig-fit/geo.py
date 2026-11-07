import sys; sys.path.insert(0,__import__('os').path.dirname(__file__) or '.')
from glb import *
def geometry():
    j,b=load(__import__('os').environ.get('GLB','chicken.glb'))
    W=worlds(j)
    p=j['meshes'][0]['primitives'][0]
    pos=acc(j,b,p['attributes']['POSITION']).astype(float)
    nrm=acc(j,b,p['attributes']['NORMAL']).astype(float)
    uv=acc(j,b,p['attributes']['TEXCOORD_0']).astype(float)
    idx=acc(j,b,p['indices']).astype(np.int64).reshape(-1)
    J=acc(j,b,p['attributes']['JOINTS_0']).astype(int); Wt=acc(j,b,p['attributes']['WEIGHTS_0']).astype(float)
    sk=j['skins'][0]; ibm=acc(j,b,sk['inverseBindMatrices']).astype(float).reshape(-1,4,4).transpose(0,2,1)
    jm=np.array([W[jn]@ibm[i] for i,jn in enumerate(sk['joints'])])
    wsum=Wt.sum(1,keepdims=True); wsum[wsum==0]=1; Wn=Wt/wsum
    ph=np.c_[pos,np.ones(len(pos))]
    out=np.zeros((len(pos),3)); outn=np.zeros((len(pos),3))
    for k in range(4):
        M=jm[J[:,k]]                      # per-vertex joint matrix
        out+=Wn[:,k:k+1]*np.einsum('nij,nj->ni',M,ph)[:,:3]
        outn+=Wn[:,k:k+1]*np.einsum('nij,nj->ni',M[:,:3,:3],nrm)
    outn/=np.linalg.norm(outn,axis=1,keepdims=True)+1e-12
    # texture
    im=j['images'][0]; bv=j['bufferViews'][im['bufferView']]
    tex=Image.open(io.BytesIO(b[bv.get('byteOffset',0):bv.get('byteOffset',0)+bv['byteLength']])).convert('RGBA')
    mat=j['materials'][0]
    return out,outn,uv,idx.reshape(-1,3),tex,mat['alphaCutoff']
if __name__=='__main__':
    P,N,uv,T,tex,cut=geometry()
    print('skinned bbox',P.min(0),P.max(0),'tex',tex.size,'tris',len(T),'cutoff',cut)
