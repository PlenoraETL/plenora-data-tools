"""Semi CRS deterministici dai corpus reali versionati; nessun PROJ a runtime."""

import json
from pathlib import Path

RADICE = Path(__file__).resolve().parents[1]
SENTINELLA = "SENTINELLA-7f3a9c"


def semi():
    yield (RADICE / "crates/plenora-io/tests/dati/io_tools_3003.json").read_bytes()
    proj = json.loads((RADICE / "crates/plenora-io/data/projjson_integrati.json").read_text(encoding="utf-8"))
    wkt = json.loads((RADICE / "crates/plenora-core/src/crs/definizioni/wkt_corpus.json").read_text(encoding="utf-8"))
    if set(proj) != set(wkt) or len(proj) != 169:
        raise ValueError("corpus CRS incompleto")
    for codice in sorted(proj):
        yield codice.encode()
        documenti = [json.dumps(proj[codice], ensure_ascii=False, separators=(",", ":"))]
        documenti += [wkt[codice][forma] for forma in
                      ("wkt2_2019", "wkt2_2015", "wkt1_assi", "wkt1_gdal")]
        for testo in documenti:
            dati = testo.encode()
            yield dati
            for taglio in sorted({0, 1, len(dati) // 2, len(dati) - 1}):
                yield dati[:taglio]
            yield dati + b"\0"
            yield dati + SENTINELLA.encode()
            # Numeri oltre f64, overflow ed esponenti estremi, senza cambiare ID.
            for numero in ("6378137.00000000001", "1e999999999999999999999", "1e-9999"):
                yield testo.replace("6378137", numero).encode()
            if testo.startswith("{"):
                yield ('{"name":"' + SENTINELLA + '",' + testo[1:]).encode()
                yield testo.replace('"name":', '"' + SENTINELLA + '":', 1).encode()
            else:
                yield testo.replace('"', '"' + SENTINELLA, 1).encode()
                yield (testo[:-1] + ',ID["EPSG",4326]]').encode()
                yield testo.replace("north", "south").encode()
    for profondita in (31, 32, 33, 128):
        yield ('{"x":' * profondita + '0' + '}' * profondita).encode()
        yield ('GEOGCRS[' * profondita + '"x"' + ']' * profondita).encode()
    for nodi in (8191, 8192, 8193):
        yield ('{"x":[' + ','.join('0' for _ in range(nodi)) + ']}').encode()
        yield ('GEOGCRS[' + ','.join('0' for _ in range(nodi)) + ']').encode()
    for lunghezza in (65535, 65536, 65537):
        yield ('{"name":"' + 'x' * (lunghezza - 11) + '"}').encode()
        yield ('GEOGCRS["' + 'x' * (lunghezza - 11) + '"]').encode()


def genera(corpus):
    for dati in semi():
        corpus.salva("definizioni_crs", dati)
