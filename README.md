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
| `plenora-data-py` | l'SDK Python `plenora-data` (`plenora_data`): le stesse operazioni su tabelle PyArrow o file ([«SDK Python»](#sdk-python)) |
| `vendor/` | `geo` (con il porting a `i_overlay` 9.0.0), `wkt` e `parquet` (il protocollo thrift che non girava a vuoto su file malformati) con le patch di `patches/` (provenienza in `vendor/*/PROVENANCE*.md`) |

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
`plenora-contracts` al commit fissato; compila i target di fuzz, che
`.github/workflows/fuzz.yml` fa girare ogni settimana;
`.github/workflows/supply-chain.yml` controlla la catena delle dipendenze e
`.github/workflows/coverage.yml` misura la copertura contro le soglie di
`scripts/coverage_budget.json` ([«Fuzz»](#fuzz), [«Catena delle dipendenze»](#catena-delle-dipendenze),
[«Copertura»](#copertura)). Non c'è ancora, e si dichiara assente, il
mutation testing. Non sono in programma l'identità del piano (`plan_hash`,
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
| l'SDK Python `plenora-data`: uso, errori, controlli, limiti, contratti | [`crates/plenora-data-py/README.md`](crates/plenora-data-py/README.md) |
| CRS integrati e riproiezione | [`docs/crs.md`](docs/crs.md), [`docs/riproiezione.md`](docs/riproiezione.md) |
| l'indice delle guide | [`docs/README.md`](docs/README.md) |
| regole non negoziabili del repository | [`AGENTS.md`](AGENTS.md) |
| perché una decisione è stata presa | `git log` |

I documenti generati (`docs/inventario.md`, `docs/operazioni.md`) non si
modificano a mano: cambia la sorgente e si rigenera.

## SDK Python

`crates/plenora-data-py` è l'SDK Python del componente (*Python SDK 1.0*):
distribuzione `plenora-data`, pacchetto `plenora_data`, le quattro
operazioni della CLI in forma sincrona e asincrona su tabelle PyArrow o
file, chiamando le stesse funzioni di `plenora_cli::api`. Uso, errori,
scadenza e annullamento, contratti adottati e limiti dichiarati sono nel
[README del crate](crates/plenora-data-py/README.md); la CI è
`.github/workflows/sdk-python.yml`.

## Costruire e provare

Serve `rustup`: la toolchain (1.98.0) è fissata in `rust-toolchain.toml`.
Il workspace comprende il modulo nativo dell'SDK Python (PyO3), che si
collega a libpython: `cargo build` e `cargo test` vogliono un Python >= 3.10
nel `PATH` (o in `PYO3_PYTHON`).
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

Modelli di costo, documentazione e commenti hanno guardie Python (3.11 o
successivo, solo libreria standard):

```sh
python scripts/genera_costi_operazioni.py --verifica  # modello di costo tabellare
python scripts/genera_costi_geo.py --verifica         # modello di costo geo
python scripts/genera_inventario.py --check           # docs/inventario.md aggiornato
python scripts/check_docs.py                          # link, ancore, comandi, generati
python scripts/check_comments.py                      # regole oggettive dei commenti
python -m unittest discover -s scripts -p "test_*.py" # prove delle guardie
```

### Fuzz

`fuzz/` è un crate cargo-fuzz con workspace e lock propri, così nightly e
libFuzzer restano fuori dalla toolchain fissata; le versioni comuni sono
quelle di `Cargo.lock`. La CI lo compila soltanto (`cargo check`, Linux e
Windows), con `cargo fmt`. Ogni target ha in testa le invarianti che prova.
Oltre a «mai panico», gli errori si controllano su ogni asse: mai
`Internal` (salvo quello documentato di una dipendenza che va in panico in
una barriera durante quell'ingresso), mai un valore di cella nel testo o
nella diagnostica per riga (una sentinella nelle celle), e due esecuzioni
uguali in testo, categoria, fase e diagnostica. Poi ognuno ha un oracolo:

| target | superficie | oracolo |
| --- | --- | --- |
| `piano` | `Pipeline::from_json`, `validate` | round-trip del piano letto, validazione deterministica, rifiuti `InvalidPlan` |
| `esecuzione_tabellare` | ogni operazione tabellare del runner, dati generati | il runner contro la chiamata diretta del kernel (uuid solo nella forma), due esecuzioni uguali |
| `ordinamento`, `ordinamento_parallelo` | `table.sort` su dieci tipi, null, discendente; il secondo oltre 32 768 righe, nel ramo parallelo | un comparatore scritto nel target e l'ordinamento stabile di `std`; righe spostate intere; idempotenza |
| `esecuzione_geo` | `geo.from_wkt` e un'operazione geo, WKT arbitrario | due esecuzioni uguali; ogni cella geometria d'uscita si rilegge |
| `wkb` | decoder WKB dei kernel e camminata WKB dei file | ricodifica rileggibile e stabile; ciò che il decoder accetta il confine dei file lo accetta |
| `lettura_ipc`, `lettura_parquet` | confine di lettura dei file | due letture uguali; riscritta (IPC file e stream, Parquet) e riletta è la stessa, o per Parquet con geometrie la trasformazione GeoParquet documentata |
| `argomenti_cli` | grammatica della CLI | rifiuti `InvalidConfiguration`, lettura deterministica |

`.github/workflows/fuzz.yml` fa girare ogni target ogni domenica (e a mano)
per 10 minuti, e per un minuto sulle PR che toccano `fuzz/`, con i corpus
iniziali di `scripts/genera_corpus_fuzz.py`; un crash fa fallire il job e i
reperti restano come artefatto. In locale le
campagne vogliono Linux (o WSL) con nightly e `cargo-fuzz`:

```sh
cargo +nightly fuzz build -O
cargo +nightly fuzz run esecuzione_tabellare -- -max_total_time=600
```

`lettura_parquet` gira con `-fork=1 -ignore_ooms=1 -malloc_limit_mb=1024
-rss_limit_mb=2048`: dentro `parquet` restano allocazioni dalle dimensioni
dichiarate nel file (limite «File costruiti apposta» in
[`docs/file.md`](docs/file.md#limiti-dichiarati)), e un caso di sola memoria
si conta e si salva senza fermare la campagna, che resta sui blocchi e sui
risultati sbagliati. Senza tetto, un caso così può far ripartire la VM di
WSL.

I panici attesi delle dipendenze dentro una barriera
(`plenora_core::panic_policy::barriera_di_dipendenza`) non fermano il fuzzer;
ogni altro panico sì, anche se una rete di sicurezza lo intercetta
(`fuzz/fuzz_targets/comune/aggancio.rs`). Corpus, crash e build restano
fuori da Git.

### Catena delle dipendenze

`deny.toml` è la policy di cargo-deny: advisory rifiutate senza eccezioni,
licenze permissive in allowlist, solo crates.io o le patch di `vendor/`. Si
applica al grafo del workspace e a quello di `fuzz/`:

```sh
python scripts/check_cargo_deny.py   # in Docker, cargo-deny 0.20.2 fissato
cargo deny check && cargo deny --manifest-path fuzz/Cargo.toml check   # se installato
```

La CI la esegue a ogni push e PR e ogni lunedì
(`.github/workflows/supply-chain.yml`): un'advisory nuova può farla
diventare rossa senza che il codice cambi.

### Copertura

`.github/workflows/coverage.yml`, a ogni push e PR, misura tre superfici con
soglie separate in `scripts/coverage_budget.json`: il prodotto Rust
(`cargo-llvm-cov`, senza l'SDK), l'SDK Python (coverage.py, righe e rami) e il
suo binding nativo, misurato dal wheel instrumentato mentre gira la suite
Python. `scripts/check_coverage.py` fallisce chiuso su un report incompleto o
incoerente. In locale:

```sh
cargo llvm-cov --workspace --exclude plenora-data-py --summary-only
```

### Rilascio

GitHub Releases è l'unica distribuzione. Pubblicare una release con tag
`v<versione del workspace>` avvia `.github/workflows/rilascio.yml`, che
costruisce e prova gli artefatti e li allega alla release:

| artefatto | superficie |
| --- | --- |
| `plenora-data-linux-x86_64`, `plenora-data-windows-x86_64.exe` | CLI |
| `plenora_data-<versione>-cp310-abi3-*.whl` (manylinux 2_34, Windows) | SDK Python |
| `plenora-data-tools-<versione>-source.tar.gz` | Rust (crate dal sorgente) |
| `plenora-data-tools-<versione>.sbom.cdx.json` | SBOM CycloneDX (`scripts/genera_sbom_rilascio.py`) |
| `plenora-data-tools-<versione>.adoption-manifest.json` | manifesto di adozione v4 |

Prima degli allegati: ogni wheel installato con la suite Python completa su
Python 3.10–3.14, Linux e Windows; la CLI Linux contro `plenora-contracts` al
commit adottato (`scripts/verifica_cli_contratti.py`); il manifesto generato
dagli artefatti veri e validato da schema e controlli dei contratti; lo SBOM
verificato contro i lock; poi le attestazioni di provenienza. Lanciato a mano
(`workflow_dispatch`) il workflow fa gli stessi passi senza toccare release.
