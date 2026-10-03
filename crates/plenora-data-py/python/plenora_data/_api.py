"""Le operazioni pubbliche: una funzione sincrona e una asincrona per ogni
operazione del catalogo, con gli stessi parametri, la stessa validazione,
gli stessi risultati e gli stessi errori.

Le due forme condividono tutto fino alla chiamata nativa: gli argomenti si
validano e si convertono una volta (`_prepara_*`), e la chiamata preparata
gira sul thread del chiamante (forma sincrona) o in un thread
dell'executor del loop (forma asincrona, che non blocca mai il loop). La
chiamata nativa esegue il lavoro in un thread suo, senza il GIL.
"""

from __future__ import annotations

import asyncio
import functools
import json
import math
import os
import time
from collections.abc import Callable, Mapping
from dataclasses import dataclass
from datetime import datetime
from types import MappingProxyType
from typing import Any, Protocol, TypeVar, Union

import pyarrow as pa

from . import _native
from ._native import CancellationToken
from .errors import (
    PlenoraError,
    _annullata_alla_consegna,
    _configurazione,
    _dati,
    _interno,
)

__all__ = [
    "ArrowStreamExportable",
    "CancellationToken",
    "RunResult",
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
]

_T = TypeVar("_T")


class ArrowStreamExportable(Protocol):
    """Un oggetto con l'Arrow PyCapsule Interface per gli stream
    (`pyarrow.Table`, `pyarrow.RecordBatchReader`, e le tabelle di altre
    librerie Arrow)."""

    def __arrow_c_stream__(self, requested_schema: object | None = None) -> object: ...


Source = Union[str, "os.PathLike[str]", ArrowStreamExportable, pa.RecordBatch]
"""Una tabella: un percorso (Arrow IPC file o stream, Parquet) o un oggetto
Arrow in memoria."""

Plan = Union[Mapping[str, Any], str, "os.PathLike[str]"]
"""Un piano `plenora-data-plan-v1`: un dizionario, il suo testo JSON (`str`)
o il percorso di un file (`os.PathLike`, per esempio `pathlib.Path`)."""

Destination = Union[str, "os.PathLike[str]"]
"""Il percorso di un output di `run` (formato dall'estensione: `.arrow`,
`.feather`, `.ipc` file Arrow IPC, `.arrows` stream, `.parquet`)."""


@dataclass(frozen=True)
class RunResult:
    """Il risultato di `run` e `arun`.

    `result` è il documento `plenora-data-execution-result-v2` (output e
    passi, mai percorsi). `tables` ha le tabelle d'uscita, nell'ordine degli
    output del piano, quando non sono state scritte su file (`outputs`
    assente); altrimenti è vuoto.
    """

    result: dict[str, Any]
    tables: Mapping[str, pa.Table]


# ---------------------------------------------------------------------------
# Conversione e validazione degli argomenti, comune alle due forme.
# ---------------------------------------------------------------------------


def _chiama(funzione: Callable[[], _T], *, con_effetti: bool = False) -> _T:
    """Chiama il modulo nativo. Le sue eccezioni pubbliche passano; ogni
    altra `Exception` (un difetto del confine) diventa `PlenoraInternalError`
    senza il suo testo. `KeyboardInterrupt` e le altre `BaseException`
    passano com'erano."""
    try:
        return funzione()
    except PlenoraError:
        raise
    except Exception:
        raise _interno(
            "il modulo nativo ha sollevato un'eccezione non pubblica",
            remote_effect="unknown" if con_effetti else "none",
        ) from None


def _classificata(preparazione: Callable[..., _T]) -> Callable[..., _T]:
    """La preparazione degli argomenti non lascia uscire eccezioni non
    pubbliche: le sue `PlenoraError` passano, ogni altra `Exception` (un
    `Mapping` che fallisce mentre si itera, un intero che non sta in un
    float) diventa `invalid_configuration` con un testo fisso, senza il suo.
    `KeyboardInterrupt`, `SystemExit` e `asyncio.CancelledError` passano."""

    @functools.wraps(preparazione)
    def classificata(*argomenti: Any, **nominati: Any) -> _T:
        try:
            return preparazione(*argomenti, **nominati)
        except PlenoraError:
            raise
        except Exception:
            raise _configurazione("argomenti non leggibili") from None

    return classificata


def _secondi(timeout: float | None) -> float | None:
    """Il timeout come istante del clock monotono, fissato adesso: la
    scadenza vale dall'ingresso della chiamata pubblica, anche se la forma
    asincrona aspetta un thread libero prima di partire."""
    if timeout is None:
        return None
    if isinstance(timeout, bool) or not isinstance(timeout, (int, float)):
        raise _configurazione("`timeout`: attesi secondi (int o float) o None")
    try:
        valore = float(timeout)
    except OverflowError:
        valore = math.inf
    if not math.isfinite(valore) or valore < 0:
        raise _configurazione(
            "`timeout`: atteso un numero finito di secondi, non negativo e rappresentabile"
        )
    return time.monotonic() + valore


def _epoca(deadline: datetime | None) -> float | None:
    if deadline is None:
        return None
    if not isinstance(deadline, datetime):
        raise _configurazione("`deadline`: atteso un datetime con fuso o None")
    if deadline.utcoffset() is None:
        # Un istante senza fuso vale ore diverse su macchine diverse.
        raise _configurazione("`deadline`: datetime senza fuso (tzinfo) non ammesso")
    return deadline.timestamp()


def _gettone(cancel: CancellationToken | None) -> list[CancellationToken]:
    if cancel is None:
        return []
    if not isinstance(cancel, CancellationToken):
        raise _configurazione("`cancel`: atteso un CancellationToken o None")
    return [cancel]


def _controlli(
    timeout: float | None, deadline: datetime | None, cancel: CancellationToken | None
) -> tuple[float | None, float | None, list[CancellationToken]]:
    monotona = _secondi(timeout)
    epoca = _epoca(deadline)
    if monotona is not None and epoca is not None:
        raise _configurazione("una sola fra `deadline` e `timeout`")
    return epoca, monotona, _gettone(cancel)


def _sorgente(valore: object, voce: str) -> tuple[str, object]:
    """Una tabella: `("path", percorso)` o `("arrow", oggetto con
    __arrow_c_stream__)`. `voce` dice quale argomento, per il messaggio."""
    if isinstance(valore, (str, os.PathLike)):
        return ("path", valore)
    try:
        if isinstance(valore, pa.RecordBatch):
            # Senza copia: la tabella condivide i buffer del blocco.
            return ("arrow", pa.Table.from_batches([valore]))
        arrow = hasattr(valore, "__arrow_c_stream__")
    except PlenoraError:
        # Già pubblica e classificata (per esempio un annullamento): resta
        # quella.
        raise
    except Exception:
        # L'oggetto fallisce già mentre lo si guarda: il suo testo può
        # portare dati, e non esce.
        raise _dati(f"{voce}: l'oggetto Arrow non si lascia leggere") from None
    if arrow:
        return ("arrow", valore)
    raise _configurazione(
        f"{voce}: atteso un percorso o un oggetto Arrow "
        "(pyarrow.Table, RecordBatch, RecordBatchReader o __arrow_c_stream__)"
    )


def _ingressi(inputs: Mapping[str, Source] | None) -> list[tuple[str, str, object]]:
    if inputs is None:
        return []
    if not isinstance(inputs, Mapping):
        raise _configurazione("`inputs`: atteso un Mapping nome -> tabella")
    voci: list[tuple[str, str, object]] = []
    for numero, (nome, valore) in enumerate(inputs.items(), start=1):
        if not isinstance(nome, str):
            raise _configurazione(f"`inputs`: la chiave numero {numero} non e' una str")
        tipo, oggetto = _sorgente(valore, f"`inputs`, valore numero {numero}")
        voci.append((nome, tipo, oggetto))
    return voci


def _piano(plan: Plan) -> tuple[str, object]:
    if isinstance(plan, os.PathLike):
        return ("path", plan)
    if isinstance(plan, str):
        return ("json", plan)
    if isinstance(plan, Mapping):
        try:
            # `allow_nan=False`: NaN e infiniti non sono JSON, e il piano
            # non deve cambiare forma passando di qui.
            return ("json", json.dumps(plan, allow_nan=False))
        except (TypeError, ValueError):
            raise _configurazione(
                "`plan`: il dizionario non e' serializzabile in JSON"
            ) from None
    raise _configurazione("`plan`: atteso un Mapping, testo JSON (str) o os.PathLike")


def _uscite(outputs: Mapping[str, Destination] | None) -> list[tuple[str, object]] | None:
    if outputs is None:
        return None
    if not isinstance(outputs, Mapping):
        raise _configurazione("`outputs`: atteso un Mapping nome -> percorso")
    uscite: list[tuple[str, object]] = []
    for numero, (nome, percorso) in enumerate(outputs.items(), start=1):
        if not isinstance(nome, str):
            raise _configurazione(f"`outputs`: la chiave numero {numero} non e' una str")
        if not isinstance(percorso, (str, os.PathLike)):
            raise _configurazione(f"`outputs`: il valore numero {numero} non e' un percorso")
        uscite.append((nome, percorso))
    return uscite


# Una chiamata preparata: riceve i gettoni d'annullamento e fa la chiamata
# nativa. La forma asincrona aggiunge il suo gettone interno.
_Preparata = Callable[[list[CancellationToken]], _T]


@_classificata
def _prepara_describe(
    data: Source,
    timeout: float | None,
    deadline: datetime | None,
    cancel: CancellationToken | None,
) -> tuple[_Preparata[dict[str, Any]], list[CancellationToken]]:
    epoca, monotona, gettoni = _controlli(timeout, deadline, cancel)
    tipo, oggetto = _sorgente(data, "`data`")

    def chiamata(tutti: list[CancellationToken]) -> dict[str, Any]:
        testo = _chiama(lambda: _native.describe(tipo, oggetto, epoca, monotona, tutti))
        documento: dict[str, Any] = json.loads(testo)
        return documento

    return chiamata, gettoni


@_classificata
def _prepara_validate(
    plan: Plan,
    inputs: Mapping[str, Source] | None,
    timeout: float | None,
    deadline: datetime | None,
    cancel: CancellationToken | None,
) -> tuple[_Preparata[dict[str, Any]], list[CancellationToken]]:
    epoca, monotona, gettoni = _controlli(timeout, deadline, cancel)
    tipo_piano, piano = _piano(plan)
    voci = _ingressi(inputs)

    def chiamata(tutti: list[CancellationToken]) -> dict[str, Any]:
        testo = _chiama(
            lambda: _native.validate(tipo_piano, piano, voci, epoca, monotona, tutti)
        )
        documento: dict[str, Any] = json.loads(testo)
        return documento

    return chiamata, gettoni


@_classificata
def _prepara_run(
    plan: Plan,
    inputs: Mapping[str, Source] | None,
    outputs: Mapping[str, Destination] | None,
    overwrite: bool,
    timeout: float | None,
    deadline: datetime | None,
    cancel: CancellationToken | None,
) -> tuple[_Preparata[RunResult], list[CancellationToken]]:
    epoca, monotona, gettoni = _controlli(timeout, deadline, cancel)
    if not isinstance(overwrite, bool):
        raise _configurazione("`overwrite`: atteso un bool")
    tipo_piano, piano = _piano(plan)
    voci = _ingressi(inputs)
    uscite = _uscite(outputs)
    con_effetti = uscite is not None

    def chiamata(tutti: list[CancellationToken]) -> RunResult:
        testo, tabelle = _chiama(
            lambda: _native.run(
                tipo_piano, piano, voci, uscite, overwrite, epoca, monotona, tutti
            ),
            con_effetti=con_effetti,
        )
        documento: dict[str, Any] = json.loads(testo)
        return RunResult(result=documento, tables=MappingProxyType(dict(tabelle)))

    return chiamata, gettoni


async def _in_thread(
    chiamata: _Preparata[_T],
    gettoni: list[CancellationToken],
    *,
    con_effetti: bool = False,
) -> _T:
    """Esegue una chiamata preparata nell'executor del loop.

    Se il task viene annullato, il gettone interno ferma il lavoro al suo
    controllo successivo; il task aspetta che il lavoro si sia fermato (mai
    lavoro che continua dopo l'annullamento) e poi propaga
    `asyncio.CancelledError`, con l'esito del lavoro come `__cause__`. Se il
    lavoro era già finito con successo, la causa è l'annullamento alla
    consegna con l'effetto vero (`committed` se `con_effetti`: gli output
    sono stati scritti), mai un risultato perso senza assi.
    """
    interno = CancellationToken()
    tutti = [*gettoni, interno]
    futuro = asyncio.get_running_loop().run_in_executor(None, chiamata, tutti)
    try:
        return await asyncio.shield(futuro)
    except asyncio.CancelledError as annullamento:
        interno.cancel()
        causa: BaseException | None = None
        while True:
            try:
                await asyncio.shield(futuro)
                causa = _annullata_alla_consegna(con_effetti=con_effetti)
            except asyncio.CancelledError:
                # Un altro annullamento mentre il lavoro si ferma: si
                # continua ad aspettarlo.
                if futuro.cancelled():
                    break
                continue
            except PlenoraError as errore:
                causa = errore
            except Exception:
                causa = None
            break
        raise annullamento from causa


# ---------------------------------------------------------------------------
# Scoperta.
# ---------------------------------------------------------------------------


def version() -> str:
    """La versione del pacchetto: la stessa dei metadati installati
    (`importlib.metadata.version("plenora-data")`), del nome del wheel e del
    componente nel documento delle capacità."""
    return _native.version()


def capabilities() -> dict[str, Any]:
    """Il documento `capabilities-v2` di questo pacchetto (interfaccia
    `python_sdk`, contratto `plenora-python-sdk-v1`): operazioni, versioni,
    contratti, tipi di contenuto, effetti e controlli."""
    documento: dict[str, Any] = json.loads(_chiama(_native.capabilities))
    return documento


# ---------------------------------------------------------------------------
# Operazioni.
# ---------------------------------------------------------------------------


def catalog() -> dict[str, Any]:
    """`data.catalog`: il registro dei kernel che un piano può usare
    (`plenora-data-catalog-result-v2`)."""
    documento: dict[str, Any] = json.loads(_chiama(_native.catalog))
    return documento


async def acatalog() -> dict[str, Any]:
    """`data.catalog`, forma asincrona di `catalog`."""
    return await _in_thread(lambda _: catalog(), [])


def describe(
    data: Source,
    *,
    timeout: float | None = None,
    deadline: datetime | None = None,
    cancel: CancellationToken | None = None,
) -> dict[str, Any]:
    """`data.describe`: descrive una tabella senza modificarla
    (`plenora-data-description-v1`).

    `data` è un percorso (Arrow IPC file o stream, Parquet) o un oggetto
    Arrow. `timeout` (secondi) o `deadline` (datetime con fuso), uno solo
    dei due; `cancel` ferma l'operazione al controllo successivo.
    """
    chiamata, gettoni = _prepara_describe(data, timeout, deadline, cancel)
    return chiamata(gettoni)


async def adescribe(
    data: Source,
    *,
    timeout: float | None = None,
    deadline: datetime | None = None,
    cancel: CancellationToken | None = None,
) -> dict[str, Any]:
    """`data.describe`, forma asincrona di `describe`."""
    chiamata, gettoni = _prepara_describe(data, timeout, deadline, cancel)
    return await _in_thread(chiamata, gettoni)


def validate(
    plan: Plan,
    inputs: Mapping[str, Source] | None = None,
    *,
    timeout: float | None = None,
    deadline: datetime | None = None,
    cancel: CancellationToken | None = None,
) -> dict[str, Any]:
    """`data.validate`: valida un piano contro gli schemi degli input, senza
    eseguirlo (`plenora-data-plan-validation-result-v1`).

    `inputs` dà una tabella per ogni input del piano, per nome: né di più
    né di meno.
    """
    chiamata, gettoni = _prepara_validate(plan, inputs, timeout, deadline, cancel)
    return chiamata(gettoni)


async def avalidate(
    plan: Plan,
    inputs: Mapping[str, Source] | None = None,
    *,
    timeout: float | None = None,
    deadline: datetime | None = None,
    cancel: CancellationToken | None = None,
) -> dict[str, Any]:
    """`data.validate`, forma asincrona di `validate`."""
    chiamata, gettoni = _prepara_validate(plan, inputs, timeout, deadline, cancel)
    return await _in_thread(chiamata, gettoni)


def run(
    plan: Plan,
    inputs: Mapping[str, Source] | None = None,
    *,
    outputs: Mapping[str, Destination] | None = None,
    overwrite: bool = False,
    timeout: float | None = None,
    deadline: datetime | None = None,
    cancel: CancellationToken | None = None,
) -> RunResult:
    """`data.run`: esegue un piano (`plenora-data-execution-result-v2`).

    Senza `outputs` le tabelle d'uscita tornano in `RunResult.tables` come
    `pyarrow.Table`, e nulla si scrive. Con `outputs` (un percorso per ogni
    output del piano, nessuno di più) le tabelle si scrivono su file, in
    modo atomico ciascuna; una destinazione esistente è un errore
    `conflict` senza `overwrite=True`.
    """
    chiamata, gettoni = _prepara_run(
        plan, inputs, outputs, overwrite, timeout, deadline, cancel
    )
    return chiamata(gettoni)


async def arun(
    plan: Plan,
    inputs: Mapping[str, Source] | None = None,
    *,
    outputs: Mapping[str, Destination] | None = None,
    overwrite: bool = False,
    timeout: float | None = None,
    deadline: datetime | None = None,
    cancel: CancellationToken | None = None,
) -> RunResult:
    """`data.run`, forma asincrona di `run`."""
    chiamata, gettoni = _prepara_run(
        plan, inputs, outputs, overwrite, timeout, deadline, cancel
    )
    # Con `outputs` il lavoro scrive file: un risultato finito e non
    # consegnato ha effetto `committed`.
    return await _in_thread(chiamata, gettoni, con_effetti=outputs is not None)
