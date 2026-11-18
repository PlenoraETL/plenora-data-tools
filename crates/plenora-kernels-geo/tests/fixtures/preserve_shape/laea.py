"""Snyder 1987 chapter 24 (24-17 through 24-29); EPSG GN 7-2 3.3.2.
Independent LAEA inverse and authalic bisection; GeographicLib Python 2.1.
Run under target/preserve-oracle, never importing the Rust kernel.
"""
import json
import math
from pathlib import Path
import geographiclib
from geographiclib.geodesic import Geodesic

class AzimuthalReference:
    def __init__(self):
        self.a=6378137.0
        self.f=1/298.257222101
        self.e=math.sqrt(self.f*(2-self.f))
        self.qp=self.authalic(math.pi/2)
        self.beta0=math.asin(self.authalic(math.radians(52))/self.qp)
        self.rq=self.a*math.sqrt(self.qp/2)
        self.d=self.a*math.cos(math.radians(52))/math.sqrt(1-self.e**2*math.sin(math.radians(52))**2)/(self.rq*math.cos(self.beta0))
        self.geodesic=Geodesic(self.a,self.f)

    def authalic(self,phi):
        s=math.sin(phi)
        return (1-self.e**2)*(s/(1-self.e**2*s*s)+math.atanh(self.e*s)/self.e)

    def inverse(self,x,y):
        dx=(x-4321000)/self.d
        dy=(y-3210000)*self.d
        rho=math.hypot(dx,dy)
        if rho==0:
            return 52.0,10.0
        c=2*math.asin(rho/(2*self.rq))
        z=math.cos(c)*math.sin(self.beta0)+dy*math.sin(c)*math.cos(self.beta0)/rho
        lon=math.radians(10)+math.atan2(dx*math.sin(c),rho*math.cos(self.beta0)*math.cos(c)-dy*math.sin(self.beta0)*math.sin(c))
        low,high=-math.pi/2,math.pi/2
        for _ in range(58):
            mid=(low+high)/2
            if self.authalic(mid)<self.qp*z: low=mid
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
    projection=AzimuthalReference()
    line=[(4121000,3010000),(4521000,3410000)]
    ring=[(4121000,3010000),(4521000,3060000),(4491000,3410000),
          (4151000,3360000),(4121000,3010000)]
    return {"source":"Snyder 1987 chapter 24; EPSG GN 7-2 method 9820",
            "geographiclib_version":geographiclib.__version__,
            "x_divisor":projection.rq*projection.d,"y_divisor":projection.rq/projection.d,
            "cases":[{"epsg":3035,"line_xy":line,"ring_xy":ring,
                      "measurements":{str(n):projection.measure(line,ring,n) for n in (16384,32768)}}]}

if __name__ == "__main__":
    Path(__file__).with_suffix(".json").write_text(json.dumps(reference(),indent=2)+"\n",encoding="utf-8",newline="\n")
