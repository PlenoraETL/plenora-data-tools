#!/usr/bin/env python3
"""Riferimenti ad alta precisione vicino alle singolarita' delle proiezioni.

Scrive `crates/plenora-core/tests/fixtures/riproiezione/singolari.csv`: per
Lambert Azimuthal Equal Area (3035), Mercator (3395), Pseudo Mercator
(3857), Lambert Conic Conformal (2154), Oblique Stereographic (28992) e
Swiss Oblique Mercator (2056), punti vicino ai poli, ai bordi dei domini e
ai limiti di latitudine, proiettati con le formule chiuse EPSG in aritmetica
a 60 cifre (mpmath). Controllano la stabilita' numerica del codice Rust
dove `f64` perde cifre (per esempio `sin(phi)` che arrotonda a 1 a pochi
millimetri dal polo), non la scelta delle formule, che prova l'oracolo
contro PROJ.

Ambiente: quello vincolato di `genera_crs_integrati.py` (pyproj 3.7.2) piu'
mpmath 1.3.0, strumento di sviluppo, non dipendenza del workspace:

    python -m pip install --only-binary=:all: --target <dir> mpmath==1.3.0
    PYTHONPATH=<dir-pyproj>:<dir-mpmath> python -B scripts/genera_riferimenti_singolari.py

Transverse Mercator resta fuori: la serie di Krueger non ha forma chiusa, e
la forma di Karney usata dal codice e' stabile fino al polo (oracolo).
"""

from __future__ import annotations

import sys
from pathlib import Path

import mpmath
from mpmath import mp, mpf

sys.path.insert(0, str(Path(__file__).resolve().parent))
import genera_crs_integrati as base  # noqa: E402
import genera_riproiezione as riproiezione  # noqa: E402

RADICE = Path(__file__).resolve().parent.parent
DESTINAZIONE = RADICE / "crates" / "plenora-core" / "tests" / "fixtures" / "riproiezione" / "singolari.csv"
MPMATH_ATTESO = "1.3.0"
mp.dps = 60


def rad(gradi):
    return mpf(gradi) * mp.pi / 180


def ellissoide(datum):
    a = mpf(datum["ellissoide"][1])
    f = 1 / mpf(datum["ellissoide"][2])
    e2 = f * (2 - f)
    return a, e2, mp.sqrt(e2)


def q(phi, e):
    s = mp.sin(phi)
    return (1 - e * e) * (s / (1 - e * e * s * s) - mp.log((1 - e * s) / (1 + e * s)) / (2 * e))


def laea(p, a, e2, e, lon, lat):
    phi0, lam0 = rad(p["lat0"]), rad(p["lon0"])
    qp = q(mp.pi / 2, e)
    b0 = mp.asin(q(phi0, e) / qp)
    rq = a * mp.sqrt(qp / 2)
    d = a * (mp.cos(phi0) / mp.sqrt(1 - e2 * mp.sin(phi0) ** 2)) / (rq * mp.cos(b0))
    phi, lam = rad(lat), rad(lon)
    b = mp.asin(q(phi, e) / qp)
    dl = lam - lam0
    bb = rq * mp.sqrt(2 / (1 + mp.sin(b0) * mp.sin(b) + mp.cos(b0) * mp.cos(b) * mp.cos(dl)))
    x = p["falsa_est"] + bb * d * mp.cos(b) * mp.sin(dl)
    y = p["falsa_nord"] + (bb / d) * (mp.cos(b0) * mp.sin(b) - mp.sin(b0) * mp.cos(b) * mp.cos(dl))
    return x, y


def psi(phi, e):
    s = mp.sin(phi)
    return mp.log(mp.tan(mp.pi / 4 + phi / 2) * ((1 - e * s) / (1 + e * s)) ** (e / 2))


def mercatore(p, a, e2, e, lon, lat):
    k = a * p["k0"]
    return p["falsa_est"] + k * (rad(lon) - rad(p["lon0"])), p["falsa_nord"] + k * psi(rad(lat), e)


def pseudo(p, a, e2, e, lon, lat):
    return (
        p["falsa_est"] + a * (rad(lon) - rad(p["lon0"])),
        p["falsa_nord"] + a * mp.log(mp.tan(mp.pi / 4 + rad(lat) / 2)),
    )


def lcc(p, a, e2, e, lon, lat):
    def m(phi):
        return mp.cos(phi) / mp.sqrt(1 - e2 * mp.sin(phi) ** 2)

    def t(phi):
        return mp.exp(-psi(phi, e))

    p1, p2, pf = rad(p["lat1"]), rad(p["lat2"]), rad(p["lat_origine"])
    n = (mp.log(m(p1)) - mp.log(m(p2))) / (mp.log(t(p1)) - mp.log(t(p2)))
    ff = m(p1) / (n * t(p1) ** n)
    r = a * ff * t(rad(lat)) ** n
    rf = a * ff * t(pf) ** n
    th = n * (rad(lon) - rad(p["lon_origine"]))
    return p["est_origine"] + r * mp.sin(th), p["nord_origine"] + rf - r * mp.cos(th)


def stereo(p, a, e2, e, lon, lat):
    phi0, lam0 = rad(p["lat0"]), rad(p["lon0"])
    s0 = mp.sin(phi0)
    rr = a * mp.sqrt(1 - e2) / (1 - e2 * s0 * s0)
    n = mp.sqrt(1 + e2 * mp.cos(phi0) ** 4 / (1 - e2))
    s1 = (1 + s0) / (1 - s0)
    s2 = (1 - e * s0) / (1 + e * s0)
    w1 = (s1 * s2**e) ** n
    sc0 = (w1 - 1) / (w1 + 1)
    c = (n + s0) * (1 - sc0) / ((n - s0) * (1 + sc0))
    w2 = c * w1
    chi0 = mp.asin((w2 - 1) / (w2 + 1))
    phi = rad(lat)
    sa = (1 + mp.sin(phi)) / (1 - mp.sin(phi))
    sb = (1 - e * mp.sin(phi)) / (1 + e * mp.sin(phi))
    w = c * (sa * sb**e) ** n
    chi = mp.asin((w - 1) / (w + 1))
    dl = n * (rad(lon) - lam0)
    bb = 1 + mp.sin(chi) * mp.sin(chi0) + mp.cos(chi) * mp.cos(chi0) * mp.cos(dl)
    k = 2 * rr * p["k0"]
    return (
        p["falsa_est"] + k * mp.cos(chi) * mp.sin(dl) / bb,
        p["falsa_nord"] + k * (mp.sin(chi) * mp.cos(chi0) - mp.cos(chi) * mp.sin(chi0) * mp.cos(dl)) / bb,
    )


def svizzera(p, a, e2, e, lon, lat):
    phi0 = rad(p["lat_centro"])
    c = mp.sqrt(1 + e2 * mp.cos(phi0) ** 4 / (1 - e2))
    sp0 = mp.sin(phi0) / c
    phip0 = mp.asin(sp0)
    kk = mp.log(mp.tan(mp.pi / 4 + phip0 / 2)) - c * psi(phi0, e)
    kr = a * p["k_centro"] * mp.sqrt(1 - e2) / (1 - e2 * mp.sin(phi0) ** 2)
    phip = 2 * mp.atan(mp.exp(c * psi(rad(lat), e) + kk)) - mp.pi / 2
    lamp = c * (rad(lon) - rad(p["lon_centro"]))
    phipp = mp.asin(mp.cos(phip0) * mp.sin(phip) - sp0 * mp.cos(phip) * mp.cos(lamp))
    lampp = mp.asin(mp.cos(phip) * mp.sin(lamp) / mp.cos(phipp))
    return p["est_centro"] + kr * lampp, p["nord_centro"] + kr * mp.log(mp.tan(mp.pi / 4 + phipp / 2))


CASI = {
    3035: (laea, [(10.0, 89.9999995), (10.0, 89.99999999), (-20.0, 89.9), (40.0, 85.0), (10.0, 52.0), (-30.0, 30.0)]),
    3395: (mercatore, [(0.0, 85.05), (179.9, -85.05), (-179.9, 84.999999), (12.0, 1e-9)]),
    3857: (pseudo, [(0.0, 85.05), (179.9, -85.05), (-179.9, 84.999999), (12.0, 1e-9)]),
    2154: (lcc, [(-9.0, 36.0), (10.0, 56.0), (3.0, 46.5), (-4.0, 52.9)]),
    28992: (stereo, [(5.3876388888888895, 52.15616055555555), (2.0, 49.0), (8.5, 55.0), (3.0, 50.2)]),
    2056: (svizzera, [(7.439583333333333, 46.95240555555556), (5.0, 44.8), (11.5, 48.8), (6.0, 47.9)]),
}


def main() -> int:
    base.verifica_ambiente()
    if mpmath.__version__ != MPMATH_ATTESO:
        print(f"rifiutato: mpmath {mpmath.__version__}, atteso {MPMATH_ATTESO}", file=sys.stderr)
        return 1
    crs, datum, _, _ = riproiezione.raccogli()
    per_codice = {v["codice"]: v for v in crs if v["codice"] is not None}
    datum_per_codice = {d["codice"]: d for d in datum}
    righe = ["crs,lon,lat,x,y"]
    for codice, (funzione, punti) in CASI.items():
        voce = per_codice[codice]
        parametri = {k: mpf(v) for k, v in voce["metodo"][1]}
        a, e2, e = ellissoide(datum_per_codice[voce["datum"]])
        for lon, lat in punti:
            x, y = funzione(parametri, a, e2, e, lon, lat)
            righe.append(f"EPSG:{codice},{lon!r},{lat!r},{float(x)!r},{float(y)!r}")
    DESTINAZIONE.write_text("\n".join(righe) + "\n", encoding="utf-8", newline="\n")
    print(f"scritto {DESTINAZIONE.relative_to(RADICE)}: {len(righe) - 1} righe")
    return 0


if __name__ == "__main__":
    sys.exit(main())
