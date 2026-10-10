"""Supporto della suite: i file dei contratti e il loro validatore.

La suite gira contro il wheel installato, fuori dal checkout (la cartella
`tests` non è un package: pytest non mette in `sys.path` la cartella dei
sorgenti `python/`). Gli schemi dei contratti sono le copie byte per byte
di `crates/plenora-cli/tests/fixtures/contratti`, con lo SHA-256 di
`provenienza.json` verificato prima dell'uso: la variabile
`PLENORA_DATA_CONTRATTI` dice dove sono, e senza la suite fallisce invece di
saltare i controlli (un controllo saltato non è un controllo passato).
"""

from __future__ import annotations

import hashlib
import json
import os
import pathlib
from collections.abc import Callable
from typing import Any

import jsonschema
import pytest
from referencing import Registry, Resource

VARIABILE = "PLENORA_DATA_CONTRATTI"
SCHEMI = (
    "error-v1.schema.json",
    "capabilities-v2.schema.json",
    "row-diagnostics-v1.schema.json",
    "surface-bindings-v1.schema.json",
)


def pytest_configure(config: pytest.Config) -> None:
    config.addinivalue_line(
        "markers",
        "sonde: usa le sonde private del modulo nativo, che esistono solo nelle "
        "build delle prove (feature Cargo `sonde-di-prova`); sul wheel di "
        "rilascio scripts/verifica_sdk_python.py --rilascio le deseleziona e "
        "ne verifica l'assenza",
    )


@pytest.fixture(scope="session")
def contratti() -> pathlib.Path:
    valore = os.environ.get(VARIABILE)
    if not valore:
        pytest.fail(
            f"{VARIABILE} non impostata: serve la cartella "
            "crates/plenora-cli/tests/fixtures/contratti del checkout"
        )
    cartella = pathlib.Path(valore)
    provenienza = json.loads((cartella / "provenienza.json").read_text(encoding="utf-8"))
    for nome, (_, atteso) in provenienza["file"].items():
        trovato = hashlib.sha256((cartella / nome).read_bytes()).hexdigest()
        assert trovato == atteso, f"{nome}: SHA-256 diverso dal commit dei contratti"
    return cartella


def leggi(cartella: pathlib.Path, nome: str) -> Any:
    return json.loads((cartella / nome).read_text(encoding="utf-8"))


@pytest.fixture(scope="session")
def valida(contratti: pathlib.Path) -> Callable[[str, object], None]:
    """`valida(nome_schema, istanza)`: jsonschema (draft 2020-12) con i
    riferimenti fra gli schemi dei contratti."""
    schemi = {nome: leggi(contratti, nome) for nome in SCHEMI}
    registro: Registry[Any] = Registry().with_resources(
        (schema["$id"], Resource.from_contents(schema)) for schema in schemi.values()
    )

    def validatore(nome: str, istanza: object) -> None:
        schema = schemi[nome]
        jsonschema.Draft202012Validator.check_schema(schema)
        jsonschema.Draft202012Validator(schema, registry=registro).validate(istanza)

    return validatore
