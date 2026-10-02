"""Consumatore statico dell'SDK: deve passare mypy --strict su ogni Python
supportato (3.10-3.14). Prova i tipi pubblici che un'applicazione usa, non
il comportamento (quello sta in python/tests)."""

from __future__ import annotations

import asyncio
import pathlib
from datetime import datetime, timezone
from typing import Any

from typing_extensions import assert_type

import plenora_data as pd


def scoperta() -> None:
    assert_type(pd.version(), str)
    documento = pd.capabilities()
    assert_type(documento, dict[str, Any])
    assert_type(pd.catalog(), dict[str, Any])


def operazioni(tabella: pd.ArrowStreamExportable) -> pd.RunResult:
    gettone = pd.CancellationToken()
    assert_type(gettone.cancelled, bool)
    descrizione = pd.describe(tabella, timeout=1.5, cancel=gettone)
    assert_type(descrizione, dict[str, Any])
    piano: dict[str, Any] = {"version": 1, "inputs": ["t"], "steps": [], "outputs": ["t"]}
    validazione = pd.validate(
        piano,
        {"t": tabella},
        deadline=datetime(2030, 1, 1, tzinfo=timezone.utc),
    )
    assert_type(validazione, dict[str, Any])
    da_file = pd.run(
        pathlib.Path("piano.json"),
        {"t": pathlib.Path("t.arrow")},
        outputs={"t": "uscita.parquet"},
        overwrite=True,
    )
    assert_type(da_file.result, dict[str, Any])
    return pd.run(piano, {"t": tabella})


async def asincrone(tabella: pd.ArrowStreamExportable) -> None:
    assert_type(await pd.acatalog(), dict[str, Any])
    assert_type(await pd.adescribe("t.arrow"), dict[str, Any])
    assert_type(await pd.avalidate("{}", {"t": tabella}), dict[str, Any])
    risultato = await pd.arun("{}", {"t": tabella}, timeout=2)
    assert_type(risultato, pd.RunResult)


def errori() -> None:
    try:
        pd.catalog()
    except pd.PlenoraTimeoutError as errore:
        assert_type(errore.code, str | None)
    except pd.PlenoraError as errore:
        assert_type(errore.category, str)
        assert_type(errore.phase, str)
        assert_type(errore.remote_effect, str)
        assert_type(errore.retry, dict[str, Any])
        assert_type(errore.message, str)
        assert_type(errore.details, dict[str, Any] | None)
        assert_type(errore.row_diagnostics, dict[str, Any] | None)
        assert_type(errore.to_dict(), dict[str, Any])


def principale() -> None:
    asyncio.run(asincrone(pd.run("{}").tables["t"]))
