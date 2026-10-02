"""Le eccezioni pubbliche dell'SDK: la radice `PlenoraError` e una
sottoclasse per categoria di `plenora-error-v1`.

Ogni istanza porta come attributi, senza bisogno di leggere il testo:

- `category`, `phase`, `remote_effect` (str, i valori dello schema
  `error-v1` di plenora-contracts);
- `retry` (dict: `{"kind": ...}`, con `delay_ms` solo per `after`);
- `code` (str o None: un codice stabile, per esempio
  `EXECUTION_DEADLINE_EXCEEDED`);
- `message` (str: testo per persone, limitato, senza valori di righe o
  colonne, senza percorsi);
- `details` (dict o None: `{"row_diagnostics": ...}`, il documento
  `plenora-row-diagnostics-v1` con indici, conteggi e codici, mai valori).

`to_dict()` rende il documento `plenora-error-v1` intero. `str(errore)` è il
messaggio. Le eccezioni del modulo nativo nascono da quel documento, lo
stesso che la CLI mette nel suo inviluppo d'errore: la stessa operazione
fallisce con gli stessi assi su ogni superficie.

`PlenoraError` discende da `RuntimeError`, come in plenora-database-tools.
`KeyboardInterrupt` e `asyncio.CancelledError` non diventano mai una
`PlenoraError`: escono come tali, con l'esito dell'operazione interrotta
come `__cause__` quando ce n'è uno.
"""

from __future__ import annotations

import json
from collections.abc import Mapping
from typing import Any

__all__ = [
    "PlenoraError",
    "PlenoraInvalidPlanError",
    "PlenoraInvalidConfigurationError",
    "PlenoraSchemaError",
    "PlenoraDataMappingError",
    "PlenoraCrsError",
    "PlenoraUnsupportedError",
    "PlenoraNotFoundError",
    "PlenoraConflictError",
    "PlenoraConcurrentModificationError",
    "PlenoraAuthenticationError",
    "PlenoraAuthorizationError",
    "PlenoraTimeoutError",
    "PlenoraCancelledError",
    "PlenoraResourceLimitError",
    "PlenoraIoError",
    "PlenoraProtocolError",
    "PlenoraTransientError",
    "PlenoraExecutionError",
    "PlenoraInternalError",
]


class PlenoraError(RuntimeError):
    """Radice delle eccezioni pubbliche (Python SDK 1.0, sezione 6)."""

    category: str
    phase: str
    remote_effect: str
    retry: dict[str, Any]
    code: str | None
    message: str
    details: dict[str, Any] | None

    def __init__(
        self,
        message: str,
        *,
        category: str,
        phase: str,
        remote_effect: str,
        retry: Mapping[str, Any],
        code: str | None = None,
        details: Mapping[str, Any] | None = None,
    ) -> None:
        super().__init__(message)
        self.category = category
        self.phase = phase
        self.remote_effect = remote_effect
        self.retry = dict(retry)
        self.code = code
        self.message = message
        self.details = None if details is None else dict(details)

    @property
    def row_diagnostics(self) -> dict[str, Any] | None:
        """Il documento `plenora-row-diagnostics-v1`, se l'errore ne ha uno."""
        if self.details is None:
            return None
        diagnostica = self.details.get("row_diagnostics")
        return diagnostica if isinstance(diagnostica, dict) else None

    def to_dict(self) -> dict[str, Any]:
        """Il documento `plenora-error-v1` dell'errore."""
        documento: dict[str, Any] = {
            "category": self.category,
            "phase": self.phase,
            "remote_effect": self.remote_effect,
            "retry": dict(self.retry),
            "message": self.message,
        }
        if self.code is not None:
            documento["code"] = self.code
        if self.details is not None:
            documento["details"] = dict(self.details)
        return documento

    def __str__(self) -> str:
        return self.message

    def __repr__(self) -> str:
        return (
            f"{type(self).__name__}(category={self.category!r}, phase={self.phase!r}, "
            f"remote_effect={self.remote_effect!r}, code={self.code!r})"
        )

    def __reduce__(self) -> tuple[Any, ...]:
        # La ricostruzione passa dal documento: `pickle` di default
        # richiamerebbe il costruttore con il solo messaggio.
        return (_da_mappa, (self.to_dict(),))


class PlenoraInvalidPlanError(PlenoraError):
    """`invalid_plan`: il piano non si legge o non si valida."""


class PlenoraInvalidConfigurationError(PlenoraError):
    """`invalid_configuration`: argomenti della chiamata non validi."""


class PlenoraSchemaError(PlenoraError):
    """`schema`: schema degli input incompatibile con il piano."""


class PlenoraDataMappingError(PlenoraError):
    """`data_mapping`: dati che non si possono leggere o convertire."""


class PlenoraCrsError(PlenoraError):
    """`crs`: CRS assente, non risolto o incompatibile."""


class PlenoraUnsupportedError(PlenoraError):
    """`unsupported`: operazione, tipo o forma non supportati."""


class PlenoraNotFoundError(PlenoraError):
    """`not_found`."""


class PlenoraConflictError(PlenoraError):
    """`conflict`: per esempio una destinazione esistente senza `overwrite`."""


class PlenoraConcurrentModificationError(PlenoraError):
    """`concurrent_modification`."""


class PlenoraAuthenticationError(PlenoraError):
    """`authentication`."""


class PlenoraAuthorizationError(PlenoraError):
    """`authorization`."""


class PlenoraTimeoutError(PlenoraError):
    """`timeout`: la scadenza (`timeout` o `deadline`) è passata."""


class PlenoraCancelledError(PlenoraError):
    """`cancelled`: l'operazione è stata annullata."""


class PlenoraResourceLimitError(PlenoraError):
    """`resource_limit`: budget di memoria o limiti del piano superati."""


class PlenoraIoError(PlenoraError):
    """`io`: lettura o scrittura di un file non riuscita."""


class PlenoraProtocolError(PlenoraError):
    """`protocol`."""


class PlenoraTransientError(PlenoraError):
    """`transient`."""


class PlenoraExecutionError(PlenoraError):
    """`execution`: un passo del piano è fallito."""


class PlenoraInternalError(PlenoraError):
    """`internal`: un difetto del componente, mai un errore del chiamante."""


_PER_CATEGORIA: dict[str, type[PlenoraError]] = {
    "invalid_plan": PlenoraInvalidPlanError,
    "invalid_configuration": PlenoraInvalidConfigurationError,
    "schema": PlenoraSchemaError,
    "data_mapping": PlenoraDataMappingError,
    "crs": PlenoraCrsError,
    "unsupported": PlenoraUnsupportedError,
    "not_found": PlenoraNotFoundError,
    "conflict": PlenoraConflictError,
    "concurrent_modification": PlenoraConcurrentModificationError,
    "authentication": PlenoraAuthenticationError,
    "authorization": PlenoraAuthorizationError,
    "timeout": PlenoraTimeoutError,
    "cancelled": PlenoraCancelledError,
    "resource_limit": PlenoraResourceLimitError,
    "io": PlenoraIoError,
    "protocol": PlenoraProtocolError,
    "transient": PlenoraTransientError,
    "execution": PlenoraExecutionError,
    "internal": PlenoraInternalError,
}


def _da_mappa(documento: Mapping[str, Any]) -> PlenoraError:
    """L'eccezione della sua categoria dal documento `plenora-error-v1`."""
    categoria = str(documento["category"])
    classe = _PER_CATEGORIA.get(categoria, PlenoraInternalError)
    return classe(
        str(documento["message"]),
        category=categoria,
        phase=str(documento["phase"]),
        remote_effect=str(documento["remote_effect"]),
        retry=documento["retry"],
        code=documento.get("code"),
        details=documento.get("details"),
    )


def _da_documento(testo: str) -> PlenoraError:
    """Il costruttore che il modulo nativo chiama con il documento JSON."""
    documento = json.loads(testo)
    if not isinstance(documento, dict):
        return _interno("documento d'errore non valido")
    return _da_mappa(documento)


def _configurazione(motivo: str) -> PlenoraInvalidConfigurationError:
    """Un argomento rifiutato prima di chiamare il modulo nativo: fase
    `prepare`, quella che il componente dà agli argomenti rifiutati (anche
    sulla CLI)."""
    return PlenoraInvalidConfigurationError(
        f"invalid configuration: {motivo}",
        category="invalid_configuration",
        phase="prepare",
        remote_effect="none",
        retry={"kind": "never"},
    )


def _dati(motivo: str) -> PlenoraDataMappingError:
    """Un oggetto Arrow che non si lascia leggere mentre si preparano gli
    argomenti, senza il testo della sua eccezione: potrebbe portare dati."""
    return PlenoraDataMappingError(
        f"data mapping: {motivo}",
        category="data_mapping",
        phase="read",
        remote_effect="none",
        retry={"kind": "never"},
    )


def _annullata_alla_consegna(*, con_effetti: bool) -> PlenoraCancelledError:
    """L'esito di un'operazione asincrona finita con successo mentre il suo
    task veniva annullato: il risultato non si consegna, e l'errore dice
    l'effetto vero (`committed` se gli output sono stati scritti; allora il
    ritentativo non è automatico). Gli stessi assi dell'annullamento alla
    consegna del modulo nativo."""
    return PlenoraCancelledError(
        "cancelled: esecuzione annullata alla consegna: il lavoro era finito, "
        "il risultato non si consegna",
        category="cancelled",
        phase="finalize",
        remote_effect="committed" if con_effetti else "none",
        retry={"kind": "requires_recovery"} if con_effetti else {"kind": "safe"},
        code="EXECUTION_CANCELLED",
    )


def _interno(motivo: str, *, remote_effect: str = "none") -> PlenoraInternalError:
    """Un difetto del confine (un'eccezione nativa inattesa), senza il suo
    testo: potrebbe portare dati."""
    return PlenoraInternalError(
        f"internal error: {motivo}",
        category="internal",
        phase="finalize",
        remote_effect=remote_effect,
        retry={"kind": "never"},
    )
