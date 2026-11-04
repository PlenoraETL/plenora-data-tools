"""Round-trip reale IO 7.0.0 -> data -> IO -> data, con footer PyArrow.

Richiede i due binari gia' compilati e PyArrow; non modifica il checkout IO.
La cartella di lavoro deve essere nuova. Il covering letto resta una colonna
ordinaria in data-tools: la rinomina esplicita evita il nome riservato di IO.
"""

import argparse
import hashlib
import json
from pathlib import Path
import subprocess

import pyarrow
import pyarrow.parquet as pq


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--io", required=True, type=Path)
    parser.add_argument("--data", required=True, type=Path)
    parser.add_argument("--work", required=True, type=Path)
    args = parser.parse_args()
    work = args.work.resolve()
    work.mkdir(parents=True, exist_ok=False)
    root = Path(__file__).resolve().parents[1]
    fixture = root / "crates/plenora-io/tests/dati/io700_multigeometria.parquet"
    provenance = json.loads(fixture.with_suffix(".provenienza.json").read_text(encoding="utf-8"))
    assert hashlib.sha256(fixture.read_bytes()).hexdigest() == provenance["files"][fixture.name]
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
    paths = [fixture, work / "data.parquet", work / "io.parquet", work / "data-final.parquet"]
    commands = [
        [str(args.data.resolve()), "run", "--plan", str(plan), "--input", f"t={paths[0]}", "--output", str(paths[1])],
        [str(args.io.resolve()), "convert", str(paths[1]), str(paths[2]), "--from", "geoparquet", "--to", "geoparquet"],
        [str(args.data.resolve()), "run", "--plan", str(identity), "--input", f"t={paths[2]}", "--output", str(paths[3])],
    ]
    for command in commands:
        result = subprocess.run(command, capture_output=True, text=True, check=True)
        envelope = json.loads(result.stdout)
        assert envelope["status"] == "ok"
        if command[0] == str(args.io.resolve()):
            assert envelope["component_version"] == "7.0.0"
    original = pq.read_table(fixture)
    reference = json.loads(pq.read_metadata(fixture).metadata[b"geo"])
    for path in paths:
        table = pq.read_table(path)
        geo = json.loads(pq.read_metadata(path).metadata[b"geo"])
        assert table.num_rows == 3
        assert geo["primary_column"] == "posizione"
        assert set(geo["columns"]) == {"traccia", "posizione"}
        for name in ["traccia", "posizione"]:
            assert table[name].to_pylist() == original[name].to_pylist()
            for key in ["encoding", "geometry_types", "crs"]:
                assert geo["columns"][name][key] == reference["columns"][name][key]
            for key in [b"plenora.field_id", b"plenora.geometry.axis_order", b"plenora.geometry.dimensions"]:
                assert table.schema.field(name).metadata[key] == original.schema.field(name).metadata[key]
        if path != fixture:
            assert table["bbox_origine"].to_pylist() == original["bbox"].to_pylist()
    report = {
        "commands": commands, "reader": f"PyArrow {pyarrow.__version__}",
        "checks": ["WKB", "null", "rows", "primary", "types", "CRS", "identities", "axes", "dimensions", "renamed covering values"],
        "sha256": {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in paths},
    }
    (work / "report.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print("IO -> data -> IO -> data e footer PyArrow: OK")


if __name__ == "__main__":
    main()
