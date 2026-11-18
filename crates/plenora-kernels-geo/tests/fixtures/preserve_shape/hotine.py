"""EPSG GN 7-2 3.2.4.1; Snyder 1987 chapter 9; Swiss Gauss-sphere formula.
Independent Swiss oblique Mercator inverse and isometric bisection; GeographicLib Python 2.1.
Run under target/preserve-oracle, never importing the Rust kernel.
"""
import json
import math
from pathlib import Path
import geographiclib
from geographiclib.geodesic import Geodesic

class HotineReference:
    def __init__(self):
        self.a=6377397.155
        self.f=1/299.1528128
        self.e=math.sqrt(self.f*(2-self.f))
        self.phi0=math.radians(46.95240555555556)
        self.lon0=math.radians(7.439583333333333)
        s,c=math.sin(self.phi0),math.cos(self.phi0)
        self.n=math.sqrt(1+self.e**2*c**4/(1-self.e**2))
        self.chi0=math.asin(s/self.n)
        self.offset=math.asinh(math.tan(self.chi0))-self.n*self.isometric(self.phi0)
        self.rk=self.a*math.sqrt(1-self.e**2)/(1-self.e**2*s*s)
        self.geodesic=Geodesic(self.a,self.f)

    def isometric(self,phi):
        return math.asinh(math.tan(phi))-self.e*math.atanh(self.e*math.sin(phi))

    def inverse(self,x,y):
        u,v=(x-2600000)/self.rk,(y-1200000)/self.rk
        rotated_latitude=math.atan(math.sinh(v))
        chi=math.asin(math.cos(self.chi0)*math.sin(rotated_latitude)
                      +math.sin(self.chi0)*math.cos(rotated_latitude)*math.cos(u))
        longitude=math.asin(math.cos(rotated_latitude)*math.sin(u)/math.cos(chi))
        lon=self.lon0+longitude/self.n
        psi=(math.asinh(math.tan(chi))-self.offset)/self.n
        low,high=-math.pi/2,math.pi/2
        for _ in range(58):
            mid=(low+high)/2
            if self.isometric(mid)<psi: low=mid
            else: high=mid
        return math.degrees((low+high)/2),math.degrees(lon)

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
    projection=HotineReference()
    line=[(2500000,1100000),(2700000,1300000)]
    ring=[(2500000,1100000),(2700000,1130000),(2680000,1300000),
          (2520000,1270000),(2500000,1100000)]
    return {"source":"EPSG GN 7-2 3.2.4.1; Snyder 1987 chapter 9; Swiss Gauss-sphere motor formula",
            "geographiclib_version":geographiclib.__version__,
            "divisor":projection.rk,
            "cases":[{"epsg":2056,"line_xy":line,"ring_xy":ring,
                      "measurements":{str(n):projection.measure(line,ring,n) for n in (16384,32768)}}]}

if __name__ == "__main__":
    Path(__file__).with_suffix(".json").write_text(json.dumps(reference(),indent=2)+"\n",encoding="utf-8",newline="\n")
