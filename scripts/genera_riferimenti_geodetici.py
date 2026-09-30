#!/usr/bin/env python3
"""Riferimenti delle misure geodetiche su ogni ellissoide della tabella.

Scrive `crates/plenora-kernels-geo/tests/fixtures/geodetica/`:

- `distanze.csv`: per ogni CRS geografico della tabella integrata, coppie di
  punti deterministiche con la distanza geodetica, l'azimut iniziale (o
  `indefinito` dove la geodetica piu' breve non e' unica o i punti
  coincidono) e la distanza di cerchio massimo sulla sfera del raggio medio
  `R1 = a (1 - f / 3)`;
- `aree.csv`: per ogni CRS geografico, poligoni (con e senza buchi) in WKT
  con l'area geodetica e linee con la lunghezza geodetica;
- `vertici.csv`: lati pseudo-casuali (seme fisso) su WGS 84 e
  Internazionale 1924, molti vicino all'equatore con azimut quasi
  est-ovest, con la latitudine massima in modulo lungo la geodetica e quella
  del vertice. Il vertice viene dalla `GeodesicLine` di GeographicLib
  (`calp0`, `salp0`: `sin(beta0) = |cos(alpha0)|`, `cos(beta0) =
  |sin(alpha0)|`), la scelta se il lato lo contiene dal verso degli azimut
  agli estremi.

I valori sono di GeographicLib 2.0 per Python (Karney, l'implementazione di
riferimento dell'algoritmo), sull'ellissoide che pyproj 3.7.2 / PROJ 9.5.1 /
EPSG v11.022 (l'ambiente vincolato di `genera_crs_integrati.py`) da' al CRS:
lo stesso da cui viene la tabella integrata. Ogni distanza e ogni area si
controllano anche con `pyproj.Geod` (l'implementazione C di PROJ, un secondo
codice indipendente): una divergenza oltre 1e-6 m (1e-3 m^2) ferma il
generatore.

Uso:

    PYTHONPATH=<dir-pyproj> python -B scripts/genera_riferimenti_geodetici.py
    PYTHONPATH=<dir-pyproj> python -B scripts/genera_riferimenti_geodetici.py --verifica

`--verifica` rigenera in memoria e confronta con i file scritti. Le colonne
numeriche sono scritte con `repr`, che rilegge lo stesso f64.

Ambiente: pyproj 3.7.2 (vedi `genera_crs_integrati.py`) piu' geographiclib
2.0, strumento di sviluppo, non dipendenza del workspace:

    python -m pip install --only-binary=:all: --target <dir> geographiclib==2.0
"""

from __future__ import annotations

import argparse
import math
import sys
from pathlib import Path

import geographiclib
from geographiclib.geodesic import Geodesic
from pyproj import CRS, Geod

sys.path.insert(0, str(Path(__file__).resolve().parent))
import genera_crs_integrati as base  # noqa: E402

RADICE = Path(__file__).resolve().parent.parent
CARTELLA = RADICE / "crates" / "plenora-kernels-geo" / "tests" / "fixtures" / "geodetica"
GEOGRAPHICLIB_ATTESO = "2.0"
# Scarto massimo ammesso fra GeographicLib e PROJ nel generatore.
SCARTO_DISTANZA_M = 1e-6
SCARTO_AREA_M2 = 1e-3


class Rifiuto(Exception):
    pass


def r(valore: float) -> str:
    if not math.isfinite(valore):
        raise Rifiuto("valore non finito")
    return repr(float(valore))


def codici_geografici():
    codici = sorted(set(base.ITALIA + base.MONDO))
    geografici = []
    for codice in codici:
        crs = CRS.from_epsg(codice)
        if crs.is_geographic:
            geografici.append((f"EPSG:{codice}", crs))
    geografici.append(("OGC:CRS84", CRS.from_user_input("OGC:CRS84")))
    return geografici


def ellissoide(crs: CRS):
    e = crs.ellipsoid
    a = float(e.semi_major_metre)
    inverso = float(e.inverse_flattening)
    if not (a > 0 and inverso > 1):
        raise Rifiuto(f"ellissoide inatteso: {crs.name}")
    return a, inverso


# Coppie (lon1, lat1, lon2, lat2): corte, medie, lunghe, meridiane,
# equatoriali, sull'antimeridiano, verso un polo, vicine agli antipodi fuori
# dal luogo di taglio e, per l'azimut, sul luogo di taglio e coincidenti.
COPPIE = [
    (11.3426, 44.4949, 10.9252, 44.6471),  # Bologna-Modena
    (12.4964, 41.9028, 9.19, 45.4642),  # Roma-Milano
    (9.0, 45.0, 9.0, 46.0),  # meridiano
    (9.0, 45.0, 10.0, 45.0),  # parallelo
    (0.0, 0.0, 1.0, 0.0),  # equatore
    (0.0, 0.0, 0.0, 1.0),  # meridiano all'equatore
    (-74.006, 40.7128, -0.1278, 51.5074),  # New York-Londra
    (151.2093, -33.8688, -70.6483, -33.4489),  # Sydney-Santiago
    (179.5, 10.0, -179.5, 10.5),  # antimeridiano
    (10.0, 89.0, -170.0, 89.0),  # oltre il polo
    (12.0, 60.0, 12.0, 90.0),  # verso il polo nord
    (5.0, -20.0, 20.0, -90.0),  # verso il polo sud
    (0.0, 0.0, 170.0, 0.0),  # equatore, lontano
    (30.0, 0.0, -151.0, -0.5),  # quasi antipodi
    (0.0, 10.0, 179.0, -10.5),  # quasi antipodi, latitudini non opposte
    (0.0, 30.0, 179.0, -30.0),  # latitudini opposte, geodetica unica
    (10.0, 20.0, 30.0, -20.0),  # latitudini opposte, simmetrica
    (0.0, 0.0, 179.9, 0.0),  # luogo di taglio sull'equatore
    (0.0, 30.0, 180.0, -30.0),  # antipodi
    (0.0, 30.0, 179.95, -30.0),  # luogo di taglio fuori dall'equatore
    (7.0, 44.0, 7.0, 44.0),  # coincidenti
    (-180.0, 12.0, 180.0, 12.0),  # coincidenti (stesso meridiano)
    (13.123456789, 42.987654321, 13.123456799, 42.987654331),  # millimetri
]

# Poligoni e linee in WKT, lon/lat.
POLIGONI = [
    "POLYGON((9 45,9.5 45,9.5 45.5,9 45.5,9 45))",
    "POLYGON((0.00388383 51.501574,0.00538587 51.502278,0.00553607 51.503299,"
    "0.00467777 51.504181,0.00327229 51.504435,0.00187754 51.504168,"
    "0.0008797 51.50338,0.00107288 51.502324,0.00185608 51.50177,0.00388383 51.501574))",
    "POLYGON((10 40,14 40,14 44,10 44,10 40),(11 41,11 42,12 42,12 41,11 41))",
    "POLYGON((-10 -10,0 -10,0 0,-10 0,-10 -10))",
    "POLYGON((160 -5,179.5 -5,179.5 5,160 5,160 -5))",
]
LINEE = [
    "LINESTRING(9 45,10 45.5,11 45)",
    "LINESTRING(0 0,90 0,180 0)",
    "LINESTRING(-74.006 40.7128,-0.1278 51.5074,12.4964 41.9028)",
]


def anelli(wkt: str):
    """Gli anelli di un POLYGON in WKT: liste di (lon, lat)."""
    corpo = wkt[len("POLYGON((") : -2]
    return [
        [tuple(float(v) for v in punto.split()) for punto in anello.split(",")]
        for anello in corpo.split("),(")
    ]


def vertici(wkt: str):
    corpo = wkt[len("LINESTRING(") : -1]
    return [tuple(float(v) for v in punto.split()) for punto in corpo.split(",")]


def azimut_o_indefinito(g: Geodesic, lon1, lat1, lon2, lat2):
    risultato = g.Inverse(lat1, lon1, lat2, lon2)
    s12, azi1, azi2 = risultato["s12"], risultato["azi1"], risultato["azi2"]
    if abs(lat1) == 90.0 or s12 == 0.0:
        return "indefinito"
    if lat2 == -lat1:
        scarto = (azi1 - azi2) % 360.0
        scarto = min(scarto, 360.0 - scarto)
        if math.radians(scarto) * s12 > 0.01:
            return "indefinito"
    return r((azi1 + 360.0) % 360.0)


def area_anello(g: Geodesic, anello, orario: bool):
    """L'area con segno di un anello, orientato come chiede il kernel
    (esterno antiorario, buchi orari)."""
    poligono = g.Polygon()
    punti = anello[:-1] if anello[0] == anello[-1] else anello
    # Verso nel piano lon/lat (formula del laccio).
    doppia = sum(
        punti[i][0] * punti[(i + 1) % len(punti)][1] - punti[(i + 1) % len(punti)][0] * punti[i][1]
        for i in range(len(punti))
    )
    antiorario = doppia > 0
    if antiorario == orario:
        punti = list(reversed(punti))
    for lon, lat in punti:
        poligono.AddPoint(lat, lon)
    _, _, area = poligono.Compute(reverse=False, sign=True)
    return area


def latitudini_massime():
    import random

    righe = ["crs,lon1,lat1,lon2,lat2,latitudine_massima,vertice"]
    caso = random.Random(20260930)
    for nome in ("EPSG:4326", "EPSG:4230"):
        crs = CRS.from_user_input(nome)
        a, inverso = ellissoide(crs)
        f = 1.0 / inverso
        g = Geodesic(a, f)
        lati = [(-85.0, 4e-8, 85.0, 4e-8)]
        for _ in range(150):
            lat = caso.choice([1e-8, 1e-7, 1e-6, 1e-4]) * caso.uniform(-5, 5)
            lon1 = caso.uniform(-90, 0)
            lati.append((lon1, lat, lon1 + caso.uniform(1, 179), lat + caso.uniform(-1e-6, 1e-6)))
        for _ in range(150):
            lon1, lat1 = caso.uniform(-180, 180), caso.uniform(-85, 85)
            lon2 = lon1 + caso.uniform(-179, 179)
            lon2 = (lon2 + 180) % 360 - 180
            lati.append((lon1, lat1, lon2, caso.uniform(-85, 85)))
        for lon1, lat1, lon2, lat2 in lati:
            linea = g.InverseLine(lat1, lon1, lat2, lon2)
            risultato = g.Inverse(lat1, lon1, lat2, lon2)
            vertice = math.degrees(math.atan2(abs(linea._calp0), (1 - f) * abs(linea._salp0)))
            c1 = math.cos(math.radians(risultato["azi1"]))
            c2 = math.cos(math.radians(risultato["azi2"]))
            massima = max(abs(lat1), abs(lat2))
            if c1 * c2 < 0:
                massima = max(massima, vertice)
            righe.append(
                f"{nome},{r(lon1)},{r(lat1)},{r(lon2)},{r(lat2)},{r(massima)},{r(vertice)}"
            )
    return "\n".join(righe) + "\n"


def genera():
    righe_distanze = ["crs,lon1,lat1,lon2,lat2,geodetica_m,azimut_gradi,sfera_m"]
    righe_aree = ["crs,tipo,wkt,valore"]
    for nome, crs in codici_geografici():
        a, inverso = ellissoide(crs)
        f = 1.0 / inverso
        g = Geodesic(a, f)
        sfera = Geodesic(a * (1.0 - f / 3.0), 0.0)
        proj = Geod(a=a, rf=inverso)
        for lon1, lat1, lon2, lat2 in COPPIE:
            s12 = g.Inverse(lat1, lon1, lat2, lon2)["s12"]
            _, _, s12_proj = proj.inv(lon1, lat1, lon2, lat2)
            if abs(s12 - s12_proj) > SCARTO_DISTANZA_M:
                raise Rifiuto(f"GeographicLib e PROJ divergono su {nome}")
            s_sfera = sfera.Inverse(lat1, lon1, lat2, lon2)["s12"]
            azimut = azimut_o_indefinito(g, lon1, lat1, lon2, lat2)
            righe_distanze.append(
                f"{nome},{r(lon1)},{r(lat1)},{r(lon2)},{r(lat2)},{r(s12)},{azimut},{r(s_sfera)}"
            )
        for wkt in POLIGONI:
            parti = anelli(wkt)
            esterno = area_anello(g, parti[0], orario=False)
            buchi = sum(abs(area_anello(g, anello, orario=True)) for anello in parti[1:])
            area = esterno - buchi
            lon = [[p[0] for p in anello] for anello in parti]
            lat = [[p[1] for p in anello] for anello in parti]
            area_proj = abs(proj.polygon_area_perimeter(lon[0], lat[0])[0]) - sum(
                abs(proj.polygon_area_perimeter(lo, la)[0]) for lo, la in zip(lon[1:], lat[1:])
            )
            if abs(area - area_proj) > SCARTO_AREA_M2 * max(1.0, area / 1e10):
                raise Rifiuto(f"GeographicLib e PROJ divergono sull'area su {nome}")
            righe_aree.append(f'{nome},area,"{wkt}",{r(area)}')
        for wkt in LINEE:
            punti = vertici(wkt)
            lunghezza = 0.0
            for (lo1, la1), (lo2, la2) in zip(punti, punti[1:]):
                lunghezza += g.Inverse(la1, lo1, la2, lo2)["s12"]
            lunghezza_proj = proj.line_length([p[0] for p in punti], [p[1] for p in punti])
            if abs(lunghezza - lunghezza_proj) > SCARTO_DISTANZA_M * len(punti):
                raise Rifiuto(f"GeographicLib e PROJ divergono sulla lunghezza su {nome}")
            righe_aree.append(f'{nome},lunghezza,"{wkt}",{r(lunghezza)}')
    return {
        "vertici.csv": latitudini_massime(),
        "distanze.csv": "\n".join(righe_distanze) + "\n",
        "aree.csv": "\n".join(righe_aree) + "\n",
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--verifica", action="store_true", help="confronta senza scrivere")
    argomenti = parser.parse_args()
    try:
        base.verifica_ambiente()
        if geographiclib.__version__ != GEOGRAPHICLIB_ATTESO:
            raise Rifiuto(
                f"geographiclib {geographiclib.__version__}, atteso {GEOGRAPHICLIB_ATTESO}"
            )
        file = genera()
    except Rifiuto as errore:
        print(f"rifiutato: {errore}", file=sys.stderr)
        return 1
    if argomenti.verifica:
        diversi = [
            nome
            for nome, testo in file.items()
            if not (CARTELLA / nome).exists()
            or (CARTELLA / nome).read_text(encoding="utf-8") != testo
        ]
        if diversi:
            print(f"riferimenti non aggiornati: {', '.join(diversi)}", file=sys.stderr)
            return 1
        print("riferimenti geodetici aggiornati")
        return 0
    CARTELLA.mkdir(parents=True, exist_ok=True)
    for nome, testo in file.items():
        (CARTELLA / nome).write_text(testo, encoding="utf-8", newline="\n")
        print(f"scritto {(CARTELLA / nome).relative_to(RADICE)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
