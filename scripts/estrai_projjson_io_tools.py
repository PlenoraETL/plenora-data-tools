#!/usr/bin/env python3
"""Estrae senza riserializzare il PROJJSON della fixture canonica IO-tools.

Uso: python scripts/estrai_projjson_io_tools.py <checkout-io-tools>
Richiede pyarrow solo per la rigenerazione, non per le prove Rust.
"""

import hashlib
import json
from pathlib import Path
import sys

import pyarrow.ipc as ipc

radice = Path(__file__).resolve().parent.parent
sorgente = Path(sys.argv[1]) / "crates/plenora-io-tools/tests/fixtures/canoniche/canonico.arrow"
schema = ipc.open_file(sorgente).schema
testo = schema.field("geometry").metadata[b"plenora.geometry.crs_definition"]
destinazione = radice / "crates/plenora-io/tests/dati/io_tools_3003.json"
destinazione.write_bytes(testo + b"\n")
provenienza = {
    "sorgente": "plenora-io-tools/tests/fixtures/canoniche/canonico.arrow",
    "sha256_arrow": hashlib.sha256(sorgente.read_bytes()).hexdigest(),
    "sha256_definizione": hashlib.sha256(testo).hexdigest(),
    "estrazione": "metadata del campo geometry, plenora.geometry.crs_definition, byte originali",
}
destinazione.with_suffix(".provenienza.json").write_text(
    json.dumps(provenienza, indent=2) + "\n", encoding="utf-8", newline="\n"
)
