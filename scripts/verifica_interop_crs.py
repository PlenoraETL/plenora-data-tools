#!/usr/bin/env python3
"""Gate reale IO-tools -> Arrow/GeoParquet -> data-tools -> rilettura PyArrow.

Richiede i due binari e PyArrow (requirements-sdk-tests.txt). Nessun download,
build o scrittura nei checkout dei produttori. Tutti i file e il rapporto
con hash dei binari rimangono nella directory di lavoro richiesta.
"""

import argparse
from decimal import Decimal
import hashlib
import json
from pathlib import Path
import struct
import subprocess

RADICE = Path(__file__).resolve().parents[1]
PREFISSO = b"plenora.geometry."
CASI = {
    "EPSG:4326": ("lon_lat", [(12.5, 45.25), (13.75, 46.5)]),
    "EPSG:6707": ("easting_northing", [(500123.0, 4987654.0), (500234.0, 4987765.0)]),
    "EPSG:7791": ("easting_northing", [(500123.0, 4987654.0), (500234.0, 4987765.0)]),
}


def json_esatto(testo):
    def oggetto(coppie):
        risultato = {}
        for chiave, valore in coppie:
            if chiave in risultato:
                raise ValueError("chiave JSON duplicata")
            risultato[chiave] = valore
        return risultato

    def costante(_):
        raise ValueError("costante JSON non finita")

    return json.loads(testo, parse_float=Decimal, parse_int=Decimal,
                      parse_constant=costante, object_pairs_hook=oggetto)


def json_uguali(a, b):
    """Confronto JSON tipato: false/true non equivalgono ai numeri 0/1."""
    if type(a) is not type(b):
        return False
    if isinstance(a, dict):
        return a.keys() == b.keys() and all(json_uguali(a[k], b[k]) for k in a)
    if isinstance(a, list):
        return len(a) == len(b) and all(json_uguali(x, y) for x, y in zip(a, b, strict=True))
    return a == b


def sha256(percorso):
    return hashlib.sha256(percorso.read_bytes()).hexdigest()


def esegui(argomenti):
    esito = subprocess.run([str(a) for a in argomenti], capture_output=True, timeout=120)
    if esito.returncode:
        raise RuntimeError(f"comando fallito ({esito.returncode}): {argomenti[0]}: "
                           + (esito.stderr + esito.stdout).decode("utf-8", errors="replace"))
    return json.loads(esito.stdout)


def leggi(percorso):
    import pyarrow.ipc as ipc
    import pyarrow.parquet as pq
    if percorso.suffix == ".parquet":
        return pq.read_table(percorso)
    with ipc.open_file(percorso) as lettore:
        return lettore.read_all()


def definizione(tabella, percorso, testo_atteso, formato, assi, proj_atteso):
    campo = tabella.schema.field("geometry")
    metadata = campo.metadata or {}
    testo = metadata.get(PREFISSO + b"crs_definition")
    for chiave, valore in ((b"axis_order", assi), (b"crs_resolution", "resolved"),
                          (b"crs_definition_format", formato)):
        if metadata.get(PREFISSO + chiave) != valore.encode():
            raise ValueError("metadati CRS incoerenti dopo rilettura")
    if testo is None:
        raise ValueError("definizione completa assente")
    if formato == "projjson":
        if not json_uguali(json_esatto(testo), proj_atteso):
            raise ValueError("PROJJSON riletto diverso dal corpus atteso")
    elif testo != testo_atteso.encode():
        raise ValueError("WKT2 riletto diverso dalla definizione originale")
    if percorso.suffix == ".parquet":
        import pyarrow.parquet as pq
        geo = json_esatto(pq.read_metadata(percorso).metadata[b"geo"])
        documento = geo["columns"]["geometry"]["crs"]
        if not json_uguali(documento, proj_atteso):
            raise ValueError("CRS GeoParquet diverso dal corpus atteso")
    if testo is None or not (testo.startswith(b"{") or testo.startswith((b"GEOGCRS[", b"PROJCRS["))):
        raise ValueError("definizione completa assente o ridotta a identificatore")
    return testo


def verifica(percorso, punti, testo_atteso, formato, assi, proj_atteso, covering=False):
    tabella = leggi(percorso)
    colonne = ["geometry", "bbox"] if covering else ["geometry"]
    if tabella.num_rows != len(punti) or tabella.column_names != colonne:
        raise ValueError("righe o schema diversi")
    if covering and tabella["bbox"].to_pylist() != [
        {"xmin": x, "ymin": y, "xmax": x, "ymax": y} for x, y in punti
    ]:
        raise ValueError("covering non coerente con le coordinate")
    for cella, (x, y) in zip(tabella["geometry"].to_pylist(), punti, strict=True):
        # Un POINT XY WKB, decodificato indipendentemente dai due prodotti.
        if cella is None or len(cella) != 21 or cella[0] not in (0, 1):
            raise ValueError("WKB non POINT XY")
        tipo, letto_x, letto_y = struct.unpack(("<" if cella[0] else ">") + "Idd", cella[1:])
        if tipo != 1 or (letto_x, letto_y) != (x, y):
            raise ValueError("coordinate alterate o assi scambiati")
    return definizione(tabella, percorso, testo_atteso, formato, assi, proj_atteso)


def sorgente(percorso, testo, formato, assi, punti):
    import pyarrow as pa
    import pyarrow.ipc as ipc
    metadata = {
        b"ARROW:extension:name": b"geoarrow.wkb",
        b"plenora.field_id": b"0",
        PREFISSO + b"crs_definition": testo.encode(),
        PREFISSO + b"crs_definition_format": formato.encode(),
        PREFISSO + b"axis_order": assi.encode(),
        PREFISSO + b"dimensions": b"xy",
        PREFISSO + b"spatial_semantics": b"geometry",
        PREFISSO + b"encoding": b"wkb",
        PREFISSO + b"precision": b"float64",
        PREFISSO + b"types_declaration": b"exact",
        PREFISSO + b"types": b"point",
        PREFISSO + b"crs_resolution": b"resolved",
    }
    schema = pa.schema([pa.field("geometry", pa.binary(), nullable=False, metadata=metadata)],
                       metadata={b"plenora.contract.version": b"1"})
    valori = [struct.pack("<BIdd", 1, 1, x, y) for x, y in punti]
    with ipc.new_file(percorso, schema) as writer:
        writer.write_table(pa.Table.from_arrays([pa.array(valori, pa.binary())], schema=schema))


def main():
    import pyarrow as pa
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--io-tools", type=Path, required=True)
    parser.add_argument("--data-tools", type=Path, required=True)
    parser.add_argument("--lavoro", type=Path, required=True)
    args = parser.parse_args()
    # Una cartella nuova impedisce di scambiare risultati vecchi per prove riuscite.
    args.lavoro.mkdir(parents=True, exist_ok=False)
    rapporto = {"cella": "data-crs-definizione", "stato": "fallito", "casi": [],
                "pyarrow": pa.__version__,
                "sorgenti": {p: sha256(RADICE / p) for p in (
                    "scripts/verifica_interop_crs.py",
                    "crates/plenora-io/data/projjson_integrati.json",
                    "crates/plenora-core/src/crs/definizioni/wkt_corpus.json")},
                "binari": {nome: {"sha256": sha256(p), "versione": esegui([p, "--version"])}
                           for nome, p in (("io", args.io_tools), ("data", args.data_tools))}}
    try:
        proj = json.loads((RADICE / "crates/plenora-io/data/projjson_integrati.json").read_text(encoding="utf-8"))
        proj_esatto = json_esatto((RADICE / "crates/plenora-io/data/projjson_integrati.json").read_bytes())
        wkt = json.loads((RADICE / "crates/plenora-core/src/crs/definizioni/wkt_corpus.json").read_text(encoding="utf-8"))
        for codice, (assi, punti) in CASI.items():
            for formato in ("projjson", "wkt2"):
                testo = json.dumps(proj[codice]) if formato == "projjson" else wkt[codice]["wkt2_2019"]
                nome = codice.replace(":", "-") + "-" + formato
                ingresso = args.lavoro / (nome + ".arrow")
                sorgente(ingresso, testo, formato, assi, punti)
                # Il writer IO GeoParquet dichiara solo PROJJSON come preservato.
                for estensione in (("arrow", "parquet") if formato == "projjson" else ("arrow",)):
                    prodotto = args.lavoro / (nome + "-io." + estensione)
                    esegui([args.io_tools, "convert", ingresso, prodotto, "--from", "ipc", "--to",
                            "ipc" if estensione == "arrow" else "geoparquet"])
                    definizione_io = verifica(prodotto, punti, testo, formato, assi, proj_esatto[codice],
                                              covering=estensione == "parquet")
                    if estensione == "arrow" and definizione_io != testo.encode():
                        raise ValueError("IO non conserva la definizione Arrow originale")
                    piano = args.lavoro / (nome + "-piano.json")
                    piano.write_text(json.dumps({"version": 1, "inputs": ["t"], "steps": [
                        {"out": "u", "op": "table.select_columns", "in": ["t"],
                         "config": {"columns": ["geometry"]}}], "outputs": ["u"]}), encoding="utf-8")
                    for uscita_ext in ("arrow", "parquet"):
                        uscita = args.lavoro / (nome + "-da-" + estensione + "-data." + uscita_ext)
                        esegui([args.data_tools, "run", "--plan", piano, "--input", f"t={prodotto}",
                                "--output", f"u={uscita}", "--format", "json"])
                        definizione_data = verifica(uscita, punti, testo, formato, assi, proj_esatto[codice])
                        if estensione == uscita_ext == "arrow" and definizione_data != definizione_io:
                            raise ValueError("data non conserva la definizione Arrow originale")
                        rapporto["casi"].append({"crs": codice, "formato": formato,
                            "ingresso_data": estensione, "uscita_data": uscita_ext,
                            "sha256_io": sha256(prodotto), "sha256_data": sha256(uscita),
                            "sha256_definizione_io": hashlib.sha256(definizione_io).hexdigest(),
                            "coordinate_verificate": len(punti)})
        if len(rapporto["casi"]) != 18:
            raise ValueError("matrice locale incompleta")
        rapporto["stato"] = "superato"
    finally:
        (args.lavoro / "rapporto.json").write_text(json.dumps(rapporto, indent=2) + "\n", encoding="utf-8")
    print("data-crs-definizione: 18 catene superate; matrice proprietaria non modificata")


if __name__ == "__main__":
    main()

