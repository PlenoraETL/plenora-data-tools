"""C1 independent point/arc and geometry references, GeographicLib Python 2.1 (G2).

Run with that exact package available on PYTHONPATH. No Rust kernel imports.
The heap encloses the *global* minimum using the metric triangle inequality;
the fixture is the final interval, not an unconstrained local minimizer.
"""

import heapq
import json

import geographiclib
from geographiclib.geodesic import Geodesic


def reference(point, start, end):
    """Return a certified numerical bracket in metres on WGS84."""
    geodesic = Geodesic.WGS84
    inverse = geodesic.Inverse(*start, *end)
    line = geodesic.Line(*start, inverse["azi1"])
    heap = []
    upper = float("inf")
    margin = 1e-6
    evaluations = 0

    def add(lo, hi):
        nonlocal upper, evaluations
        middle = (lo + hi) / 2
        position = line.Position(middle)
        distance = geodesic.Inverse(
            *point, position["lat2"], position["lon2"]
        )["s12"]
        evaluations += 1
        upper = min(upper, distance + margin)
        lower = max(0.0, distance - (hi - lo) / 2 - 4 * margin)
        heapq.heappush(heap, (lower, lo, hi))

    add(0.0, inverse["s12"])
    while heap[0][0] < upper - 0.0005:
        _, lo, hi = heapq.heappop(heap)
        mid = (lo + hi) / 2
        add(lo, mid)
        add(mid, hi)
    return dict(
        point_latlon=point,
        start_latlon=start,
        end_latlon=end,
        lower_m=heap[0][0],
        upper_m=upper,
        evaluations=evaluations,
    )



def arc_pairs(left, right):
    """Independent two-parameter global metric enclosure; coordinates lon/lat."""
    g = Geodesic.WGS84
    def arc(pair):
        a, b = pair
        inv = g.Inverse(a[1], a[0], b[1], b[0])
        return g.Line(a[1], a[0], inv["azi1"]), inv["s12"]
    aa, bb = [arc(pair) for pair in left], [arc(pair) for pair in right]
    heap, upper, serial = [], float("inf"), 0
    def position(line, offset):
        p = line.Position(offset)
        return p["lat2"], p["lon2"]
    def add(i, j, lo, hi, start, end):
        nonlocal upper, serial
        d = g.Inverse(*position(aa[i][0], (lo + hi) / 2),
                      *position(bb[j][0], (start + end) / 2))["s12"]
        upper = min(upper, d + 1e-6)
        lower = max(0., d - (hi - lo + end - start) / 2 - 4e-6)
        serial += 1
        heapq.heappush(heap, (lower, serial, i, j, lo, hi, start, end))
    for i, (a, length) in enumerate(aa):
        for j, (b, size) in enumerate(bb):
            for x in (0., length):
                for y in (0., size):
                    upper = min(upper, g.Inverse(*position(a, x), *position(b, y))["s12"] + 1e-6)
            add(i, j, 0., length, 0., size)
    while heap[0][0] < upper - 0.0005:
        _, _, i, j, lo, hi, start, end = heapq.heappop(heap)
        if hi - lo >= end - start:
            mid = (hi + lo) / 2
            add(i, j, lo, mid, start, end)
            add(i, j, mid, hi, start, end)
        else:
            mid = (start + end) / 2
            add(i, j, lo, hi, start, mid)
            add(i, j, lo, hi, mid, end)
    return [heap[0][0], upper]


def fixtures():
    """Small convex rings, away from the cut locus: azimuth winding is unambiguous."""
    g = Geodesic.WGS84
    def rect(lo, hi):
        return [[lo, lo], [hi, lo], [hi, hi], [lo, hi], [lo, lo]]
    def polygon(lo, hi):
        return {"kind": "polygon", "parts": [rect(lo, hi)]}
    def point(x, y):
        return {"kind": "point", "parts": [[[x, y]]]}
    def line(coords):
        return {"kind": "line", "parts": [coords]}
    def edges(geometry):
        return [(a, b) for part in geometry["parts"] for a, b in
                (zip(part, part[1:]) if len(part) > 1 else [(part[0], part[0])])]
    def ring_contains(p, ring):
        az = [g.Inverse(p[1], p[0], v[1], v[0])["azi1"] for v in ring]
        winding = sum((b - a + 180) % 360 - 180 for a, b in zip(az, az[1:]))
        return abs(winding) > 180
    def contains(surface, other):
        if surface["kind"] not in ("polygon", "multi_polygon"):
            return False
        for part in other["parts"]:
            p = part[0]
            if surface["kind"] == "multi_polygon":
                if any(ring_contains(p, ring) for ring in surface["parts"]):
                    return True
            elif ring_contains(p, surface["parts"][0]) and not any(
                ring_contains(p, hole) for hole in surface["parts"][1:]
            ):
                return True
        return False
    result = []
    def add(name, a, b):
        bracket = [0., 0.] if contains(a, b) or contains(b, a) else arc_pairs(edges(a), edges(b))
        result.append(dict(name=name, a=a, b=b, bracket=bracket))
    for metres in (.001, .02, 1., 10., 100.):
        for sign in (-1., 1.):
            p = g.Direct(0., .005, 0. if sign > 0 else 180., metres)
            add(f"border_{sign}_{metres}", point(p["lon2"], p["lat2"]), polygon(0., .01))
    for width, away in ((.0001, 6.3), (.0002, .009), (.001, .09)):
        add(f"small_{width}", point(-away, width / 2), polygon(0., width))
    for half, longitude in ((.00045, 10.13), (.000045, 19.), (.000045, 10.0005)):
        translated = [[10 + x, 45 + y] for x, y in rect(-half, half)]
        add(f"translated_small_{half}_{longitude}", point(longitude, 45.),
            {"kind": "polygon", "parts": [translated]})
    for width in (.001, 1.):
        multi = {"kind": "multi_polygon", "parts": [rect(0., width), rect(2 * width, 3 * width)]}
        add(f"multi_outside_{width}", point(-width, 0.), multi)
        add(f"multi_inside_{width}", point(2.5 * width, 2.5 * width), multi)
    holes = {"kind": "polygon", "parts": [rect(0., .01), rect(.002, .003), rect(.006, .007)]}
    add("two_holes_solid", point(.005, .005), holes)
    add("two_holes_first", point(.0025, .0025), holes)
    add("two_holes_second", point(.0065, .0065), holes)
    add("line_polygon", line([[-.002, 0.], [-.001, 0.]]), polygon(0., .001))
    add("polygon_polygon", polygon(-.002, -.001), polygon(0., .001))
    add("indexed_lines", line([[1., 2.], [1.2, 2.1], [1.4, 2.]]),
        line([[2., 3.], [2.2, 3.1], [2.4, 3.]]))
    for latitude in (0., 45., 80.):
        for length in (.002, 100_000.):
            for gap in (1.001, 1.1):
                vertices = [g.Direct(latitude, 1., 0., offset) for offset in (-length, 0., gap, gap + length)]
                coords = [[v["lon2"], v["lat2"]] for v in vertices]
                add(f"taylor_{latitude}_{length}_{gap}", line(coords[:2]), line(coords[2:]))
    for latitude in (0., 45., 80.):
        endpoints = [g.Direct(latitude, 1., 0., offset) for offset in (-50_000., 50_000.)]
        coords = [[v["lon2"], v["lat2"]] for v in endpoints]
        for gap in (1.001, 1.1):
            p = g.Direct(latitude, 1., 90., gap)
            add(f"taylor_interior_{latitude}_{gap}", point(p["lon2"], p["lat2"]), line(coords))
    return result


if __name__ == "__main__":
    import argparse
    from pathlib import Path
    if geographiclib.__version__ != "2.1":
        raise RuntimeError("GeographicLib Python 2.1 required")
    parser = argparse.ArgumentParser()
    parser.add_argument("--fixtures", type=Path)
    args = parser.parse_args()
    if args.fixtures:
        args.fixtures.write_text(json.dumps(fixtures(), indent=2) + "\n", encoding="utf-8", newline="\n")
    else:
        cases = [((14.1, 35.0), (12.3, 34.5), (13.7, 37.2)),
                 ((72.0, 170.0), (70.0, 165.0), (73.0, -175.0))]
        print(json.dumps([reference(*case) for case in cases], indent=2))
