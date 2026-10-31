#!/usr/bin/env python3
"""Genera il modello interno dai documenti e dalle entita' del registro.

Invocato da genera_projjson_integrati.py nello stesso ambiente vincolato.
Nessun numero attraversa float durante la lettura dei documenti: le cifre
serializzate da PROJ sono il contratto decimale, distinto dai double delle
API di riproiezione. I nomi canonici sono gli unici alias senza ID ammessi.
"""

from __future__ import annotations

import json
from pathlib import Path

RADICE = Path(__file__).resolve().parent.parent
DESTINAZIONE = RADICE / "crates/plenora-core/src/crs/definizioni/semantica.json"
PONTE = DESTINAZIONE.with_name("metodi_generati.rs")


def leggi(testo):
    return json.loads(testo, parse_int=str, parse_float=str)


def identita(voce):
    return {"id": voce.get("id"), "nome": voce["name"]}


def unita(nome):
    # Forme simboliche del profilo PROJJSON; i fattori sono quelli emessi
    # da PROJ per le corrispondenti unita' WKT2, senza epsilon angolare.
    codice, tipo, fattore = {
        "metre": ("9001", "linear", "1"),
        "degree": ("9122", "angular", "0.0174532925199433"),
        "unity": ("9201", "scale", "1"),
    }[nome]
    return {"identita": {"id": {"authority": "EPSG", "code": codice}, "nome": nome},
            "tipo": tipo, "fattore": fattore}


def sistema(voce):
    return {"tipo": voce["subtype"], "assi": [
        {"ordine": i + 1, "direzione": a["direction"], "unita": unita(a["unit"])}
        for i, a in enumerate(voce["axis"])]}


def descrivi(crs):
    documento = leggi(crs.to_json())
    datum = leggi(crs.datum.to_json())
    ellissoide = leggi(crs.ellipsoid.to_json())
    meridiano = leggi(crs.prime_meridian.to_json())
    if meridiano["longitude"] != "0" or crs.prime_meridian.unit_name != "degree":
        raise ValueError("meridiano fuori dal profilo")
    if datum["type"] not in ("DatumEnsemble", "GeodeticReferenceFrame"):
        raise ValueError("datum fuori dal profilo")
    base = documento.get("base_crs", documento)
    conversione = None
    if "conversion" in documento:
        c = leggi(crs.coordinate_operation.to_json())
        conversione = {"id": c.get("id"), "metodo": identita(c["method"]), "parametri": [
            {"identita": identita(p), "valore": p["value"], "unita": unita(p["unit"])}
            for p in c["parameters"]]}
    return {
        "id": documento["id"], "tipo": documento["type"],
        "base": {"id": base.get("id"), "sistema": sistema(base["coordinate_system"]),
                 "datum": {
                     "identita": identita(datum), "tipo": datum["type"],
                     "membri": [identita(m) for m in datum.get("members", [])],
                     "accuratezza": datum.get("accuracy"),
                     "ellissoide": {"identita": identita(ellissoide),
                                    "semiasse": ellissoide["semi_major_axis"],
                                    "forma": "inverse_flattening" if "inverse_flattening" in ellissoide else "semi_minor_axis",
                                    "seconda_misura": ellissoide.get("inverse_flattening", ellissoide.get("semi_minor_axis")),
                                    "unita": unita("metre")},
                     "meridiano": {"identita": identita(meridiano),
                                   "longitudine": meridiano["longitude"], "unita": unita("degree")}}},
        "conversione": conversione, "sistema": sistema(documento["coordinate_system"]),
    }


def genera(documenti, verifica=False):
    from pyproj import CRS
    import genera_crs_integrati as integrati
    import genera_riproiezione as riproiezione

    integrati.verifica_ambiente()
    modelli = []
    for chiave, documento in documenti.items():
        crs = CRS(chiave)
        if crs.to_json_dict() != documento:
            raise ValueError("PROJJSON non coerente con il registro vincolato")
        modello = descrivi(crs)
        if crs.is_projected:
            # Lo stesso controllo dei codici, cardinalita' e unita' usato
            # dal generatore dei parametri effettivi di riproiezione.
            riproiezione.parametri_proiezione(crs, chiave)
        if crs.ellipsoid.semi_major_metre != float(modello["base"]["datum"]["ellissoide"]["semiasse"]):
            raise ValueError("ellissoide discordante")
        modelli.append(modello)
    testo = "[\n" + ",\n".join(json.dumps(m, ensure_ascii=False, separators=(",", ":")) for m in modelli) + "\n]\n"
    righe = [
        "//! GENERATO da `scripts/genera_projjson_integrati.py`: non modificare a mano.",
        "//! Mappa di controllo tra codici EPSG e campi della riproiezione.",
        "use crate::crs::riproiezione::tabella::MetodoProiezione;",
        "// La corrispondenza generata mantiene insieme tutti i metodi del registro.",
        "#[allow(clippy::too_many_lines)]",
        "pub(super) fn parametri(metodo: MetodoProiezione) -> (u32, Vec<(u32, f64)>) {",
        "    match metodo {",
        "        MetodoProiezione::Geografico => (0, vec![]),",
    ]
    for codice, (variante, campi) in riproiezione.PROIEZIONI.items():
        nomi = ", ".join(campo for campo, _, _ in campi)
        valori = ", ".join(f"({codice_p}, {campo})" for campo, codice_p, _ in campi)
        righe.append(f"        MetodoProiezione::{variante} {{ {nomi} }} => ({codice}, vec![{valori}]),")
    righe += ["    }", "}", ""]
    # rustfmt e' lo stesso della toolchain del workspace; il check confronta
    # il risultato gia' formattato, senza cambiare file sul disco.
    import subprocess
    ponte = subprocess.run(["rustfmt", "--edition", "2021"], input="\n".join(righe),
                           text=True, capture_output=True, check=True).stdout
    for percorso, contenuto in ((DESTINAZIONE, testo), (PONTE, ponte)):
        if verifica:
            if percorso.read_text(encoding="utf-8") != contenuto:
                raise ValueError(f"generato non aggiornato: {percorso.name}")
        else:
            percorso.write_text(contenuto, encoding="utf-8", newline="\n")


if __name__ == "__main__":
    import sys
    documenti = json.loads((RADICE / "crates/plenora-io/data/projjson_integrati.json").read_text(encoding="utf-8"))
    genera(documenti, verifica="--verifica" in sys.argv)
