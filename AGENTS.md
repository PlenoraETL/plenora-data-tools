# Standard di lavoro — plenora-data-tools

**Direttiva permanente (2026-07-27, dal maintainer): questo progetto va
trattato come una libreria safety-critical.** Ogni modifica — umana o di
agente — segue queste regole. Non sono opzionali.

## Regole

1. **Niente failure silenziose.** Un risultato sbagliato è sempre peggio di
   un errore. Ordinamenti, confronti, conversioni numeriche e formati dati
   devono essere esatti per costruzione, non "di solito giusti". Se un caso
   limite non è gestibile, si rifiuta l'input con errore esplicito.
2. **I test verdi non bastano.** Ogni modifica a logica critica (contabilità,
   comparatori, serializzazione, concorrenza) richiede revisione del diff da
   parte di un secondo lettore (umano o coordinatore), non solo la sintesi di
   chi ha scritto il codice.
3. **Ogni bug è una classe.** Trovato un bug, si cerca la stessa classe in
   tutto il codebase prima di dichiarare chiusa la fix (esempio: comparatore
   Int64 via f64 trovato in review 2026-07-27 — tre siti, una classe).
4. **Deviazioni esplicite.** Ogni scostamento dai contratti documentati o
   dagli invarianti va scritto nel codice **e** nel registro dei limiti
   (`docs/errori-e-limiti.md`), con regola, ambito, hazard e condizione di
   rientro. Una garanzia indebolita va dichiarata come tale.
5. **Determinismo testato.** architettura.md#determinismo: stesso input → stesso output, sempre.
   Ordine logico (BatchSequence), mai temporale. Le ottimizzazioni si
   verificano con oracoli contro il percorso generico.
6. **Nessun `unsafe`** nel workspace (lint attivo). Nessuna dipendenza nuova
   senza motivazione documentata; pin esatti delle versioni.
7. **Suite completa prima del commit**: `cargo test --workspace
   --no-fail-fast` (container `rust:1.98`). CI su Linux+Windows deve restare
   verde.
8. **Errori senza dati.** Mai valori di righe/colonne nei messaggi di errore
   (regola di `plenora-core/src/error.rs`), neanche in modalità diagnostica.

## Riferimenti

La superficie documentale è **chiusa**: `README.md`, `AGENTS.md` e i sette
documenti sotto `docs/`. Non se ne aggiungono altri senza aggiornare
l'allowlist di `scripts/verifica_documentazione.py`, che li presidia insieme
ai collegamenti interni.

- [`docs/architettura.md`](docs/architettura.md) — crate, flusso, determinismo,
  memoria, backend.
- [`docs/piano-v5.md`](docs/piano-v5.md) — formato del piano, contratti,
  identità, migrazione.
- [`docs/cli.md`](docs/cli.md) — comandi, canali, exit code.
- [`docs/operazioni.md`](docs/operazioni.md) — **generato**: non si modifica a
  mano, si rigenera con `python docs/_build/assemble.py`.
- [`docs/errori-e-limiti.md`](docs/errori-e-limiti.md) — tassonomia, privacy,
  e il **registro dei limiti dichiarati**: ogni limite con regola, ambito,
  hazard e condizione di rientro. È l'unico punto di raccolta.
- [`docs/stato-e-roadmap.md`](docs/stato-e-roadmap.md) — solo il lavoro aperto.
- [`docs/release.md`](docs/release.md) — gate, piattaforme, procedura.
- Catalogo operazioni: snapshot test
  (`crates/plenora-engine/tests/catalog_snapshot.snap`) — ogni cambio di
  catalogo è esplicito in PR.

## Build e test

```sh
# test completi (container, toolchain del progetto)
docker run --rm -v $PWD:/work -w /work rust:1.98 cargo test --workspace --no-fail-fast
# gate R6 (identico alla CI, bloccante): nessuna primitiva di panic nel
# codice di produzione — lib di tutti i crate + bin della CLI.
# MAI aggiungere --cap-lints=warn: cappera' anche i -D espliciti (li
# declassa a warn) e il gate smette di bloccare — regressione trovata il
# 2026-07-29, 27 siti accumulati mentre il gate era inefficace.
cargo clippy -p plenora-core -p plenora-engine -p plenora-kernels-table \
  -p plenora-kernels-geo -p plenora-cli --lib --bins --locked -- -D unsafe-code \
  -D clippy::unwrap_used -D clippy::expect_used -D clippy::panic \
  -D clippy::unreachable -D clippy::todo -D clippy::unimplemented
# stesso gate sul perimetro feature-gated (rami geos/proj, non compilati
# dal comando sopra): richiede cmake + sqlite3, vedi architettura.md#geometrie.
cargo clippy -p plenora-kernels-geo -p plenora-engine -p plenora-cli \
  --lib --bins --locked --features full-backends -- -D unsafe-code \
  -D clippy::unwrap_used -D clippy::expect_used -D clippy::panic \
  -D clippy::unreachable -D clippy::todo -D clippy::unimplemented
# gli altri due passi feature-gated del job `backends`, che qui mancavano: il
# comando sopra copre `--lib --bins`, quindi il codice di TEST dietro
# `cfg(feature = "geos-backend")`/`"proj-backend"` non veniva compilato in
# locale. Un import caduto in un file di test e' arrivato cosi' fino alla CI.
cargo clippy -p plenora-kernels-geo -p plenora-engine -p plenora-cli \
  --all-targets --locked --features full-backends
cargo test -p plenora-kernels-geo -p plenora-engine -p plenora-cli \
  --locked --features full-backends
# gate assert (bloccante, identico alla CI): le macro `assert!`/`assert_eq!`/
# `debug_assert*` sono primitive di panic che clippy non sa nominare, quindi
# il gate R6 non le vede. Perimetro identico: crates/*/src meno il codice di
# test.
python scripts/verifica_assenza_assert.py
# gate pin delle action (bloccante): ogni `uses:` dei workflow riferisce una
# SHA completa con il commento della versione. Tag e rami sono mobili.
python scripts/verifica_pin_workflow.py
# gate di coverage (soglie identiche alla CI: lines 90/functions 85/regions 89)
scripts/coverage.sh
# fuzzing: CI notturna (.github/workflows/fuzz.yml); smoke locale:
scripts/fuzz-smoke.sh
# gate clippy anche per il target Windows (la CI gira su Linux+Windows e
# il codice cfg(windows)/cfg(not(unix)) non compila nel container Linux):
rustup target add x86_64-pc-windows-msvc
cargo clippy --workspace --all-targets --locked --target x86_64-pc-windows-msvc
```
