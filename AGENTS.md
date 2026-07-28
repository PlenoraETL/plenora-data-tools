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
4. **Deviazioni esplicite.** Ogni scostamento da ADR, contratti documentati
   o invarianti va scritto nel codice E nell'ADR pertinente (`docs/adr/`),
   con motivazione. Una garanzia indebolita va dichiarata come tale.
5. **Determinismo testato.** ADR-0001: stesso input → stesso output, sempre.
   Ordine logico (BatchSequence), mai temporale. Le ottimizzazioni si
   verificano con oracoli contro il percorso generico.
6. **Nessun `unsafe`** nel workspace (lint attivo). Nessuna dipendenza nuova
   senza motivazione documentata; pin esatti delle versioni.
7. **Suite completa prima del commit**: `cargo test --workspace
   --no-fail-fast` (container `rust:1.92`). CI su Linux+Windows deve restare
   verde.
8. **Errori senza dati.** Mai valori di righe/colonne nei messaggi di errore
   (regola di `plenora-core/src/error.rs`), neanche in modalità diagnostica.

## Riferimenti

- ADR in `docs/adr/` — contratto architetturale; lo stato di attuazione è
  riportato in coda a ciascuno.
- Registro delle deroghe: `docs/deroghe.md` (ICD §16 R16.2) — unico punto
  di raccolta; ogni deroga dichiara regola, motivo, hazard, owner e
  condizione di rientro (R16.1). Fonte normativa citata nelle CIA:
  `plenora-contracts`, tag `v2.0-rc4`.
- `Architetture.md`, `Prestazioni.md` — decisioni (D*) e invarianti (I*, P*).
- Catalogo operazioni: snapshot test
  (`crates/plenora-engine/tests/catalog_snapshot.snap`) — ogni cambio di
  catalogo è esplicito in PR.

## Build e test

```sh
# test completi (container, toolchain del progetto)
docker run --rm -v $PWD:/work -w /work rust:1.92 cargo test --workspace --no-fail-fast
# gate R6 (identico alla CI, bloccante): nessuna primitiva di panic nei lib
cargo clippy -p plenora-core -p plenora-engine -p plenora-kernels-table \
  -p plenora-kernels-geo --lib --locked -- --cap-lints=warn -D unsafe-code \
  -D clippy::unwrap_used -D clippy::expect_used -D clippy::panic \
  -D clippy::unreachable -D clippy::todo -D clippy::unimplemented
# fuzzing: CI notturna (.github/workflows/fuzz.yml); smoke locale:
scripts/fuzz-smoke.sh
# gate clippy anche per il target Windows (la CI gira su Linux+Windows e
# il codice cfg(windows)/cfg(not(unix)) non compila nel container Linux):
rustup target add x86_64-pc-windows-msvc
cargo clippy --workspace --all-targets --locked --target x86_64-pc-windows-msvc
```
