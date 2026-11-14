"""Parita completa Rust/CLI/runtime/SDK sync+async sul corpus delle schede.

Prima: cargo run -p plenora-io --example esporta_parita -- target/parita
       cargo build -p plenora-cli --bin plenora-data --example parita_superfici
       target/debug/examples/parita_superfici target/parita target/debug/plenora-data
Poi, con il wheel installato: python scripts/verifica_parita_superfici.py target/parita
Fallisce se manca una sola operazione: nessun salto. UUID casuali confrontati per
schema, cardinalita e UUID validi, come dichiarato dalla scheda.
"""
import asyncio
import json
import os
from pathlib import Path
import sys
import uuid

import plenora_data as pd
import pyarrow as pa


def read_table(path):
    with pa.memory_map(str(path), "r") as source:
        return pa.ipc.open_file(source).read_all()


def main():
    root = Path(sys.argv[1]).resolve()
    os.environ["PLENORA_ESEMPIO_CHIAVE_HMAC"] = "chiave-di-esempio"
    cases = json.loads((root / "cases.json").read_text(encoding="utf-8"))
    # Il produttore Rust verifica gia' l'uguaglianza con CATALOG. Qui si
    # confronta anche il documento che il wheel effettivamente pubblica.
    catalog = pd.catalog()
    (root / "sdk-catalog.json").write_text(json.dumps(catalog), encoding="utf-8")
    assert {c["operation"] for c in cases} == {k["id"] for k in catalog["kernels"]}
    count = 0
    for case in cases:
        op = case["operation"]
        folder = root / op
        expected = json.loads((folder / "expected.json").read_text(encoding="utf-8"))
        inputs = {name: read_table(folder / file) for name, file in case["inputs"].items()}
        for async_mode in (False, True):
            try:
                result = asyncio.run(pd.arun(case["plan"], inputs)) if async_mode else pd.run(case["plan"], inputs)
            except pd.PlenoraError as error:
                assert "error" in expected, (op, async_mode, error.category)
                for key in ("category", "phase", "remote_effect", "retry"):
                    assert getattr(error, key) == expected["error"][key], (op, async_mode, key)
            else:
                assert "error" not in expected, (op, async_mode)
                assert result.result["steps"] == expected["report"]["steps"], (op, async_mode, "effects")
                output = result.tables[case["plan"]["outputs"][0]]
                oracle = read_table(folder / "expected.arrow")
                assert output.schema.equals(oracle.schema, check_metadata=True), (op, async_mode, "schema")
                assert output.num_rows == oracle.num_rows, (op, async_mode, "rows")
                if case["compare_values"]:
                    assert output.equals(oracle, check_metadata=True), (op, async_mode, "values")
                else:
                    assert op == "table.uuid_generator"
                    for field in output.schema:
                        if field.name == case["plan"]["steps"][0]["config"]["output_column"]:
                            for value in output[field.name].to_pylist():
                                assert str(uuid.UUID(value)) == value and uuid.UUID(value).version == 4
                        else:
                            assert output[field.name].equals(oracle[field.name]), (op, field.name)
            count += 1
        print(f"{op}: SDK sync async OK", flush=True)
    assert count == 2 * len(cases) and count > 0
    print(f"Parita cinque superfici: {len(cases)} operazioni, {count} esecuzioni SDK, zero salti.")


if __name__ == "__main__":
    main()
