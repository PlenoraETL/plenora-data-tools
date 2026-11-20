"""Round-trip reale IO corrente -> data -> IO -> data, con footer PyArrow.

Richiede i due binari gia' compilati e PyArrow; non modifica il checkout IO.
La cartella di lavoro deve essere nuova. Il covering letto resta una colonna
ordinaria in data-tools: la rinomina esplicita evita il nome riservato di IO.
"""

import argparse
import hashlib
import json
from pathlib import Path
import subprocess

from campagna import raccogli, fallita
import strict_json


def prepara(work):
    """Ingresso indipendente dal produttore, secondo il contratto attuale."""
    import pyarrow as pa
    import pyarrow.ipc as ipc
    from verifica_interop_crs import sorgente
    root = Path(__file__).resolve().parents[1]
    proj = strict_json.loads((root / "crates/plenora-io/data/projjson_integrati.json").read_text("utf-8"))
    fields, arrays = [], []
    for index, (name, crs, axes, points) in enumerate([
        ("traccia", "EPSG:32632", "easting_northing", [(500000., 4980000.), (500100., 4980100.)]),
        ("posizione", "OGC:CRS84", "lon_lat", [(12., 45.), (13., 46.)]),
    ]):
        path = work / (name + ".arrow")
        sorgente(path, json.dumps(proj[crs]), "projjson", axes, points)
        with ipc.open_file(path) as reader:
            table = reader.read_all()
        metadata = dict(table.schema.field(0).metadata)
        metadata[b"plenora.field_id"] = str(index).encode()
        fields.append(pa.field(name, pa.binary(), nullable=True, metadata=metadata))
        values = table.column(0).to_pylist()
        arrays.append(pa.array([values[0], None if index == 0 else values[1],
                                values[1] if index == 0 else None], pa.binary()))
    schema = pa.schema(fields, metadata={b"plenora.contract.version": b"1",
                                         b"plenora.primary_geometry": b"1"})
    source = work / "source.arrow"
    with ipc.new_file(source, schema) as writer:
        writer.write_table(pa.Table.from_arrays(arrays, schema=schema))
    return source


def verifica_tabella(original, table, geo):
    from verifica_interop_crs import json_esatto, json_uguali
    if table.num_rows != original.num_rows:
        raise ValueError("numero di righe diverso")
    if geo["primary_column"] != "posizione" or set(geo["columns"]) != set(original.column_names):
        raise ValueError("primaria o geometrie diverse")
    for name in original.column_names:
        if table[name].to_pylist() != original[name].to_pylist():
            raise ValueError("valori geometrici diversi")
        column = geo["columns"][name]
        expected = original.schema.field(name).metadata[b"plenora.geometry.crs_definition"]
        if not json_uguali(column["crs"], json_esatto(expected)):
            raise ValueError("CRS diverso dall'ingresso")
        if column["encoding"] != "WKB" or column["geometry_types"] != ["Point"]:
            raise ValueError("encoding o tipi geometrici diversi")
        for key in [b"plenora.field_id", b"plenora.geometry.axis_order", b"plenora.geometry.dimensions"]:
            if table.schema.field(name).metadata[key] != original.schema.field(name).metadata[key]:
                raise ValueError("metadati geometrici diversi")


def esegui_catena(args) -> int:
    import pyarrow
    import pyarrow.ipc as ipc
    import pyarrow.parquet as pq
    work = args.work.resolve()
    provenance = strict_json.loads(args.pin.read_text(encoding="utf-8"))
    source = prepara(work)
    fixture = work / "io-current.parquet"
    plan = work / "rename.json"
    plan.write_text(json.dumps({
        "version": 1, "inputs": ["t"], "steps": [{
            "op": "table.rename", "in": ["t"], "out": "r",
            "config": {"renames": [{"old_name": "bbox", "new_name": "bbox_origine"}]},
        }], "outputs": ["r"],
    }), encoding="utf-8")
    identity = work / "identity.json"
    identity.write_text(json.dumps({
        "version": 1, "inputs": ["t"], "steps": [], "outputs": ["t"],
    }), encoding="utf-8")
    paths = [source, fixture, work / "data.parquet", work / "io.parquet", work / "data-final.parquet"]
    commands = [
        [str(args.io.resolve()), "convert", str(source), str(fixture), "--from", "ipc", "--to", "geoparquet"],
        [str(args.data.resolve()), "run", "--plan", str(plan), "--input", f"t={paths[1]}", "--output", f"r={paths[2]}"],
        [str(args.io.resolve()), "convert", str(paths[2]), str(paths[3]), "--from", "geoparquet", "--to", "geoparquet"],
        [str(args.data.resolve()), "run", "--plan", str(identity), "--input", f"t={paths[3]}", "--output", f"t={paths[4]}"],
    ]
    results = []
    for index, command in enumerate(commands):
        if not paths[index].is_file() or (results and results[-1]["stato"] != "superato"):
            results.append({"caso": f"comando-{index}", "stato": "bloccato"})
            continue
        def execute(_):
            result = subprocess.run(command, capture_output=True, text=True, check=True, timeout=120)
            envelope = strict_json.loads(result.stdout)
            if envelope["status"] != "ok":
                raise ValueError("comando senza esito positivo")
            if command[0] == str(args.io.resolve()):
                if envelope["component_version"] != provenance["version"]:
                    raise ValueError("versione IO diversa dalla release")
        results.extend(raccogli([f"comando-{index}"], execute))
    with ipc.open_file(source) as reader:
        original = reader.read_all()
    from verifica_interop_crs import json_esatto
    def verify(path):
        table = pq.read_table(path)
        geo = json_esatto(pq.read_metadata(path).metadata[b"geo"])
        verifica_tabella(original, table, geo)
        if path != fixture:
            if table["bbox_origine"].to_pylist() != pq.read_table(fixture)["bbox"].to_pylist():
                raise ValueError("covering originale diverso")
        return {"colonne_geometriche": sorted(original.column_names), "righe": original.num_rows}
    results.extend(raccogli([str(p) for p in paths[1:]], lambda name: verify(Path(name))))
    report = {
        "results": results, "stato": "fallito" if fallita(results) else "superato",
        "commands": commands, "reader": f"PyArrow {pyarrow.__version__}",
        "io_release": provenance,
        "sha256": {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in paths if p.is_file()},
    }
    (work / "report.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print("IO -> data -> IO -> data e footer PyArrow: " + report["stato"])
    return int(fallita(results))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--io", required=True, type=Path)
    parser.add_argument("--data", required=True, type=Path)
    parser.add_argument("--work", required=True, type=Path)
    parser.add_argument("--pin", required=True, type=Path)
    args = parser.parse_args()
    args.work.mkdir(parents=True, exist_ok=False)
    try:
        return esegui_catena(args)
    except Exception as error:
        report = {"stato": "fallito", "preparazione": type(error).__name__}
        (args.work / "report.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
        print("multigeometria: preparazione fallita")
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
