"""Scadenza, annullamento (gettone, Ctrl-C, task asyncio) e forma
asincrona che non blocca il loop."""

from __future__ import annotations

import _thread
import asyncio
import contextlib
import subprocess
import sys
import threading
import time
from collections.abc import Callable
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timedelta, timezone
from typing import Any

import pytest

import plenora_data as pd
from plenora_data import _native
from aiuti import piano_identita, piano_lungo, tabella_semplice

# Passi del piano delle prove che fermano il lavoro in corso: una fermata
# prima dell'ultimo passo non scrive l'output.
PASSI = 3

# Tetto di attesa delle prove a eventi: non misura niente, ferma con un
# fallimento (invece di un blocco della suite) una prova il cui evento non
# arriva mai.
TETTO = 60

# La fase di un'interruzione vista dal lavoro in corso al suo primo
# controllo, prima di caricare gli input (la sonda del lavoro gira prima):
# né `prepare` (il controllo d'ingresso, sul thread del chiamante) né
# `finalize` (la consegna, a lavoro finito).
FASE_DEL_LAVORO = "read"


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


def test_la_scadenza_ferma_il_lavoro_in_corso() -> None:
    """Deterministico: la sonda tiene il lavoro, già partito, finché la sua
    scadenza non scatta; il lavoro si ferma allora al suo primo controllo.
    Se la scadenza non arrivasse al lavoro, la sonda aspetterebbe fino al
    tetto e la chiamata riuscirebbe.

    Una scadenza scattata già al controllo d'ingresso (fase `prepare`,
    sonda mai chiamata: il thread fermo più a lungo della scadenza) non
    prova il lavoro in corso: si riprova con una scadenza doppia."""
    scadenza = 0.05
    while True:
        chiamate: list[None] = []

        def sonda(chiamate: list[None] = chiamate) -> float:
            chiamate.append(None)
            return TETTO

        with _sonda_lavoro(sonda), pytest.raises(pd.PlenoraTimeoutError) as errore:
            pd.run(piano_lungo(PASSI), {"t": tabella_semplice()}, timeout=scadenza)
        if chiamate or scadenza > TETTO:
            break
        assert errore.value.phase == "prepare"
        scadenza *= 2
    assert chiamate == [None]
    assert errore.value.phase == FASE_DEL_LAVORO
    assert errore.value.code == "EXECUTION_DEADLINE_EXCEEDED"
    assert errore.value.remote_effect == "none"
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
    """Un gettone alzato in ognuno degli istanti intorno al lavoro, ciascuno
    raggiunto in modo deterministico (nessun timer): prima della chiamata,
    a lavoro in corso (sonda del lavoro), fra la fine del lavoro e la
    consegna (sonda della consegna), dopo la consegna. O la chiamata riesce
    e il gettone è arrivato dopo, o fallisce con `cancelled` e l'effetto
    dice se il file d'uscita è stato scritto."""
    piano = piano_lungo(PASSI)
    uscita_del_piano = f"s{PASSI - 1}"

    def nessuna(_: pd.CancellationToken) -> contextlib.AbstractContextManager[None]:
        return contextlib.nullcontext()

    def durante(gettone: pd.CancellationToken) -> contextlib.AbstractContextManager[None]:
        def sonda() -> float:
            gettone.cancel()
            return TETTO

        return _sonda_lavoro(sonda)

    def alla_consegna(gettone: pd.CancellationToken) -> contextlib.AbstractContextManager[None]:
        return _sonda(gettone.cancel)

    istanti: list[
        tuple[
            str,
            bool,
            Callable[[pd.CancellationToken], contextlib.AbstractContextManager[None]],
            str,
        ]
    ] = [
        ("prima", True, nessuna, "none"),
        ("durante", False, durante, "none"),
        ("alla consegna", False, alla_consegna, "committed"),
        ("dopo", False, nessuna, "ok"),
    ]
    for nome, prima, sonda, atteso in istanti:
        uscita = tmp_path / f"{nome.replace(' ', '_')}.arrow"
        gettone = pd.CancellationToken()
        if prima:
            gettone.cancel()
        try:
            with sonda(gettone):
                pd.run(
                    piano,
                    {"t": tabella_semplice()},
                    outputs={uscita_del_piano: uscita},
                    cancel=gettone,
                )
        except pd.PlenoraCancelledError as errore:
            esito = errore.remote_effect
            assert esito == ("committed" if uscita.exists() else "none"), nome
        else:
            esito = "ok"
            assert uscita.exists(), nome
            gettone.cancel()
        assert esito == atteso, nome


@contextlib.contextmanager
def _sonda(azione: Callable[[], object]) -> Any:
    """Registra la sonda privata del modulo nativo: `azione` gira fra la
    fine del lavoro e il controllo della consegna."""
    _native._sonda_consegna(azione)
    try:
        yield
    finally:
        _native._sonda_consegna(None)


@contextlib.contextmanager
def _sonda_lavoro(azione: Callable[[], float | None]) -> Any:
    """Registra la sonda privata del lavoro: `azione` gira nel thread del
    lavoro prima che cominci; se restituisce dei secondi, il lavoro aspetta
    al più per quel tempo che scatti la sua interruzione."""
    _native._sonda_lavoro(azione)
    try:
        yield
    finally:
        _native._sonda_lavoro(None)


def test_una_sonda_del_lavoro_che_fallisce_non_passa_in_silenzio() -> None:
    """Una sonda del lavoro che solleva, o che chiede un'attesa che non è
    un numero di secondi valido, ferma la chiamata con `internal`: una
    prova scritta male non diventa una prova passata."""

    def solleva() -> float:
        raise RuntimeError("sonda")

    def negativa() -> float:
        return -1.0

    def testo() -> Any:
        return "1"

    for sonda in (solleva, negativa, testo):
        with _sonda_lavoro(sonda), pytest.raises(pd.PlenoraInternalError):
            pd.run(piano_identita(), {"t": tabella_semplice()})


def test_sostituire_la_sonda_non_si_blocca_sul_suo_del() -> None:
    """La sonda tolta si rilascia fuori dal lucchetto: il suo `__del__` può
    richiamare `_sonda_consegna`. In un processo a parte, con un tempo
    massimo: un deadlock non deve fermare la suite."""
    programma = (
        "from plenora_data import _native\n"
        "class Sonda:\n"
        "    def __call__(self):\n"
        "        pass\n"
        "    def __del__(self):\n"
        "        _native._sonda_consegna(None)\n"
        "_native._sonda_consegna(Sonda())\n"
        "_native._sonda_consegna(Sonda())\n"
        "_native._sonda_consegna(None)\n"
        "print('ok')\n"
    )
    esito = subprocess.run(
        [sys.executable, "-c", programma],
        capture_output=True,
        text=True,
        timeout=60,
        check=False,
    )
    assert esito.returncode == 0, esito.stderr
    assert esito.stdout.strip() == "ok"


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
    """Deterministico: a lavoro in corso la sonda alza il gettone da un
    altro thread e tiene il lavoro finché il segnale non gli arriva. Il
    thread del chiamante porta il gettone nel lavoro mentre aspetta: senza,
    la sonda aspetterebbe fino al tetto e l'annullamento si vedrebbe solo
    alla consegna (fase `finalize`)."""
    gettone = pd.CancellationToken()

    def sonda() -> float:
        altro = threading.Thread(target=gettone.cancel)
        altro.start()
        altro.join()
        return TETTO

    with _sonda_lavoro(sonda), pytest.raises(pd.PlenoraCancelledError) as errore:
        pd.run(piano_lungo(PASSI), {"t": tabella_semplice()}, cancel=gettone)
    assert errore.value.phase == FASE_DEL_LAVORO
    assert errore.value.remote_effect == "none"
    assert errore.value.code == "EXECUTION_CANCELLED"


def _ctrl_c_a_lavoro_in_corso() -> float:
    """Sonda del lavoro: arma SIGINT (`interrupt_main`, lo stesso segnale
    di Ctrl-C) e tiene il lavoro finché il thread del chiamante non l'ha
    visto e ha alzato il segnale del lavoro."""
    _thread.interrupt_main()
    return TETTO


def test_ctrl_c_annulla_e_propaga_keyboard_interrupt() -> None:
    """Ctrl-C mentre il lavoro gira: il lavoro si ferma al controllo
    successivo e `KeyboardInterrupt` esce con l'esito come causa.
    Deterministico: il segnale si arma a lavoro in corso, e senza la
    sorveglianza dei segnali durante l'attesa l'esito sarebbe
    l'annullamento alla consegna (fase `finalize`)."""
    with _sonda_lavoro(_ctrl_c_a_lavoro_in_corso), pytest.raises(
        KeyboardInterrupt
    ) as interruzione:
        pd.run(piano_lungo(PASSI), {"t": tabella_semplice()})
    causa = interruzione.value.__cause__
    assert isinstance(causa, pd.PlenoraCancelledError), repr(causa)
    assert causa.phase == FASE_DEL_LAVORO
    assert causa.remote_effect == "none"


def test_ctrl_c_durante_la_scrittura_dice_l_effetto(tmp_path: Any) -> None:
    """Con output su file l'esito che accompagna il Ctrl-C porta l'effetto
    vero: nessun file scritto se il lavoro si è fermato prima della fine.
    Deterministico come la prova sopra."""
    uscita = tmp_path / "u.arrow"
    with _sonda_lavoro(_ctrl_c_a_lavoro_in_corso), pytest.raises(
        KeyboardInterrupt
    ) as interruzione:
        pd.run(
            piano_lungo(PASSI),
            {"t": tabella_semplice()},
            outputs={f"s{PASSI - 1}": uscita},
        )
    causa = interruzione.value.__cause__
    assert isinstance(causa, pd.PlenoraCancelledError)
    assert causa.phase == FASE_DEL_LAVORO
    assert causa.remote_effect == "none"
    assert not uscita.exists()


def test_la_forma_asincrona_non_blocca_il_loop() -> None:
    """Deterministico, senza contare giri a tempo: la sonda gira nel thread
    del lavoro, a lavoro finito e prima della consegna, e aspetta un evento
    che solo il loop può alzare. Se `arun` tenesse il loop (il lavoro nel
    thread del loop), il callback non girerebbe e l'attesa scadrebbe."""
    alzato = threading.Event()
    nella_sonda: list[tuple[bool, bool]] = []

    async def principale() -> Any:
        loop = asyncio.get_running_loop()
        thread_del_loop = threading.get_ident()

        def sonda() -> None:
            loop.call_soon_threadsafe(alzato.set)
            nella_sonda.append((threading.get_ident() != thread_del_loop, alzato.wait(TETTO)))

        with _sonda(sonda):
            return await pd.arun(piano_identita(), {"t": tabella_semplice()})

    risultato = asyncio.run(principale())
    assert risultato.tables["t"].num_rows == 5
    # Il lavoro è fuori dal thread del loop, e il loop ha girato mentre la
    # chiamata era in corso.
    assert nella_sonda == [(True, True)]


def test_annullare_il_task_ferma_il_lavoro() -> None:
    """Deterministico: a lavoro in corso la sonda annulla il task dal loop
    e tiene il lavoro finché l'annullamento non gli arriva. Se annullare il
    task non fermasse il lavoro, la sonda aspetterebbe fino al tetto e la
    causa sarebbe l'annullamento alla consegna (fase `finalize`)."""

    async def principale() -> BaseException:
        loop = asyncio.get_running_loop()
        compiti: list[asyncio.Task[Any]] = []

        def sonda() -> float:
            loop.call_soon_threadsafe(compiti[0].cancel)
            return TETTO

        with _sonda_lavoro(sonda):
            compiti.append(
                asyncio.create_task(pd.arun(piano_lungo(PASSI), {"t": tabella_semplice()}))
            )
            try:
                await compiti[0]
            except asyncio.CancelledError as annullamento:
                return annullamento
        raise AssertionError("il task doveva essere annullato")

    annullamento = asyncio.run(principale())
    # Da Python 3.11 chi aspetta il task riceve il `CancelledError` sollevato
    # dentro il task; in 3.10 ne riceve uno nuovo, con quello come
    # `__context__`. In entrambi i casi l'esito del lavoro è la sua causa.
    sollevato = annullamento if annullamento.__cause__ is not None else annullamento.__context__
    assert sollevato is not None
    assert isinstance(sollevato.__cause__, pd.PlenoraCancelledError)
    assert sollevato.__cause__.phase == FASE_DEL_LAVORO
    assert sollevato.__cause__.remote_effect == "none"


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

    for con_effetti, effetto, ritentativo in (
        (True, "committed", "requires_recovery"),
        (False, "none", "safe"),
    ):
        # Deterministico: il lavoro parte, il task si annulla mentre il
        # lavoro aspetta, poi il lavoro finisce. Nessuna durata conta.
        partito = threading.Event()
        procedi = threading.Event()

        def lavoro(
            _: list[pd.CancellationToken],
            partito: threading.Event = partito,
            procedi: threading.Event = procedi,
        ) -> str:
            partito.set()
            assert procedi.wait(TETTO)
            return "finito"

        async def principale(
            con_effetti: bool = con_effetti,
            lavoro: Callable[[list[pd.CancellationToken]], str] = lavoro,
            partito: threading.Event = partito,
            procedi: threading.Event = procedi,
        ) -> BaseException:
            compito = asyncio.create_task(_in_thread(lavoro, [], con_effetti=con_effetti))
            assert await asyncio.to_thread(partito.wait, TETTO)
            compito.cancel()
            procedi.set()
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
    scadenza = 0.1

    async def principale() -> None:
        loop = asyncio.get_running_loop()
        loop.set_default_executor(ThreadPoolExecutor(max_workers=1))
        # L'unico thread resta occupato finché la scadenza non è passata
        # davvero: una condizione sull'orologio, non una durata supposta.
        libera = threading.Event()
        occupato = loop.run_in_executor(None, libera.wait, TETTO)
        chiamata = asyncio.create_task(
            pd.arun(
                piano_identita(),
                {"t": tabella_semplice()},
                outputs={"t": uscita},
                timeout=scadenza,
            )
        )
        # Il task entra in `arun` (e fissa la scadenza) al primo giro del
        # loop: l'orologio si legge dopo, quindi la scadenza vera non è
        # più tarda di `inizio + scadenza`.
        await asyncio.sleep(0)
        inizio = time.monotonic()
        while time.monotonic() - inizio <= scadenza:
            await asyncio.sleep(scadenza)
        libera.set()
        with pytest.raises(pd.PlenoraTimeoutError) as errore:
            await chiamata
        assert await occupato
        assert errore.value.phase == "prepare"
        assert errore.value.remote_effect == "none"

    asyncio.run(principale())
    assert not uscita.exists()
