"""Genera un envelope PLNGEO3 (trasporto Arrow v3) per i benchmark di baseline.

Replica la fixture di `spike_arrow.py` di plenora-geo-tools-arrow: box casuali
(seed deterministico), attributi id/label/weight, 5% di geometrie nulle,
colonna `geometry` binaria con metadati `geoarrow.wkb`, payload Arrow IPC
stream dentro l'envelope PLNGEO3 con checksum SHA-256.

Uso: python make_geo_fixture.py --rows 1000000 --out fixture.plngeo3 [--seed 42]
"""

from __future__ import annotations

import argparse
import hashlib
import struct
from pathlib import Path

import numpy as np
import pyarrow as pa
import shapely

MAGIC = b"PLNGEO3\0"
TRAILER_MAGIC = b"GEOEND3\0"
GEOMETRY_COLUMN = "geometry"
CRS = "EPSG:3857"


def build_fixture(rows: int, seed: int) -> pa.RecordBatch:
    generator = np.random.default_rng(seed)
    centers = generator.uniform(0.0, 10_000.0, size=(rows, 2))
    widths = generator.uniform(0.1, 50.0, size=rows)
    heights = generator.uniform(0.1, 50.0, size=rows)
    geometries = shapely.box(
        centers[:, 0],
        centers[:, 1],
        centers[:, 0] + widths,
        centers[:, 1] + heights,
    )
    null_mask = generator.uniform(0.0, 1.0, size=rows) < 0.05
    geometries[null_mask] = None
    wkb = shapely.to_wkb(
        geometries, hex=False, output_dimension=2, byte_order=1, include_srid=False
    )
    geometry_field = pa.field(
        GEOMETRY_COLUMN,
        pa.binary(),
        nullable=True,
        metadata={b"ARROW:extension:name": b"geoarrow.wkb"},
    )
    schema = pa.schema(
        [
            pa.field("id", pa.int64(), nullable=False),
            pa.field("label", pa.utf8(), nullable=True),
            pa.field("weight", pa.float64(), nullable=True),
            geometry_field,
        ]
    )
    return pa.record_batch(
        [
            pa.array(np.arange(rows, dtype=np.int64)),
            pa.array([f"riga-{index}" for index in range(rows)], type=pa.utf8()),
            pa.array(generator.uniform(0.0, 1.0, size=rows), type=pa.float64()),
            pa.array(list(wkb), type=pa.binary()),
        ],
        schema=schema,
    )


def write_envelope(path: Path, payload: bytes) -> None:
    header = MAGIC + struct.pack("<Q", len(payload))
    digest = hashlib.sha256(header)
    digest.update(payload)
    with path.open("wb") as stream:
        stream.write(header)
        stream.write(payload)
        stream.write(TRAILER_MAGIC)
        stream.write(digest.digest())


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--rows", type=int, required=True)
    parser.add_argument("--seed", type=int, default=42)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()

    batch = build_fixture(args.rows, args.seed)
    sink = pa.BufferOutputStream()
    with pa.ipc.new_stream(sink, batch.schema) as writer:
        writer.write_batch(batch)
    payload = sink.getvalue().to_pybytes()
    write_envelope(args.out, payload)
    print(
        f"fixture rows={args.rows} seed={args.seed} "
        f"payload_bytes={len(payload)} file={args.out} ({args.out.stat().st_size} byte)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
