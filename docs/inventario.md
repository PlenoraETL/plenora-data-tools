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
| `plenora-cli` | 5.0.0 | CLI pubblica `plenora-data` di plenora-data-tools2 (CLI 2.0 di plenora-contracts) e la sua superficie Rust |
| `plenora-core` | 5.0.0 | Fondamenta condivise di plenora-data-tools2: re-export Arrow, errori, limiti, catalogo, CRS |
| `plenora-data-py` | 5.0.0 | SDK Python `plenora-data` di plenora-data-tools2: il modulo nativo PyO3 `plenora_data._native` |
| `plenora-io` | 5.0.0 | Ingresso e uscita su file: Arrow IPC (file e stream), Parquet e GeoParquet 1.1, scrittura atomica, esecuzione di un piano da file a file |
| `plenora-kernels-geo` | 5.0.0 | Kernel geografici su geo::Geometry e adapter Arrow GeoArrow-WKB |
| `plenora-kernels-table` | 5.0.0 | Kernel tabellari puri su RecordBatch |
| `plenora-pipeline` | 5.0.0 | Runner minimo di pipeline: piano SSA di kernel tabellari e geografici su RecordBatch interi in memoria |

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
| `plenora-cli` | 7 | 74 | 81 |
| `plenora-core` | 215 | 103 | 318 |
| `plenora-data-py` | 0 | 0 | 0 |
| `plenora-io` | 20 | 264 | 284 |
| `plenora-kernels-geo` | 533 | 113 | 646 |
| `plenora-kernels-table` | 477 | 138 | 615 |
| `plenora-pipeline` | 12 | 177 | 189 |
| **totale** | 1264 | 869 | 2133 |

### Prove Python dell'SDK

Funzioni `test_*` (anche `async`) della suite pytest di `plenora-data-py`,
che gira sul wheel installato (`scripts/verifica_sdk_python.py`): il
`plenora-data-py` della tabella sopra conta solo i `#[test]` Rust.

| file | prove |
| --- | --- |
| `crates/plenora-data-py/python/tests/test_arrow_import.py` | 9 |
| `crates/plenora-data-py/python/tests/test_controlli.py` | 21 |
| `crates/plenora-data-py/python/tests/test_errori.py` | 12 |
| `crates/plenora-data-py/python/tests/test_operazioni.py` | 15 |
| `crates/plenora-data-py/python/tests/test_qualifica_415.py` | 4 |
| `crates/plenora-data-py/python/tests/test_struct_bitmap.py` | 3 |
| `crates/plenora-data-py/python/tests/test_superficie.py` | 9 |
| **totale** | 73 |

### Prove delle guardie

Funzioni `test_*` dei self-test Python in `scripts/`.

| file | prove |
| --- | --- |
| `scripts/test_allineamento_dipendenze.py` | 2 |
| `scripts/test_arrow_ingressi.py` | 16 |
| `scripts/test_avvisi_crate.py` | 18 |
| `scripts/test_check_cargo_deny.py` | 4 |
| `scripts/test_check_comments.py` | 27 |
| `scripts/test_check_coverage.py` | 10 |
| `scripts/test_check_docs.py` | 14 |
| `scripts/test_check_esito_unico.py` | 23 |
| `scripts/test_clippy_esito.py` | 1 |
| `scripts/test_fuzz_crs.py` | 4 |
| `scripts/test_genera_inventario.py` | 6 |
| `scripts/test_genera_sbom_rilascio.py` | 5 |
| `scripts/test_geometry_metadata_guard.py` | 4 |
| `scripts/test_geometry_text_classification.py` | 1 |
| `scripts/test_prove_fonti.py` | 14 |
| `scripts/test_python_version.py` | 4 |
| `scripts/test_rust_esito_tokens.py` | 11 |
| `scripts/test_semantica_crs.py` | 2 |
| `scripts/test_strict_json.py` | 5 |
| `scripts/test_verifica_adozione.py` | 14 |
| `scripts/test_verifica_sdk_python.py` | 2 |
