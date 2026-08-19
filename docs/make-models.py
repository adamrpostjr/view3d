"""Generates the showcase models used by the README screenshots.

Usage: python3 docs/make-models.py OUTDIR

Everything here is procedural, so the repository carries no third-party models.
"""
import struct, math, sys, zipfile

def knot(nu=400, nv=48, p=2, q=3, R=20, r=5.5):
    """Trefoil-ish (p,q) torus knot as a tube mesh."""
    def C(t):
        return ( (R + r*2.2*math.cos(q*t))*math.cos(p*t),
                 (R + r*2.2*math.cos(q*t))*math.sin(p*t),
                 r*2.2*math.sin(q*t) )
    pts=[]
    for i in range(nu):
        t=i*2*math.pi/nu
        c=C(t); c1=C(t+1e-4)
        T=[c1[k]-c[k] for k in range(3)]
        n=math.sqrt(sum(x*x for x in T)); T=[x/n for x in T]
        A=[0,0,1] if abs(T[2])<0.9 else [1,0,0]
        N=[T[1]*A[2]-T[2]*A[1], T[2]*A[0]-T[0]*A[2], T[0]*A[1]-T[1]*A[0]]
        n=math.sqrt(sum(x*x for x in N)); N=[x/n for x in N]
        B=[T[1]*N[2]-T[2]*N[1], T[2]*N[0]-T[0]*N[2], T[0]*N[1]-T[1]*N[0]]
        ring=[]
        for j in range(nv):
            a=j*2*math.pi/nv
            ring.append(tuple(c[k]+r*(math.cos(a)*N[k]+math.sin(a)*B[k]) for k in range(3)))
        pts.append(ring)
    tris=[]
    for i in range(nu):
        for j in range(nv):
            a=pts[i][j]; b=pts[(i+1)%nu][j]; c=pts[(i+1)%nu][(j+1)%nv]; d=pts[i][(j+1)%nv]
            tris.append((a,b,c)); tris.append((a,c,d))
    return tris

def write_stl(path, tris):
    with open(path,'wb') as f:
        f.write(b'\0'*80); f.write(struct.pack('<I', len(tris)))
        for t in tris:
            f.write(struct.pack('<3f',0,0,0))
            for v in t: f.write(struct.pack('<3f', *v))
            f.write(struct.pack('<H',0))

def write_3mf_colored(path, tris, colors):
    """One object, per-triangle colors from a color group."""
    verts={}; order=[]
    def vid(v):
        if v not in verts:
            verts[v]=len(order); order.append(v)
        return verts[v]
    faces=[(vid(a),vid(b),vid(c)) for a,b,c in tris]
    pal="".join('<m:color color="%s"/>' % c for c in colors)
    xml=['<?xml version="1.0" encoding="UTF-8"?>',
         '<model unit="millimeter" xml:lang="en-US" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02" xmlns:m="http://schemas.microsoft.com/3dmanufacturing/material/2015/02">',
         '<resources><m:colorgroup id="9">%s</m:colorgroup>' % pal,
         '<object id="1" type="model"><mesh><vertices>']
    for v in order: xml.append('<vertex x="%.4f" y="%.4f" z="%.4f"/>'%v)
    xml.append('</vertices><triangles>')
    for i,(a,b,c) in enumerate(faces):
        band=(i//(2*48))%len(colors)
        xml.append('<triangle v1="%d" v2="%d" v3="%d" pid="9" p1="%d"/>'%(a,b,c,band))
    xml.append('</triangles></mesh></object></resources>')
    xml.append('<build><item objectid="1" transform="1 0 0 0 1 0 0 0 1 0 0 12"/></build></model>')
    rels='<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rel0" Target="/3D/3dmodel.model" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/></Relationships>'
    with zipfile.ZipFile(path,'w',zipfile.ZIP_DEFLATED) as z:
        z.writestr('_rels/.rels', rels)
        z.writestr('3D/3dmodel.model', "".join(xml))

d=sys.argv[1]
k=knot()
write_stl(d+"/knot.stl", k)
write_3mf_colored(d+"/knot_color.3mf", k, ["#E4572EFF","#F3A712FF","#4DA167FF","#2E86ABFF","#8367C7FF"])
print("triangles:", len(k))
