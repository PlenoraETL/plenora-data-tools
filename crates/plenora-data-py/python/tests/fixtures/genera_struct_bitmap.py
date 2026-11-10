"""Fixture IPC deterministiche, generate con PyArrow 25.0.1.

Eseguire questo script per rigenerare file e stream. Come nel commit
a8b330d9 di plenora-database-tools, cambia solo il descriptor del buffer
di validita del padre: nove righe richiedono due byte, ne dichiariamo uno.
Il corpo, il figlio stringa e il framing restano integri.
"""

from pathlib import Path
import struct

import pyarrow as pa

MARKER = "PAYLOAD_MUST_NOT_LEAK"


def campo(data: bytearray, table: int, index: int) -> int:
    vtable = table - struct.unpack_from("<i", data, table)[0]
    offset = struct.unpack_from("<H", data, vtable + 4 + index * 2)[0]
    assert offset
    return table + offset


def genera(stream: bool) -> bytes:
    values = pa.StructArray.from_arrays(
        [pa.array([MARKER] * 9)], names=["child"],
        mask=pa.array([True] + [False] * 8),
    )
    batch = pa.record_batch([values], names=["parent"])
    batch.validate(full=True)
    sink = pa.BufferOutputStream()
    writer = pa.ipc.new_stream if stream else pa.ipc.new_file
    with writer(sink, batch.schema) as output:
        output.write_batch(batch)
    raw = sink.getvalue().to_pybytes()
    reader = (pa.ipc.open_stream if stream else pa.ipc.open_file)(raw)
    assert reader.read_all().equals(pa.Table.from_batches([batch]))
    data = bytearray(raw)
    start = 0 if stream else 8
    start += 8 + struct.unpack_from("<I", data, start + 4)[0]
    assert data[start:start + 4] == b"\xff" * 4
    message = start + 8
    table = message + struct.unpack_from("<I", data, message)[0]
    assert data[campo(data, table, 1)] == 3  # RecordBatch
    header = campo(data, table, 2)
    record = header + struct.unpack_from("<I", data, header)[0]
    assert struct.unpack_from("<q", data, campo(data, record, 0))[0] == 9
    buffers = campo(data, record, 2)
    vector = buffers + struct.unpack_from("<I", data, buffers)[0]
    length = vector + 4 + 8  # Primo Buffer: validita del padre Struct.
    assert struct.unpack_from("<q", data, length)[0] == 2
    struct.pack_into("<q", data, length, 1)
    assert sum(a != b for a, b in zip(raw, data, strict=True)) == 1
    return bytes(data)


if __name__ == "__main__":
    for stream, suffix in ((False, "arrow"), (True, "arrows")):
        Path(__file__).with_name(f"struct-short-bitmap.{suffix}").write_bytes(genera(stream))
