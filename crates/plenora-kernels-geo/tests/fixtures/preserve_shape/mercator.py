"""C1: Snyder 1987, eq. 7-7 and 7-9; GeographicLib Python 2.1.

No Rust import. Inverse by bisection of isometric latitude, not Karney Newton.
pyproj is unavailable offline. Run with PYTHONPATH=target/preserve-oracle.
"""
import json
import math
import geographiclib
from geographiclib.geodesic import Geodesic

A = 6378137.0
F = 1 / 298.257223563
E = math.sqrt(F * (2 - F))


def inverse(x, y):
    lo, hi = -math.pi / 2, math.pi / 2
    for _ in range(60):
        phi = (lo + hi) / 2
        psi = math.asinh(math.tan(phi)) - E * math.atanh(E * math.sin(phi))
        if psi < y / A:
            lo = phi
        else:
            hi = phi
    return math.degrees((lo + hi) / 2), math.degrees(x / A)


def samples(start, end, count):
    for i in range(count):
        t = i / count
        yield inverse(start[0] + t * (end[0] - start[0]),
                      start[1] + t * (end[1] - start[1]))


def reference(count):
    line = [(0, 4000000), (2000000, 6000000)]
    points = list(samples(*line, count)) + [inverse(*line[-1])]
    length = math.fsum(Geodesic.WGS84.Inverse(*p, *q)["s12"]
                       for p, q in zip(points, points[1:]))
    vertices = [(0, 4000000), (2000000, 4200000),
                (1800000, 6000000), (-100000, 5700000), (0, 4000000)]
    polygon = Geodesic.WGS84.Polygon()
    for p, q in zip(vertices, vertices[1:]):
        for lat, lon in samples(p, q, count):
            polygon.AddPoint(lat, lon)
    return {"length_m": length, "area_m2": polygon.Compute(False, True)[2]}


if __name__ == "__main__":
    if geographiclib.__version__ != "2.1":
        raise RuntimeError("versione oracolo non supportata")
    print(json.dumps({"source": "Snyder 1987 eq. 7-7, 7-9; GeographicLib Python 2.1",
                      **{str(n): reference(n) for n in (16384, 32768)}}, indent=2))
