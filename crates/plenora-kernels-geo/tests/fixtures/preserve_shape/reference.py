"""C1: GeographicLib Python 2.1, G1/G2, nessun import del motore Rust.

Eseguire con geographiclib==2.1 e salvare stdout come reference.json.
Due densificazioni uniformi indipendenti dell'originale, senza estrapolazione.
L'inversa Web Mercator e' la formula EPSG 1024, non il codice del motore.
"""

import json
import math

import geographiclib
from geographiclib.geodesic import Geodesic


def web_point(fraction):
    x = 2_000_000 * fraction
    y = 4_000_000 + 2_000_000 * fraction
    return math.degrees(math.atan(math.sinh(y / 6_378_137))), math.degrees(x / 6_378_137)


def web_length(divisions):
    previous = web_point(0)
    distances = []
    for index in range(1, divisions + 1):
        point = web_point(index / divisions)
        distances.append(Geodesic.WGS84.Inverse(*previous, *point)["s12"])
        previous = point
    return math.fsum(distances)


def rectangle(west, south, east, north, divisions):
    polygon = Geodesic.WGS84.Polygon()
    vertices = [(west, south), (east, south), (east, north), (west, north), (west, south)]
    for start, end in zip(vertices, vertices[1:]):
        for index in range(divisions):
            fraction = index / divisions
            polygon.AddPoint(start[1] + fraction * (end[1] - start[1]),
                             start[0] + fraction * (end[0] - start[0]))
    return polygon.Compute(False, True)[2]


if __name__ == "__main__":
    if geographiclib.__version__ != "2.1":
        raise RuntimeError("versione dell'oracolo non supportata")
    output = {"source": "GeographicLib Python 2.1; Karney 2013; EPSG 1024"}
    for count in (32768, 65536):
        output[str(count)] = {
            "web_length_m": web_length(count),
            "rectangle_m2": rectangle(10, 40, 20, 50, count),
            "hole_m2": rectangle(12, 42, 14, 44, count),
        }
    print(json.dumps(output, indent=2))
