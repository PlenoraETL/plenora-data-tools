#!/usr/bin/env python3
"""Genera le fixture Parquet e GeoParquet di `plenora-io` con uno scrittore esterno.

I file li scrive pyarrow (Parquet C++), non `parquet-rs`: provano che il
lettore di `plenora-io` legge GeoParquet prodotto da un'altra
implementazione, nella forma in cui lo scrive GeoPandas (colonna WKB
`binary` senza metadati di campo, metadato di file `geo` con il PROJJSON
completo di PROJ). Il PROJJSON viene da pyproj, con la terna del generatore
dei CRS integrati.

Uso (pyarrow e l'ambiente pyproj di `genera_crs_integrati.py`):

    PYTHONPATH=<dir> python scripts/genera_fixture_geoparquet.py

I file sono piccoli e committati: rigenerarli cambia i byte (pyarrow scrive
la sua versione in `created_by`), non il contenuto.
"""

from __future__ import annotations

import json
import struct
import sys
from pathlib import Path

import pyarrow as pa
import pyarrow.parquet as pq

sys.path.insert(0, str(Path(__file__).resolve().parent))

import genera_crs_integrati as tabella  # noqa: E402
from pyproj import CRS  # noqa: E402

DESTINAZIONE = tabella.RADICE / "crates" / "plenora-io" / "tests" / "dati"


def punto(x: float, y: float) -> bytes:
    return struct.pack("<BIdd", 1, 1, x, y)


def punto_z(x: float, y: float, z: float) -> bytes:
    return struct.pack("<BIddd", 1, 1001, x, y, z)


def poligono(anello: list[tuple[float, float]]) -> bytes:
    corpo = struct.pack("<BII", 1, 3, 1) + struct.pack("<I", len(anello))
    for x, y in anello:
        corpo += struct.pack("<dd", x, y)
    return corpo


def scrivi(nome: str, tabella_arrow: pa.Table, geo: dict, **opzioni) -> None:
    metadati = dict(tabella_arrow.schema.metadata or {})
    metadati[b"geo"] = json.dumps(geo).encode("utf-8")
    tabella_arrow = tabella_arrow.replace_schema_metadata(metadati)
    pq.write_table(tabella_arrow, DESTINAZIONE / nome, **opzioni)


def main() -> int:
    tabella.verifica_ambiente()
    DESTINAZIONE.mkdir(parents=True, exist_ok=True)

    # UTM 32N, punti e poligoni, una geometria nulla, due row group, SNAPPY.
    geometrie = [
        punto(500000.0, 5000000.0),
        poligono([(500000.0, 5000000.0), (500010.0, 5000000.0), (500010.0, 5000010.0), (500000.0, 5000000.0)]),
        None,
        punto(500020.5, 4999990.25),
    ]
    utm = pa.table(
        {
            "id": pa.array([1, 2, 3, 4], pa.int64()),
            "nome": pa.array(["a", "b", None, "d"], pa.string()),
            "geometry": pa.array(geometrie, pa.binary()),
        }
    )
    scrivi(
        "pyarrow_utm32.parquet",
        utm,
        {
            "version": "1.1.0",
            "primary_column": "geometry",
            "columns": {
                "geometry": {
                    "encoding": "WKB",
                    "geometry_types": ["Point", "Polygon"],
                    "crs": CRS.from_epsg(32632).to_json_dict(),
                    "edges": "planar",
                    "bbox": [500000.0, 4999990.25, 500020.5, 5000010.0],
                }
            },
        },
        compression="snappy",
        row_group_size=2,
    )

    # CRS assente (= OGC:CRS84), punti 3D, ZSTD, versione 1.0.0.
    crs84 = pa.table(
        {
            "valore": pa.array([1.5, -0.0], pa.float64()),
            "geom": pa.array([punto_z(12.5, 41.9, 21.0), punto_z(9.19, 45.46, 120.0)], pa.binary()),
        }
    )
    scrivi(
        "pyarrow_crs84_z.parquet",
        crs84,
        {
            "version": "1.0.0",
            "primary_column": "geom",
            "columns": {"geom": {"encoding": "WKB", "geometry_types": ["Point Z"]}},
        },
        compression="zstd",
    )

    # Timestamp INT96 (Impala/Spark), senza GeoParquet: il lettore lo rifiuta.
    int96 = pa.table({"quando": pa.array([0, 1_700_000_000_000_000_000], pa.timestamp("ns"))})
    pq.write_table(
        int96,
        DESTINAZIONE / "pyarrow_int96.parquet",
        use_deprecated_int96_timestamps=True,
        store_schema=False,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
