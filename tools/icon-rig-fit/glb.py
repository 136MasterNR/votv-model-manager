import struct, json, numpy as np, io
from PIL import Image
def load(path):
    d=open(path,'rb').read()
    l=struct.unpack('<I',d[12:16])[0]; j=json.loads(d[20:20+l]); off=20+l+8; bin_=d[off:]
    return j,bin_
CT={5120:np.int8,5121:np.uint8,5122:np.int16,5123:np.uint16,5125:np.uint32,5126:np.float32}
NC={'SCALAR':1,'VEC2':2,'VEC3':3,'VEC4':4,'MAT4':16}
def acc(j,b,i):
    a=j['accessors'][i]; bv=j['bufferViews'][a['bufferView']]
    n=NC[a['type']]; dt=CT[a['componentType']]
    o=bv.get('byteOffset',0)+a.get('byteOffset',0)
    stride=bv.get('byteStride',0)
    if stride and stride!=n*np.dtype(dt).itemsize:
        raw=np.frombuffer(b,dtype=np.uint8,count=a['count']*stride,offset=o).reshape(a['count'],stride)
        return np.frombuffer(raw[:,:n*np.dtype(dt).itemsize].tobytes(),dtype=dt).reshape(a['count'],n)
    return np.frombuffer(b,dtype=dt,count=a['count']*n,offset=o).reshape(a['count'],n)
def local_mat(n):
    if 'matrix' in n: return np.array(n['matrix'],float).reshape(4,4).T
    T=np.eye(4); R=np.eye(4); S=np.eye(4)
    if 'translation' in n: T[:3,3]=n['translation']
    if 'rotation' in n:
        x,y,z,w=n['rotation']
        R[:3,:3]=[[1-2*(y*y+z*z),2*(x*y-z*w),2*(x*z+y*w)],[2*(x*y+z*w),1-2*(x*x+z*z),2*(y*z-x*w)],[2*(x*z-y*w),2*(y*z+x*w),1-2*(x*x+y*y)]]
    if 'scale' in n: S[0,0],S[1,1],S[2,2]=n['scale']
    return T@R@S
def worlds(j):
    N=j['nodes']; W={}
    def go(i,P):
        W[i]=P@local_mat(N[i])
        for c in N[i].get('children',[]): go(c,W[i])
    for r in j['scenes'][j.get('scene',0)]['nodes']: go(r,np.eye(4))
    return W
