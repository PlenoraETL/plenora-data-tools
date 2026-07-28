# Registro delle deroghe — plenora-data-tools

Unico punto di raccolta delle deroghe attive del componente (ICD §16 R16.2,
plenora-contracts `v2.0-rc4`). Ogni deroga dichiara: regola, motivo, impatto
sugli hazard, owner e condizione di rientro (R16.1). Una deroga senza
condizione di rientro e' permanente e va dichiarata tale. Le deviazioni dai
contratti documentati restano registrate anche nell'ADR pertinente
(`docs/adr/`, regola 4 di AGENTS.md).

Riferimento normativo citato nelle CIA: `plenora-contracts`, tag `v2.0-rc4`.

## DER-001 — Toolchain nightly per il fuzzing con sanitizer

- **Regola derogata:** R13.1 (ICD §13, ratificata) — tutti i componenti
  compilano con la stessa versione esatta del compilatore, fissata da
  `rust-toolchain.toml` (1.92.0).
- **Ambito:** solo lo step `cargo fuzz run` del workflow
  `.github/workflows/fuzz.yml` (`RUSTUP_TOOLCHAIN: nightly`). Build, test,
  clippy e gate R6 restano sulla toolchain pinnata. Gli script locali
  (`scripts/fuzz-smoke.sh`, `scripts/fuzz-campaign.sh`) usano l'immagine
  dedicata `plenora-rust:nightly-fuzz`: stessa deroga, stesso ambito.
- **Motivo:** `-Zsanitizer=address` e le flag di coverage di cargo-fuzz
  (`-Cpasses=sancov-module`, `-Z*`) sono accettate solo dal compilatore
  nightly; non esiste modo di eseguire il fuzzing con sanitizer sulla
  toolchain stabile pinnata. Senza la deroga il fuzz notturno fallisce in
  partenza ("the option `Z` is only accepted on the nightly compiler",
  run del 2026-07-28) e la rete di sicurezza resta muta.
- **Impatto sugli hazard:** il fuzzing gira su un compilatore diverso da
  quello di produzione (H-07: difetto di compilatore indistinguibile da
  difetto di codice). Controllo: un crash conta solo se riproducibile; la
  riproduzione e la minimizzazione dei casi avvengono sulla toolchain
  pinnata 1.92.0 prima di aprire una fix, quindi nessun artefatto del
  compilatore nightly puo' entrare nel percorso di produzione.
- **Owner:** maintainer di plenora-data-tools.
- **Condizione di rientro:** quando le flag sanitizer/coverage equivalenti
  saranno disponibili sulla toolchain stabile pinnata, oppure quando R13.1
  sara' emendata per ammettere esplicitamente la toolchain nightly per il
  fuzzing. Fino ad allora la deroga e' attiva e va riesaminata a ogni
  bump di toolchain (R13.5).
