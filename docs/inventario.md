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
| `plenora-cli` | 0.1.0 | CLI pubblica `plenora-data` di plenora-data-tools2 (CLI 2.0 di plenora-contracts) e la sua superficie Rust |
| `plenora-core` | 0.1.0 | Fondamenta condivise di plenora-data-tools2: re-export Arrow, errori, limiti, catalogo, CRS |
| `plenora-io` | 0.1.0 | Ingresso e uscita su file: Arrow IPC (file e stream), Parquet e GeoParquet 1.1, scrittura atomica, esecuzione di un piano da file a file |
| `plenora-kernels-geo` | 0.1.0 | Kernel geografici su geo::Geometry e adapter Arrow GeoArrow-WKB |
| `plenora-kernels-table` | 0.1.0 | Kernel tabellari puri su RecordBatch |
| `plenora-pipeline` | 0.1.0 | Runner minimo di pipeline: piano SSA di kernel tabellari e geografici su RecordBatch interi in memoria |

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
| `data.catalog` | 1 | `catalog` | `Nessuno` | false | false | false |
| `data.describe` | 1 | `describe` | `Nessuno` | true | true | true |
| `data.validate` | 1 | `validate` | `Nessuno` | true | true | true |
| `data.run` | 1 | `run` | `Locale` | true | true | true |

## Test

Funzioni annotate `#[test]` (comprese quelle dentro `proptest!`), per
crate: nei sorgenti i test unitari, in `tests/` quelli d'integrazione.
Doctest ed esempi non sono contati. Quanti casi gira un test dipende
dalla suite ([«Suite lunga»](../README.md#suite-lunga)).

| crate | unitari | integrazione | totale |
| --- | --- | --- | --- |
| `plenora-cli` | 3 | 25 | 28 |
| `plenora-core` | 172 | 62 | 234 |
| `plenora-io` | 19 | 73 | 92 |
| `plenora-kernels-geo` | 513 | 108 | 621 |
| `plenora-kernels-table` | 456 | 118 | 574 |
| `plenora-pipeline` | 10 | 141 | 151 |
| **totale** | 1173 | 527 | 1700 |

### Prove delle guardie

Funzioni `test_*` dei self-test Python in `scripts/`.

| file | prove |
| --- | --- |
| `scripts/test_check_comments.py` | 20 |
| `scripts/test_check_docs.py` | 12 |
| `scripts/test_genera_inventario.py` | 5 |
