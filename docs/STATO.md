# Stato

<!-- Generato da `python scripts/genera_stato.py`: non si modifica a mano. -->

Che cosa il codice dichiara oggi, letto dal codice. Che cosa manca sta in
[`stato-e-roadmap.md`](stato-e-roadmap.md).

## Versioni

| | |
|---|---|
| workspace | `1.0.3` |
| toolchain Rust | `1.98.0` |
| versioni del piano DAG riconosciute | v4, v5, v6 — la v4 si migra alla v5 ([`piano-v5.md`](piano-v5.md)) |

## Catalogo

146 operazioni, in [`operazioni.md`](operazioni.md).

| famiglia | operazioni |
|---|---|
| `geo` | 75 |
| `table` | 71 |

| maturità | operazioni |
|---|---|
| `backend_pending` | 4 |
| `kernel_validated` | 76 |
| `public_protocol` | 66 |

## Comandi della CLI

- `catalog`
- `describe` (anche `inspect-dataset`)
- `validate`
- `run`
- `capabilities`
- `transform`
- `transform-arrow`
- `pair-arrow`
- `spatial-join`
- `self-test`

## Feature della CLI

| feature | abilita |
|---|---|
| `default` | `[]` |
| `geos-backend` | `["plenora-engine/geos-backend", "plenora-kernels-geo/geos-backend"]` |
| `proj-backend` | `["plenora-engine/proj-backend", "plenora-kernels-geo/proj-backend"]` |
| `full-backends` | `["geos-backend", "proj-backend"]` |

## Job di CI

`.github/workflows/ci.yml`:

- test ${{ matrix.os }}
- gate assert nel codice di produzione
- gate pin delle action
- gate commenti al presente
- test full-backends (linux)
- gate coverage (linux)
- gate documentazione
- gate manifesto storico ${{ matrix.manifest }}
- gate test dei checker di rilascio
- gate audit supply-chain
- gate compilazione target fuzz

`.github/workflows/fuzz.yml`:

- elenco dei target
- fuzz ${{ matrix.target }}

## Gate Python

| script | che cosa presidia |
|---|---|
| `verifica_assenza_assert.py` | Gate: nessuna macro di assert nel codice di produzione. |
| `verifica_commenti.py` | Gate: i commenti dicono il presente, non il diario del lavoro. |
| `verifica_documentazione.py` | Gate: la documentazione resta risolvibile e allineata al codice. |
| `verifica_filtro_sperimentale.py` | Prova la provenienza del vendor FILTRATO sperimentale (`vendor/geo-0.33.1-exact-filtered`), separata dalla provenienza del candidato congelato adottato (quella resta `verifica_vendor_provenienza.py`, non toccata da questo script). |
| `verifica_memoria_governata.py` | Verifica che la memoria governata sia ancora quella descritta. |
| `verifica_nome_budget_memoria.py` | Gate: il nome della v4 non torna nel codice, e il nome della v5 non entra dove non appartiene. |
| `verifica_pin_workflow.py` | Gate: ogni action dei workflow e' riferita a una SHA completa. |
| `verifica_privacy_dipendenze.py` | Presidio AGGIUNTIVO sulla privacy dei messaggi di errore (vedi `errori-e-limiti.md#privacy-dei-messaggi`), non prova completa. |
| `verifica_risoluzione_vendor.py` | Prova che geo/wkt/i_shape risolvano davvero al vendor patchato, nel workspace principale E in `fuzz/` (workspace a se', non eredita `[patch]`). |
| `verifica_target_fuzz.py` | Gate: ogni target fuzz installa l'hook comune dei panici. |
| `verifica_vendor_provenienza.py` | Prova la provenienza di vendor/{geo,wkt,i_shape}-*: pacchetto .crate verificato per checksum + patch congelate applicate in ordine, digest dell'albero risultante confrontato con quanto committato. |

## Target fuzz

20 target, dai `[[bin]]` di `fuzz/Cargo.toml`: `plan_contract`, `string_chain`, `candidate_chain`, `binary_ops`, `reshape_policies`, `extended_ops`, `advanced_ops`, `wkb_contract`, `wkt_operations`, `arrow_envelope`, `arrow_ipc_decode`, `arrow_transform`, `plan_v5_parse`, `analyze_table`, `analyze_geo`, `diff_kernels`, `executor_dag`, `protocollo_frame`, `verifica_artefatto`, `geo_frame_stream`.

## Test

Occorrenze di `#[test]` nel sorgente, per crate: un conteggio statico, non l'esito di un'esecuzione, e comprende i test dietro feature e piattaforme.

| crate | `#[test]` |
|---|---|
| `plenora-cli` | 203 |
| `plenora-core` | 144 |
| `plenora-engine` | 1300 |
| `plenora-kernels-geo` | 376 |
| `plenora-kernels-table` | 413 |
| **totale** | **2436** |
