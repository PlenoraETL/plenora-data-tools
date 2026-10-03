"""Identità del pacchetto, superficie pubblica e scoperta (Python SDK 1.0,
sezioni 2, 3, 7 e 12)."""

from __future__ import annotations

import importlib.metadata
import inspect
import pathlib
from collections.abc import Callable
from typing import Any

import plenora_data as pd
from conftest import leggi

ATTESI = {
    "ArrowStreamExportable",
    "CancellationToken",
    "Destination",
    "Plan",
    "PlenoraAuthenticationError",
    "PlenoraAuthorizationError",
    "PlenoraCancelledError",
    "PlenoraConcurrentModificationError",
    "PlenoraConflictError",
    "PlenoraCrsError",
    "PlenoraDataMappingError",
    "PlenoraError",
    "PlenoraExecutionError",
    "PlenoraInternalError",
    "PlenoraInvalidConfigurationError",
    "PlenoraInvalidPlanError",
    "PlenoraIoError",
    "PlenoraNotFoundError",
    "PlenoraProtocolError",
    "PlenoraResourceLimitError",
    "PlenoraSchemaError",
    "PlenoraTimeoutError",
    "PlenoraTransientError",
    "PlenoraUnsupportedError",
    "RunResult",
    "Source",
    "acatalog",
    "adescribe",
    "arun",
    "avalidate",
    "capabilities",
    "catalog",
    "describe",
    "run",
    "validate",
    "version",
}


def test_il_pacchetto_viene_dal_wheel_installato() -> None:
    radice = pathlib.Path(pd.__file__).resolve().parent
    assert "site-packages" in radice.parts, f"plenora_data importato da {radice}"
    nativi = [p.name for p in radice.iterdir() if p.name.startswith("_native.")]
    assert any(nome.endswith((".pyd", ".so")) for nome in nativi), nativi
    # PEP 561: il marcatore e lo stub del modulo nativo sono nel pacchetto.
    assert (radice / "py.typed").is_file()
    assert (radice / "_native.pyi").is_file()


def test_versione_uguale_ai_metadati_e_al_componente() -> None:
    versione = pd.version()
    assert versione == importlib.metadata.version("plenora-data")
    assert pd.capabilities()["component_version"] == versione
    # Il nome del wheel installato porta la stessa versione (RECORD della
    # distribuzione).
    distribuzione = importlib.metadata.distribution("plenora-data")
    assert distribuzione.metadata["Name"] == "plenora-data"
    assert distribuzione.metadata["Requires-Python"] == ">=3.10"


def test_i_nomi_pubblici_sono_esattamente_quelli_dichiarati() -> None:
    assert set(pd.__all__) == ATTESI
    for nome in pd.__all__:
        assert hasattr(pd, nome), nome
    # Il modulo nativo non è esportato.
    assert "_native" not in pd.__all__


def test_le_capacita_sono_un_documento_capabilities_v2(
    valida: Callable[[str, object], None],
) -> None:
    documento = pd.capabilities()
    valida("capabilities-v2.schema.json", documento)
    assert documento["component"] == "plenora-data-tools"
    assert documento["interfaces"] == [
        {
            "kind": "python_sdk",
            "contract": "plenora-python-sdk-v1",
            "version": 1,
            "artifact": "plenora-data",
        }
    ]
    # CAP-005, CAP-007.
    identita = [(op["id"], op["version"]) for op in documento["operations"]]
    assert len(identita) == len(set(identita))
    for operazione in documento["operations"]:
        assert operazione["surfaces"] == ["python_sdk"]


def test_le_operazioni_sono_quelle_del_catalogo_pubblico(contratti: pathlib.Path) -> None:
    catalogo = leggi(contratti, "data-tools-v2.json")
    nostre = {op["id"]: op for op in pd.capabilities()["operations"]}
    assert set(nostre) == {op["id"] for op in catalogo["operations"]}
    for pubblica in catalogo["operations"]:
        nostra = nostre[pubblica["id"]]
        assert nostra["version"] == pubblica["version"]
        assert nostra["status"] == "available"
        for lato in ("input", "output"):
            assert nostra[lato]["contract"] == pubblica[lato]["contract"]
            assert nostra[lato]["content_types"] == pubblica[lato]["content_types"]
        assert nostra["controls"] == pubblica["controls"]
        assert nostra["side_effect"] == pubblica["side_effect"]


def _simboli(operazione: str) -> list[Callable[..., Any]]:
    nomi = {
        "data.catalog": ("catalog", "acatalog"),
        "data.describe": ("describe", "adescribe"),
        "data.validate": ("validate", "avalidate"),
        "data.run": ("run", "arun"),
    }[operazione]
    return [getattr(pd, nome) for nome in nomi]


def test_ogni_operazione_ha_una_forma_sincrona_e_una_asincrona_equivalenti() -> None:
    for operazione in pd.capabilities()["operations"]:
        sincrona, asincrona = _simboli(operazione["id"])
        assert not inspect.iscoroutinefunction(sincrona)
        assert inspect.iscoroutinefunction(asincrona)
        # Stessi parametri, stessi default, stesse annotazioni (Python SDK
        # 1.0, sezione 4); cambia solo il tipo di ritorno, che è awaitable.
        firma_s = inspect.signature(sincrona)
        firma_a = inspect.signature(asincrona)
        assert list(firma_s.parameters.values()) == list(firma_a.parameters.values())
        assert firma_s.return_annotation == firma_a.return_annotation


def test_i_controlli_dichiarati_sono_parametri() -> None:
    for operazione in pd.capabilities()["operations"]:
        sincrona, _ = _simboli(operazione["id"])
        parametri = inspect.signature(sincrona).parameters
        controlli = operazione["controls"]
        assert ("timeout" in parametri) == controlli["deadline"]
        assert ("deadline" in parametri) == controlli["deadline"]
        assert ("cancel" in parametri) == controlli["cancellation"]
        assert "idempotency_key" not in parametri


def test_la_sezione_dei_binding_e_quella_esposta(
    contratti: pathlib.Path, valida: Callable[[str, object], None]
) -> None:
    """La sezione di bindings/python-sdk-v1.json che l'aggiornamento dei
    contratti deve scrivere: ogni simbolo esiste ed è dell'operazione."""
    sezione = {
        "component": "plenora-data-tools",
        "artifact": "plenora-data / plenora_data",
        "discovery": ["plenora_data.version", "plenora_data.capabilities"],
        "bindings": [
            {
                "operation": operazione["id"],
                "version": operazione["version"],
                "requirement": "required",
                "entrypoints": [f"plenora_data.{f.__name__}" for f in _simboli(operazione["id"])],
            }
            for operazione in pd.capabilities()["operations"]
        ],
    }
    documento = leggi(contratti, "python-sdk-v1.json")
    documento["components"] = [
        sezione if voce["component"] == "plenora-data-tools" else voce
        for voce in documento["components"]
    ]
    valida("surface-bindings-v1.schema.json", documento)
    for nome in sezione["discovery"]:
        assert callable(getattr(pd, nome.removeprefix("plenora_data.")))


def test_il_gettone_di_annullamento() -> None:
    gettone = pd.CancellationToken()
    assert not gettone.cancelled
    gettone.cancel()
    gettone.cancel()
    assert gettone.cancelled
    assert repr(gettone) == "CancellationToken(cancelled=True)"
    assert repr(pd.CancellationToken()) == "CancellationToken(cancelled=False)"
