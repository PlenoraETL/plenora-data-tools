#!/usr/bin/env python3
"""Genera il PROJJSON completo dei CRS integrati, per la scrittura GeoParquet.

GeoParquet 1.1 vuole il CRS di una colonna come documento PROJJSON. Il
canonical di `plenora_core::crs` e' un sottoinsieme (tipo, nome, assi, `id`)
che PROJ non rilegge; qui si scrive, per ogni CRS della tabella integrata, il
documento che PROJ stesso produce, con la stessa terna pyproj/PROJ/EPSG del
generatore della tabella (`genera_crs_integrati.py`, di cui si riusano le
liste e il controllo d'ambiente).

Uso (stesso ambiente di `genera_crs_integrati.py`, vedi docs/crs.md,
«Aggiungere un codice»):

    PYTHONPATH=<dir> python scripts/genera_projjson_integrati.py

Scrive `crates/plenora-io/data/projjson_integrati.json`: un oggetto con le
chiavi `OGC:CRS84` e `EPSG:<codice>` in ordine, valori PROJJSON compatti.
Un test di `plenora-io` verifica che le chiavi siano esattamente gli
identificatori di `builtin_crs_identifiers()` e che l'`id` di ogni documento
sia la sua chiave.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import genera_crs_integrati as tabella  # noqa: E402
from pyproj import CRS  # noqa: E402

DESTINAZIONE = (
    tabella.RADICE / "crates" / "plenora-io" / "data" / "projjson_integrati.json"
)


def main() -> int:
    tabella.verifica_ambiente()
    codici = sorted(
        set(
            tabella.ITALIA
            + tabella.MONDO
            + tabella.UTM_WGS84_NORD
            + tabella.UTM_WGS84_SUD
            + tabella.UTM_ETRS89
        )
    )
    documenti = {"OGC:CRS84": CRS("OGC:CRS84").to_json_dict()}
    for codice in codici:
        documenti[f"EPSG:{codice}"] = CRS.from_epsg(codice).to_json_dict()
    for chiave, documento in documenti.items():
        autorita, codice = chiave.split(":")
        identita = documento.get("id", {})
        if str(identita.get("authority")) != autorita or str(identita.get("code")) != codice:
            raise tabella.Rifiuto(f"{chiave}: id del PROJJSON diverso dalla chiave")
    testo = json.dumps(documenti, ensure_ascii=False, separators=(",", ":"))
    DESTINAZIONE.write_text(testo + "\n", encoding="utf-8", newline="\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
