# plenora-data-tools2

Kernel tabellari e geografici su Arrow `RecordBatch`, in Rust puro: tabelle
in ingresso, una trasformazione, tabelle in uscita.

Deriva da `plenora-data-tools` al commit `190c493` (fase 1 di un successore
semplificato). I nomi dei crate sono rimasti quelli, così le correzioni del
progetto d'origine si portano qui senza rinomine.

## Che cosa c'è

| crate | contenuto |
| --- | --- |
| `plenora-core` | re-export Arrow, `PlenoraError`, limiti, catalogo delle operazioni, contratti dati, contratto CRS fail-closed e riproiezione fra i CRS integrati ([«Riproiezione»](docs/riproiezione.md#riproiezione)), politica dei panici |
| `plenora-kernels-table` | kernel tabellari (filtri, ordinamenti, aggregazioni, join, espressioni, date, stringhe, qualità), tutti in memoria |
| `plenora-kernels-geo` | kernel geografici su `geo::Geometry` e adapter GeoArrow-WKB; `rust_backend` per `geo.make_valid`, `geo.polygonize` e `geo.split` senza GEOS, e i controlli di precisione della griglia degli overlay (`rust_backend::griglia`); `riproiezione` per `geo.reproject` senza PROJ |
| `plenora-pipeline` | runner minimo: piano SSA di operazioni tabellari e geo, validazione senza dati, esecuzione su tabelle intere con byte vivi contati per allocazione e budget di memoria per passo ([«Runner»](docs/runner.md#runner)) |
| `plenora-io` | tabelle da e verso file: Arrow IPC (file e stream), Parquet, GeoParquet 1.1; scrittura atomica; un piano da file a file ([«File»](docs/file.md#file)) |
| `plenora-cli` | la CLI pubblica `plenora-data` (CLI 2.0 di `plenora-contracts`: `catalog`, `describe`, `validate`, `run`, `capabilities`) e la stessa superficie in Rust ([«CLI `plenora-data`»](docs/cli.md#cli-plenora-data)) |
| `vendor/` | `geo` (con il porting a `i_overlay` 9.0.0) e `wkt` con le patch di `patches/` (provenienza in `vendor/*/PROVENANCE*.md`) |

## Che cosa non c'è ancora

- **Risoluzione CRS fuori tabella**: senza PROJ `resolve_crs` risolve solo
  gli identificatori d'autorità della tabella integrata
  ([«CRS integrati»](docs/crs.md#crs-integrati)); un codice fuori tabella fallisce
  chiuso con `CRS_NOT_BUILTIN`, una definizione WKT, WKT2, PROJJSON o
  proj-string con `CRS_BACKEND_UNAVAILABLE`, e un CRS così entra solo già
  risolto dal chiamante.

Engine, CLI, isolamento e protocollo del progetto d'origine non sono stati
portati: la CLI `plenora-data` è nuova, scritta sui contratti pubblici
([«CLI `plenora-data`»](docs/cli.md#cli-plenora-data)).

La CI (`.github/workflows/ci.yml`) esegue i gate di
[`AGENTS.md`](AGENTS.md) su Linux e Windows a ogni push su `main` e a ogni
pull request, con la suite lunga, e verifica la CLI contro
`plenora-contracts` al commit fissato. Non ci sono ancora, e si dichiarano
assenti: fuzzing, misura della copertura, mutation testing e controllo della
catena delle dipendenze. Non sono in programma l'identità del piano (`plan_hash`,
fingerprint del catalogo) e l'interruzione di un kernel a metà: scadenza e
annullamento si controllano solo fra i passi
([«Scadenza e annullamento»](docs/runner.md#scadenza-e-annullamento)).

Lo stato generato dai sorgenti (crate, operazioni del catalogo, operazioni
pubbliche della CLI, numero dei test) è in
[`docs/inventario.md`](docs/inventario.md).

## Le operazioni

Il riferimento delle operazioni del catalogo è
[`docs/operazioni.md`](docs/operazioni.md): una scheda per operazione con
parametri, schema d'uscita, semantica delle righe, ordine, errori,
complessità e un esempio. Le regole e i limiti dichiarati stanno nelle guide
di [`docs/`](docs/README.md); le schede li collegano senza ripeterli.

`docs/operazioni.md` è generato dalle schede `docs/schede/<id>.md` e dal
catalogo (`plenora_core::catalog`) da `crates/plenora-io/tests/operazioni_doc.rs`,
che esegue anche l'esempio di ogni scheda e fallisce se il documento non è
aggiornato. Un'operazione nuova o cambiata aggiorna la sua scheda, poi:

```sh
PLENORA_RIGENERA_DOC=1 cargo test -p plenora-io --test operazioni_doc
```

## Orientarsi nel repository

| serve | sta in |
| --- | --- |
| stato generato: crate, operazioni, CLI, test | [`docs/inventario.md`](docs/inventario.md) |
| le operazioni, una scheda ciascuna | [`docs/operazioni.md`](docs/operazioni.md) |
| il registro dei limiti dichiarati | [`docs/limiti.md`](docs/limiti.md) |
| `geo.make_valid`, `geo.polygonize`, `geo.split` senza GEOS | [`docs/topologia.md`](docs/topologia.md) |
| categorie d'errore ed effetto di un errore | [`docs/errori.md`](docs/errori.md) |
| piano, validazione, esecuzione, budget di memoria | [`docs/runner.md`](docs/runner.md) |
| metadati Arrow in ingresso e in uscita | [`docs/metadati-arrow.md`](docs/metadati-arrow.md) |
| formati, GeoParquet, confine di lettura | [`docs/file.md`](docs/file.md) |
| la CLI `plenora-data`, uscite, codici, deviazioni | [`docs/cli.md`](docs/cli.md) |
| CRS integrati e riproiezione | [`docs/crs.md`](docs/crs.md), [`docs/riproiezione.md`](docs/riproiezione.md) |
| l'indice delle guide | [`docs/README.md`](docs/README.md) |
| regole non negoziabili del repository | [`AGENTS.md`](AGENTS.md) |
| perché una decisione è stata presa | `git log` |

I documenti generati (`docs/inventario.md`, `docs/operazioni.md`) non si
modificano a mano: cambia la sorgente e si rigenera.

## Costruire e provare

Serve `rustup`: la toolchain (1.98.0) è fissata in `rust-toolchain.toml`.
L'unico codice nativo è libzstd, che `zstd-sys` compila con `cc` (niente
cmake): basta il compilatore C che il linker del target già richiede
(MSVC su Windows, `cc` su Linux).

```sh
cargo build --workspace --locked
cargo test --workspace --locked
```

### Suite lunga

Gli oracoli di massa (differenziali della validazione OGC, proptest con
migliaia di casi, soglie e configurazioni esplorate una per una) hanno due
misure. Senza variabili `cargo test` gira un sottoinsieme deterministico
degli stessi casi, scelto perché ogni ramo resti coperto; con
`PLENORA_TEST_LUNGHI=1` gira tutto:

```sh
PLENORA_TEST_LUNGHI=1 cargo test --workspace --locked
```

La suite lunga è il gate prima del merge; quella di default serve mentre si
lavora. Un valore diverso da `0` e `1` ferma i test che la leggono.

I gate completi prima di un commit sono in [`AGENTS.md`](AGENTS.md).

### Cosa fa girare le prove

I comandi dei gate, gli stessi che la CI (`.github/workflows/ci.yml`)
esegue su Linux e Windows:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo clippy --workspace --lib --locked -- -D unsafe-code \
  -D clippy::unwrap_used -D clippy::expect_used -D clippy::panic \
  -D clippy::unreachable -D clippy::todo -D clippy::unimplemented
cargo test --workspace --locked
PLENORA_TEST_LUNGHI=1 cargo test --workspace --locked   # prima del merge
```

Modelli di costo e inventario generato hanno guardie Python (3.11 o
successivo, solo libreria standard):

```sh
python scripts/genera_costi_operazioni.py --verifica  # modello di costo tabellare
python scripts/genera_costi_geo.py --verifica         # modello di costo geo
python scripts/genera_inventario.py --check           # docs/inventario.md aggiornato
```
