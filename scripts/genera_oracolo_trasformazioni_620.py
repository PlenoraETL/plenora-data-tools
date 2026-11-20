"""Oracolo indipendente contratti 1 e 2: Decimal a 80 cifre, senza runtime PROJ.

Fonti: EPSG v11.022 operazione 1662, metodi 9606 e 9602; PROJ
https://proj.org/en/stable/operations/transformations/helmert.html
https://proj.org/en/stable/operations/projections/webmerc.html
Parametri dal registro fissato, formule implementate indipendentemente.
L'inversa Helmert segue PROJ 9.5.1 (trasposta della matrice linearizzata).
La latitudine geocentrica inversa usa punto fisso, non Bowring del kernel.
Eseguire con --check per verificare la fixture, senza dipendenze esterne.
"""

import argparse
from decimal import Decimal as D, localcontext
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUTPUT = ROOT / "crates/plenora-core/tests/fixtures/riproiezione/policy_620.json"
PI = D("3.1415926535897932384626433832795028841971693993751058209749445923078164062862089986280348253421170679")


def sincos(x):
    x %= 2 * PI
    if x > PI:
        x -= 2 * PI
    sine, cosine = x, D(1)
    st, ct = x, D(1)
    for n in range(1, 160):
        st *= -x * x / ((2 * n) * (2 * n + 1))
        ct *= -x * x / ((2 * n - 1) * (2 * n))
        sine += st
        cosine += ct
    return sine, cosine


def atan(x):
    for _ in range(8):
        x /= 1 + (1 + x * x).sqrt()
    result, term = x, x
    for n in range(1, 160):
        term *= -x * x
        result += term / (2 * n + 1)
    return result * 256


def atan2(y, x):
    if x == 0:
        if y == 0:
            raise ValueError("undefined angle")
        return PI / 2 if y > 0 else -PI / 2
    angle = atan(y / x)
    if x < 0:
        angle += PI if y >= 0 else -PI
    return angle


def ellipsoid(inverse_flattening, major):
    f = 1 / D(inverse_flattening)
    return D(major), f * (2 - f)


def cartesian(lon, lat, ell):
    major, e2 = ell
    slon, clon = sincos(lon * PI / 180)
    slat, clat = sincos(lat * PI / 180)
    radius = major / (1 - e2 * slat * slat).sqrt()
    return [radius * clat * clon, radius * clat * slon, radius * (1 - e2) * slat]


def geographic(v, ell):
    x, y, z = v
    major, e2 = ell
    radius = (x * x + y * y).sqrt()
    phi = atan2(z, radius)
    for _ in range(80):
        sine, _ = sincos(phi)
        normal = major / (1 - e2 * sine * sine).sqrt()
        phi = atan2(z + e2 * normal * sine, radius)
    return atan2(y, x) * 180 / PI, phi * 180 / PI


def helmert(lon, lat, inverse):
    intl = ellipsoid("297", "6378388")
    wgs = ellipsoid("298.257223563", "6378137")
    vector = cartesian(lon, lat, wgs if inverse else intl)
    rx, ry, rz = [D(v) * PI / 648000 for v in ("-0.374", "-0.679", "-1.379")]
    matrix = [[D(1), -rz, ry], [rz, D(1), -rx], [-ry, rx, D(1)]]
    translation = list(map(D, ("-168.6", "-34.0", "38.6")))
    scale = 1 + D("-9.48") / 1000000
    if inverse:
        vector = [(v - t) / scale for v, t in zip(vector, translation)]
        transformed = [sum(matrix[j][i] * vector[j] for j in range(3)) for i in range(3)]
    else:
        transformed = [scale * sum(row[j] * vector[j] for j in range(3)) + t
                       for row, t in zip(matrix, translation)]
    return geographic(transformed, intl if inverse else wgs)


def generate():
    with localcontext() as ctx:
        ctx.prec = 80
        rows = []
        for inverse in (False, True):
            for lon, lat in (("9", "40"), ("12.5", "41.9"), ("0", "0"),
                             ("179.999", "30"), ("-179.999", "-30"), ("120", "80")):
                x, y = helmert(D(lon), D(lat), inverse)
                rows.append(dict(inverse=inverse, lon=float(lon), lat=float(lat), x=float(x), y=float(y)))
        return json.dumps(dict(source="EPSG v11.022:1662; Decimal 80 digits; PROJ Helmert equations",
                               rows=rows), indent=2) + "\n"


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    text = generate()
    if args.check:
        if OUTPUT.read_text(encoding="utf-8") != text:
            raise SystemExit("fixture out of date")
    else:
        OUTPUT.write_text(text, encoding="utf-8")
