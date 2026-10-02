"""Scadenza, annullamento (gettone, Ctrl-C, task asyncio) e forma
asincrona che non blocca il loop."""

from __future__ import annotations

import _thread
import asyncio
import functools
import threading
import time
from collections.abc import Callable
from datetime import datetime, timedelta, timezone
from typing import Any

import pyarrow as pa
import pytest

import plenora_data as pd
from aiuti import piano_identita, piano_lungo, tabella_grande, tabella_semplice

# Abbastanza passi da durare secondi senza annullamento; ognuno dura
# decine di millisecondi, così l'annullamento si vede presto.
PASSI = 200


@functools.cache
def grande() -> pa.Table:
    return tabella_grande()


def test_scadenza_passata_e_timeout_zero() -> None:
    passata = datetime.now(timezone.utc) - timedelta(seconds=1)
    for argomenti in ({"timeout": 0}, {"timeout": 0.0}, {"deadline": passata}):
        for chiamata in (
            lambda: pd.describe(tabella_semplice(), **argomenti),
            lambda: pd.validate(piano_identita(), {"t": tabella_semplice()}, **argomenti),
            lambda: pd.run(piano_identita(), {"t": tabella_semplice()}, **argomenti),
        ):
            with pytest.raises(pd.PlenoraTimeoutError) as errore:
                chiamata()
            assert errore.value.code == "EXECUTION_DEADLINE_EXCEEDED"
            assert errore.value.remote_effect == "none"


def test_scadenza_futura_lascia_finire() -> None:
    futura = datetime.now(timezone.utc) + timedelta(hours=1)
    risultato = pd.run(piano_identita(), {"t": tabella_semplice()}, deadline=futura)
    assert risultato.tables["t"].num_rows == 5
    assert pd.describe(tabella_semplice(), timeout=3600)["rows"] == 5


def test_controlli_non_validi() -> None:
    senza_fuso = datetime(2030, 1, 1)
    casi: list[dict[str, Any]] = [
        {"timeout": -1},
        {"timeout": float("nan")},
        {"timeout": float("inf")},
        {"timeout": True},
        {"timeout": "1"},
        {"deadline": senza_fuso},
        {"deadline": "2030-01-01T00:00:00Z"},
        {"timeout": 1, "deadline": datetime.now(timezone.utc)},
    ]
    for argomenti in casi:
        with pytest.raises(pd.PlenoraInvalidConfigurationError):
            pd.describe(tabella_semplice(), **argomenti)


def test_la_scadenza_ferma_un_piano_lungo_fra_due_passi() -> None:
    inizio = time.perf_counter()
    with pytest.raises(pd.PlenoraTimeoutError) as errore:
        pd.run(piano_lungo(PASSI), {"t": grande()}, timeout=0.2)
    assert time.perf_counter() - inizio < 5
    assert errore.value.code == "EXECUTION_DEADLINE_EXCEEDED"
    assert errore.value.retry == {"kind": "safe"}


def test_gettone_gia_alzato() -> None:
    gettone = pd.CancellationToken()
    gettone.cancel()
    with pytest.raises(pd.PlenoraCancelledError) as errore:
        pd.run(piano_identita(), {"t": tabella_semplice()}, cancel=gettone)
    assert errore.value.code == "EXECUTION_CANCELLED"
    assert errore.value.phase == "read"


def test_gettone_alzato_da_un_altro_thread() -> None:
    gettone = pd.CancellationToken()
    threading.Timer(0.2, gettone.cancel).start()
    inizio = time.perf_counter()
    with pytest.raises(pd.PlenoraCancelledError):
        pd.run(piano_lungo(PASSI), {"t": grande()}, cancel=gettone)
    assert time.perf_counter() - inizio < 5


def test_ctrl_c_annulla_e_propaga_keyboard_interrupt() -> None:
    """Ctrl-C mentre il lavoro gira (simulato con `interrupt_main`, che
    arma lo stesso segnale SIGINT): il lavoro si ferma al controllo
    successivo e `KeyboardInterrupt` esce con l'esito come causa."""
    timer = threading.Timer(0.2, _thread.interrupt_main)
    timer.start()
    inizio = time.perf_counter()
    try:
        with pytest.raises(KeyboardInterrupt) as interruzione:
            pd.run(piano_lungo(PASSI), {"t": grande()})
    finally:
        timer.cancel()
    assert time.perf_counter() - inizio < 5
    causa = interruzione.value.__cause__
    assert isinstance(causa, pd.PlenoraCancelledError), repr(causa)
    assert causa.remote_effect == "none"


def test_ctrl_c_durante_la_scrittura_dice_l_effetto(tmp_path: Any) -> None:
    """Con output su file l'esito che accompagna il Ctrl-C porta l'effetto
    vero: nessun file scritto se il lavoro si è fermato fra i passi."""
    timer = threading.Timer(0.2, _thread.interrupt_main)
    timer.start()
    try:
        with pytest.raises(KeyboardInterrupt) as interruzione:
            pd.run(
                piano_lungo(PASSI),
                {"t": grande()},
                outputs={f"s{PASSI - 1}": tmp_path / "u.arrow"},
            )
    finally:
        timer.cancel()
    causa = interruzione.value.__cause__
    assert isinstance(causa, pd.PlenoraCancelledError)
    assert causa.remote_effect == "none"
    assert not (tmp_path / "u.arrow").exists()


async def _con_ticker(lavoro: Callable[[], Any]) -> tuple[Any, int]:
    """Esegue `lavoro` (una coroutine) mentre un ticker conta i giri del
    loop: se il lavoro bloccasse il loop, il conto resterebbe fermo."""
    giri = 0
    fine = asyncio.Event()

    async def ticker() -> None:
        nonlocal giri
        while not fine.is_set():
            giri += 1
            await asyncio.sleep(0.01)

    compito = asyncio.create_task(ticker())
    try:
        esito = await lavoro()
    finally:
        fine.set()
        await compito
    return esito, giri


def test_la_forma_asincrona_non_blocca_il_loop() -> None:
    piano = piano_lungo(20)

    async def principale() -> tuple[Any, int]:
        return await _con_ticker(lambda: pd.arun(piano, {"t": grande()}))

    risultato, giri = asyncio.run(principale())
    assert risultato.tables
    assert giri > 3


def test_annullare_il_task_ferma_il_lavoro() -> None:
    async def principale() -> BaseException:
        compito = asyncio.create_task(pd.arun(piano_lungo(PASSI), {"t": grande()}))
        await asyncio.sleep(0.2)
        compito.cancel()
        try:
            await compito
        except asyncio.CancelledError as annullamento:
            return annullamento
        raise AssertionError("il task doveva essere annullato")

    inizio = time.perf_counter()
    annullamento = asyncio.run(principale())
    assert time.perf_counter() - inizio < 5
    # Da Python 3.11 chi aspetta il task riceve il `CancelledError` sollevato
    # dentro il task; in 3.10 ne riceve uno nuovo, con quello come
    # `__context__`. In entrambi i casi l'esito del lavoro è la sua causa.
    sollevato = annullamento if annullamento.__cause__ is not None else annullamento.__context__
    assert sollevato is not None
    assert isinstance(sollevato.__cause__, pd.PlenoraCancelledError)


def test_le_forme_asincrone_hanno_gli_stessi_errori() -> None:
    gettone = pd.CancellationToken()
    gettone.cancel()

    async def principale() -> None:
        with pytest.raises(pd.PlenoraCancelledError):
            await pd.arun(piano_identita(), {"t": tabella_semplice()}, cancel=gettone)
        with pytest.raises(pd.PlenoraTimeoutError):
            await pd.adescribe(tabella_semplice(), timeout=0)
        with pytest.raises(pd.PlenoraInvalidPlanError):
            await pd.avalidate("{", {})
        with pytest.raises(pd.PlenoraInvalidConfigurationError):
            await pd.arun(piano_identita(), {"t": tabella_semplice()}, timeout=-1)

    asyncio.run(principale())
