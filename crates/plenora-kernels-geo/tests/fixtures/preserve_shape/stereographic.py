"""EPSG GN 7-2 3.3.1.1 (Gauss sphere); Snyder 1987 chapter 21 (sphere).
Independent stereographic inverse and isometric bisection; GeographicLib Python 2.1.
Run under target/preserve-oracle, never importing the Rust kernel.
"""
import json
import math
from pathlib import Path
import geographiclib
from geographiclib.geodesic import Geodesic

class StereographicReference:
    def __init__(self):
        self.a=6377397.155
        self.f=1/299.15281
        self.e=math.sqrt(self.f*(2-self.f))
        self.phi0=math.radians(52.15616055555555)
        self.lon0=math.radians(5.3876388888888895)
        s,c=math.sin(self.phi0),math.cos(self.phi0)
        self.n=math.sqrt(1+self.e**2*c**4/(1-self.e**2))
        w1=math.exp(2*self.n*self.isometric(self.phi0))
        chi00=(w1-1)/(w1+1)
        self.c=(self.n+s)*(1-chi00)/((self.n-s)*(1+chi00))
        w2=self.c*w1
        self.chi0=math.asin((w2-1)/(w2+1))
        self.rk=self.a*math.sqrt(1-self.e**2)/(1-self.e**2*s*s)*0.9999079
        self.geodesic=Geodesic(self.a,self.f)

    def isometric(self,phi):
        return math.asinh(math.tan(phi))-self.e*math.atanh(self.e*math.sin(phi))

    def inverse(self,x,y):
        dx,dy=x-155000,y-463000
        rho=math.hypot(dx,dy)
        if rho==0:
            return math.degrees(self.phi0),math.degrees(self.lon0)
        angle=2*math.atan(rho/(2*self.rk))
        z=math.cos(angle)*math.sin(self.chi0)+dy*math.sin(angle)*math.cos(self.chi0)/rho
        lon=self.lon0+math.atan2(dx*math.sin(angle),rho*math.cos(self.chi0)*math.cos(angle)-dy*math.sin(self.chi0)*math.sin(angle))/self.n
        psi=math.log((1+z)/(self.c*(1-z)))/(2*self.n)
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
    projection=StereographicReference()
    line=[(55000,363000),(255000,563000)]
    ring=[(55000,363000),(255000,393000),(235000,563000),
          (75000,533000),(55000,363000)]
    return {"source":"EPSG GN 7-2 3.3.1.1 method 9809; Snyder 1987 chapter 21 sphere",
            "geographiclib_version":geographiclib.__version__,
            "divisor":projection.rk,
            "cases":[{"epsg":28992,"line_xy":line,"ring_xy":ring,
                      "measurements":{str(n):projection.measure(line,ring,n) for n in (16384,32768)}}]}

if __name__ == "__main__":
    Path(__file__).with_suffix(".json").write_text(json.dumps(reference(),indent=2)+"\n",encoding="utf-8",newline="\n")
