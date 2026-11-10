"""Matrice degli ingressi Arrow, prima e dopo il confine C."""

import asyncio
import json
from pathlib import Path

import pyarrow as pa
import pytest

import plenora_data as pd
from plenora_data import _native
from aiuti import piano_identita


CASES = sorted((Path(__file__).parent / "fixtures" / "arrow_import").glob("*.arrow"))


CAPSULE_CASES = {"list-offset-oob", "list-offset-negative", "map-offset-oob", "dictionary-oob", "projection-list", "struct-children-different"}


@pytest.mark.parametrize("case,form", [
    (case, form) for case in CASES
    for form in ("batch", "table", "lazy", "capsule_stream", "capsule_array")
    if not form.startswith("capsule") or case.stem in CAPSULE_CASES
], ids=lambda item: item.stem if isinstance(item, Path) else item)
@pytest.mark.parametrize("surface", ["native", "sdk", "async"])
@pytest.mark.parametrize("operation", ["describe", "validate", "run"])
def test_originale_rifiutato(case, form, surface, operation, tmp_path):
    reader = pa.ipc.open_file(case)
    if form == "lazy":
        def batches():
            yield reader.get_batch(0)
        value = pa.RecordBatchReader.from_batches(reader.schema, batches())
    else:
        value = reader.get_batch(0)
        if form == "table":
            value = pa.Table.from_batches([value])
    if form.startswith("capsule"):
        batch = value
        class Stream:
            def __arrow_c_stream__(self, requested_schema=None):
                return batch.__arrow_c_stream__(requested_schema)
        class Array:
            def __arrow_c_array__(self, requested_schema=None):
                return batch.__arrow_c_array__(requested_schema)
        value = Stream() if form == "capsule_stream" else Array()
    output = tmp_path / "non-scritto.arrow"
    with pytest.raises(pd.PlenoraDataMappingError) as caught:
        if surface == "native":
            if operation == "describe":
                _native.describe("arrow", value, None, None, [])
            elif operation == "validate":
                _native.validate("json", json.dumps(piano_identita()), [("t", "arrow", value)], None, None, [])
            else:
                _native.run("json", json.dumps(piano_identita()), [("t", "arrow", value)], [("t", output)], False, None, None, [])
        else:
            fn = getattr(pd, ("a" if surface == "async" else "") + operation)
            if operation == "describe":
                result = fn(value)
            elif operation == "validate":
                result = fn(piano_identita(), {"t": value})
            else:
                result = fn(piano_identita(), {"t": value}, outputs={"t": output})
            if surface == "async":
                asyncio.run(result)
    assert (caught.value.category, caught.value.phase) == ("data_mapping", "read")
    context = "input" if operation == "describe" else "input `t`"
    assert str(caught.value) in {
        f"{context}: {reason}" for reason in (
            "l'oggetto non da' uno stream Arrow (Arrow PyCapsule Interface, `__arrow_c_stream__`)",
            "stream Arrow non leggibile (C Data Interface)",
            "array Arrow non valido dopo l'import",
        )
    }
    assert not output.exists()


@pytest.mark.parametrize("case", ["list-offset-oob", "map-offset-oob", "dictionary-oob"])
def test_capsula_terza_validazione_rust(case):
    batch = pa.ipc.open_file(Path(__file__).parent / "fixtures" / "arrow_import" / f"{case}.arrow").get_batch(0)

    class Producer:
        def __arrow_c_stream__(self, requested_schema=None):
            return pa.Table.from_batches([batch]).__arrow_c_stream__(requested_schema)

    with pytest.raises(pd.PlenoraDataMappingError) as caught:
        pd.describe(Producer())
    assert (caught.value.category, caught.value.phase) == ("data_mapping", "read")


def _registro_python(surface, module, registered=None):
    import inspect
    import os
    import re
    import sys
    # Il verificatore wheel copia i test fuori checkout; la provenienza
    # dei contratti continua a indicare il checkout esaminato.
    contracts = os.environ.get("PLENORA_DATA_CONTRATTI")
    root = Path(contracts).resolve().parents[4] if contracts else Path(__file__).resolve().parents[4]
    sys.path.insert(0, str(root / "scripts"))
    from check_arrow_ingressi import REGISTRY, check_members, python_inventory, check_python_sources
    check_python_sources()
    if registered is None:
        registered = {key.split("::", 1)[1]: value
                      for key, value in json.loads(REGISTRY.read_text(encoding="utf-8"))["python"].items()
                      if key.startswith(surface + "::")}
    actual = python_inventory(module)
    check_members(actual, registered)
    for name, function in actual.items():
        entry = registered[name]
        signature_text = str(inspect.signature(function))
        assert signature_text == entry["signature"]
        assert entry["reason"]
        assert entry["classification"] in {"ingresso guardato", "non riceve dati Arrow esterni", "helper con precondizione dichiarata"}
        if entry["classification"] == "helper con precondizione dichiarata":
            assert entry["precondition"]
        if entry["classification"] == "non riceve dati Arrow esterni":
            import dataclasses
            if entry["kind"] == "output_container":
                assert dataclasses.is_dataclass(getattr(module, name.split(".")[0]))
            elif entry["kind"] == "protocol":
                assert getattr(module, name.split(".")[0])._is_protocol
            else:
                assert entry["kind"] == "control"
                signature = inspect.signature(function)
                parameters = list(signature.parameters.values())
                if "." in name:
                    parameters = parameters[1:]  # ricevitore del metodo
                assert all(param.annotation is not inspect.Parameter.empty for param in parameters)
                annotations = [str(param.annotation).strip("'") for param in signature.parameters.values()]
                assert not any(re.search(r"\b(?:Source|Table|RecordBatch|Array|ArrayData|ChunkedArray|RecordBatchReader)\b", annotation) for annotation in annotations)
    return registered


@pytest.mark.parametrize("surface", ["sdk", "native"])
def test_enumerazione_ingressi_pubblici(monkeypatch, surface):
    from plenora_data import _arrow
    module = pd if surface == "sdk" else _native
    registered = _registro_python(surface, module)
    original = _arrow.prepara
    seen = []
    def spy(value):
        seen.append(value)
        return original(value)
    monkeypatch.setattr(_arrow, "prepara", spy)
    batch = pa.record_batch({"value": [1, None, 3]})
    for name, entry in registered.items():
        if entry["classification"] != "ingresso guardato":
            continue
        before = len(seen)
        fn = getattr(module, name)
        operation = entry["operation"]
        if surface == "sdk":
            result = fn(batch) if operation == "describe" else fn(piano_identita(), {"t": batch})
            if name.startswith("a"):
                asyncio.run(result)
        elif operation == "describe":
            fn("arrow", batch, None, None, [])
        elif operation == "validate":
            fn("json", json.dumps(piano_identita()), [("t", "arrow", batch)], None, None, [])
        else:
            fn("json", json.dumps(piano_identita()), [("t", "arrow", batch)], None, False, None, None, [])
        assert len(seen) == before + 1
        assert seen[-1] is batch


@pytest.mark.parametrize("surface", ["sdk", "native"])
@pytest.mark.parametrize("asynchronous", [False, True])
def test_ingest_non_classificato_fallisce(monkeypatch, surface, asynchronous):
    module = pd if surface == "sdk" else _native
    def ingest(data):
        return data.to_batches()
    if asynchronous:
        async def ingest(data):
            return data.to_batches()
    monkeypatch.setattr(module, "ingest", ingest, raising=False)
    with pytest.raises(AssertionError, match="non classificati.*ingest"):
        _registro_python(surface, module)


@pytest.mark.parametrize("surface", ["sdk", "native"])
@pytest.mark.parametrize("asynchronous", [False, True])
def test_metodo_non_classificato_fallisce(monkeypatch, surface, asynchronous):
    module = pd if surface == "sdk" else _native
    class Reader:
        def ingest(self, data):
            return data.to_batches()
    if asynchronous:
        async def ingest(self, data):
            return data.to_batches()
        Reader.ingest = ingest
    monkeypatch.setattr(module, "Reader", Reader, raising=False)
    with pytest.raises(AssertionError, match="non classificati.*Reader.ingest"):
        _registro_python(surface, module)


def test_secondo_batch_corrotto_non_consegnato():
    from plenora_data._arrow import prepara
    path = Path(__file__).parent / "fixtures" / "arrow_import" / "dictionary-oob.arrow"
    bad = pa.ipc.open_file(path).get_batch(0)
    good = bad.slice(0, 1)
    guarded = prepara(pa.RecordBatchReader.from_batches(bad.schema, [good, bad]))
    assert guarded.read_next_batch().equals(good)
    with pytest.raises(pa.ArrowInvalid):
        guarded.read_next_batch()


@pytest.mark.parametrize("surface", ["native", "sdk", "async"])
@pytest.mark.parametrize("operation", ["describe", "validate", "run"])
@pytest.mark.parametrize("form", ["batch", "table", "lazy", "capsule_stream", "capsule_array"])
def test_ree_malformato(surface, operation, form, tmp_path):
    array = pa.RunEndEncodedArray.from_arrays(pa.array([1000003, 2000007], pa.int32()), pa.array([7, 9]))
    batch = pa.record_batch([array], names=["v"])
    sink = pa.BufferOutputStream()
    with pa.ipc.new_file(sink, batch.schema) as writer:
        writer.write_batch(batch)
    data = sink.getvalue().to_pybytes()
    ends = (1000003).to_bytes(4, "little") + (2000007).to_bytes(4, "little")
    assert data.count(ends) == 1
    data = data.replace(ends, ends[:4] + (2000006).to_bytes(4, "little"))
    batch = pa.ipc.open_file(pa.BufferReader(data)).get_batch(0)
    with pytest.raises(pa.ArrowInvalid):
        batch.validate(full=True)
    if form == "table":
        value = pa.Table.from_batches([batch])
    elif form == "lazy":
        value = pa.RecordBatchReader.from_batches(batch.schema, [batch])
    elif form == "capsule_stream":
        class Stream:
            def __arrow_c_stream__(self, requested_schema=None):
                return batch.__arrow_c_stream__(requested_schema)
        value = Stream()
    elif form == "capsule_array":
        class Array:
            def __arrow_c_array__(self, requested_schema=None):
                return batch.__arrow_c_array__(requested_schema)
        value = Array()
    else:
        value = batch
    error = pd.PlenoraDataMappingError if form in {"batch", "table", "capsule_array"} else pd.PlenoraUnsupportedError
    output = tmp_path / "non-scritto.arrow"
    with pytest.raises(error) as caught:
        if surface == "native":
            if operation == "describe":
                _native.describe("arrow", value, None, None, [])
            elif operation == "validate":
                _native.validate("json", json.dumps(piano_identita()), [("t", "arrow", value)], None, None, [])
            else:
                _native.run("json", json.dumps(piano_identita()), [("t", "arrow", value)], [("t", output)], False, None, None, [])
        else:
            fn = getattr(pd, ("a" if surface == "async" else "") + operation)
            if operation == "describe":
                result = fn(value)
            elif operation == "validate":
                result = fn(piano_identita(), {"t": value})
            else:
                result = fn(piano_identita(), {"t": value}, outputs={"t": output})
            if surface == "async":
                asyncio.run(result)
    assert caught.value.phase == "read"
    assert not output.exists()


@pytest.mark.parametrize("form", ["batch", "table", "lazy", "capsule_stream", "capsule_array"])
@pytest.mark.parametrize("asynchronous", [False, True])
def test_batch_validi_invariati(form, asynchronous):
    batch = pa.record_batch({"value": [1, None, 3], "label": ["a", "b", None]})
    class Stream:
        def __arrow_c_stream__(self, requested_schema=None):
            return batch.__arrow_c_stream__(requested_schema)
    class Array:
        def __arrow_c_array__(self, requested_schema=None):
            return batch.__arrow_c_array__(requested_schema)
    sources = {
        "batch": batch,
        "table": pa.Table.from_batches([batch]),
        "lazy": pa.RecordBatchReader.from_batches(batch.schema, [batch]),
        "capsule_stream": Stream(),
        "capsule_array": Array(),
    }
    fn = pd.arun if asynchronous else pd.run
    result = fn(piano_identita(), {"t": sources[form]})
    if asynchronous:
        result = asyncio.run(result)
    assert result.tables["t"].to_pydict() == batch.to_pydict()


@pytest.mark.parametrize("surface,name", [("sdk", "run"), ("native", "describe")])
def test_riclassificazione_in_controllo_fallisce(surface, name):
    module = pd if surface == "sdk" else _native
    registered = _registro_python(surface, module)
    registered[name] = {**registered[name], "classification": "non riceve dati Arrow esterni", "kind": "control"}
    with pytest.raises(AssertionError):
        _registro_python(surface, module, registered)
