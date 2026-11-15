# Changelog

Le modifiche di plenora-data-tools per versione. Le versioni seguono il
versionamento semantico in senso stretto: un piano, una config o un uso
dell'API valido in una versione e rifiutato o interpretato in modo diverso
nella successiva è un cambiamento incompatibile e fa salire la major.

Il corpo di ogni release GitHub riprende la voce della sua versione. Una
voce è «non rilasciata» finché il tag non esiste; prima di pubblicare la
release si sostituisce con la data, e `rilascio.yml` rifiuta una release
la cui versione non ha qui una voce datata.

## [6.0.0] - non rilasciata

### Cambiamenti incompatibili

- Rimossi registro e risoluzione degli id storici: analizzatori e runner
  accettano solo gli id del catalogo corrente; ogni altro id e' sconosciuto.
- Rimossi note e istruzioni di migrazione delle release ritirate.
- Riferimenti CLI/SDK allineati al pin contratti
  `38d68b402389855edbcecb7ce57c31e91282cb4e`; delta e deviazioni adottati
  restano nel registro di adozione.

## [5.0.0] - 2026-11-14

### Cambiamenti incompatibili

- Python minimo 3.14 per SDK, strumenti, CI e rilascio; ABI stabile cp314.
  Python precedenti non sono piu' supportati: il requisito richiede la major
  5.0.0. `.python-version` e' la fonte unica, verificata in CI.
- Rimossi i fallback per interpreti precedenti e i backport non necessari.

### Rapporti di esecuzione

- C-01: effetti localizzati per campo e chiave nel rapporto Rust e nei risultati
  CLI/SDK sincroni e asincroni, dalla stessa politica dei derivati.
  Estensione component-owned `plenora-data-localized-effects-v1` del risultato
  v2; il manifesto artefatti v3 mantiene il contratto del pin.

### Binding runtime

- Capability reale `plenora.data-tools` (Runtime Binding 2.0, wire 1):
  catalog@2, describe@1, validate@2, run@3; gli entrypoint riusano le API comuni.
- Ammissione e identita del protocollo, errori senza dati, pubblicazioni
  con effetti provati, risultato limitato senza troncamento e vettori del pin.
- Parita Rust, CLI, SDK sync/async e runtime per ogni operazione del catalogo,
  enumerata dalle schede, con gli stessi risultati o rifiuti dichiarati.
- `data.run@3` resta assente da CLI/SDK e `@2` dal runtime, come richiesto
  da DT-ABS-001/002; gli effetti localizzati runtime sono metadati separati
  dal manifesto v3 chiuso.

### Dipendenze

- Prima ondata della politica comune: log =0.4.34, proptest =1.12.0 e
  allineamento delle transitive compatibili nei lock, compresi quelli separati.
- Snapshot approvato del manifesto e checker in `.ci/plenora-dependencies`,
  con verifica Linux in CI; nessuna nuova eccezione locale.
