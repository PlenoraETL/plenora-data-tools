"""Scadenza, annullamento (gettone, Ctrl-C, task asyncio) e forma
asincrona che non blocca il loop."""

from __future__ import annotations

import _thread
import asyncio
import contextlib
import functools
import threading
import time
from collections.abc import Callable
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timedelta, timezone
from typing import Any

import pyarrow as pa
import pytest

import plenora_data as pd
from plenora_data import _native
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
    # Fermata all'ingresso, prima di qualunque lavoro.
    assert errore.value.phase == "prepare"


def test_gettone_gia_alzato_non_legge_ne_scrive(tmp_path: Any) -> None:
    """Un input da file che non esiste: se la chiamata lo leggesse
    fallirebbe con `not_found`; un output che non deve comparire."""
    gettone = pd.CancellationToken()
    gettone.cancel()
    uscita = tmp_path / "u.arrow"
    for chiamata in (
        lambda: pd.run(
            piano_identita(), {"t": tmp_path / "manca.arrow"}, outputs={"t": uscita}, cancel=gettone
        ),
        lambda: pd.validate(piano_identita(), {"t": tmp_path / "manca.arrow"}, cancel=gettone),
        lambda: pd.describe(tmp_path / "manca.arrow", cancel=gettone),
    ):
        with pytest.raises(pd.PlenoraCancelledError) as errore:
            chiamata()
        assert errore.value.remote_effect == "none"
    assert not uscita.exists()


def test_annullamento_in_corsa_con_la_fine_non_e_mai_un_successo_taciuto(
    tmp_path: Any,
) -> None:
    """Un gettone alzato a istanti diversi intorno alla fine del lavoro:
    o la chiamata riesce e il gettone è arrivato dopo, o fallisce con
    `cancelled` e l'effetto dice se il file d'uscita è stato scritto."""
    piano = piano_lungo(6)
    tabella = tabella_grande(200_000)
    inizio = time.perf_counter()
    pd.run(piano, {"t": tabella}, outputs={"s5": tmp_path / "misura.arrow"})
    durata = time.perf_counter() - inizio
    esiti = set()
    for indice in range(24):
        uscita = tmp_path / f"u{indice}.arrow"
        gettone = pd.CancellationToken()
        threading.Timer(durata * indice / 12, gettone.cancel).start()
        try:
            pd.run(piano, {"t": tabella}, outputs={"s5": uscita}, cancel=gettone)
        except pd.PlenoraCancelledError as errore:
            esiti.add(errore.remote_effect)
            assert errore.remote_effect == ("committed" if uscita.exists() else "none")
        else:
            esiti.add("ok")
            assert uscita.exists()
    # Gli istanti vanno da subito a due volte la durata: qualche
    # annullamento si vede di certo.
    assert esiti & {"none", "committed"}


@contextlib.contextmanager
def _sonda(azione: Callable[[], object]) -> Any:
    """Registra la sonda privata del modulo nativo: `azione` gira fra la
    fine del lavoro e il controllo della consegna."""
    _native._sonda_consegna(azione)
    try:
        yield
    finally:
        _native._sonda_consegna(None)


def test_gettone_alzato_fra_la_fine_e_la_consegna(tmp_path: Any) -> None:
    """Deterministico: il gettone si alza esattamente dopo che il lavoro è
    finito con successo e prima della consegna."""
    uscita = tmp_path / "u.arrow"
    for outputs, effetto, ritentativo in (
        ({"t": uscita}, "committed", "requires_recovery"),
        (None, "none", "safe"),
    ):
        gettone = pd.CancellationToken()
        with _sonda(gettone.cancel), pytest.raises(pd.PlenoraCancelledError) as errore:
            pd.run(
                piano_identita(), {"t": tabella_semplice()}, outputs=outputs, cancel=gettone
            )
        assert errore.value.phase == "finalize"
        assert errore.value.remote_effect == effetto
        assert errore.value.retry == {"kind": ritentativo}
        assert errore.value.code == "EXECUTION_CANCELLED"
    assert uscita.exists()


def test_ctrl_c_fra_la_fine_e_la_consegna(tmp_path: Any) -> None:
    """Deterministico: SIGINT armato dopo la fine del lavoro e prima della
    consegna esce come `KeyboardInterrupt` con l'annullamento alla consegna
    come causa, con l'effetto vero."""
    uscita = tmp_path / "u.arrow"
    for outputs, effetto in (({"t": uscita}, "committed"), (None, "none")):
        with _sonda(_thread.interrupt_main), pytest.raises(KeyboardInterrupt) as interruzione:
            pd.run(piano_identita(), {"t": tabella_semplice()}, outputs=outputs)
        causa = interruzione.value.__cause__
        assert isinstance(causa, pd.PlenoraCancelledError), repr(causa)
        assert causa.phase == "finalize"
        assert causa.remote_effect == effetto
    assert uscita.exists()


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


def _causa_dell_annullamento(annullamento: BaseException) -> BaseException | None:
    """La causa del `CancelledError` sollevato dentro il task (in 3.10 chi
    aspetta ne riceve uno nuovo, con quello come `__context__`)."""
    if annullamento.__cause__ is not None:
        return annullamento.__cause__
    contesto = annullamento.__context__
    return None if contesto is None else contesto.__cause__


def test_un_task_annullato_mentre_il_lavoro_finisce_dice_l_esito() -> None:
    """Il lavoro finisce con successo dopo l'annullamento del task (qui una
    chiamata che ignora i gettoni): il risultato non si consegna, ma la causa
    dice l'effetto, `committed` quando gli output sono stati scritti."""
    from plenora_data._api import _in_thread

    def lavoro(_: list[pd.CancellationToken]) -> str:
        time.sleep(0.3)
        return "finito"

    for con_effetti, effetto, ritentativo in (
        (True, "committed", "requires_recovery"),
        (False, "none", "safe"),
    ):

        async def principale(con_effetti: bool = con_effetti) -> BaseException:
            compito = asyncio.create_task(_in_thread(lavoro, [], con_effetti=con_effetti))
            await asyncio.sleep(0.05)
            compito.cancel()
            try:
                await compito
            except asyncio.CancelledError as annullamento:
                return annullamento
            raise AssertionError("il task doveva essere annullato")

        causa = _causa_dell_annullamento(asyncio.run(principale()))
        assert isinstance(causa, pd.PlenoraCancelledError)
        assert causa.remote_effect == effetto
        assert causa.retry == {"kind": ritentativo}
        assert causa.phase == "finalize"


def test_la_scadenza_asincrona_conta_dall_ingresso(tmp_path: Any) -> None:
    """Con l'executor saturo la chiamata aspetta un thread libero: la
    scadenza vale dall'ingresso di `arun`, e una scadenza passata in coda
    ferma la chiamata prima di qualunque lavoro."""
    uscita = tmp_path / "u.arrow"

    async def principale() -> None:
        loop = asyncio.get_running_loop()
        loop.set_default_executor(ThreadPoolExecutor(max_workers=1))
        occupato = loop.run_in_executor(None, time.sleep, 0.5)
        with pytest.raises(pd.PlenoraTimeoutError) as errore:
            await pd.arun(
                piano_identita(), {"t": tabella_semplice()}, outputs={"t": uscita}, timeout=0.1
            )
        await occupato
        assert errore.value.phase == "prepare"
        assert errore.value.remote_effect == "none"

    asyncio.run(principale())
    assert not uscita.exists()
