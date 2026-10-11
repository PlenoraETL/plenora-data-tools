"""Genera la matrice degli scrittori Parquet del riferimento.

Uso:

    python genera.py <cartella> pyarrow25    # ambiente requisiti-pyarrow25.txt
    python genera.py <cartella> principale   # ambiente requisiti-principale.txt

I due ambienti sono dichiarati con le versioni e gli hash esatti:
`requisiti-pyarrow25.txt` (CPython 3.11.15, pyarrow 25.0.1) e
`requisiti-principale.txt` (CPython 3.14.6, pyarrow 26.0.0, fastparquet
2026.9.0, DuckDB 1.5.6, polars 2.0.0). Ogni insieme si genera con il suo
seme (`random.seed(7)`), nello stesso ordine in cui è stata prodotta la
matrice del riferimento (`../parquet_esiti.tsv`); i nomi dei file portano i
parametri. Le cartelle prodotte sono quelle che la prova
`tests/esiti_di_riferimento.rs` legge sotto `PLENORA_SCRITTORI`.

Un file non ha byte riproducibili: `scritti-flba/duckdb/duckdb-uuid-dec.parquet`
(UUID casuali, `uuid()` di DuckDB). Per lui la prova confronta l'esito, non
lo sha256 (`NON_RIPRODUCIBILI` in `esiti_di_riferimento.rs`).
"""

import datetime as dt
import os
import random
import sys


def _colonne_base(n, forse):
    return {
        "i32": [forse(random.randint(-1000, 1000)) for _ in range(n)],
        "i64": [forse(random.randint(-(2**40), 2**40)) for _ in range(n)],
        "f64": [forse(random.random() * 100) for _ in range(n)],
        "b": [forse(random.random() < 0.5) for _ in range(n)],
        "s": [forse(random.choice(["alfa", "beta", "gamma", "δέλτα", ""])) for _ in range(n)],
        "s_unici": [forse(f"valore-{i}-{random.random():.6f}") for i in range(n)],
        "d": [forse(dt.date(2020, 1, 1) + dt.timedelta(days=random.randint(0, 2000))) for _ in range(n)],
        "ts": [forse(dt.datetime(2021, 1, 1) + dt.timedelta(seconds=random.randint(0, 10**8))) for _ in range(n)],
    }


def colonne(n, piatte):
    """Le colonne di una tabella di `n` righe, con il 20% di null."""

    def forse(v, p=0.2):
        return None if random.random() < p else v

    if piatte:
        return _colonne_base(n, forse)
    return {
        "i32": [forse(random.randint(-1000, 1000)) for _ in range(n)],
        "i64": [forse(random.randint(-(2**40), 2**40)) for _ in range(n)],
        "f64": [forse(random.random() * 100) for _ in range(n)],
        "b": [forse(random.random() < 0.5) for _ in range(n)],
        "s": [forse(random.choice(["alfa", "beta", "gamma", "δέλτα", ""])) for _ in range(n)],
        "s_unici": [forse(f"valore-{i}-{random.random():.6f}") for i in range(n)],
        "d": [forse(dt.date(2020, 1, 1) + dt.timedelta(days=random.randint(0, 2000))) for _ in range(n)],
        "ts": [forse(dt.datetime(2021, 1, 1) + dt.timedelta(seconds=random.randint(0, 10**8))) for _ in range(n)],
        "li": [forse([forse(random.randint(0, 9)) for _ in range(random.randint(0, 4))]) for _ in range(n)],
        "ls": [forse([random.choice(["x", "yy", "zzz"]) for _ in range(random.randint(0, 3))]) for _ in range(n)],
    }


TAGLIE = [3, 1000, 20000]


def gruppo(out, nome, piatte=False):
    """Un gruppo della matrice (`pyarrow`, `fastparquet`, `duckdb`, `polars`)."""
    os.makedirs(out, exist_ok=True)
    random.seed(7)
    if nome == "pyarrow":
        import pyarrow as pa
        import pyarrow.parquet as pq

        v = pa.__version__.split(".")[0]
        for n in TAGLIE:
            t = pa.table(colonne(n, piatte))
            for versione in ["1.0", "2.0"]:
                for dizionario in [True, False]:
                    for compressione in ["none", "snappy", "zstd"]:
                        file = f"pyarrow{v}-n{n}-v{versione}-diz{int(dizionario)}-{compressione}.parquet"
                        pq.write_table(
                            t,
                            os.path.join(out, file),
                            data_page_version=versione,
                            use_dictionary=dizionario,
                            compression=compressione,
                            data_page_size=4096,
                        )
    elif nome == "fastparquet":
        import fastparquet
        import pandas as pd

        for n in TAGLIE:
            c = colonne(n, piatte)
            df = pd.DataFrame(
                {
                    "i32": pd.array(c["i32"], dtype="Int32"),
                    "i64": pd.array(c["i64"], dtype="Int64"),
                    "f64": pd.array(c["f64"], dtype="float64"),
                    "b": pd.array(c["b"], dtype="boolean"),
                    "s": pd.Series(c["s"], dtype="object"),
                    "s_unici": pd.Series(c["s_unici"], dtype="object"),
                    "cat": pd.Categorical(c["s"]),
                    "ts": pd.to_datetime(pd.Series(c["ts"])),
                    "nonnull": pd.array([random.randint(0, 9) for _ in range(n)], dtype="int64"),
                }
            )
            for compressione in [None, "SNAPPY", "ZSTD"]:
                for stats in [True, False]:
                    for rg in [None, 700]:
                        file = f"fastparquet-n{n}-{compressione}-stats{int(stats)}-rg{rg}.parquet"
                        kwargs = dict(compression=compressione, stats=stats, object_encoding="utf8")
                        if rg:
                            kwargs["row_group_offsets"] = rg
                        fastparquet.write(os.path.join(out, file), df, **kwargs)
            df2 = pd.DataFrame({"li": pd.Series(c["li"], dtype="object")})
            fastparquet.write(os.path.join(out, f"fastparquet-n{n}-lista-json.parquet"), df2, object_encoding="json")
    elif nome == "duckdb":
        import duckdb
        import pyarrow as pa

        for n in TAGLIE:
            t = pa.table(colonne(n, piatte))
            con = duckdb.connect()
            con.register("t", t)
            for versione in ["V1", "V2"]:
                for compressione in ["uncompressed", "snappy", "zstd"]:
                    file = os.path.join(out, f"duckdb-n{n}-{versione}-{compressione}.parquet").replace("\\", "/")
                    con.execute(
                        f"COPY t TO '{file}' (FORMAT parquet, PARQUET_VERSION {versione}, COMPRESSION {compressione})"
                    )
    elif nome == "polars":
        import polars as pl
        import pyarrow as pa

        for n in TAGLIE:
            t = pl.from_arrow(pa.table(colonne(n, piatte)))
            for compressione in ["uncompressed", "snappy", "zstd"]:
                for stats in [True, False]:
                    t.write_parquet(
                        os.path.join(out, f"polars-n{n}-{compressione}-stats{int(stats)}.parquet"),
                        compression=compressione,
                        statistics=stats,
                    )
                t.write_parquet(
                    os.path.join(out, f"polars-n{n}-{compressione}-pyarrow.parquet"),
                    compression=compressione,
                    use_pyarrow=True,
                )
    else:
        raise SystemExit(f"gruppo sconosciuto: {nome}")


def flba(out):
    """FLBA di 16, 64, 65 e 200 byte (statistiche lunghe), pyarrow."""
    import pyarrow as pa
    import pyarrow.parquet as pq

    os.makedirs(out, exist_ok=True)
    for larghezza in [16, 64, 65, 200]:
        valori = [bytes([i % 251]) * larghezza for i in range(5)] + [None]
        t = pa.table({"f": pa.array(valori, pa.binary(larghezza))})
        for v in ["1.0", "2.0"]:
            for diz in [False, True]:
                pq.write_table(
                    t,
                    f"{out}/pyarrow-flba{larghezza}-v{v}-diz{int(diz)}.parquet",
                    data_page_version=v,
                    use_dictionary=diz,
                    write_page_index=False,
                )


def nulli_pyarrow(out):
    import pyarrow as pa
    import pyarrow.parquet as pq

    os.makedirs(out, exist_ok=True)
    t = pa.table(
        {
            "i": pa.array([None] * 50, pa.int32()),
            "s": pa.array([None] * 50, pa.string()),
            "j": pa.array([1] + [None] * 49, pa.int64()),
        }
    )
    v = pa.__version__.split(".")[0]
    for versione in ["1.0", "2.0"]:
        for c in ["none", "snappy", "gzip", "brotli", "zstd", "lz4"]:
            pq.write_table(t, f"{out}/pyarrow{v}-nulli-v{versione}-{c}.parquet", data_page_version=versione, compression=c)


def nulli_fastparquet(out):
    import fastparquet
    import fastparquet.writer
    import pandas as pd

    os.makedirs(out, exist_ok=True)
    df = pd.DataFrame({"i": pd.array([None] * 50, dtype="Int32"), "s": pd.Series([None] * 50, dtype="object")})
    for c in [None, "SNAPPY", "GZIP", "ZSTD", "BROTLI", "LZ4"]:
        try:
            fastparquet.write(f"{out}/fastparquet-nulli-{c}.parquet", df, compression=c, object_encoding="utf8")
        except Exception as e:  # un codec non installato: il file non c'è
            print("fastparquet", c, type(e).__name__)
    fastparquet.writer.DATAPAGE_VERSION = 2
    try:
        for c in [None, "SNAPPY", "GZIP", "ZSTD"]:
            fastparquet.write(f"{out}/fastparquetv2-nulli-{c}.parquet", df, compression=c, object_encoding="utf8")
    finally:
        fastparquet.writer.DATAPAGE_VERSION = 1


def duckdb_uuid(out):
    import duckdb

    os.makedirs(out, exist_ok=True)
    con = duckdb.connect()
    con.execute(
        "COPY (SELECT uuid() AS u, 12345678901234567890.123456::DECIMAL(38,6) AS d FROM range(10)) "
        f"TO '{out}/duckdb-uuid-dec.parquet' (FORMAT parquet)".replace("\\", "/")
    )


def main():
    radice, ambiente = sys.argv[1], sys.argv[2]
    p = lambda *parti: os.path.join(radice, *parti)  # noqa: E731
    if ambiente == "pyarrow25":
        gruppo(p("scritti", "pyarrow25"), "pyarrow")
        gruppo(p("scritti-piatte", "pyarrow25"), "pyarrow", piatte=True)
        nulli_pyarrow(p("scritti-nulli"))
        flba(p("scritti-flba", "pyarrow25"))
    elif ambiente == "principale":
        gruppo(p("scritti", "pyarrow26"), "pyarrow")
        gruppo(p("scritti", "fastparquet"), "fastparquet")
        gruppo(p("scritti", "duckdb"), "duckdb")
        gruppo(p("scritti", "polars"), "polars")
        gruppo(p("scritti-piatte", "pyarrow26"), "pyarrow", piatte=True)
        gruppo(p("scritti-piatte", "duckdb"), "duckdb", piatte=True)
        gruppo(p("scritti-piatte", "polars"), "polars", piatte=True)
        import fastparquet.writer

        fastparquet.writer.DATAPAGE_VERSION = 2
        try:
            gruppo(p("scritti-fpv2", "fastparquet"), "fastparquet")
        finally:
            fastparquet.writer.DATAPAGE_VERSION = 1
        nulli_fastparquet(p("scritti-nulli"))
        flba(p("scritti-flba", "pyarrow26"))
        duckdb_uuid(p("scritti-flba", "duckdb"))
    else:
        raise SystemExit(f"ambiente sconosciuto: {ambiente}")


if __name__ == "__main__":
    main()
