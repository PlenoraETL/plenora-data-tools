# Registro delle deroghe — plenora-data-tools

Unico punto di raccolta delle deroghe attive del componente (ICD §16 R16.2,
plenora-contracts `v2.0-rc8`). Ogni deroga dichiara: regola, motivo, impatto
sugli hazard, owner e condizione di rientro (R16.1). Una deroga senza
condizione di rientro e' permanente e va dichiarata tale. Le deviazioni dai
contratti documentati restano registrate anche nell'ADR pertinente
(`docs/adr/`, regola 4 di AGENTS.md).

Riferimento normativo citato nelle CIA: `plenora-contracts`, tag `v2.0-rc8`
(revisione `62b12e3496466d2c908dac3cc098640b99b52e21`).

## DER-002 — Emissione delle chiavi canoniche §2 prima della ratifica

- **Regola derogata:** §15.4 passo 1 (emendata 2.0-rc5) — prima della
  ratifica di §2, l'emissione delle chiavi candidate e' ammessa solo con
  deroga registrata che dichiari l'hazard per i consumatori non allineati
  e la condizione di rientro. La deroga gemella a livello ICD e'
  DER-ICD-002 (tutti e tre i componenti nella stessa condizione).
- **Ambito:** emissione delle chiavi `plenora.geometry.*` e
  `plenora.contract.version` nel percorso DAG v4 (milestone B/C/D di
  ADR-0009), in DOPPIA emissione con le chiavi standard GeoArrow — la sola
  forma compatibile con DER-ICD-002: il percorso legacy resta invariato.
- **Motivo:** il protocollo serve alla cooperazione applicativa ora (la
  catena bordo-centro-bordo); §2 e' `proposta` non ratificata, ma la
  doppia lettura/emissione non rompe i consumatori esistenti.
- **Impatto sugli hazard:** un consumatore non allineato riceve chiavi di
  metadata sconosciute. Arrow le preserva per costruzione e nessun
  consumatore attuale le interpreta: il rischio e' limitato a
  incompatibilita' di NOMI se §2 fosse ratificata con nomi diversi dai
  candidati — coperto dalla condizione di rientro (migrazione).
- **Owner:** maintainer di plenora-data-tools.
- **Condizione di rientro:** ratifica di §2 con nomi compatibili, oppure
  migrazione delle emissioni ai nomi ratificati (come da DER-ICD-002).

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
