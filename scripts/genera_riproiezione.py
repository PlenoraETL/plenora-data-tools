#!/usr/bin/env python3
"""Genera i parametri di riproiezione dei CRS integrati di plenora-core.

Scrive `crates/plenora-core/src/crs/riproiezione/epsg.rs` dal registro EPSG
distribuito con PROJ, letto con `pyproj` (conversioni dei CRS) e con il
database `proj.db` della stessa ruota (trasformazioni fra datum). Stesso
ambiente vincolato di `genera_crs_integrati.py` (pyproj 3.7.2, PROJ 9.5.1,
EPSG v11.022): il generatore rifiuta di girare fuori da quella terna.

Uso:

    python -m pip install --only-binary=:all: --target <dir> pyproj==3.7.2
    PYTHONPATH=<dir> python -B scripts/genera_riproiezione.py
    cargo fmt

Che cosa scrive:

- per ogni CRS della tabella integrata: il datum (codice del CRS geografico
  di base), il metodo di proiezione con i parametri EPSG nelle unita' del
  registro (gradi, metri, fattore di scala) e, per Transverse Mercator e
  Mercator, la regione lon/lat da cui nasce il dominio di validita'
  (`genera_crs_integrati.regione_geografica`);
- i datum: nome ed ellissoide;
- le trasformazioni EPSG fra due datum della tabella che non richiedono
  griglie: traslazioni geocentriche (9603), Position Vector (9606),
  Coordinate Frame (9607), con parametri,
  accuratezza EPSG e riquadro dell'area d'uso;
- le trasformazioni EPSG a griglia NTv2 (9615) fra due datum della tabella,
  con accuratezza e nome del file del registro: il file lo fornisce
  l'utente, il generatore non scarica nulla.

Restano fuori, con un errore o una nota esplicita: le trasformazioni
deprecate, quelle dipendenti dal tempo (tassi non nulli: nessuna fra i datum
della tabella a v11.022), quelle senza accuratezza, quelle sostituite
(`supersession`) da un'altra trasformazione inclusa, le operazioni
concatenate del registro (il percorso fra datum lo compone il codice Rust)
e le griglie in formati diversi da NTv2 (NADCON, NADCON5, ...). Il metodo
Molodensky-Badekas (9636) compare fra questi datum solo in trasformazioni
sostituite (Amersfoort to ETRS89 (2), (4), (6)): non e' implementato, e una
trasformazione non sostituita con quel metodo e' un rifiuto.
"""

from __future__ import annotations

import sqlite3
import sys
from pathlib import Path

import pyproj
from pyproj import CRS

sys.path.insert(0, str(Path(__file__).resolve().parent))
import genera_crs_integrati as base  # noqa: E402

RADICE = Path(__file__).resolve().parent.parent
DESTINAZIONE = RADICE / "crates" / "plenora-core" / "src" / "crs" / "riproiezione" / "epsg.rs"

# Metodi di proiezione: codice EPSG -> (variante Rust, [(campo, codice
# parametro EPSG, unita' attesa)]).
PROIEZIONI = {
    9807: (
        "TrasversaDiMercatore",
        [
            ("lat0", 8801, "degree"),
            ("lon0", 8802, "degree"),
            ("k0", 8805, "unity"),
            ("falsa_est", 8806, "metre"),
            ("falsa_nord", 8807, "metre"),
        ],
    ),
    9804: (
        "MercatoreA",
        [
            ("lat0", 8801, "degree"),
            ("lon0", 8802, "degree"),
            ("k0", 8805, "unity"),
            ("falsa_est", 8806, "metre"),
            ("falsa_nord", 8807, "metre"),
        ],
    ),
    1024: (
        "PseudoMercatore",
        [
            ("lat0", 8801, "degree"),
            ("lon0", 8802, "degree"),
            ("falsa_est", 8806, "metre"),
            ("falsa_nord", 8807, "metre"),
        ],
    ),
    9802: (
        "LambertConicaConforme2Sp",
        [
            ("lat_origine", 8821, "degree"),
            ("lon_origine", 8822, "degree"),
            ("lat1", 8823, "degree"),
            ("lat2", 8824, "degree"),
            ("est_origine", 8826, "metre"),
            ("nord_origine", 8827, "metre"),
        ],
    ),
    9820: (
        "LambertAzimutaleEquivalente",
        [
            ("lat0", 8801, "degree"),
            ("lon0", 8802, "degree"),
            ("falsa_est", 8806, "metre"),
            ("falsa_nord", 8807, "metre"),
        ],
    ),
    9809: (
        "StereograficaObliqua",
        [
            ("lat0", 8801, "degree"),
            ("lon0", 8802, "degree"),
            ("k0", 8805, "unity"),
            ("falsa_est", 8806, "metre"),
            ("falsa_nord", 8807, "metre"),
        ],
    ),
    9815: (
        "HotineObliquaB",
        [
            ("lat_centro", 8811, "degree"),
            ("lon_centro", 8812, "degree"),
            ("azimut", 8813, "degree"),
            ("angolo_reticolo", 8814, "degree"),
            ("k_centro", 8815, "unity"),
            ("est_centro", 8816, "metre"),
            ("nord_centro", 8817, "metre"),
        ],
    ),
}

# Trasformazioni senza griglia: codice del metodo EPSG -> variante Rust.
METODI_HELMERT = {
    9603: "Traslazioni",
    9606: "VettorePosizione",
    9607: "TelaioCoordinate",
}
METODO_NTV2 = 9615

# Unita' dei parametri delle trasformazioni: si leggono i fattori di
# conversione di `unit_of_measure` e si scrivono metri, secondi d'arco e parti
# per milione. Tipi d'unita' attesi per parametro.
SECONDO_D_ARCO_IN_RADIANTI = 3.141592653589793 / 648000.0


class Rifiuto(Exception):
    """Dato che il generatore non sa descrivere senza ambiguita'."""


def fmt(valore: float) -> str:
    """Letterale f64 Rust: la forma piu' corta che rilegge lo stesso valore.

    A differenza di `genera_crs_integrati.fmt` ammette la notazione
    esponenziale (parametri piccoli delle rotazioni), che Rust legge uguale.
    """
    testo = repr(float(valore))
    if testo in ("nan", "inf", "-inf"):
        raise Rifiuto(f"valore non finito: {testo}")
    if "e" in testo:
        mantissa, esponente = testo.split("e")
        if "." not in mantissa:
            mantissa += ".0"
        return f"{mantissa}e{int(esponente)}"
    if "." not in testo:
        testo += ".0"
    return testo


def stringa(testo: str) -> str:
    return base.stringa(testo)


def codici_integrati():
    return sorted(set(base.ITALIA + base.MONDO + base.UTM_WGS84_NORD + base.UTM_WGS84_SUD + base.UTM_ETRS89))


def datum_di(crs: CRS, identificativo: str) -> int:
    if identificativo == "OGC:CRS84":
        # CRS84 e' WGS 84 con longitudine prima: stesso datum di EPSG:4326.
        return 4326
    geodetico = crs.geodetic_crs
    codice = geodetico.to_epsg(min_confidence=100)
    if codice is None:
        raise Rifiuto(f"CRS geografico di base senza codice EPSG: {identificativo}")
    if geodetico.type_name != "Geographic 2D CRS":
        raise Rifiuto(f"CRS di base non geografico 2D: {identificativo}")
    return codice


def parametri_proiezione(crs: CRS, identificativo: str):
    operazione = crs.coordinate_operation
    codice = int(operazione.method_code)
    if codice not in PROIEZIONI:
        raise Rifiuto(f"metodo di proiezione non supportato: {operazione.method_name} ({identificativo})")
    variante, attesi = PROIEZIONI[codice]
    per_codice = {int(p.code): p for p in operazione.params}
    if set(per_codice) != {c for _, c, _ in attesi}:
        raise Rifiuto(f"parametri inattesi per {operazione.method_name}: {identificativo}")
    campi = []
    for campo, codice_parametro, unita in attesi:
        voce = per_codice[codice_parametro]
        if voce.unit_name != unita:
            raise Rifiuto(f"unita' {voce.unit_name} per {voce.name}: {identificativo}")
        campi.append((campo, float(voce.value)))
    if variante == "MercatoreA" and dict(campi)["lat0"] != 0.0:
        raise Rifiuto(f"Mercator (variant A) con latitudine d'origine non nulla: {identificativo}")
    if variante == "HotineObliquaB" and (dict(campi)["azimut"], dict(campi)["angolo_reticolo"]) != (90.0, 90.0):
        # Con azimut e angolo del reticolo di 90 gradi PROJ usa la Swiss
        # Oblique Mercator (`somerc`), e cosi' il codice Rust; le altre
        # varianti non hanno un'implementazione verificata.
        raise Rifiuto(f"Hotine variant B con azimut o angolo del reticolo diversi da 90: {identificativo}")
    if variante == "PseudoMercatore" and dict(campi)["lat0"] != 0.0:
        raise Rifiuto(f"Pseudo Mercator con latitudine d'origine non nulla: {identificativo}")
    return variante, campi


def descrivi_crs(codice):
    if codice is None:
        crs = CRS("OGC:CRS84")
        identificativo = "OGC:CRS84"
    else:
        crs = CRS.from_epsg(codice)
        identificativo = f"EPSG:{codice}"
    datum = datum_di(crs, identificativo)
    if crs.type_name == "Geographic 2D CRS":
        return {"id": identificativo, "codice": codice, "datum": datum, "metodo": None, "regione": None}
    if crs.type_name != "Projected CRS":
        raise Rifiuto(f"tipo non supportato: {identificativo}")
    variante, campi = parametri_proiezione(crs, identificativo)
    area = tuple(float(v) for v in crs.area_of_use.bounds)
    regione = base.regione_geografica(crs, area)
    return {
        "id": identificativo,
        "codice": codice,
        "datum": datum,
        "metodo": (variante, campi),
        "regione": regione,
    }


def descrivi_datum(codice: int):
    crs = CRS.from_epsg(codice)
    if crs.type_name != "Geographic 2D CRS":
        raise Rifiuto(f"datum senza CRS geografico 2D: EPSG:{codice}")
    if crs.prime_meridian.longitude != 0.0:
        raise Rifiuto(f"meridiano fondamentale diverso da Greenwich: EPSG:{codice}")
    ellissoide = crs.ellipsoid
    return {
        "codice": codice,
        "nome": crs.datum.name,
        "ellissoide": (ellissoide.name, ellissoide.semi_major_metre, ellissoide.inverse_flattening),
    }


def apri_database():
    percorso = Path(pyproj.datadir.get_data_dir()) / "proj.db"
    if not percorso.is_file():
        raise Rifiuto(f"proj.db assente: {percorso}")
    return sqlite3.connect(f"file:{percorso}?mode=ro", uri=True)


def area_d_uso(cursore, tabella: str, codice: int):
    righe = list(
        cursore.execute(
            "select e.name, e.west_lon, e.south_lat, e.east_lon, e.north_lat from usage u "
            "join extent e on e.auth_name = u.extent_auth_name and e.code = u.extent_code "
            "where u.object_table_name = ? and u.object_auth_name = 'EPSG' and u.object_code = ?",
            (tabella, str(codice)),
        )
    )
    if len(righe) != 1:
        raise Rifiuto(f"{tabella} EPSG:{codice}: {len(righe)} aree d'uso, attesa una")
    nome, ovest, sud, est, nord = righe[0]
    return nome, (float(ovest), float(sud), float(est), float(nord))


def sostituite(cursore, incluse):
    """Codici sostituiti (`supersession`) da una trasformazione inclusa."""
    risultato = set()
    for tabella, codice, tabella_nuova, codice_nuovo in cursore.execute(
        "select superseded_table_name, superseded_code, replacement_table_name, replacement_code "
        "from supersession where superseded_auth_name = 'EPSG' and replacement_auth_name = 'EPSG'"
    ):
        if (tabella, int(codice)) in incluse and (tabella_nuova, int(codice_nuovo)) in incluse:
            risultato.add((tabella, int(codice)))
    return risultato


def converti(cursore, valore, unita, tipo: str, codice) -> float:
    """Valore nell'unita' di riferimento del tipo (metro, radiante, unita')."""
    righe = list(
        cursore.execute(
            "select type, conv_factor from unit_of_measure where auth_name = 'EPSG' and code = ?",
            (str(unita),),
        )
    )
    if len(righe) != 1 or righe[0][0] != tipo or righe[0][1] is None:
        raise Rifiuto(f"unita' {unita} non convertibile come {tipo}: EPSG:{codice}")
    return float(valore) * float(righe[0][1])


def trasformazioni_helmert(cursore, datum, non_supportate):
    elenco = ",".join(str(c) for c in sorted(datum))
    righe = cursore.execute(
        "select code, name, method_code, source_crs_code, target_crs_code, accuracy, "
        "tx, ty, tz, translation_uom_code, rx, ry, rz, rotation_uom_code, "
        "scale_difference, scale_difference_uom_code, px, py, pz, pivot_uom_code, "
        "rate_tx, rate_ty, rate_tz, rate_rx, rate_ry, rate_rz, rate_scale_difference, epoch "
        "from helmert_transformation_table where auth_name = 'EPSG' and deprecated = 0 "
        "and source_crs_auth_name = 'EPSG' and target_crs_auth_name = 'EPSG' "
        f"and source_crs_code in ({elenco}) and target_crs_code in ({elenco}) order by code"
    ).fetchall()
    voci = []
    for riga in righe:
        (codice, nome, metodo, da, a, accuratezza, tx, ty, tz, u_t, rx, ry, rz, u_r, ds, u_s, px, py, pz, u_p) = riga[:20]
        if any(v is not None for v in riga[20:27]) or riga[27] is not None:
            raise Rifiuto(f"trasformazione dipendente dal tempo: EPSG:{codice}")
        if metodo not in METODI_HELMERT:
            # Molodensky-Badekas (9636) fra i datum della tabella c'e' solo in
            # trasformazioni sostituite (Amersfoort to ETRS89 (2), (4), (6)):
            # la sostituzione si applica dopo, quindi qui si annotano e si
            # scartano; se una restasse, il controllo in `genera` la rifiuta.
            print(f"nota: EPSG:{codice} ({nome}) metodo {metodo} senza supporto")
            non_supportate.append((int(codice), int(metodo)))
            continue
        if accuratezza is None:
            print(f"nota: EPSG:{codice} ({nome}) senza accuratezza, esclusa")
            continue
        parametri = [(nome_t, converti(cursore, v, u_t, "length", codice)) for nome_t, v in (("tx", tx), ("ty", ty), ("tz", tz))]
        variante = METODI_HELMERT[metodo]
        if variante != "Traslazioni":
            parametri += [
                (nome_r, converti(cursore, v, u_r, "angle", codice) / SECONDO_D_ARCO_IN_RADIANTI)
                for nome_r, v in (("rx", rx), ("ry", ry), ("rz", rz))
            ]
            parametri.append(("ds", converti(cursore, ds, u_s, "scale", codice) * 1e6))
        elif any(v is not None for v in (rx, ry, rz, ds)):
            raise Rifiuto(f"traslazioni con rotazioni o scala: EPSG:{codice}")
        if any(v is not None for v in (px, py, pz)):
            raise Rifiuto(f"punto di valutazione inatteso: EPSG:{codice}")
        area_nome, area = area_d_uso(cursore, "helmert_transformation", codice)
        voci.append(
            {
                "tabella": "helmert_transformation",
                "codice": int(codice),
                "nome": nome,
                "da": int(da),
                "a": int(a),
                "accuratezza": float(accuratezza),
                "metodo": (variante, parametri),
                "area_nome": area_nome,
                "area": area,
            }
        )
    return voci


def trasformazioni_griglia(cursore, datum):
    elenco = ",".join(str(c) for c in sorted(datum))
    voci = []
    for codice, nome, metodo, da, a, accuratezza, griglia, griglia2 in cursore.execute(
        "select code, name, method_code, source_crs_code, target_crs_code, accuracy, grid_name, grid2_name "
        "from grid_transformation where auth_name = 'EPSG' and deprecated = 0 "
        "and source_crs_auth_name = 'EPSG' and target_crs_auth_name = 'EPSG' "
        f"and source_crs_code in ({elenco}) and target_crs_code in ({elenco}) order by code"
    ).fetchall():
        if int(metodo) != METODO_NTV2:
            print(f"nota: EPSG:{codice} ({nome}) griglia non NTv2 (metodo {metodo}), esclusa")
            continue
        if griglia2 is not None:
            raise Rifiuto(f"NTv2 con due griglie: EPSG:{codice}")
        if accuratezza is None:
            print(f"nota: EPSG:{codice} ({nome}) senza accuratezza, esclusa")
            continue
        area_nome, area = area_d_uso(cursore, "grid_transformation", codice)
        voci.append(
            {
                "tabella": "grid_transformation",
                "codice": int(codice),
                "nome": nome,
                "da": int(da),
                "a": int(a),
                "accuratezza": float(accuratezza),
                "metodo": ("GrigliaNtv2", [("file_registro", griglia)]),
                "area_nome": area_nome,
                "area": area,
            }
        )
    return voci


def escluse_informative(cursore, datum):
    elenco = ",".join(str(c) for c in sorted(datum))
    note = []
    for tabella in ("other_transformation", "concatenated_operation"):
        for codice, nome in cursore.execute(
            f"select code, name from {tabella} where auth_name = 'EPSG' and deprecated = 0 "
            "and source_crs_auth_name = 'EPSG' and target_crs_auth_name = 'EPSG' "
            f"and source_crs_code in ({elenco}) and target_crs_code in ({elenco}) order by code"
        ):
            note.append((tabella, int(codice), nome))
    return note


def raccogli():
    """CRS, datum, trasformazioni incluse e note: i dati del file generato.

    La usa anche `genera_oracolo_riproiezione.py`, cosi' l'oracolo prova
    esattamente le trasformazioni della tabella.
    """
    crs = [descrivi_crs(None)] + [descrivi_crs(c) for c in codici_integrati()]
    codici_datum = sorted({voce["datum"] for voce in crs})
    datum = [descrivi_datum(c) for c in codici_datum]

    database = apri_database()
    cursore = database.cursor()
    non_supportate = []
    trasformazioni = trasformazioni_helmert(cursore, codici_datum, non_supportate) + trasformazioni_griglia(
        cursore, codici_datum
    )
    incluse = {(t["tabella"], t["codice"]) for t in trasformazioni}
    via = sostituite(cursore, incluse)
    # Un metodo non supportato si tollera solo se la trasformazione e'
    # sostituita da una inclusa: altrimenti mancherebbe un percorso che PROJ
    # userebbe.
    candidate = incluse | {("helmert_transformation", c) for c, _ in non_supportate}
    via_tutte = sostituite(cursore, candidate)
    for codice, metodo in non_supportate:
        if ("helmert_transformation", codice) not in via_tutte:
            raise Rifiuto(f"metodo di trasformazione non supportato {metodo}: EPSG:{codice}")
    for t in trasformazioni:
        if (t["tabella"], t["codice"]) in via:
            print(f"nota: EPSG:{t['codice']} ({t['nome']}) sostituita da un'altra inclusa, esclusa")
    trasformazioni = [t for t in trasformazioni if (t["tabella"], t["codice"]) not in via]
    trasformazioni.sort(key=lambda t: t["codice"])
    note = escluse_informative(cursore, codici_datum)
    database.close()
    return crs, datum, trasformazioni, note


def genera() -> str:
    crs, datum, trasformazioni, note = raccogli()

    ellissoidi = {}
    for voce in datum:
        ellissoidi.setdefault(voce["ellissoide"], base.nome_costante("ELLISSOIDE", voce["ellissoide"][0]))
    if len(set(ellissoidi.values())) != len(ellissoidi):
        raise Rifiuto("due ellissoidi con lo stesso nome e parametri diversi")

    versione = pyproj.database.get_database_metadata("EPSG.VERSION")
    data = pyproj.database.get_database_metadata("EPSG.DATE")
    righe = [
        "//! Parametri di riproiezione dei CRS integrati.",
        "//!",
        "//! GENERATA da `scripts/genera_riproiezione.py`: non si modifica a mano.",
        f"//! Fonte: registro EPSG {versione} ({data}), come distribuito con PROJ",
        f"//! {pyproj.proj_version_str} e letto con pyproj {pyproj.__version__} (conversioni dei",
        "//! CRS) e dal suo `proj.db` (trasformazioni fra datum).",
        "//!",
        "//! Per ogni CRS: datum (codice del CRS geografico di base), metodo di",
        "//! proiezione con i parametri EPSG (gradi, metri, fattore di scala) e,",
        "//! per Transverse Mercator e Mercator, la regione lon/lat del dominio di",
        "//! validita'. Per ogni datum: ellissoide. Trasformazioni EPSG fra datum",
        "//! della tabella: metodo, parametri (metri, secondi d'arco, parti per",
        "//! milione), accuratezza EPSG in metri, riquadro dell'area d'uso.",
    ]
    if note:
        righe.append("//!")
        righe.append("//! Operazioni del registro fra questi datum lasciate fuori (concatenate o")
        righe.append("//! di altri metodi; il percorso fra datum lo compone `percorsi`):")
        for tabella, codice, nome in note:
            righe.append(f"//! - EPSG:{codice} {nome} (`{tabella}`)")
    righe += [
        "",
        "// Dati generati: i letterali restano nella forma piu' corta che rilegge",
        "// lo stesso f64, senza separatori.",
        "#![allow(clippy::unreadable_literal)]",
        "",
        "use super::super::integrati::Identificativo;",
        "use super::super::{Ellipsoid, GeographicBounds};",
        "use super::tabella::{Datum, Definizione, MetodoProiezione, MetodoTrasformazione, Trasformazione};",
        "",
        f'pub(in crate::crs) const VERSIONE_EPSG: &str = "{versione}";',
        f'pub(in crate::crs) const VERSIONE_PROJ: &str = "{pyproj.proj_version_str}";',
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
    righe.append("")
    righe.append("/// Datum della tabella, in ordine di codice del CRS geografico di base.")
    righe.append("pub(in crate::crs) const DATUM: &[Datum] = &[")
    for voce in datum:
        righe.append(
            f"    Datum {{ codice: {voce['codice']}, nome: {stringa(voce['nome'])}, "
            f"ellissoide: {ellissoidi[voce['ellissoide']]} }},"
        )
    righe.append("];")
    righe.append("")

    def regione(voce):
        if voce["regione"] is None:
            return "None"
        return "Some(GeographicBounds::new({}, {}, {}, {}))".format(*(fmt(v) for v in voce["regione"]))

    def metodo(voce):
        if voce["metodo"] is None:
            return "MetodoProiezione::Geografico"
        variante, campi = voce["metodo"]
        corpo = ", ".join(f"{campo}: {fmt(valore)}" for campo, valore in campi)
        return f"MetodoProiezione::{variante} {{ {corpo} }}"

    def identificativo(voce):
        if voce["codice"] is None:
            return "Identificativo::OgcCrs84"
        return f"Identificativo::Epsg({voce['codice']})"

    righe.append("/// Definizioni dei CRS: `OGC:CRS84` per primo, poi i codici EPSG in ordine.")
    righe.append("pub(in crate::crs) const DEFINIZIONI: &[Definizione] = &[")
    for voce in crs:
        righe.append(
            f"    Definizione {{ crs: {identificativo(voce)}, datum: {voce['datum']}, "
            f"metodo: {metodo(voce)}, regione: {regione(voce)} }},"
        )
    righe.append("];")
    righe.append("")
    righe.append("/// Trasformazioni fra datum della tabella, in ordine di codice.")
    righe.append("pub(in crate::crs) const TRASFORMAZIONI: &[Trasformazione] = &[")
    for t in trasformazioni:
        variante, parametri = t["metodo"]
        if variante == "GrigliaNtv2":
            corpo = f"file_registro: {stringa(parametri[0][1])}"
        else:
            corpo = ", ".join(f"{campo}: {fmt(valore)}" for campo, valore in parametri)
        area = "GeographicBounds::new({}, {}, {}, {})".format(*(fmt(v) for v in t["area"]))
        righe += [
            f"    // {stringa(t['area_nome'])[1:-1]}",
            f"    Trasformazione {{ codice: {t['codice']}, nome: {stringa(t['nome'])}, da: {t['da']}, a: {t['a']}, "
            f"accuratezza_m: {fmt(t['accuratezza'])}, metodo: MetodoTrasformazione::{variante} {{ {corpo} }}, "
            f"area: {area} }},",
        ]
    righe.append("];")
    righe.append("")
    return "\n".join(righe)


def main() -> int:
    try:
        base.verifica_ambiente()
        testo = genera()
    except (Rifiuto, base.Rifiuto) as errore:
        print(f"rifiutato: {errore}", file=sys.stderr)
        return 1
    DESTINAZIONE.write_text(testo, encoding="utf-8", newline="\n")
    print(f"scritto {DESTINAZIONE.relative_to(RADICE)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
