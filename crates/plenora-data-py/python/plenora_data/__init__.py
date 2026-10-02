"""plenora-data: l'SDK Python di plenora-data-tools.

Le quattro operazioni del catalogo pubblico, ognuna in forma sincrona e
asincrona con la stessa semantica:

| operazione      | sincrona   | asincrona   |
| --------------- | ---------- | ----------- |
| `data.catalog`  | `catalog`  | `acatalog`  |
| `data.describe` | `describe` | `adescribe` |
| `data.validate` | `validate` | `avalidate` |
| `data.run`      | `run`      | `arun`      |

Scoperta: `version()` e `capabilities()` (documento `capabilities-v2`).
Le tabelle entrano come percorsi o oggetti Arrow (`pyarrow.Table`,
`RecordBatch`, `RecordBatchReader`, o qualunque `__arrow_c_stream__`) ed
escono come `pyarrow.Table`; gli errori sono la gerarchia `PlenoraError`
(`plenora_data.errors`). Scadenza (`timeout`, `deadline`) e annullamento
(`CancellationToken`, Ctrl-C, annullamento del task asyncio) valgono per
`describe`, `validate` e `run`.

I nomi pubblici sono quelli di `__all__`; `plenora_data._native` e i moduli
con il trattino basso non sono API.
"""

from ._api import (
    ArrowStreamExportable,
    CancellationToken,
    Destination,
    Plan,
    RunResult,
    Source,
    acatalog,
    adescribe,
    arun,
    avalidate,
    capabilities,
    catalog,
    describe,
    run,
    validate,
    version,
)
from .errors import (
    PlenoraAuthenticationError,
    PlenoraAuthorizationError,
    PlenoraCancelledError,
    PlenoraConcurrentModificationError,
    PlenoraConflictError,
    PlenoraCrsError,
    PlenoraDataMappingError,
    PlenoraError,
    PlenoraExecutionError,
    PlenoraInternalError,
    PlenoraInvalidConfigurationError,
    PlenoraInvalidPlanError,
    PlenoraIoError,
    PlenoraNotFoundError,
    PlenoraProtocolError,
    PlenoraResourceLimitError,
    PlenoraSchemaError,
    PlenoraTimeoutError,
    PlenoraTransientError,
    PlenoraUnsupportedError,
)

__all__ = [
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
]
