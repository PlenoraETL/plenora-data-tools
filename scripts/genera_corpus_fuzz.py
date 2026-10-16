#!/usr/bin/env python3
"""Corpus iniziali dei target di fuzz (`fuzz/`), generati e non versionati.

Un seme per ogni caso del catalogo dei test del runner (piani e config), WKT
e WKB di ogni tipo (vuoti, buchi, auto-intersezioni, una sentinella che non
deve mai finire in un errore), file Arrow IPC (file e stream) e Parquet
(senza compressione, SNAPPY, ZSTD, con checksum di pagina) scritti da PyArrow,
i Parquet di prova di `plenora-io`, argomenti della CLI. Serve PyArrow
(requirements-sdk-tests.txt).

Uso:

    python scripts/genera_corpus_fuzz.py fuzz/corpus

Scrive `<cartella>/<target>/<sha1>`; un seme già presente si riscrive uguale.
"""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import re
import struct
from pathlib import Path

import pyarrow as pa
import pyarrow.ipc as ipc
import pyarrow.parquet as pq

RADICE = Path(__file__).resolve().parents[1]
SENTINELLA = "SENTINELLA-7f3a9c"


class Corpus:
    def __init__(self, cartella: Path) -> None:
        self.cartella = cartella
        self.conteggi: dict[str, int] = {}

    def salva(self, target: str, dati: bytes) -> None:
        destinazione = self.cartella / target
        destinazione.mkdir(parents=True, exist_ok=True)
        (destinazione / hashlib.sha1(dati).hexdigest()).write_bytes(dati)
        self.conteggi[target] = self.conteggi.get(target, 0) + 1


def casi_del_catalogo() -> list[tuple[str, str, object]]:
    testo = (RADICE / "crates/plenora-pipeline/tests/comune/mod.rs").read_text(encoding="utf-8")
    casi = re.findall(r'caso!\(\s*"([^"]+)",\s*(\w+),\s*(r#"(.*?)"#|"(.*?)")\s*\)', testo, re.S)
    if not casi:
        raise SystemExit("nessun caso! in crates/plenora-pipeline/tests/comune/mod.rs")
    return [(op, fixture, json.loads(grezzo or semplice.replace('\\"', '"'))) for op, fixture, _, grezzo, semplice in casi]


def piani(corpus: Corpus, casi: list[tuple[str, str, object]]) -> None:
    for op, fixture, config in casi:
        ingressi = ["sinistra", "destra"] if fixture in {"Binary", "Set"} else ["sinistra"]
        piano = {"version": 1, "inputs": ingressi,
                 "steps": [{"out": "uscita", "op": op, "in": ingressi, "config": config}],
                 "outputs": ["uscita"]}
        corpus.salva("piano", json.dumps(piano).encode())
    corpus.salva("piano", json.dumps({
        "version": 1, "inputs": ["t"], "crs": "EPSG:32632",
        "limits": {"max_rows_per_edge": 1000, "max_expansion_factor": 0.1},
        "steps": [{"out": "g", "op": "geo.from_wkt", "in": ["t"], "config": {"wkt_column": "wkt"}},
                  {"out": "b", "op": "geo.buffer", "in": ["g"], "config": {"distance": 1.5}}],
        "outputs": ["b", "g"],
    }).encode())


def esecuzione_tabellare(corpus: Corpus, numero_casi: int) -> None:
    for indice in range(numero_casi):
        corpus.salva("esecuzione_tabellare", bytes([indice % 256, 1, 6]) + bytes(range(1, 200)))


WKT = [
    "POINT (500000 5000000)",
    "LINESTRING (500000 5000000, 500010 5000010, 500020 5000000)",
    "POLYGON ((500000 5000000, 500010 5000000, 500010 5000010, 500000 5000010, 500000 5000000))",
    "POLYGON ((500000 5000000, 500010 5000000, 500010 5000010, 500000 5000010, 500000 5000000), "
    "(500002 5000002, 500004 5000002, 500004 5000004, 500002 5000004, 500002 5000002))",
    "POLYGON ((500000 5000000, 500010 5000010, 500010 5000000, 500000 5000010, 500000 5000000))",
    "MULTIPOLYGON (((500000 5000000, 500005 5000000, 500005 5000005, 500000 5000000)), "
    "((500003 5000003, 500008 5000003, 500008 5000008, 500003 5000003)))",
    "GEOMETRYCOLLECTION (POINT (500000 5000000), LINESTRING (500000 5000000, 500001 5000001))",
    "POLYGON EMPTY",
    "POINT (12.5 41.9)",
    "POLYGON ((12 41, 13 41, 13 42, 12 42, 12 41))",
]


def esecuzione_geo(corpus: Corpus) -> None:
    for scelta in range(0, 256, 7):
        corpus.salva("esecuzione_geo", bytes([scelta]) + "\n".join(WKT[scelta % 7: scelta % 7 + 4]).encode())
    for scelta in (0, 9, 13, 21, 22, 128):
        corpus.salva("esecuzione_geo", bytes([scelta]) + f"POINT ({SENTINELLA} 1)\n{WKT[7]}\n{WKT[2]}".encode())


def wkb(corpus: Corpus) -> None:
    def punto(x: float, y: float) -> bytes:
        return struct.pack("<BIdd", 1, 1, x, y)

    def anello(coordinate: list[tuple[float, float]]) -> bytes:
        return struct.pack("<I", len(coordinate)) + b"".join(struct.pack("<dd", *c) for c in coordinate)

    quadrato = [(0, 0), (10, 0), (10, 10), (0, 10), (0, 0)]
    buco = [(2, 2), (4, 2), (4, 4), (2, 4), (2, 2)]
    for seme in (
        punto(1.5, -2.0),
        struct.pack("<BI", 1, 2) + anello([(0, 0), (1, 1), (2, 0)]),
        struct.pack("<BII", 1, 3, 1) + anello(quadrato),
        struct.pack("<BII", 1, 3, 2) + anello(quadrato) + anello(buco),
        struct.pack("<BI", 1, 3) + struct.pack("<I", 0),
        struct.pack("<BII", 1, 4, 2) + punto(0, 0) + punto(1, 1),
        struct.pack("<BII", 1, 6, 1) + struct.pack("<BII", 1, 3, 1) + anello(quadrato),
        struct.pack("<BII", 1, 7, 2) + punto(0, 0) + struct.pack("<BI", 1, 2) + anello([(0, 0), (1, 1)]),
        struct.pack(">BIdd", 0, 1, 3.0, 4.0),
        struct.pack("<BIdd", 1, 1, float("nan"), float("nan")),
        struct.pack("<BIddd", 1, 1001, 1.0, 2.0, 3.0),
    ):
        corpus.salva("wkb", seme)


def tabelle() -> list[pa.Table]:
    return [
        pa.table({"id": pa.array([1, 2, None], pa.int64()), "nome": ["a", None, "é"],
                  "x": pa.array([0.5, float("nan"), -0.0])}),
        pa.table({"d": pa.array([0, 19000], pa.date32()),
                  "t": pa.array([0, 1_700_000_000_000], pa.timestamp("ms", tz="UTC")),
                  "b": pa.array([True, False]),
                  "bin": pa.array([b"x", b""], pa.binary())}),
        pa.table({"lst": pa.array([[1, 2], [], None], pa.list_(pa.int32())),
                  "st": pa.array([{"a": 1}, {"a": None}, None], pa.struct([("a", pa.int64())])),
                  "dic": pa.array(["x", "y", "x"]).dictionary_encode()}),
        pa.table({"vuota": pa.array([], pa.int64())}),
    ]


def file_tabellari(corpus: Corpus) -> None:
    for tabella in tabelle():
        for scrittore, opzioni in ((ipc.new_file, {}), (ipc.new_stream, {"max_chunksize": 1})):
            buffer = io.BytesIO()
            with scrittore(buffer, tabella.schema) as uscita:
                uscita.write_table(tabella, **opzioni)
            corpus.salva("lettura_ipc", buffer.getvalue())
        for compressione in ("none", "snappy", "zstd"):
            buffer = io.BytesIO()
            pq.write_table(tabella, buffer, compression=compressione,
                           write_page_checksum=True, row_group_size=2)
            corpus.salva("lettura_parquet", buffer.getvalue())
    for esistente in sorted((RADICE / "crates/plenora-io/tests/dati").glob("*.parquet")):
        corpus.salva("lettura_parquet", esistente.read_bytes())
    # I file delle prove dei decoder e del footer (`tests/parquet_decoder.rs`,
    # `tests/parquet_footer.rs`).
    for seme in sorted((RADICE / "crates/plenora-io/tests/dati").glob("fuzz-*/*.parquet")):
        corpus.salva("lettura_parquet", seme.read_bytes())


def ordinamento(corpus: Corpus) -> None:
    for testa in range(6):
        for tipo in range(10):
            corpus.salva("ordinamento", bytes([testa, 40, tipo, (tipo + 3) % 10, (tipo + 7) % 10]) + bytes(range(256)))
    for testa in (0, 1):
        for tipo in range(10):
            corpus.salva("ordinamento_parallelo", bytes([testa, 4, tipo, (tipo + 3) % 10]) + bytes(range(256)))


def argomenti_cli(corpus: Corpus) -> None:
    for argomenti in (
        ["--help"], ["--version"], ["catalog"], ["--format", "json", "capabilities"],
        ["describe", "--input", "t.parquet"],
        ["validate", "--plan", "piano.json", "--input", "t=t.arrow"],
        ["run", "--plan", "piano.json", "--input", "t=t.arrow", "--output", "u=u.parquet",
         "--timeout-ms", "10000"],
    ):
        corpus.salva("argomenti_cli", "\0".join(argomenti).encode())


def main(argv: list[str] | None = None) -> int:
    lettore = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    lettore.add_argument("cartella", type=Path)
    letti = lettore.parse_args(argv)
    corpus = Corpus(letti.cartella)
    casi = casi_del_catalogo()
    piani(corpus, casi)
    esecuzione_tabellare(corpus, len(casi))
    esecuzione_geo(corpus)
    wkb(corpus)
    file_tabellari(corpus)
    ordinamento(corpus)
    argomenti_cli(corpus)
    for target, numero in sorted(corpus.conteggi.items()):
        print(f"{target}: {numero}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
