# plenora-data-tools

Motore Rust Arrow-in/Arrow-out per pipeline dichiarative di trasformazione
tabellare e geografica. Valida integralmente il piano e i contratti prima
dell'esecuzione, applica limiti di risorsa e pubblica gli output in modo
atomico.

> Stato: patch candidate `1.0.1` in preparazione, senza tag né pubblicazione.
> La versione e le evidenze effettivamente qualificate sono registrate nei
> manifesti sotto `release/`; questo README non sostituisce il gate di release.

## Ruolo nell'ecosistema Plenora

```text
plenora-IO-tools       file e formati  <-> Arrow
plenora-data-tools     Arrow           <-> Arrow
plenora-database-tools database        <-> Arrow
```

I tre componenti comunicano tramite schema Arrow e metadata canonici definiti
da `plenora-contracts`. `plenora-data-tools` non legge direttamente CSV, XLSX,
SHP, GeoPackage o database: riceve `RecordBatch`/Arrow IPC prodotti dai
componenti di bordo, preserva il contratto e restituisce Arrow.

## Requisiti

- Rust `1.92` (fissato in `rust-toolchain.toml`);
- dipendenze bloccate da `Cargo.lock`;
- CMake, Clang e SQLite per il backend PROJ bundled;
- toolchain native GEOS/PROJ solo quando richieste dalle relative feature.

Il workspace usa Arrow `59.1.0` con pin esatti.

## Build e test

```sh
cargo build --workspace --locked
cargo test --workspace --no-fail-fast --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
```

Esecuzione riproducibile in container:

```sh
docker run --rm -v "$PWD:/work" -w /work rust:1.92 \
  cargo test --workspace --no-fail-fast --locked
```

Il gate safety-critical completo, inclusi i lint anti-panic sulle librerie, è
documentato in [`AGENTS.md`](AGENTS.md).

## Feature

La CLI non abilita backend geografici per impostazione predefinita:

- `geos-backend`: operazioni che richiedono GEOS;
- `proj-backend`: risoluzione e riproiezione CRS tramite PROJ;
- `full-backends`: abilita entrambi.

Esempio:

```sh
cargo build -p plenora-cli --features full-backends --locked
```

## CLI

```sh
cargo run -p plenora-cli -- --help
cargo run -p plenora-cli -- catalog
cargo run -p plenora-cli -- capabilities
cargo run -p plenora-cli -- validate \
  --plan plan.json --inputs input.arrow
cargo run -p plenora-cli -- run \
  --plan plan.json --inputs input.arrow --output output.arrow
```

`validate` non esegue il piano. `run` accetta piani legacy fino alla versione 3
e DAG v4; per i DAG v4 i percorsi di `--inputs` seguono l'ordine degli input
dichiarati nel piano.

Gli errori CLI sono emessi su stderr come envelope JSON con categoria, fase,
effetto remoto e disposizione di retry. Gli output esistenti non vengono
sovrascritti silenziosamente.

## Contratti e compatibilità

- Nessun CRS predefinito e nessuna riproiezione implicita.
- Le dichiarazioni CRS incompatibili non vengono conciliate silenziosamente.
- Schema, metadata, nullability e dimensionalità fanno parte del contratto.
- Arrow è parte della superficie pubblica Rust: un aggiornamento maggiore o
  incompatibile è trattato come cambiamento potenzialmente breaking.
- Le baseline normative e Git esatte appartengono ai documenti di release, non
  a riferimenti mobili in questo file.

Per il modello completo vedere:

- [`Architetture.md`](Architetture.md);
- [`Prestazioni.md`](Prestazioni.md);
- [`docs/adr/`](docs/adr/);
- [`docs/deroghe.md`](docs/deroghe.md).

## Release

Una release stabile richiede sulla stessa revisione immutabile:

1. test default e all-features;
2. Clippy, safety lint e build sulle piattaforme dichiarate;
3. conformance e roundtrip con `plenora-IO-tools` e
   `plenora-database-tools`;
4. aggiornamento del manifesto `release/` e della baseline normativa;
5. revisione indipendente prima del tag.

Il bump di versione, il tag e la pubblicazione non sono impliciti nel semplice
superamento della suite locale.
