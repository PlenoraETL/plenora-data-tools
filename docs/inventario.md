# Inventario del codice

<!-- Generato da scripts/genera_inventario.py: non si modifica a mano.
Rigenerare con python scripts/genera_inventario.py -->

Ogni numero qui sotto è letto dai sorgenti. Se è sbagliato, è
sbagliato nel codice, oppure il documento non è stato rigenerato:
`python scripts/genera_inventario.py --check` lo dice, e la CI lo
esegue.

## Crate

| crate | versione | descrizione |
| --- | --- | --- |
| `plenora-cli` | 1.1.0 | CLI pubblica `plenora-data` di plenora-data-tools2 (CLI 2.0 di plenora-contracts) e la sua superficie Rust |
| `plenora-core` | 1.1.0 | Fondamenta condivise di plenora-data-tools2: re-export Arrow, errori, limiti, catalogo, CRS |
| `plenora-data-py` | 1.1.0 | SDK Python `plenora-data` di plenora-data-tools2: il modulo nativo PyO3 `plenora_data._native` |
| `plenora-io` | 1.1.0 | Ingresso e uscita su file: Arrow IPC (file e stream), Parquet e GeoParquet 1.1, scrittura atomica, esecuzione di un piano da file a file |
| `plenora-kernels-geo` | 1.1.0 | Kernel geografici su geo::Geometry e adapter Arrow GeoArrow-WKB |
| `plenora-kernels-table` | 1.1.0 | Kernel tabellari puri su RecordBatch |
| `plenora-pipeline` | 1.1.0 | Runner minimo di pipeline: piano SSA di kernel tabellari e geografici su RecordBatch interi in memoria |

## Catalogo delle operazioni

`plenora_core::catalog::CATALOG` ha 146 operazioni (71 tabellari, 75 geografiche) e `ALIASES` 127 alias. Le schede sono in
[`operazioni.md`](operazioni.md).

### Per famiglia e provenienza

| famiglia | provenienza | operazioni |
| --- | --- | --- |
| `Geo` | `Extension` | 42 |
| `Geo` | `ManipolaCompat` | 33 |
| `Table` | `Extension` | 34 |
| `Table` | `ManipolaCompat` | 37 |

### Per classe di esecuzione

| esecuzione | operazioni |
| --- | --- |
| `BinaryBlocking` | 22 |
| `Blocking` | 31 |
| `Streaming` | 93 |

### Per annullamento

| annullamento | operazioni |
| --- | --- |
| `BoundaryOnly` | 52 |
| `Cooperative` | 90 |
| `NonInterruptible` | 4 |

### Per maturità

| maturita | operazioni |
| --- | --- |
| `KernelValidated` | 80 |
| `PublicProtocol` | 66 |

## Operazioni pubbliche della CLI

Da `plenora_cli::operazioni::OPERAZIONI`, la tabella da cui la CLI
ricava aiuto, `capabilities` e mappa degli export Rust
([`cli.md`](cli.md)).

| id | versione | comando | effetto | annullamento | scadenza | tabelle intere in memoria |
| --- | --- | --- | --- | --- | --- | --- |
| `data.catalog` | 2 | `catalog` | `Nessuno` | false | false | false |
| `data.describe` | 1 | `describe` | `Nessuno` | true | true | true |
| `data.validate` | 2 | `validate` | `Nessuno` | true | true | true |
| `data.run` | 2 | `run` | `Locale` | true | true | true |
| `data.run` | 3 | `` | `Remoto` | true | true | true |

## Test

Funzioni annotate `#[test]` (comprese quelle dentro `proptest!`), per
crate: nei sorgenti i test unitari, in `tests/` quelli d'integrazione.
Doctest ed esempi non sono contati. Quanti casi gira un test dipende
dalla suite ([«Suite lunga»](../README.md#suite-lunga)).

| crate | unitari | integrazione | totale |
| --- | --- | --- | --- |
| `plenora-cli` | 3 | 49 | 52 |
| `plenora-core` | 174 | 65 | 239 |
| `plenora-data-py` | 0 | 0 | 0 |
| `plenora-io` | 19 | 85 | 104 |
| `plenora-kernels-geo` | 515 | 108 | 623 |
| `plenora-kernels-table` | 456 | 118 | 574 |
| `plenora-pipeline` | 10 | 143 | 153 |
| **totale** | 1177 | 568 | 1745 |

### Prove Python dell'SDK

Funzioni `test_*` (anche `async`) della suite pytest di `plenora-data-py`,
che gira sul wheel installato (`scripts/verifica_sdk_python.py`): il
`plenora-data-py` della tabella sopra conta solo i `#[test]` Rust.

| file | prove |
| --- | --- |
| `crates/plenora-data-py/python/tests/test_controlli.py` | 18 |
| `crates/plenora-data-py/python/tests/test_errori.py` | 12 |
| `crates/plenora-data-py/python/tests/test_operazioni.py` | 12 |
| `crates/plenora-data-py/python/tests/test_superficie.py` | 9 |
| **totale** | 51 |

### Prove delle guardie

Funzioni `test_*` dei self-test Python in `scripts/`.

| file | prove |
| --- | --- |
| `scripts/test_check_cargo_deny.py` | 4 |
| `scripts/test_check_comments.py` | 27 |
| `scripts/test_check_coverage.py` | 10 |
| `scripts/test_check_docs.py` | 14 |
| `scripts/test_genera_inventario.py` | 6 |
| `scripts/test_genera_sbom_rilascio.py` | 5 |
