"""Golden indipendenti: Shapely 2.1.2 / GEOS e concaveman JS 2.0.0.

Dipendenze solo per rigenerare, installate sotto target/raccolta:
python -m pip install --target target/raccolta/pipe-oracle-python shapely==2.1.2
npm.cmd install --prefix target/raccolta/pipe-oracle-js concaveman@2.0.0
Non importa ne' esegue codice plenora. I test Rust leggono solo il JSON.
"""
import json
import math
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / "target/raccolta/pipe-oracle-python"))
import shapely
from shapely import affinity
from shapely.geometry import Point, LineString, MultiPoint, Polygon, MultiPolygon, MultiLineString, mapping
from shapely.ops import substring

assert shapely.__version__ == "2.1.2"
x, y = 500000, 5000000
polygons = [Polygon([(x+a,y+b),(x+a+n,y+b),(x+a+n,y+b+n),(x+a,y+b+n)]) if n else None for a,b,n in [(0,0,100),(0,0,0),(50,50,100),(500,0,30)]]
lines = [LineString([(x,y),(x+100,y+7),(x+200,y)]), LineString([(x+200,y),(x+300,y+50)]), None]
points = [Point(x,y),Point(x+10,y+3),None,Point(x+400,y-20),Point(x+405,y-22)]
cloud = MultiPoint([(x+i*10,y+(i*i%7)*10) for i in range(12)])
module = (ROOT / "target/raccolta/pipe-oracle-js/node_modules/concaveman/index.js").as_uri()
js = f'import concaveman from {json.dumps(module)}; console.log(JSON.stringify(concaveman({json.dumps([list(p.coords[0]) for p in cloud.geoms])},2,0)));'
concave = Polygon(json.loads(subprocess.check_output(["node", "--input-type=module", "-e", js], text=True)))

def buffer(g, d, cap):
    p=g.buffer(d,quad_segs=256,cap_style=cap,join_style="round")
    return MultiPolygon([p]) if isinstance(p, Polygon) else p

def snap(g):
    return LineString([(math.floor(a/4+0.5)*4,math.floor(b/4+0.5)*4) for a,b in g.coords])

cases = [
    ("geo.centroid",polygons,lambda g:g.centroid),
    ("geo.convex_hull",[cloud],lambda g:g.convex_hull),
    ("geo.envelope",lines,lambda g:g.envelope),
    ("geo.boundary",polygons,lambda g:MultiLineString([g.exterior])),
    ("geo.point_on_surface",polygons,lambda g:g.representative_point()),
    ("geo.buffer",lines,lambda g:buffer(g,5,"flat")),
    ("geo.buffer",polygons,lambda g:buffer(g,3,"round")),
    # Le aree dei triangoli sono 700 e nessuna: min_area=10 conserva i vertici.
    ("geo.simplify",lines,lambda g:g),
    ("geo.affine_transform",polygons,lambda g:affinity.affine_transform(g,[1,0,1,5,0,-5])),
    ("geo.translate",points,lambda g:affinity.translate(g,10,-3)),
    ("geo.scale",polygons,lambda g:affinity.scale(g,1.5,1,origin=(x,y))),
    ("geo.rotate",lines,lambda g:affinity.rotate(g,30,origin=(x,y))),
    ("geo.concave_hull",[cloud],lambda g:concave),
    ("geo.densify",lines,lambda g:shapely.segmentize(g,25)),
    ("geo.snap_to_grid",lines,snap),
    ("geo.line_substring",lines,lambda g:substring(g,0.25,0.75,normalized=True)),
    ("geo.line_interpolate_point",lines,lambda g:g.interpolate(0.4,normalized=True)),
]
# Oracolo completo dei pareggi: nessun ordine di visita dell'R-tree e'
# fissato dal contratto. Enumera ogni scavo ammesso, fino a esaurire la coda,
# con distanze razionali e copertura/topologia verificate da GEOS.
from fractions import Fraction
from functools import lru_cache
P = [(i*10,(i*i%7)*10) for i in range(12)]
def dist2(a,b): return (a[0]-b[0])**2+(a[1]-b[1])**2
def segment2(p,a,b):
    dx,dy=b[0]-a[0],b[1]-a[1]
    t=max(Fraction(0),min(Fraction(1),Fraction((p[0]-a[0])*dx+(p[1]-a[1])*dy,dx*dx+dy*dy)))
    return (p[0]-a[0]-t*dx)**2+(p[1]-a[1]-t*dy)**2
def canonical(ids):
    ids=list(ids); variants=[]
    for seq in [ids,ids[::-1]]:
        variants.extend(tuple(seq[i:]+seq[:i]) for i in range(len(seq)))
    return min(variants)
initial=canonical([P.index(tuple(c)) for c in MultiPoint(P).convex_hull.exterior.coords[:-1]])
def choices_for(hull, ia, ib):
    import itertools
    i=hull.index(ia)
    assert hull[(i+1)%len(hull)]==ib
    before=hull[i-1]; after=hull[(i+2)%len(hull)]
    a,b=P[ia],P[ib]; limit=Fraction(dist2(a,b),4)
    # Protezione dei punti piu' vicini gia' scartati, secondo il passo 2
    # dell'algoritmo documentato dal porting (non nel JS upstream).
    groups={}
    for j,p in enumerate(P):
        if j not in hull and segment2(p,a,b)<=limit:
            groups.setdefault(segment2(p,a,b),[]).append(j)
    tie_orders=[list(itertools.permutations(groups[d])) for d in sorted(groups)]
    results=set()
    for choices in itertools.product(*tie_orders):
        checked=[];found=None
        for j in itertools.chain.from_iterable(choices):
            p=P[j]; distance=segment2(p,a,b)
            expanded=hull[:i+1]+(j,)+hull[i+1:]
            poly=Polygon([P[k] for k in expanded])
            if not distance<segment2(p,P[before],a) or not distance<segment2(p,b,P[after]) or not poly.is_valid:
                checked.append(j);continue
            triangle=Polygon([a,p,b])
            chosen=next((k for k in checked if triangle.contains(Point(P[k]))),j)
            if min(dist2(P[chosen],a),dist2(P[chosen],b))<limit:
                found=chosen
            break
        results.add(found)
    return results
@lru_cache(None)
def terminal(hull, queue):
    if not queue:
        shape=Polygon([P[k] for k in hull])
        assert shape.is_valid and all(shape.covers(Point(q)) for q in P)
        return frozenset([canonical(hull)])
    ia,ib=queue[0]; pending=queue[1:];result=set()
    for chosen in choices_for(hull,ia,ib):
        if chosen is None:
            result.update(terminal(hull,pending))
        else:
            i=hull.index(ia);expanded=hull[:i+1]+(chosen,)+hull[i+1:]
            result.update(terminal(expanded,pending+((ia,chosen),(chosen,ib))))
    return frozenset(result)
# Tutti gli ordini iniziali dei lati: l'oracolo non dipende dalla scelta
# del primo vertice del convex hull o dall'ordine della coda del porting.
# Ogni lato e' visitato una volta; soltanto i due figli entrano di nuovo
# in coda dopo uno scavo. Non si risaturano lati gia' visitati.
import itertools
all_terminal=set()
for orientation in [initial,tuple(reversed(initial))]:
    initial_edges=tuple(zip(orientation,orientation[1:]+orientation[:1]))
    for queue in itertools.permutations(initial_edges):
        all_terminal.update(terminal(orientation,queue))
allowed=sorted(all_terminal)
assert allowed
concave_allowed=[mapping(Polygon([(x+P[i][0],y+P[i][1]) for i in ids])) for ids in allowed]
out={"source":{"shapely":"2.1.2","geos":shapely.geos_version_string,"concaveman":"2.0.0","buffer_quad_segs":256},"concave_allowed":concave_allowed,"cases":[{"op":op,"geometries":[mapping(f(g)) if g is not None else None for g in inputs]} for op,inputs,f in cases]}
Path(__file__).with_name("trasformazioni_attese.json").write_text(json.dumps(out,separators=(",",":"))+"\n",encoding="utf8")
