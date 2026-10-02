"""Gli errori: gerarchia, assi di `plenora-error-v1` come campi, diagnostica
per riga, budget, nessun dato e nessuna eccezione nativa al confine."""

from __future__ import annotations

import json
import pathlib
import pickle
from collections.abc import Callable
from typing import Any

import pyarrow as pa
import pyarrow.feather as feather
import pytest

import plenora_data as pd
from aiuti import CANARINO, piano_identita, tabella_semplice

CATEGORIE = {
    "invalid_plan": pd.PlenoraInvalidPlanError,
    "invalid_configuration": pd.PlenoraInvalidConfigurationError,
    "schema": pd.PlenoraSchemaError,
    "data_mapping": pd.PlenoraDataMappingError,
    "crs": pd.PlenoraCrsError,
    "unsupported": pd.PlenoraUnsupportedError,
    "not_found": pd.PlenoraNotFoundError,
    "conflict": pd.PlenoraConflictError,
    "concurrent_modification": pd.PlenoraConcurrentModificationError,
    "authentication": pd.PlenoraAuthenticationError,
    "authorization": pd.PlenoraAuthorizationError,
    "timeout": pd.PlenoraTimeoutError,
    "cancelled": pd.PlenoraCancelledError,
    "resource_limit": pd.PlenoraResourceLimitError,
    "io": pd.PlenoraIoError,
    "protocol": pd.PlenoraProtocolError,
    "transient": pd.PlenoraTransientError,
    "execution": pd.PlenoraExecutionError,
    "internal": pd.PlenoraInternalError,
}


def verifica(errore: pd.PlenoraError, valida: Callable[[str, object], None]) -> dict[str, Any]:
    """Il documento dell'errore: valido per error-v1, nei limiti di ERR-011,
    coerente con la classe e con gli attributi."""
    documento = errore.to_dict()
    valida("error-v1.schema.json", documento)
    assert len(json.dumps(documento, separators=(",", ":")).encode()) <= 524_288
    assert type(errore) is CATEGORIE[errore.category]
    assert str(errore) == errore.message == documento["message"]
    assert (errore.category, errore.phase, errore.remote_effect) == (
        documento["category"],
        documento["phase"],
        documento["remote_effect"],
    )
    assert errore.retry == documento["retry"]
    if errore.row_diagnostics is not None:
        valida("row-diagnostics-v1.schema.json", errore.row_diagnostics)
    return documento


def test_la_gerarchia_ha_una_classe_per_categoria(contratti: pathlib.Path) -> None:
    schema = json.loads((contratti / "error-v1.schema.json").read_text(encoding="utf-8"))
    assert set(CATEGORIE) == set(schema["properties"]["category"]["enum"])
    for classe in CATEGORIE.values():
        assert issubclass(classe, pd.PlenoraError)
    assert issubclass(pd.PlenoraError, RuntimeError)


def test_un_piano_non_valido(valida: Callable[[str, object], None]) -> None:
    with pytest.raises(pd.PlenoraInvalidPlanError) as errore:
        pd.validate("{non json", {})
    documento = verifica(errore.value, valida)
    assert documento["remote_effect"] == "none"
    assert documento["retry"] == {"kind": "never"}
    with pytest.raises(pd.PlenoraInvalidPlanError):
        pd.run({"version": 4, "inputs": [], "steps": [], "outputs": []})


def test_la_diagnostica_per_riga_e_strutturata_e_senza_valori(
    valida: Callable[[str, object], None],
) -> None:
    tabella = pa.table({"a": pa.array([4.0, 7.0, 9.0]), "b": pa.array([2.0, 0.0, 0.0])})
    piano = {
        "version": 1,
        "inputs": ["t"],
        "steps": [
            {
                "out": "q",
                "op": "table.formula",
                "in": ["t"],
                "config": {"formula": "a / b", "new_column": "q", "on_division_by_zero": "error"},
            }
        ],
        "outputs": ["q"],
    }
    with pytest.raises(pd.PlenoraDataMappingError) as errore:
        pd.run(piano, {"t": tabella})
    verifica(errore.value, valida)
    diagnostica = errore.value.row_diagnostics
    assert diagnostica is not None
    assert diagnostica["counts"] == {"evaluation.division_by_zero": 2}
    assert [esempio["source_index"] for esempio in diagnostica["examples"]] == [1, 2]
    testo = json.dumps(errore.value.to_dict()) + repr(errore.value)
    for valore in ("7.0", "9.0", "4.0"):
        assert valore not in testo


def test_il_budget_del_piano_vale_anche_per_le_tabelle_in_memoria(
    valida: Callable[[str, object], None],
) -> None:
    piano = {**piano_identita(), "limits": {"max_governed_memory_bytes": 64}}
    for operazione in (pd.validate, pd.run):
        with pytest.raises(pd.PlenoraResourceLimitError) as errore:
            operazione(piano, {"t": tabella_semplice()})  # type: ignore[operator]
        documento = verifica(errore.value, valida)
        assert documento["phase"] == "read"
        assert "input `t`" in documento["message"]


def test_i_messaggi_non_portano_dati_ne_percorsi(
    tmp_path: pathlib.Path, valida: Callable[[str, object], None]
) -> None:
    cartella = tmp_path / CANARINO
    cartella.mkdir()
    assente = cartella / "assente.arrow"
    with pytest.raises(pd.PlenoraNotFoundError) as errore:
        pd.describe(assente)
    verifica(errore.value, valida)
    assert CANARINO not in repr(errore.value) + json.dumps(errore.value.to_dict())
    # Un percorso esistente come destinazione senza overwrite.
    destinazione = cartella / "uscita.arrow"
    feather.write_feather(tabella_semplice(), destinazione)
    with pytest.raises(pd.PlenoraConflictError) as conflitto:
        pd.run(piano_identita(), {"t": tabella_semplice()}, outputs={"t": destinazione})
    verifica(conflitto.value, valida)
    assert CANARINO not in json.dumps(conflitto.value.to_dict())
    # Un valore di cella in una colonna che il piano non trova.
    tabella = pa.table({"x": pa.array([CANARINO])})
    piano = {
        "version": 1,
        "inputs": ["t"],
        "steps": [
            {
                "out": "u",
                "op": "table.filter",
                "in": ["t"],
                "config": {"column": "manca", "operator": "==", "value": 1},
            }
        ],
        "outputs": ["u"],
    }
    with pytest.raises(pd.PlenoraError) as valore:
        pd.run(piano, {"t": tabella})
    verifica(valore.value, valida)
    assert CANARINO not in json.dumps(valore.value.to_dict())


class _ProduttoreRotto:
    """Un oggetto Arrow il cui stream fallisce con un testo che non deve
    uscire."""

    def __arrow_c_stream__(self, requested_schema: object | None = None) -> object:
        raise ValueError(CANARINO)


def test_le_eccezioni_dei_produttori_non_attraversano_il_confine(
    valida: Callable[[str, object], None],
) -> None:
    with pytest.raises(pd.PlenoraDataMappingError) as errore:
        pd.describe(_ProduttoreRotto())
    verifica(errore.value, valida)
    assert errore.value.phase == "read"
    assert CANARINO not in repr(errore.value) + str(errore.value)
    assert errore.value.__cause__ is None


def test_argomenti_non_validi(valida: Callable[[str, object], None]) -> None:
    casi: list[Callable[[], object]] = [
        lambda: pd.describe(42),  # type: ignore[arg-type]
        lambda: pd.validate(42),  # type: ignore[arg-type]
        lambda: pd.validate({"a": float("nan")}),
        lambda: pd.run(piano_identita(), [tabella_semplice()]),  # type: ignore[arg-type]
        lambda: pd.run(piano_identita(), {1: tabella_semplice()}),  # type: ignore[dict-item]
        lambda: pd.run(piano_identita(), {"t": tabella_semplice()}, overwrite=True),
        lambda: pd.run(piano_identita(), {"t": tabella_semplice()}, overwrite=1),  # type: ignore[arg-type]
        lambda: pd.run(piano_identita(), {"t": tabella_semplice()}, outputs={"t": 3}),  # type: ignore[dict-item]
        lambda: pd.describe(tabella_semplice(), cancel=object()),  # type: ignore[arg-type]
    ]
    for caso in casi:
        with pytest.raises(pd.PlenoraInvalidConfigurationError) as errore:
            caso()
        documento = verifica(errore.value, valida)
        assert documento["phase"] == "prepare"
        assert documento["remote_effect"] == "none"


def test_le_eccezioni_si_ricostruiscono_con_pickle(valida: Callable[[str, object], None]) -> None:
    with pytest.raises(pd.PlenoraInvalidPlanError) as errore:
        pd.validate("[]", {})
    copia = pickle.loads(pickle.dumps(errore.value))
    assert type(copia) is pd.PlenoraInvalidPlanError
    assert copia.to_dict() == errore.value.to_dict()
    verifica(copia, valida)
