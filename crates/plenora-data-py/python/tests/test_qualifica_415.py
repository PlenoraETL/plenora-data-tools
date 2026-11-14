import json
"""B-08..10: gli stessi vettori attraversano run e arun, senza normalizzare metadata."""
import asyncio
import struct
import traceback

import pyarrow as pa
import pytest
import plenora_data as pd


def table(extension, geography, conflict=False):
    md = {
        "plenora.field_id": "3", "ARROW:extension:name": "geoarrow.wkb",
        "plenora.geometry.encoding": "wkb", "plenora.geometry.dimensions": "xy",
        "plenora.geometry.types_declaration": "exact", "plenora.geometry.types": "polygon",
        "plenora.geometry.crs_resolution": "resolved", "plenora.geometry.crs_id": "EPSG:32632",
        "plenora.geometry.srid": "32632", "plenora.geometry.axis_order": "easting_northing", "plenora.geometry.precision": "float64",
        "plenora.geometry.spatial_semantics": "geography" if geography else "geometry",
        "plenora.postgres.native_declaration": "geometry(Polygon,32632)",
        "plenora.postgres.future": "SENTINEL_415",
    }
    if extension:
        crs = "EPSG:4326" if conflict else "EPSG:32632"
        edges = "spherical" if geography else "planar"
        md["ARROW:extension:metadata"] = f'{{ "crs_type" : "authority_code", "crs" : "{crs}", "edges" : "{edges}" }}'
    outer = [(0,0),(40,0),(40,40),(0,40),(0,0)]
    hole = [(10,10),(10,20),(20,20),(20,10),(10,10)]
    triangle = [(0,0),(10,0),(10,10),(0,0)]
    def polygon(rings):
        return struct.pack("<BII", 1, 3, len(rings)) + b"".join(
            struct.pack("<I", len(ring)) + b"".join(struct.pack("<dd", 500000+x, 4650000+y) for x,y in ring) for ring in rings)
    values = [polygon([outer, hole]), polygon([triangle]), None]
    schema = pa.schema([
        pa.field("id", pa.int64(), False, metadata={"plenora.field_id": "1", "plenora.postgres.native_type": "int8"}),
        pa.field("g", pa.binary(), True, metadata=md),
    ], metadata={"plenora.contract.version": "1"})
    return pa.Table.from_arrays([pa.array([1, 2, 3]), pa.array(values)], schema=schema)


def plan(compute):
    return {"version": 1, "inputs": ["t"], "steps": [
        {"out": "o", "op": "geo.centroid", "in": ["t"], "config": {}}
    ] if compute else [], "outputs": ["o" if compute else "t"]}


@pytest.mark.parametrize("async_mode", [False, True])
@pytest.mark.parametrize("extension", [False, True])
@pytest.mark.parametrize("geography", [False, True])
@pytest.mark.parametrize("compute", [False, True])
def test_parity(async_mode, extension, geography, compute):
    source = table(extension, geography)
    def run():
        return asyncio.run(pd.arun(plan(compute), {"t": source})) if async_mode else pd.run(plan(compute), {"t": source})
    if geography and compute:
        with pytest.raises(pd.PlenoraUnsupportedError) as caught:
            run()
        check_error(caught.value, "unsupported")
    else:
        output = run().tables["o" if compute else "t"]
        if not compute:
            assert source.equals(output, check_metadata=True)
        else:
            # H-07: ordine float64 pesato dichiarato, senza epsilon o golden.
            hole = lambda base: ((base+20.)*1600. - (base+15.)*100.)/1500.
            triangle = lambda base, offset: ((base+offset/3.)*100.)/100.
            expected = [struct.pack("<BIdd", 1, 1, hole(500000.), hole(4650000.)),
                        struct.pack("<BIdd", 1, 1, triangle(500000.,20.), triangle(4650000.,10.)), None]
            assert output["g"].to_pylist() == expected
            assert source.schema.field("id").equals(output.schema.field("id"), check_metadata=True)
            md = output.schema.field("g").metadata
            assert md[b"plenora.field_id"] == b"3"
            assert md[b"plenora.geometry.types"] == b"point"
            assert not any(k.startswith(b"plenora.postgres.") for k in md)


def check_error(error, category):
    assert (error.category, error.phase, error.remote_effect, error.retry) == (
        category, "validate", "none", {"kind": "never"})
    assert "SENTINEL_415" not in "".join(traceback.format_exception(error))


@pytest.mark.parametrize("async_mode", [False, True])
@pytest.mark.parametrize("raw", ["", "{}", " { } ", '{"crs_type":"authority_code","crs":"epsg:32632"}',
                                '{"crs_type":"authority_code","crs":42}'])
def test_extension_shapes_on_published_rows(async_mode, raw, tmp_path):
    source = table(False, False)
    field = source.schema.field("g")
    md = dict(field.metadata)
    md[b"ARROW:extension:metadata"] = raw.encode()
    source = source.cast(source.schema.set(1, field.with_metadata(md)))
    pipeline = {"version": 1, "inputs": ["t"], "steps": [
        {"out": "o", "op": "table.limit", "in": ["t"], "config": {"n": 3}}
    ], "outputs": ["o"]}
    destination = tmp_path / "out.arrow"
    if "42" in raw:
        with pytest.raises(pd.PlenoraCrsError) as caught:
            if async_mode:
                asyncio.run(pd.arun(pipeline, {"t": source}, outputs={"o": destination}))
            else:
                pd.run(pipeline, {"t": source}, outputs={"o": destination})
        check_error(caught.value, "crs")
        assert not destination.exists()
    else:
        result = asyncio.run(pd.arun(pipeline, {"t": source})) if async_mode else pd.run(pipeline, {"t": source})
        assert result.tables["o"].equals(source, check_metadata=True)


@pytest.mark.parametrize("async_mode", [False, True])
def test_conflict_and_cancellation_publish_nothing(async_mode, tmp_path):
    destination = tmp_path / "out.arrow"
    source = table(True, False, conflict=True)
    with pytest.raises(pd.PlenoraCrsError) as caught:
        if async_mode:
            asyncio.run(pd.arun(plan(False), {"t": source}, outputs={"t": destination}))
        else:
            pd.run(plan(False), {"t": source}, outputs={"t": destination})
    check_error(caught.value, "crs")
    assert not destination.exists()
    token = pd.CancellationToken()
    token.cancel()
    with pytest.raises(pd.PlenoraCancelledError) as caught:
        if async_mode:
            asyncio.run(pd.arun(plan(False), {"t": table(True, True)}, outputs={"t": destination}, cancel=token))
        else:
            pd.run(plan(False), {"t": table(True, True)}, outputs={"t": destination}, cancel=token)
    assert caught.value.code == "EXECUTION_CANCELLED"
    assert not destination.exists()


@pytest.mark.parametrize("async_mode", [False, True])
@pytest.mark.parametrize("extension", [False, True])
def test_localized_effects_c01(async_mode, extension):
    source = table(extension, False)
    result = asyncio.run(pd.arun(plan(True), {"t": source})) if async_mode else pd.run(plan(True), {"t": source})
    report = result.result
    assert report["effects_contract"] == "plenora-data-localized-effects-v1"
    effects = report["steps"][0]["effects"]
    for key in ["/plenora.postgres.native_declaration", "/plenora.postgres.future"]:
        assert any(e["field_index"] == 1 and e["input_index"] == 0
                   and e["source_field_index"] == 1 and e["key"] == key
                   and e["action"] == "metadata_invalidated" for e in effects)
    assert any(e["field_index"] == 1 and e["action"] == "derived" for e in effects)
    assert any(e["field_index"] == 1 and e["key"] == "/plenora.geometry.types"
               and e["action"] == "metadata_reconstructed" for e in effects)
    assert not any(e["field_index"] == 0 for e in effects)
    assert "SENTINEL_415" not in json.dumps(report)
