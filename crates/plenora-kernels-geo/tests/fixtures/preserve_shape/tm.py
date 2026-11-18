"""Karney 2011, equations 14, 16, 18, 35, 36; EPSG GN 7-2, method 9807.

Independent order-six Krueger reference, followed by GeographicLib Python 2.1.
This driver does not import Rust and does not certify the infinite-series tail.
Run from the worktree with PYTHONPATH=target/preserve-oracle.
"""

import json
import math
from pathlib import Path

import geographiclib
from geographiclib.geodesic import Geodesic


class KruegerReference:
    def __init__(self, a, inverse_flattening, lat0, lon0, k0, east, north):
        self.a = a
        self.f = 1 / inverse_flattening
        self.e = math.sqrt(self.f * (2 - self.f))
        self.lon0 = math.radians(lon0)
        self.east, self.north = east, north
        n = self.f / (2 - self.f)
        self.scale = k0 * a / (1 + n) * (1 + n**2 / 4 + n**4 / 64 + n**6 / 256)
        # Karney 2011 eq. 35 and 36, rational coefficients at orders one to six.
        alpha = (
            (1/2, -2/3, 5/16, 41/180, -127/288, 7891/37800),
            (0, 13/48, -3/5, 557/1440, 281/630, -1983433/1935360),
            (0, 0, 61/240, -103/140, 15061/26880, 167603/181440),
            (0, 0, 0, 49561/161280, -179/168, 6601661/7257600),
            (0, 0, 0, 0, 34729/80640, -3418889/1995840),
            (0, 0, 0, 0, 0, 212378941/319334400),
        )
        beta = (
            (1/2, -2/3, 37/96, -1/360, -81/512, 96199/604800),
            (0, 1/48, 1/15, -437/1440, 46/105, -1118711/3870720),
            (0, 0, 17/480, -37/840, -209/4480, 5569/90720),
            (0, 0, 0, 4397/161280, -11/504, -830251/7257600),
            (0, 0, 0, 0, 4583/161280, -108847/3991680),
            (0, 0, 0, 0, 0, 20648693/638668800),
        )
        def polynomial(row):
            return math.fsum(c * n**j for j, c in enumerate(row, 1))
        self.beta = tuple(map(polynomial, beta))
        origin = math.atan(math.sinh(self.isometric(math.radians(lat0))))
        self.xi0 = origin + math.fsum(
            polynomial(row) * math.sin(2*j*origin)
            for j, row in enumerate(alpha, 1)
        )
        self.geodesic = Geodesic(a, self.f)

    def isometric(self, phi):
        return math.asinh(math.tan(phi)) - self.e * math.atanh(self.e * math.sin(phi))

    def inverse(self, x, y):
        xi = (y - self.north) / self.scale + self.xi0
        eta = (x - self.east) / self.scale
        real = xi - math.fsum(
            b * math.sin(2*j*xi) * math.cosh(2*j*eta)
            for j, b in enumerate(self.beta, 1)
        )
        imag = eta - math.fsum(
            b * math.cos(2*j*xi) * math.sinh(2*j*eta)
            for j, b in enumerate(self.beta, 1)
        )
        longitude = self.lon0 + math.atan2(math.sinh(imag), math.cos(real))
        psi = math.asinh(math.sin(real) / math.hypot(math.sinh(imag), math.cos(real)))
        # Bisection of the monotone isometric function, independent of core Newton.
        low, high = -math.pi/2, math.pi/2
        for _ in range(58):
            middle = (low + high) / 2
            if self.isometric(middle) < psi:
                low = middle
            else:
                high = middle
        return math.degrees((low + high) / 2), math.degrees(longitude)

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
    cases = (
        (32632, (6378137.0, 298.257223563, 0, 9, 0.9996, 500000, 0),
         [(400000, 4900000), (650000, 5200000)],
         [(400000, 4900000), (650000, 4950000), (620000, 5200000),
          (420000, 5150000), (400000, 4900000)]),
        (27700, (6377563.396, 299.3249646, 49, -2, 0.9996012717, 400000, -100000),
         [(300000, 200000), (550000, 500000)],
         [(300000, 200000), (550000, 250000), (520000, 500000),
          (320000, 450000), (300000, 200000)]),
    )
    result = {"source": "https://arxiv.org/abs/1002.1417; EPSG GN 7-2 method 9807",
              "geographiclib_version": geographiclib.__version__,
              "status": "numerical reference, not a certified remainder", "cases": []}
    for epsg, parameters, line, ring in cases:
        tm = KruegerReference(*parameters)
        origin = tm.inverse(parameters[5], parameters[6])
        if max(abs(origin[0]-parameters[2]), abs(origin[1]-parameters[3])) > 1e-12:
            raise RuntimeError("origine dell'oracolo non verificata")
        result["cases"].append({
            "epsg": epsg, "parameters": parameters, "line_xy": line, "ring_xy": ring,
            "vertices_lat_lon": [tm.inverse(*point) for point in ring],
            "measurements": {str(n): tm.measure(line, ring, n) for n in (16384, 32768)},
        })
    return result


if __name__ == "__main__":
    destination = Path(__file__).with_name("tm.json")
    destination.write_text(json.dumps(reference(), indent=2) + "\n", encoding="utf-8", newline="\n")
