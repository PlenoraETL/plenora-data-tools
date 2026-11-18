"""Snyder 1987 chapter 15, equations 15-7 through 15-11; EPSG GN 7-2 3.1.1.1.
Independent LCC formulas, bisection and GeographicLib Python 2.1; no Rust imports.
Run with PYTHONPATH=target/preserve-oracle.
"""
import json
import math
from pathlib import Path
import geographiclib
from geographiclib.geodesic import Geodesic

class LambertReference:
    def __init__(self, a, inverse_flattening, lat0, lon0, lat1, lat2, east, north):
        self.a = a
        self.e = math.sqrt((2-1/inverse_flattening)/inverse_flattening)
        self.geodesic = Geodesic(a, 1/inverse_flattening)
        self.east, self.north = east, north
        self.lon0 = math.radians(lon0)
        first, second = map(math.radians, (lat1, lat2))
        def m(phi):
            return math.cos(phi)/math.sqrt(1-self.e**2*math.sin(phi)**2)
        self.n = math.log(m(first)/m(second))/(self.isometric(second)-self.isometric(first))
        self.factor = m(first)*math.exp(self.n*self.isometric(first))/self.n
        self.radius = a*self.factor*math.exp(-self.n*self.isometric(math.radians(lat0)))

    def isometric(self, phi):
        return math.asinh(math.tan(phi))-self.e*math.atanh(self.e*math.sin(phi))

    def inverse(self, x, y):
        sign = math.copysign(1, self.n)
        u, v = sign*(x-self.east), sign*(self.radius-y+self.north)
        psi = -math.log(math.hypot(u, v)/abs(self.a*self.factor))/self.n
        longitude = self.lon0+math.atan2(u, v)/self.n
        low, high = -math.pi/2, math.pi/2
        for _ in range(58):
            middle = (low+high)/2
            if self.isometric(middle)<psi:
                low=middle
            else:
                high=middle
        return math.degrees((low+high)/2), math.degrees(longitude)

    def samples(self, start, end, count):
        for i in range(count):
            t = i / count
            yield self.inverse(*(p + t*(q-p) for p, q in zip(start, end)))

    def measure(self, line, ring, count):
        points = list(self.samples(*line, count)) + [self.inverse(*line[-1])]
        length = math.fsum(self.geodesic.Inverse(*p, *q)["s12"]
                           for p, q in zip(points, points[1:]))
        polygon = self.geodesic.Polygon()
        for start, end in zip(ring, ring[1:]):
            for latitude, longitude in self.samples(start, end, count):
                polygon.AddPoint(latitude, longitude)
        return {"length_m": length, "area_m2": abs(polygon.Compute(False, True)[2])}


def reference():
    if geographiclib.__version__ != "2.1":
        raise RuntimeError("versione oracolo non supportata")
    cases = [
        (2154, (6378137.0, 298.257222101, 46.5, 3, 49, 44, 700000, 6600000),
         [(500000, 6400000), (850000, 6800000)],
         [(500000, 6400000), (850000, 6450000), (820000, 6800000),
          (520000, 6750000), (500000, 6400000)]),
    ]
    result = {"source": "Snyder 1987 chapter 15; EPSG GN 7-2 method 9802",
              "geographiclib_version": geographiclib.__version__, "cases": []}
    for epsg, parameters, line, ring in cases:
        lcc = LambertReference(*parameters)
        result["cases"].append({
            "epsg": epsg, "parameters": parameters, "line_xy": line, "ring_xy": ring,
            "vertices_lat_lon": [lcc.inverse(*p) for p in ring],
            "measurements": {str(n): lcc.measure(line, ring, n) for n in (16384, 32768)},
        })
    return result

if __name__ == "__main__":
    Path(__file__).with_suffix(".json").write_text(json.dumps(reference(), indent=2)+"\n", encoding="utf-8", newline="\n")
