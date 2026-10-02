# Stub del modulo nativo privato `plenora_data._native` (crates/plenora-data-py/src/lib.rs).
# Non e' API pubblica: la usa solo `plenora_data._api`, che valida gli
# argomenti prima di arrivare qui. I documenti escono come testo JSON.
# `scadenza` e' un istante in secondi dall'epoca Unix (`deadline`),
# `scadenza_monotona` un istante di `time.monotonic()` fissato
# all'ingresso della chiamata pubblica (`timeout`).

from collections.abc import Callable
from typing import final

@final
class CancellationToken:
    """Segnale di annullamento cooperativo, condivisibile fra thread."""

    def __init__(self) -> None: ...
    def cancel(self) -> None:
        """Alza il segnale (idempotente): l'operazione si ferma al suo
        controllo successivo con `PlenoraCancelledError`."""
    @property
    def cancelled(self) -> bool:
        """Se il segnale e' alzato."""

def version() -> str: ...
def _sonda_consegna(sonda: Callable[[], object] | None) -> None:
    """Solo per le prove: un callable chiamato fra la fine del lavoro e il
    controllo della consegna; None lo toglie."""
def capabilities() -> str: ...
def catalog() -> str: ...
def describe(
    tipo: str,
    sorgente: object,
    scadenza: float | None,
    scadenza_monotona: float | None,
    gettoni: list[CancellationToken],
) -> str: ...
def validate(
    tipo_piano: str,
    piano_dato: object,
    voci: list[tuple[str, str, object]],
    scadenza: float | None,
    scadenza_monotona: float | None,
    gettoni: list[CancellationToken],
) -> str: ...
def run(
    tipo_piano: str,
    piano_dato: object,
    voci: list[tuple[str, str, object]],
    uscite: list[tuple[str, object]] | None,
    sovrascrivi: bool,
    scadenza: float | None,
    scadenza_monotona: float | None,
    gettoni: list[CancellationToken],
) -> tuple[str, list[tuple[str, object]]]: ...
