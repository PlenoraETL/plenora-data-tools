#!/usr/bin/env python3
"""Genera le fixture dell'oracolo di `geo.reproject` contro PROJ.

Scrive in `crates/plenora-core/tests/fixtures/riproiezione/` i risultati di
PROJ 9.5.1 (pyproj 3.7.2, registro EPSG v11.022: l'ambiente vincolato di
`genera_crs_integrati.py`) su punti campione deterministici. Ogni confronto
obbliga PROJ alla STESSA operazione EPSG del codice Rust, senza griglie di
rete e senza la scelta automatica dell'operazione: la differenza misura la
matematica, non la scelta.

Uso:

    PYTHONPATH=<dir> python -B scripts/genera_oracolo_riproiezione.py

File:

- `proiezioni.csv`: per ogni CRS proiettato della tabella, lon/lat nel
  datum del CRS e le coordinate proiettate di PROJ (sola conversione:
  `Transformer.from_crs(crs.geodetic_crs, crs)`);
- `trasformazioni.csv`: per ogni trasformazione senza griglia della tabella
  generata, punti nella sua area d'uso, in avanti (`from_pipeline` della
  stessa operazione EPSG) e all'indietro (verso inverso della stessa
  pipeline, come PROJ la usa per «Inverse of ...»);
- `catene.csv`: CRS -> CRS con il percorso di trasformazioni indicato per
  riga: ogni CRS della tabella da e verso WGS 84 geografico (EPSG:4326) e le
  coppie rappresentative; PROJ compone conversione inversa, passi e
  conversione diretta;
- `griglia_sintetica.gsb` e `griglia.csv`: una griglia NTv2 sintetica a due
  livelli (una radice e un figlio) e il suo `hgridshift` di PROJ, avanti e
  indietro; `catene.csv` la usa come file di EPSG:9734 (Monte Mario to
  RDN2008 (5)).

Le colonne numeriche sono scritte con `repr`, che rilegge lo stesso f64.
"""

from __future__ import annotations

import math
import struct
import sys
from pathlib import Path

import pyproj
from pyproj import CRS, Transformer
from pyproj.crs import CoordinateOperation

sys.path.insert(0, str(Path(__file__).resolve().parent))
import genera_crs_integrati as base  # noqa: E402
import genera_riproiezione as riproiezione  # noqa: E402

RADICE = Path(__file__).resolve().parent.parent
CARTELLA = RADICE / "crates" / "plenora-core" / "tests" / "fixtures" / "riproiezione"

# Nessuna griglia di rete: PROJ non deve scaricare ne' cercare altro.
pyproj.network.set_network_enabled(False)


class Rifiuto(Exception):
    pass


def r(valore: float) -> str:
    if not math.isfinite(valore):
        raise Rifiuto("valore non finito")
    return repr(float(valore))


def codici_integrati():
    return sorted(set(base.ITALIA + base.MONDO + base.UTM_WGS84_NORD + base.UTM_WGS84_SUD + base.UTM_ETRS89))


def lin(a: float, b: float, n: int):
    return [a + (b - a) * (i + 0.5) / n for i in range(n)]


# ---------------------------------------------------------------------------
# Proiezioni.
# ---------------------------------------------------------------------------


def campioni_proiezione(crs: CRS):
    area = tuple(float(v) for v in crs.area_of_use.bounds)
    regione = base.regione_geografica(crs, area)
    metodo = crs.coordinate_operation.method_name
    if metodo == "Transverse Mercator":
        centrale = base.parametro(crs, "Longitude of natural origin")
        _, lat_min, _, lat_max = regione
        # Dal meridiano centrale fino al bordo della regione (15 gradi).
        dl = [-14.5, -7.0, -1.3, 0.0, 3.1, 14.5]
        lat = lin(lat_min, lat_max, 4)
        return [(centrale + d, p) for d in dl for p in lat]
    if metodo in ("Popular Visualisation Pseudo Mercator", "Mercator (variant A)"):
        return [(lo, la) for lo in (-179.5, -120.0, -45.0, 0.0, 33.3, 179.5) for la in (-85.0, -60.0, -10.0, 0.0, 25.0, 71.0, 85.0)]
    ovest, sud, est, nord = area
    return [(lo, la) for lo in lin(ovest, est, 6) for la in lin(sud, nord, 6)]


def proiezioni():
    righe = ["crs,lon,lat,x,y"]
    for codice in codici_integrati():
        crs = CRS.from_epsg(codice)
        if not crs.is_projected:
            continue
        conversione = Transformer.from_crs(crs.geodetic_crs, crs, always_xy=True)
        for lon, lat in campioni_proiezione(crs):
            x, y = conversione.transform(lon, lat, errcheck=True)
            righe.append(f"EPSG:{codice},{r(lon)},{r(lat)},{r(x)},{r(y)}")
    return righe


# ---------------------------------------------------------------------------
# Trasformazioni fra datum.
# ---------------------------------------------------------------------------


def trasformazioni_della_tabella():
    """Le trasformazioni della tabella generata, dallo stesso raccoglitore."""
    _, _, voci, _ = riproiezione.raccogli()
    return [
        {
            "codice": v["codice"],
            "da": v["da"],
            "a": v["a"],
            "accuratezza": v["accuratezza"],
            "metodo": v["metodo"][0],
            "area": v["area"],
        }
        for v in voci
    ]


class Operazione:
    """Una pipeline PROJ di un'operazione EPSG, con l'ordine degli assi."""

    def __init__(self, codice: int):
        self.codice = codice
        testo = CoordinateOperation.from_epsg(codice).to_proj4()
        self.nulla = testo == "+proj=noop"
        self.scambia = testo.startswith("+proj=pipeline +step +proj=axisswap +order=2,1")
        if not self.nulla and not self.scambia:
            raise Rifiuto(f"pipeline senza scambio d'assi: EPSG:{codice}")
        self.trasformatore = None if self.nulla else Transformer.from_pipeline(testo)

    def applica(self, lon: float, lat: float, inversa: bool):
        if self.nulla:
            return lon, lat
        direzione = "INVERSE" if inversa else "FORWARD"
        la, lo = self.trasformatore.transform(lat, lon, direction=direzione, errcheck=True)
        return lo, la


def punti_in_area(area, n: int = 3):
    ovest, sud, est, nord = area
    if est < ovest:
        est += 360.0
    punti = []
    for lo in lin(ovest, est, n):
        for la in lin(sud, nord, n):
            punti.append((lo - 360.0 if lo > 180.0 else lo, la))
    return punti


def trasformazioni(voci):
    righe = ["codice,inversa,lon,lat,lon_uscita,lat_uscita"]
    for voce in voci:
        if voce["metodo"] == "GrigliaNtv2":
            continue
        operazione = Operazione(voce["codice"])
        for lon, lat in punti_in_area(voce["area"]):
            lo, la = operazione.applica(lon, lat, False)
            righe.append(f"{voce['codice']},0,{r(lon)},{r(lat)},{r(lo)},{r(la)}")
            # All'indietro dal punto d'arrivo: stessa pipeline, verso inverso.
            li, ai = operazione.applica(lo, la, True)
            righe.append(f"{voce['codice']},1,{r(lo)},{r(la)},{r(li)},{r(ai)}")
    return righe


# ---------------------------------------------------------------------------
# Griglia NTv2 sintetica.
# ---------------------------------------------------------------------------


def record(chiave: str, valore: bytes) -> bytes:
    assert len(chiave) == 8 and len(valore) == 8
    return chiave.encode("ascii") + valore


def intero(v: int) -> bytes:
    return struct.pack("<i", v) + b"\0\0\0\0"


def reale(v: float) -> bytes:
    return struct.pack("<d", v)


def testo8(v: str) -> bytes:
    return v.ljust(8).encode("ascii")


def sottogriglia(nome, padre, sud, nord, est, ovest, passo, funzione):
    """Sottogriglia in secondi, longitudini positive a ovest."""
    righe = round((nord - sud) / passo) + 1
    colonne = round((ovest - est) / passo) + 1
    testa = b"".join(
        [
            record("SUB_NAME", testo8(nome)),
            record("PARENT  ", testo8(padre)),
            record("CREATED ", testo8("20260929")),
            record("UPDATED ", testo8("20260929")),
            record("S_LAT   ", reale(sud)),
            record("N_LAT   ", reale(nord)),
            record("E_LONG  ", reale(est)),
            record("W_LONG  ", reale(ovest)),
            record("LAT_INC ", reale(passo)),
            record("LONG_INC", reale(passo)),
            record("GS_COUNT", intero(righe * colonne)),
        ]
    )
    nodi = []
    for j in range(righe):
        lat = (sud + j * passo) / 3600.0
        for i in range(colonne):
            lon = -(est + i * passo) / 3600.0
            dlat, dlon_w = funzione(lon, lat)
            nodi.append(struct.pack("<ffff", dlat, dlon_w, 0.01, 0.01))
    return testa + b"".join(nodi)


def griglia_sintetica() -> bytes:
    # Radice: Italia, lon 6..19 E, lat 35..48 N, passo 30'; figlio: lon
    # 10..12 E, lat 43..45 N, passo 6'. Spostamenti lisci di qualche
    # secondo, diversi nel figlio (la sottogriglia piu' fine deve vincere).
    def radice(lon, lat):
        return (1.2 + 0.3 * math.sin(math.radians(lon * 20)) + 0.05 * lat, -2.5 + 0.2 * math.cos(math.radians(lat * 15)))

    def figlio(lon, lat):
        dlat, dlon = radice(lon, lat)
        return (dlat + 0.15 * math.sin(math.radians(lon * 90)), dlon - 0.1 * math.cos(math.radians(lat * 90)))

    testa = b"".join(
        [
            record("NUM_OREC", intero(11)),
            record("NUM_SREC", intero(11)),
            record("NUM_FILE", intero(2)),
            record("GS_TYPE ", testo8("SECONDS")),
            record("VERSION ", testo8("NTv2.0")),
            record("SYSTEM_F", testo8("PROVA_F")),
            record("SYSTEM_T", testo8("PROVA_T")),
            record("MAJOR_F ", reale(6378388.0)),
            record("MINOR_F ", reale(6356911.946)),
            record("MAJOR_T ", reale(6378137.0)),
            record("MINOR_T ", reale(6356752.314)),
        ]
    )
    corpo = sottogriglia("RADICE", "NONE", 35 * 3600.0, 48 * 3600.0, -19 * 3600.0, -6 * 3600.0, 1800.0, radice)
    corpo += sottogriglia("FIGLIO", "RADICE", 43 * 3600.0, 45 * 3600.0, -12 * 3600.0, -10 * 3600.0, 360.0, figlio)
    return testa + corpo + record("END     ", b"\0" * 8)


def griglia(percorso: Path):
    pipeline = Transformer.from_pipeline(
        "+proj=pipeline +step +proj=unitconvert +xy_in=deg +xy_out=rad "
        f"+step +proj=hgridshift +grids={percorso.as_posix()} "
        "+step +proj=unitconvert +xy_in=rad +xy_out=deg"
    )
    righe = ["inversa,lon,lat,lon_uscita,lat_uscita"]
    punti = [(lo, la) for lo in lin(6.2, 18.8, 7) for la in lin(35.2, 47.8, 7)]
    punti += [(lo, la) for lo in lin(10.05, 11.95, 5) for la in lin(43.05, 44.95, 5)]
    for lon, lat in punti:
        lo, la = pipeline.transform(lon, lat, errcheck=True)
        righe.append(f"0,{r(lon)},{r(lat)},{r(lo)},{r(la)}")
        li, ai = pipeline.transform(lo, la, direction="INVERSE", errcheck=True)
        righe.append(f"1,{r(lo)},{r(la)},{r(li)},{r(ai)}")
    return righe, pipeline


# ---------------------------------------------------------------------------
# Catene CRS -> CRS.
# ---------------------------------------------------------------------------


def datum_di(codice: int) -> int:
    crs = CRS.from_epsg(codice)
    return codice if crs.is_geographic else crs.geodetic_crs.to_epsg(min_confidence=100)


def intersezione(a, b):
    """Intersezione di due riquadri lon/lat, anche oltre l'antimeridiano.

    Il risultato ha `est > ovest`, con l'est eventualmente oltre 180 (il
    chiamante riporta le longitudini in [-180, 180]); None se disgiunti.
    """

    def normale(x):
        ovest, sud, est, nord = x
        return (ovest, sud, est + 360.0 if est < ovest else est, nord)

    pa, pb = normale(a), normale(b)
    for spostamento in (-360.0, 0.0, 360.0):
        ovest = max(pa[0], pb[0] + spostamento)
        est = min(pa[2], pb[2] + spostamento)
        sud, nord = max(pa[1], pb[1]), min(pa[3], pb[3])
        if ovest < est and sud < nord:
            return (ovest, sud, est, nord)
    return None


def area_crs(codice: int):
    crs = CRS.from_epsg(codice)
    area = tuple(float(v) for v in crs.area_of_use.bounds)
    if crs.is_projected:
        regione = base.regione_geografica(crs, area)
        if regione is not None and area[0] > area[2]:
            area = regione
    return area


def scegli_passo_verso_wgs84(datum: int, area, voci):
    """Il passo diretto datum <-> 4326 senza griglia di accuratezza minima
    la cui area interseca quella del CRS; a parita', il codice minore."""
    candidati = []
    for voce in voci:
        if voce["metodo"] == "GrigliaNtv2":
            continue
        if {voce["da"], voce["a"]} != {datum, 4326}:
            continue
        comune = intersezione(voce["area"], area)
        if comune is None:
            continue
        candidati.append((voce["accuratezza"], voce["codice"], voce, comune))
    if not candidati:
        return None
    candidati.sort(key=lambda c: (c[0], c[1]))
    _, _, voce, comune = candidati[0]
    return voce, comune


class Catena:
    def __init__(self, da: int, a: int, passi, file_griglia=None):
        self.da = da
        self.a = a
        self.passi = passi  # [(codice, inversa)]
        sorgente = CRS.from_epsg(da)
        destinazione = CRS.from_epsg(a)
        self.inversa_sorgente = (
            Transformer.from_crs(sorgente, sorgente.geodetic_crs, always_xy=True) if sorgente.is_projected else None
        )
        self.diretta_destinazione = (
            Transformer.from_crs(destinazione.geodetic_crs, destinazione, always_xy=True)
            if destinazione.is_projected
            else None
        )
        self.operazioni = []
        for codice, inversa in passi:
            if file_griglia is not None and codice == 9734:
                self.operazioni.append((file_griglia, inversa))
            else:
                self.operazioni.append((Operazione(codice), inversa))

    def applica(self, x: float, y: float):
        if self.inversa_sorgente is not None:
            x, y = self.inversa_sorgente.transform(x, y, errcheck=True)
        for operazione, inversa in self.operazioni:
            if isinstance(operazione, Operazione):
                x, y = operazione.applica(x, y, inversa)
            else:
                x, y = operazione.transform(x, y, direction="INVERSE" if inversa else "FORWARD", errcheck=True)
        if self.diretta_destinazione is not None:
            x, y = self.diretta_destinazione.transform(x, y, errcheck=True)
        return x, y


def ingresso(codice: int, lon: float, lat: float, datum_catena: int):
    """Coordinate del CRS `codice` per il punto lon/lat nel suo datum."""
    crs = CRS.from_epsg(codice)
    if crs.is_projected:
        return Transformer.from_crs(crs.geodetic_crs, crs, always_xy=True).transform(lon, lat, errcheck=True)
    return lon, lat


def catene(voci, file_griglia):
    righe = ["da,a,percorso,accuratezza,x,y,x_uscita,y_uscita"]

    def scrivi(da, a, passi, accuratezza, punti, griglia=None):
        andata = Catena(da, a, passi, griglia)
        ritorno = Catena(a, da, [(c, not inv) for c, inv in reversed(passi)], griglia)
        percorso = "+".join(f"{c}{'i' if inv else ''}" for c, inv in passi)
        for lon, lat in punti:
            x, y = ingresso(da, lon, lat, None)
            xo, yo = andata.applica(x, y)
            righe.append(f"EPSG:{da},EPSG:{a},{percorso},{r(accuratezza)},{r(x)},{r(y)},{r(xo)},{r(yo)}")
            ritorno_percorso = "+".join(f"{c}{'i' if inv else ''}" for c, inv in reversed([(c, not i) for c, i in passi]))
            xr, yr = ritorno.applica(xo, yo)
            righe.append(f"EPSG:{a},EPSG:{da},{ritorno_percorso},{r(accuratezza)},{r(xo)},{r(yo)},{r(xr)},{r(yr)}")

    saltati = []
    for codice in codici_integrati():
        if codice == 4326:
            continue
        datum = datum_di(codice)
        area = area_crs(codice)
        if datum == 4326:
            passi, accuratezza, comune = [], 0.0, area
        else:
            scelta = scegli_passo_verso_wgs84(datum, area, voci)
            if scelta is None:
                saltati.append(codice)
                continue
            voce, comune = scelta
            passi = [(voce["codice"], voce["da"] != datum)]
            accuratezza = voce["accuratezza"]
        comune = intersezione(comune, comune)
        punti = [
            (((lo + 180.0) % 360.0) - 180.0, la)
            for lo in lin(comune[0], comune[2], 2)
            for la in lin(max(comune[1], -84.0), min(comune[3], 84.0), 2)
        ]
        scrivi(codice, 4326, passi, accuratezza, punti)

    def per_codice(c):
        return next(v for v in voci if v["codice"] == c)

    # Coppie rappresentative: (da, a, passi [(codice, inversa)], riquadro lon/lat).
    coppie = [
        (3003, 7791, [(1659, False), (6710, True)], (7.0, 43.0, 12.0, 46.5)),
        (3003, 6707, [(1659, False), (6710, True)], (7.0, 43.0, 12.0, 46.5)),
        (3004, 7792, [(1659, False), (6710, True)], (12.5, 38.5, 17.5, 42.5)),
        (23032, 25832, [(1626, False)], (8.5, 54.8, 11.8, 57.5)),
        (23032, 4258, [(1626, False)], (8.5, 54.8, 11.8, 57.5)),
        (27700, 4326, [(1314, False)], (-5.0, 50.5, 1.5, 58.0)),
        (27700, 4258, [(1314, False), (1149, True)], (-5.0, 50.5, 1.5, 58.0)),
        (28992, 4258, [(9281, False)], (3.6, 50.9, 7.0, 53.4)),
        (28992, 25831, [(9281, False)], (3.6, 50.9, 5.9, 53.4)),
        (2056, 4258, [(1647, False)], (6.0, 45.9, 10.4, 47.7)),
        (2056, 25832, [(1647, False)], (6.1, 45.9, 10.4, 47.7)),
        (4267, 4269, [(1173, False), (1188, True)], (-120.0, 30.0, -75.0, 47.0)),
        (4283, 7844, [(8048, False)], (115.0, -40.0, 150.0, -12.0)),
        (31467, 25832, [(1777, False), (1149, True)], (7.0, 47.5, 10.5, 54.5)),
        (2154, 3857, [(1671, False)], (-4.0, 43.0, 7.5, 50.5)),
        (3035, 4326, [(1149, False)], (-8.0, 37.0, 25.0, 65.0)),
    ]
    for da, a, passi, riquadro in coppie:
        accuratezza = sum(per_codice(c)["accuratezza"] for c, _ in passi)
        punti = [(lo, la) for lo in lin(riquadro[0], riquadro[2], 3) for la in lin(riquadro[1], riquadro[3], 3)]
        scrivi(da, a, passi, accuratezza, punti)

    # Griglia NTv2 sintetica come EPSG:9734 (Monte Mario -> RDN2008).
    punti = [(lo, la) for lo in lin(7.0, 12.0, 3) for la in lin(43.0, 46.5, 3)] + [(10.9, 43.8), (11.3, 44.4)]
    scrivi(3003, 7791, [(9734, False)], 0.1, punti, file_griglia)
    scrivi(3003, 4326, [(9734, False), (6711, False)], 1.1, punti, file_griglia)
    return righe, saltati


def main() -> int:
    try:
        base.verifica_ambiente()
        voci = trasformazioni_della_tabella()
        CARTELLA.mkdir(parents=True, exist_ok=True)
        percorso_griglia = CARTELLA / "griglia_sintetica.gsb"
        percorso_griglia.write_bytes(griglia_sintetica())
        righe_griglia, pipeline_griglia = griglia(percorso_griglia)
        righe_catene, saltati = catene(voci, pipeline_griglia)
        uscite = {
            "proiezioni.csv": proiezioni(),
            "trasformazioni.csv": trasformazioni(voci),
            "catene.csv": righe_catene,
            "griglia.csv": righe_griglia,
        }
    except (Rifiuto, base.Rifiuto, pyproj.exceptions.ProjError) as errore:
        print(f"rifiutato: {errore}", file=sys.stderr)
        return 1
    for nome, righe in uscite.items():
        (CARTELLA / nome).write_text("\n".join(righe) + "\n", encoding="utf-8", newline="\n")
        print(f"scritto {nome}: {len(righe) - 1} righe")
    if saltati:
        print(f"nota: CRS senza trasformazione diretta verso WGS 84, nessuna catena: {saltati}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
