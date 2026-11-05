"""Oracolo indipendente dello scavo, derivato da target/rif/comune_geo.

Solo interi/Fraction e libreria standard: nessun risultato o predicato geo.
Enumera orientamenti, permutazioni della coda iniziale e tutti i pareggi.
Rigenerare: python crates/plenora-kernels-geo/tests/fixtures/concave_oracolo.py
"""
from fractions import Fraction
from functools import lru_cache
from itertools import permutations, product
import json
from pathlib import Path


def cross(a, b, c):
    return (b[0]-a[0])*(c[1]-a[1])-(b[1]-a[1])*(c[0]-a[0])


def dist2(a, b):
    return sum((x-y)**2 for x, y in zip(a, b))


def segment2(p, a, b):
    if a == b:
        return Fraction(dist2(p, a))
    t = max(0, min(1, Fraction(sum((p[k]-a[k])*(b[k]-a[k]) for k in (0, 1)), dist2(a, b))))
    return sum((p[k]-a[k]-t*(b[k]-a[k]))**2 for k in (0, 1))


def canonical(ids):
    ids = tuple(ids)
    return min(s[i:]+s[:i] for s in (ids, ids[::-1]) for i in range(len(ids)))


def intersects(a, b, c, d):
    signs = [cross(a, b, c), cross(a, b, d), cross(c, d, a), cross(c, d, b)]
    return (signs[0]*signs[1] < 0 and signs[2]*signs[3] < 0) or any(
        segment2(p, x, y) == 0 for p, x, y in ((a,c,d),(b,c,d),(c,a,b),(d,a,b)))


def simple(points):
    edges = list(zip(points, points[1:]+points[:1]))
    for i, (a, b) in enumerate(edges):
        for j, (c, d) in enumerate(edges[:i]):
            if j == i-1 or (j == 0 and i == len(edges)-1):
                # Adjacent collinear edges may meet but must not overlap.
                shared = set((a,b)) & set((c,d))
                if any(segment2(p,c,d) == 0 for p in (a,b) if p not in shared):
                    return False
                if any(segment2(p,a,b) == 0 for p in (c,d) if p not in shared):
                    return False
            elif intersects(a,b,c,d):
                return False
    return True


def convex(points):
    chains = []
    for seq in (points, points[::-1]):
        chain = []
        for p in seq:
            while len(chain) >= 2 and cross(chain[-2], chain[-1], p) <= 0:
                chain.pop()
            chain.append(p)
        chains.extend(chain[:-1])
    return tuple(points.index(p) for p in chains)


def oracle(raw, concavity=2, threshold=0):
    points = sorted(set(map(tuple, raw)))
    initial = convex(points)
    if len(initial) < 3:
        return []

    @lru_cache(None)
    def choices(hull, ia, ib):
        i = hull.index(ia)
        assert hull[(i+1)%len(hull)] == ib
        a,b = points[ia],points[ib]
        if dist2(a,b) <= threshold**2:
            return (None,)
        limit = Fraction(dist2(a,b), concavity**2)
        groups = {}
        for j,p in enumerate(points):
            distance = segment2(p,a,b)
            if j not in hull and distance <= limit:
                groups.setdefault(distance, []).append(j)
        results = set()
        for order in product(*(permutations(groups[d]) for d in sorted(groups))):
            checked = []
            found = None
            for j in (v for group in order for v in group):
                p = points[j]
                distance = segment2(p,a,b)
                expanded = hull[:i+1]+(j,)+hull[i+1:]
                if (distance >= segment2(p,points[hull[i-1]],a)
                        or distance >= segment2(p,b,points[hull[(i+2)%len(hull)]])
                        or not simple([points[k] for k in expanded])):
                    checked.append(j)
                    continue
                def inside(k):
                    signs = [cross(a,p,points[k]),cross(p,b,points[k]),cross(b,a,points[k])]
                    return all(s > 0 for s in signs) or all(s < 0 for s in signs)
                chosen = next((k for k in checked if inside(k)),j)
                if min(dist2(points[chosen],a),dist2(points[chosen],b)) < limit:
                    found = chosen
                break
            results.add(found)
        return tuple(results)

    @lru_cache(None)
    def terminal(hull, queue):
        if not queue:
            return frozenset((canonical(hull),))
        ia,ib = queue[0]
        result = set()
        for chosen in choices(hull,ia,ib):
            if chosen is None:
                result.update(terminal(hull,queue[1:]))
            else:
                i = hull.index(ia)
                result.update(terminal(hull[:i+1]+(chosen,)+hull[i+1:],queue[1:]+((ia,chosen),(chosen,ib))))
        return frozenset(result)

    allowed = set()
    for hull in (initial,initial[::-1]):
        for queue in permutations(zip(hull,hull[1:]+hull[:1])):
            allowed.update(terminal(hull,queue))
    return sorted(allowed)


def main():
    clouds = [[(i*10,i*i%7*10) for i in range(12)]]
    square = [(0,0),(60,0),(60,60),(0,60)]
    interior = [(10,20),(20,10),(30,30),(50,30)]
    clouds.extend(square+[p for i,p in enumerate(interior) if mask & (1 << i)] for mask in range(16))
    clouds.extend([
        square+[(30,0),(60,30),(30,60),(0,30),(30,30)],
        square+[(30,30)]*3+square,
        [(0,0),(10,10),(20,20),(10,10)],
        [(0,0)]*4,
        [(0,30),(30,0),(60,30),(30,60),(30,30)],
    ])
    cases = []
    for raw in clouds:
        for concavity, threshold in ((1,0),(2,0),(3,0),(2,100)):
            cases.append(dict(points=raw, unique=sorted(set(raw)), concavity=concavity,
                              threshold=threshold, allowed=oracle(raw,concavity,threshold)))
    Path(__file__).with_suffix('.json').write_text(json.dumps(cases,separators=(',',':'))+'\n',encoding='utf8')
    print(f'{len(cases)} casi razionali')


if __name__ == '__main__':
    main()
