#!/usr/bin/env python3
"""Genera la tabella dei CRS integrati di plenora-core.

Scrive `crates/plenora-core/src/crs/epsg_integrati.rs` dal database EPSG
distribuito con PROJ, letto tramite `pyproj`. `pyproj` e' uno strumento di
sviluppo: non e' una dipendenza del workspace e il codice Rust non lo usa.

Uso:

    python -m pip install --target <dir> pyproj==3.7.2
    PYTHONPATH=<dir> python scripts/genera_crs_integrati.py
    cargo fmt

Per aggiungere un codice: aggiungerlo a una delle liste qui sotto, rigenerare,
formattare e rieseguire i test di `plenora-core` (che verificano le invarianti
della tabella). Il generatore rifiuta con un errore esplicito ogni CRS che non
sa descrivere senza ambiguita': deprecato, non bidimensionale, meridiano
fondamentale diverso da Greenwich, unita' diverse da grado/metro, assi
diversi da nord/est, area d'uso assente, metodo di proiezione senza regola di
dominio.

Due insiemi di limiti per CRS:

- area d'uso EPSG, stretta: il riquadro lon/lat del registro e, per i
  proiettati, il suo inviluppo in coordinate proiettate. E' un metadato;
- dominio di validita', largo: il controllo che rifiuta le coordinate. Per i
  geografici e' il mondo; per i proiettati dipende dal metodo (vedi
  `dominio_di_validita`).
"""

from __future__ import annotations

import math
import sys
from pathlib import Path

import pyproj
from pyproj import CRS, Transformer

RADICE = Path(__file__).resolve().parent.parent
DESTINAZIONE = RADICE / "crates" / "plenora-core" / "src" / "crs" / "epsg_integrati.rs"

# CRS italiani (oltre a quelli mondiali e ai fusi UTM generati sotto).
ITALIA = [
    4265,  # Monte Mario
    3003,  # Monte Mario / Italy zone 1 (Gauss-Boaga fuso Ovest)
    3004,  # Monte Mario / Italy zone 2 (Gauss-Boaga fuso Est)
    4670,  # IGM95
    3064,  # IGM95 / UTM zone 32N
    3065,  # IGM95 / UTM zone 33N
    6706,  # RDN2008
    6707,  # RDN2008 / UTM zone 32N (N-E)
    6708,  # RDN2008 / UTM zone 33N (N-E)
    6709,  # RDN2008 / UTM zone 34N (N-E)
    7791,  # RDN2008 / UTM zone 32N
    7792,  # RDN2008 / UTM zone 33N
    7793,  # RDN2008 / UTM zone 34N
    6875,  # RDN2008 / Italy zone (N-E)
    7794,  # RDN2008 / Italy zone (E-N)
    4230,  # ED50
    23032,  # ED50 / UTM zone 32N
    23033,  # ED50 / UTM zone 33N
    23034,  # ED50 / UTM zone 34N
]

# CRS piu' usati nel mondo.
MONDO = [
    4326,  # WGS 84
    3857,  # WGS 84 / Pseudo-Mercator
    3395,  # WGS 84 / World Mercator
    4258,  # ETRS89
    3035,  # ETRS89-extended / LAEA Europe
    4269,  # NAD83
    4267,  # NAD27
    4283,  # GDA94
    7844,  # GDA2020
    4171,  # RGF93 v1
    2154,  # RGF93 v1 / Lambert-93
    4277,  # OSGB36
    27700,  # OSGB36 / British National Grid
    4674,  # SIRGAS 2000
    4490,  # China Geodetic Coordinate System 2000
    2056,  # CH1903+ / LV95
    31467,  # DHDN / 3-degree Gauss-Kruger zone 3
    28992,  # Amersfoort / RD New
    2193,  # NZGD2000 / New Zealand Transverse Mercator 2000
]

# Fusi UTM: WGS 84 nord e sud, ETRS89 28N..37N. 25838 (ETRS89 / UTM zone 38N)
# e' deprecato nel registro e resta fuori.
UTM_WGS84_NORD = list(range(32601, 32661))
UTM_WGS84_SUD = list(range(32701, 32761))
UTM_ETRS89 = list(range(25828, 25838))

# Semiampiezza in longitudine del dominio dei CRS Transverse Mercator: 15
# gradi dal meridiano centrale, cioe' al piu' circa 1670 km all'equatore.
# L'algoritmo di Krueger/Karney usato da PROJ per `tmerc` resta accurato a
# pochi nanometri entro 3900 km dal meridiano centrale (Karney 2011,
# "Transverse Mercator with an accuracy of a few nanometers", J. Geodesy
# 85:475-485): il dominio e' largo ma la matematica vi resta solida.
SEMIAMPIEZZA_TM_GRADI = 15.0
# Limiti di latitudine del sistema UTM.
UTM_LAT_NORD = 84.0
UTM_LAT_SUD = -80.0
# Latitudine limite dei Mercator (area d'uso EPSG di 3857).
MERCATOR_LAT = 85.06
# Allargamento per lato, in frazione dell'estensione, dei domini che derivano
# dall'area d'uso (reticoli nazionali non TM; latitudini dei TM nazionali).
ALLARGAMENTO = 0.5

# Campioni per lato dell'inviluppo proiettato; il controllo di stabilita'
# confronta con il doppio dei campioni.
CAMPIONI = 400
TOLLERANZA_STABILITA_M = 1e-4


# Ambiente che ha prodotto il file committato. pyproj==3.7.2 da solo non lo
# fissa: la ruota binaria win_amd64 per CPython 3.11 porta PROJ 9.5.1 con il
# registro EPSG v11.022, ma altre ruote o una build contro un PROJ di sistema
# possono portare PROJ 9.8.1 / EPSG v12.029, con limiti diversi (per esempio
# 2056 e 23032). Il generatore rifiuta di girare fuori da questa terna:
# cambiarla e' una decisione da prendere in PR, con il diff dei dati.
PYPROJ_ATTESO = "3.7.2"
PROJ_ATTESO = "9.5.1"
EPSG_ATTESO = "v11.022"


class Rifiuto(Exception):
    """CRS che il generatore non sa descrivere senza ambiguita'."""


def verifica_ambiente() -> None:
    """Rifiuta un ambiente diverso da quello che ha prodotto la tabella."""
    trovato = (
        pyproj.__version__,
        pyproj.proj_version_str,
        pyproj.database.get_database_metadata("EPSG.VERSION"),
    )
    atteso = (PYPROJ_ATTESO, PROJ_ATTESO, EPSG_ATTESO)
    if trovato != atteso:
        raise Rifiuto(
            "ambiente diverso da quello della tabella committata: "
            f"pyproj/PROJ/EPSG trovati {trovato}, attesi {atteso}. "
            "Installare la ruota binaria pyproj 3.7.2 (vedi docs/crs.md, "
            "'Aggiungere un codice') o aggiornare le costanti in PR."
        )


def normalizza_lon(lon: float) -> float:
    if -180.0 <= lon <= 180.0:
        return lon
    return ((lon + 180.0) % 360.0) - 180.0


def inviluppo(crs: CRS, ovest: float, sud: float, est: float, nord: float, campioni: int):
    """Inviluppo proiettato (easting, northing) del riquadro lon/lat.

    Proietta i quattro lati nel CRS geografico di base (solo la conversione,
    nessun cambio di datum): gli estremi di un'applicazione regolare e
    invertibile stanno sul bordo della regione. Ogni lato si campiona, e
    attorno al campione estremo di ciascuna delle quattro coordinate si
    raffina con una ricerca a sezione aurea: un estremo interno a un lato
    (per esempio il nord minimo sul meridiano centrale) non dipende dal
    passo di campionamento.
    """
    if ovest >= est:
        raise Rifiuto(f"riquadro che attraversa l'antimeridiano: {crs.to_epsg()}")
    trasformatore = Transformer.from_crs(crs.geodetic_crs, crs, always_xy=True)

    def punto(lato: int, t: float):
        if lato == 0:
            lon, lat = ovest + (est - ovest) * t, sud
        elif lato == 1:
            lon, lat = ovest + (est - ovest) * t, nord
        elif lato == 2:
            lon, lat = ovest, sud + (nord - sud) * t
        else:
            lon, lat = est, sud + (nord - sud) * t
        x, y = trasformatore.transform(normalizza_lon(lon), lat)
        if not (math.isfinite(x) and math.isfinite(y)):
            raise Rifiuto(f"proiezione non finita sul bordo: {crs.to_epsg()}")
        return x, y

    # (indice della coordinata, segno): minimo come massimo del negato.
    obiettivi = [(0, -1.0), (1, -1.0), (0, 1.0), (1, 1.0)]
    migliori = [-math.inf] * 4
    for lato in range(4):
        valori = [punto(lato, i / campioni) for i in range(campioni + 1)]
        for k, (indice, segno) in enumerate(obiettivi):
            punteggi = [segno * v[indice] for v in valori]
            i = max(range(len(punteggi)), key=punteggi.__getitem__)
            basso = max(0, i - 1) / campioni
            alto = min(campioni, i + 1) / campioni
            f = lambda t: segno * punto(lato, t)[indice]  # noqa: E731
            rapporto = (math.sqrt(5.0) - 1.0) / 2.0
            for _ in range(80):
                c = alto - rapporto * (alto - basso)
                d = basso + rapporto * (alto - basso)
                if f(c) > f(d):
                    alto = d
                else:
                    basso = c
            migliori[k] = max(migliori[k], punteggi[i], f(basso), f(alto))
    return -migliori[0], -migliori[1], migliori[2], migliori[3]


def inviluppo_stabile(crs: CRS, ovest: float, sud: float, est: float, nord: float):
    primo = inviluppo(crs, ovest, sud, est, nord, CAMPIONI)
    secondo = inviluppo(crs, ovest, sud, est, nord, CAMPIONI * 2)
    if any(abs(a - b) > TOLLERANZA_STABILITA_M for a, b in zip(primo, secondo)):
        raise Rifiuto(f"inviluppo instabile al campionamento: {crs.to_epsg()}")
    return secondo


# Guardia prima dell'arrotondamento: la ricerca a sezione aurea valuta solo
# punti reali, quindi l'estremo trovato puo' stare dentro quello vero di una
# quantita' del secondo ordine (sotto il nanometro). Spostarlo di un micrometro
# verso l'esterno prima di arrotondare al millimetro rende il limite
# arrotondato esterno per costruzione, anche quando l'estremo trovato cade
# esattamente su un millimetro (l'equatore degli UTM: northing 0 e 1e7).
GUARDIA_M = 1e-6


def verso_il_basso(valore: float) -> float:
    """Arrotonda al millimetro verso -inf, almeno GUARDIA_M sotto il valore."""
    obiettivo = valore - GUARDIA_M
    arrotondato = math.floor(obiettivo * 1000.0) / 1000.0
    while arrotondato > obiettivo:
        arrotondato = math.nextafter(arrotondato, -math.inf)
    return arrotondato


def verso_l_alto(valore: float) -> float:
    """Arrotonda al millimetro verso +inf, almeno GUARDIA_M sopra il valore."""
    obiettivo = valore + GUARDIA_M
    arrotondato = math.ceil(obiettivo * 1000.0) / 1000.0
    while arrotondato < obiettivo:
        arrotondato = math.nextafter(arrotondato, math.inf)
    return arrotondato


def verso_l_esterno(limiti):
    min_x, min_y, max_x, max_y = limiti
    return (verso_il_basso(min_x), verso_il_basso(min_y), verso_l_alto(max_x), verso_l_alto(max_y))


def parametro(crs: CRS, nome: str) -> float:
    for voce in crs.coordinate_operation.params:
        if voce.name == nome:
            return float(voce.value)
    raise Rifiuto(f"parametro {nome} assente: {crs.to_epsg()}")


def regione_geografica(crs: CRS, area):
    """Regione lon/lat da cui nasce il dominio di un proiettato, o `None`.

    Transverse Mercator: +/- `SEMIAMPIEZZA_TM_GRADI` dal meridiano centrale,
    latitudini dei fusi UTM o dell'area d'uso allargata; Mercator: il mondo
    fino a `MERCATOR_LAT`. Gli altri metodi derivano il dominio dall'area
    d'uso proiettata e non hanno una regione lon/lat (`None`). La usa anche
    `genera_riproiezione.py`, che la riporta per il controllo di
    `geo.reproject` sulle coordinate geografiche.
    """
    metodo = crs.coordinate_operation.method_name
    _, sud, _, nord = area
    if metodo == "Transverse Mercator":
        centrale = parametro(crs, "Longitude of natural origin")
        if " / UTM zone " in crs.name:
            falso_nord = parametro(crs, "False northing")
            if falso_nord == 0.0:
                lat_min, lat_max = 0.0, max(UTM_LAT_NORD, nord)
            elif falso_nord == 10_000_000.0:
                lat_min, lat_max = min(UTM_LAT_SUD, sud), 0.0
            else:
                raise Rifiuto(f"UTM con falso nord inatteso: {crs.to_epsg()}")
        else:
            estensione = nord - sud
            lat_min = max(-90.0, sud - ALLARGAMENTO * estensione)
            lat_max = min(90.0, nord + ALLARGAMENTO * estensione)
        return (
            centrale - SEMIAMPIEZZA_TM_GRADI,
            lat_min,
            centrale + SEMIAMPIEZZA_TM_GRADI,
            lat_max,
        )
    if metodo in ("Popular Visualisation Pseudo Mercator", "Mercator (variant A)"):
        return (-180.0, -MERCATOR_LAT, 180.0, MERCATOR_LAT)
    return None


def dominio_di_validita(crs: CRS, area):
    """Dominio di validita' di un CRS proiettato e la sua regola, in chiaro."""
    metodo = crs.coordinate_operation.method_name
    ovest, sud, est, nord = area
    regione = regione_geografica(crs, area)
    if metodo == "Transverse Mercator":
        centrale = parametro(crs, "Longitude of natural origin")
        lon_min, lat_min, lon_max, lat_max = regione
        regola = "UTM" if " / UTM zone " in crs.name else "TM nazionale"
        limiti = inviluppo_stabile(crs, lon_min, lat_min, lon_max, lat_max)
        descrizione = (
            f"{regola}: longitudine {breve(centrale)} +/- {breve(SEMIAMPIEZZA_TM_GRADI)}, "
            f"latitudine [{breve(lat_min)}, {breve(lat_max)}]"
        )
        return verso_l_esterno(limiti), descrizione
    if metodo in ("Popular Visualisation Pseudo Mercator", "Mercator (variant A)"):
        limiti = inviluppo_stabile(crs, *regione)
        descrizione = f"Mercator: longitudine [-180, 180], latitudine [-{MERCATOR_LAT}, {MERCATOR_LAT}]"
        return verso_l_esterno(limiti), descrizione
    if metodo in (
        "Lambert Conic Conformal (2SP)",
        "Lambert Azimuthal Equal Area",
        "Hotine Oblique Mercator (variant B)",
        "Oblique Stereographic",
    ):
        min_x, min_y, max_x, max_y = inviluppo_stabile(crs, ovest, sud, est, nord)
        dx = (max_x - min_x) * ALLARGAMENTO
        dy = (max_y - min_y) * ALLARGAMENTO
        limiti = (min_x - dx, min_y - dy, max_x + dx, max_y + dy)
        descrizione = (
            f"{metodo}: inviluppo dell'area d'uso EPSG allargato del "
            f"{int(ALLARGAMENTO * 100)}% dell'estensione per lato"
        )
        return verso_l_esterno(limiti), descrizione
    raise Rifiuto(f"metodo senza regola di dominio: {metodo} ({crs.to_epsg()})")


def breve(valore: float) -> str:
    """Numero leggibile per i commenti (non per i dati)."""
    return f"{valore:.6f}".rstrip("0").rstrip(".")


def fmt(valore: float) -> str:
    """Letterale f64 Rust: la forma piu' corta che rilegge lo stesso valore."""
    testo = repr(float(valore))
    if "e" in testo or "E" in testo:
        raise Rifiuto(f"letterale in notazione esponenziale: {testo}")
    if "." not in testo:
        testo += ".0"
    return testo


def stringa(testo: str) -> str:
    if any(c in testo for c in '"\\') or not testo.isprintable():
        raise Rifiuto(f"nome non rappresentabile come letterale semplice: {testo!r}")
    return f'"{testo}"'


def descrivi(crs: CRS, identificativo: str):
    if crs.is_deprecated:
        raise Rifiuto(f"deprecato: {identificativo}")
    if crs.type_name == "Geographic 2D CRS":
        kind = "Geographic"
    elif crs.type_name == "Projected CRS":
        kind = "Projected"
    else:
        raise Rifiuto(f"tipo non supportato ({crs.type_name}): {identificativo}")
    if crs.prime_meridian is None or crs.prime_meridian.longitude != 0.0:
        raise Rifiuto(f"meridiano fondamentale diverso da Greenwich: {identificativo}")
    assi_json = crs.to_json_dict()["coordinate_system"]["axis"]
    if len(assi_json) != 2 or len(crs.axis_info) != 2:
        raise Rifiuto(f"non bidimensionale: {identificativo}")
    assi = []
    for asse, info in zip(assi_json, crs.axis_info):
        if asse["direction"] not in ("north", "east"):
            raise Rifiuto(f"direzione d'asse non supportata: {identificativo}")
        if kind == "Geographic":
            if info.unit_name != "degree":
                raise Rifiuto(f"unita' angolare non in gradi: {identificativo}")
        elif info.unit_name != "metre" or info.unit_conversion_factor != 1.0:
            raise Rifiuto(f"unita' lineare non in metri: {identificativo}")
        assi.append((asse["name"], asse["abbreviation"], asse["direction"]))
    if {a[2] for a in assi} != {"north", "east"}:
        raise Rifiuto(f"assi non ortogonali nord/est: {identificativo}")
    area = crs.area_of_use
    if area is None:
        raise Rifiuto(f"area d'uso assente: {identificativo}")
    riquadro = tuple(float(v) for v in area.bounds)
    ellissoide = crs.ellipsoid
    voce = {
        "id": identificativo,
        "nome": crs.name,
        "kind": kind,
        "assi": tuple(assi),
        "ellissoide": (ellissoide.name, ellissoide.semi_major_metre, ellissoide.inverse_flattening),
        "area": riquadro,
        "area_proiettata": None,
        "dominio": None,
        "regola": "geografico: longitudine [-180, 180], latitudine [-90, 90]",
    }
    if kind == "Projected":
        voce["area_proiettata"] = verso_l_esterno(inviluppo_stabile(crs, *riquadro))
        voce["dominio"], voce["regola"] = dominio_di_validita(crs, riquadro)
    return voce


def verifica_fuso_utm(voce, codice: int, base: int, emisfero: str, prefisso: str):
    """I fusi UTM WGS 84 seguono la regola programmatica del registro."""
    fuso = codice - base
    atteso = f"{prefisso} / UTM zone {fuso}{emisfero}"
    if voce["nome"] != atteso:
        raise Rifiuto(f"nome inatteso per {codice}: {voce['nome']}")
    ovest = -180.0 + 6.0 * (fuso - 1)
    sud, nord = (0.0, 84.0) if emisfero == "N" else (-80.0, 0.0)
    regola = (ovest, sud, ovest + 6.0, nord)
    # Il registro ha scarti di un centesimo di grado su qualche fuso (32629:
    # -12.01 e 84.01): si conservano i valori del registro, e uno scarto piu'
    # grande e' un rifiuto.
    if any(abs(a - b) > 0.01 + 1e-9 for a, b in zip(voce["area"], regola)):
        raise Rifiuto(f"area d'uso fuori dalla regola dei fusi: {codice}")
    if voce["area"] != regola:
        print(f"nota: {voce['id']} ha l'area del registro {voce['area']}, regola {regola}")


def nome_costante(prefisso: str, testo: str) -> str:
    pulito = "".join(c if c.isalnum() else "_" for c in testo.upper())
    while "__" in pulito:
        pulito = pulito.replace("__", "_")
    return f"{prefisso}_{pulito.strip('_')}"


def genera() -> str:
    voci = []
    for codice in sorted(set(ITALIA + MONDO + UTM_WGS84_NORD + UTM_WGS84_SUD + UTM_ETRS89)):
        voce = descrivi(CRS.from_epsg(codice), f"EPSG:{codice}")
        if codice in UTM_WGS84_NORD:
            verifica_fuso_utm(voce, codice, 32600, "N", "WGS 84")
        if codice in UTM_WGS84_SUD:
            verifica_fuso_utm(voce, codice, 32700, "S", "WGS 84")
        voce["codice"] = codice
        voci.append(voce)
    crs84 = descrivi(CRS("OGC:CRS84"), "OGC:CRS84")
    crs84["codice"] = None

    ellissoidi = {}
    assi = {}
    for voce in [crs84] + voci:
        ellissoidi.setdefault(voce["ellissoide"], nome_costante("ELLISSOIDE", voce["ellissoide"][0]))
        if voce["assi"] not in assi:
            nome = nome_costante("ASSI", "_".join(f"{a[1]}_{a[2]}" for a in voce["assi"]))
            if nome in assi.values():
                nome = f"{nome}_{len(assi)}"
            assi[voce["assi"]] = nome
    if len(set(ellissoidi.values())) != len(ellissoidi):
        raise Rifiuto("due ellissoidi con lo stesso nome e parametri diversi")

    versione = pyproj.database.get_database_metadata("EPSG.VERSION")
    data = pyproj.database.get_database_metadata("EPSG.DATE")
    righe = [
        "//! Tabella dei CRS integrati.",
        "//!",
        "//! GENERATA da `scripts/genera_crs_integrati.py`: non si modifica a mano.",
        f"//! Fonte: registro EPSG {versione} ({data}), come distribuito con PROJ",
        f"//! {pyproj.proj_version_str} e letto con pyproj {pyproj.__version__}.",
        "//!",
        "//! Per ogni CRS: nome, tipo, assi dell'autorita', ellissoide, area d'uso",
        "//! EPSG (riquadro lon/lat del registro e, per i proiettati, il suo",
        "//! inviluppo proiettato arrotondato al millimetro verso l'esterno) e",
        "//! dominio di validita' dei proiettati, con la regola che lo ha prodotto.",
        "",
        "// Dati generati: i letterali restano nella forma piu' corta che rilegge",
        "// lo stesso f64, senza separatori.",
        "#![allow(clippy::unreadable_literal)]",
        "",
        "use super::integrati::{",
        "    geografico, proiettato, Asse, CrsIntegrato, Direzione, Identificativo,",
        "};",
        "use super::{Ellipsoid, GeographicBounds, ProjectedBounds};",
        "",
        f'pub(super) const VERSIONE_EPSG: &str = "{versione}";',
        f'pub(super) const DATA_EPSG: &str = "{data}";',
        f'pub(super) const VERSIONE_PROJ: &str = "{pyproj.proj_version_str}";',
        "",
    ]
    for (nome, semiasse, inverso), costante in ellissoidi.items():
        righe += [
            f"/// {nome}",
            f"const {costante}: Ellipsoid = Ellipsoid {{",
            f"    semi_major_axis_metre: {fmt(semiasse)},",
            f"    inverse_flattening: {fmt(inverso)},",
            "};",
        ]
    for tupla, costante in assi.items():
        righe.append(f"const {costante}: [Asse; 2] = [")
        for nome, abbreviazione, direzione in tupla:
            righe.append(
                f"    Asse {{ nome: {stringa(nome)}, abbreviazione: {stringa(abbreviazione)}, "
                f"direzione: Direzione::{direzione.capitalize()} }},"
            )
        righe.append("];")
    righe.append("")

    def espressione(voce, identificativo: str):
        ovest, sud, est, nord = voce["area"]
        area = f"GeographicBounds::new({fmt(ovest)}, {fmt(sud)}, {fmt(est)}, {fmt(nord)})"
        testa = (
            f"{identificativo}, {stringa(voce['nome'])}, {assi[voce['assi']]}, "
            f"{ellissoidi[voce['ellissoide']]}, {area}"
        )
        commento = f"// {voce['id']}. Dominio: {voce['regola']}."
        if voce["kind"] == "Geographic":
            return commento, f"geografico({testa})"
        proiettata = "ProjectedBounds::new({}, {}, {}, {})".format(
            *(fmt(v) for v in voce["area_proiettata"])
        )
        dominio = "ProjectedBounds::new({}, {}, {}, {})".format(*(fmt(v) for v in voce["dominio"]))
        return commento, f"proiettato({testa}, {proiettata}, {dominio})"

    commento, valore = espressione(crs84, "Identificativo::OgcCrs84")
    righe.append("/// OGC:CRS84: WGS 84 con longitudine prima della latitudine.")
    righe.append(commento)
    righe.append(f"pub(super) const CRS84: CrsIntegrato = {valore};")
    righe.append("")
    righe.append("/// CRS EPSG integrati, in ordine crescente di codice.")
    righe.append("pub(super) const EPSG: &[CrsIntegrato] = &[")
    for voce in voci:
        commento, valore = espressione(voce, f"Identificativo::Epsg({voce['codice']})")
        righe += [f"    {commento}", f"    {valore},"]
    righe.append("];")
    righe.append("")
    return "\n".join(righe)


def main() -> int:
    try:
        verifica_ambiente()
        testo = genera()
    except Rifiuto as errore:
        print(f"rifiutato: {errore}", file=sys.stderr)
        return 1
    DESTINAZIONE.parent.mkdir(parents=True, exist_ok=True)
    DESTINAZIONE.write_text(testo, encoding="utf-8", newline="\n")
    print(f"scritto {DESTINAZIONE.relative_to(RADICE)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
