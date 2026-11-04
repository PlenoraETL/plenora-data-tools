#!/usr/bin/env python3
"""Corpus WKT dalla stessa sorgente dei PROJJSON integrati.

Richiede pyproj 3.7.2 / PROJ 9.5.1 / EPSG v11.022, come la tabella.
WKT2 e WKT1_GDAL grezzo sono esportati senza adattamenti. La variante
WKT1 con assi sostituisce il GEOGCS interno (che PROJ esporta senza AXIS)
con l'export autonomo della stessa base, con output_axis_rule=True.
Il datum WKT1 richiede lo stesso codice EPSG del datum/ensemble integrato.
Uso: PYTHONPATH=<ambiente fissato> python scripts/genera_wkt_integrati.py
"""

import json
import re
from pathlib import Path


def wkt1(crs, base, esclusione_ellissoide):
    from pyproj import CRS
    grezzo = crs.to_wkt("WKT1_GDAL", output_axis_rule=True)
    esplicito = grezzo
    if crs.is_projected:
        identificativo = base["id"]
        base_crs = CRS(f'{identificativo["authority"]}:{identificativo["code"]}')
        senza_assi = base_crs.to_wkt("WKT1_GDAL", output_axis_rule=False)
        con_assi = base_crs.to_wkt("WKT1_GDAL", output_axis_rule=True)
        if grezzo.count(senza_assi) != 1:
            raise ValueError("Base WKT1 non individuata univocamente")
        esplicito = grezzo.replace(senza_assi, con_assi, 1)
    return {
        "wkt1_gdal": grezzo,
        "wkt1_assi": esplicito,
        **esclusioni_wkt1(grezzo, esplicito, json.loads(crs.datum.to_json())["id"], esclusione_ellissoide),
    }

def esclusioni_wkt1(grezzo, esplicito, atteso, esclusione_ellissoide):
    risultato = {}
    for campo, testo in (("esclusione_wkt1_gdal", grezzo),
                         ("esclusione_wkt1", esplicito)):
        # L'export fissato ha SPHEROID con AUTHORITY opzionale, poi
        # l'AUTHORITY del DATUM: non confondere i due identificativi.
        trovato = re.search(
            r'DATUM\["[^"]*",SPHEROID\["[^"]*",[^,\]]+,[^,\]]+'
            r'(?:,AUTHORITY\["[^"]*","[^"]*"\])?\]'
            r',AUTHORITY\["EPSG","([^"\]]+)"\]\]', testo)
        risultato[campo] = (
            esclusione_ellissoide if trovato and atteso["authority"] == "EPSG"
            and trovato[1] == str(atteso["code"]) else
            "DATUM WKT1 senza codice EPSG identico al datum/ensemble del registro."
        )
        metodo = re.search(r'PROJECTION\["([^"]+)"', testo)
        if metodo and metodo[1] not in (
            "Transverse_Mercator", "Lambert_Conformal_Conic_2SP",
            "Hotine_Oblique_Mercator_Azimuth_Center", "Oblique_Stereographic",
        ):
            risultato[campo] = "Metodo WKT1 fuori dal profilo dei quattro alias ammessi."
        if "EXTENSION[" in testo:
            risultato[campo] = "Metodo WKT1 fuori profilo e nodo EXTENSION non ammesso."
    return risultato


def main():
    import genera_crs_integrati as tabella
    from pyproj import CRS
    tabella.verifica_ambiente()
    radice = Path(__file__).resolve().parent.parent
    fonte = radice / "crates/plenora-io/data/projjson_integrati.json"
    documenti = json.loads(fonte.read_text(encoding="utf-8"))
    corpus = {}
    for codice, documento in documenti.items():
        crs = CRS(codice)
        if crs.to_json_dict() != documento:
            raise ValueError("PROJJSON diverso dalla sorgente fissata")
        base = documento.get("base_crs", documento)
        datum = base.get("datum", base.get("datum_ensemble"))
        esclusione_ellissoide = (
            "Ellissoide definito da semi_minor_axis: WKT esporta un inverso "
            "dello schiacciamento arrotondato, non equivalente decimale esatto."
            if "semi_minor_axis" in datum["ellipsoid"] else None
        )
        corpus[codice] = {
            "wkt2_2019": crs.to_wkt("WKT2_2019"),
            "wkt2_2015": crs.to_wkt("WKT2_2015"),
            "esclusione_2019": esclusione_ellissoide,
            "esclusione_2015": (
                "Il formato 2015 sostituisce l'ensemble con un datum storico."
                if "datum_ensemble" in base else esclusione_ellissoide
            ),
        }
        corpus[codice].update(wkt1(crs, base, esclusione_ellissoide))
    destinazione = radice / "crates/plenora-core/src/crs/definizioni/wkt_corpus.json"
    destinazione.write_text(json.dumps(corpus, ensure_ascii=False, indent=2) + "\n",
                           encoding="utf-8", newline="\n")


if __name__ == "__main__":
    main()
