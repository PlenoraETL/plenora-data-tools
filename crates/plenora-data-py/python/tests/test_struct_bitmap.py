"""Bitmap del padre Struct corta: decoder Rust da path e PyArrow lazy."""

import asyncio
import json
from pathlib import Path

import pyarrow as pa
import pytest

import plenora_data as pd
from plenora_data import _native
from aiuti import piano_identita
from fixtures.genera_struct_bitmap import MARKER, genera


@pytest.mark.parametrize("stream", [False, True])
def test_fixture_struct_riproducibile(stream: bool) -> None:
    suffix = "arrows" if stream else "arrow"
    path = Path(__file__).parent / "fixtures" / f"struct-short-bitmap.{suffix}"
    assert path.read_bytes() == genera(stream) == genera(stream)
    reader = (pa.ipc.open_stream if stream else pa.ipc.open_file)(path)
    batch = reader.read_next_batch() if stream else reader.get_batch(0)
    assert batch.num_rows == 9
    assert batch.column(0).buffers()[0].size == 1
    batch.column(0).field(0).validate(full=True)
    with pytest.raises(pa.ArrowInvalid):
        batch.validate(full=True)


@pytest.mark.parametrize("stream", [False, True])
@pytest.mark.parametrize("source", ["path", "lazy"])
@pytest.mark.parametrize("surface", ["native", "sdk", "async"])
@pytest.mark.parametrize("operation", ["describe", "validate", "run"])
def test_struct_bitmap_padre_corta(
    stream: bool, source: str, surface: str, operation: str, tmp_path: Path,
) -> None:
    suffix = "arrows" if stream else "arrow"
    path = Path(__file__).parent / "fixtures" / f"struct-short-bitmap.{suffix}"
    delivered = []
    requested = []
    value = path
    kind = "path"
    if source == "lazy":
        # Apertura e schema validi: il decoder PyArrow viene chiamato solo
        # mentre il binding legge. La spia registra la lettura originale,
        # prima della validazione e dell'esportazione C.
        reader = (pa.ipc.open_stream if stream else pa.ipc.open_file)(path)

        def batches():
            requested.append(True)
            batch = reader.read_next_batch() if stream else reader.get_batch(0)
            delivered.append(True)
            yield batch

        value = pa.RecordBatchReader.from_batches(reader.schema, batches())
        kind = "arrow"
    output = tmp_path / "non-consegnato.arrow"
    with pytest.raises(pd.PlenoraDataMappingError) as error:
        if surface == "native":
            if operation == "describe":
                _native.describe(kind, value, None, None, [])
            elif operation == "validate":
                _native.validate("json", json.dumps(piano_identita()), [("t", kind, value)], None, None, [])
            else:
                _native.run("json", json.dumps(piano_identita()), [("t", kind, value)], [("t", output)], False, None, None, [])
        else:
            function = getattr(pd, ("a" if surface == "async" else "") + operation)
            if operation == "describe":
                result = function(value)
            elif operation == "validate":
                result = function(piano_identita(), {"t": value})
            else:
                result = function(piano_identita(), {"t": value}, outputs={"t": output})
            if surface == "async":
                asyncio.run(result)
    assert (error.value.category, error.value.phase) == ("data_mapping", "read")
    assert MARKER not in json.dumps(error.value.to_dict())
    if source == "path":
        prefix = "" if operation == "describe" else "input `t`: "
        assert str(error.value) == prefix + "arrow error: invalid_argument"
    else:
        assert requested == [True]
        assert "stream Arrow non leggibile (C Data Interface)" in str(error.value)
    assert delivered == ([True] if source == "lazy" else [])
    assert not output.exists()


def test_reader_validante_non_esporta_bitmap_corta() -> None:
    from plenora_data._arrow import prepara

    path = Path(__file__).parent / "fixtures" / "struct-short-bitmap.arrow"
    reader = pa.ipc.open_file(path)
    original = pa.RecordBatchReader.from_batches(reader.schema, [reader.get_batch(0)])
    guarded = prepara(original)
    with pytest.raises(pa.ArrowInvalid):
        guarded.read_next_batch()
