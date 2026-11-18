# Changelog

Le modifiche di plenora-data-tools per versione. Le versioni seguono il
versionamento semantico in senso stretto: un piano, una config o un uso
dell'API valido in una versione e rifiutato o interpretato in modo diverso
nella successiva è un cambiamento incompatibile e fa salire la major.

Il corpo di ogni release GitHub riprende la voce della sua versione. Una
voce è «non rilasciata» finché il tag non esiste; prima di pubblicare la
release si sostituisce con la data, e `rilascio.yml` rifiuta una release
la cui versione non ha qui una voce datata.

## [6.1.0] - non rilasciata

### Aggiunte

- API scalare `coordinate_usng`: `format_usng`, `parse_usng`, `UsngDatum`,
  `DecodedUsng` e `UsngError`. NAD83/GRS80 o NAD27/Clarke 1866 espliciti,
  senza trasformazioni; lettere e celle MGRS condivise. Grammatica ASCII
  completa con spazi singoli e suffisso NAD27 obbligatorio; 0..5 cifre,
  troncamento, rappresentante obbligatorio e massimo 27 byte. Dominio UTM,
  regioni polari `unsupported`. TM parametrizzata sugli ellissoidi ammessi.

- API scalare `coordinate_mgrs` WGS84: `format_mgrs`, `parse_mgrs`,
  `MgrsCell`, `DecodedMgrs` e `MgrsError`. Riferimenti completi UTM/UPS,
  precisione esplicita 0..5, troncamento, rappresentante obbligatorio
  `grid_text::CellRepresentative` e limiti interi della cella sorgente.
  UPS ellissoidale nel core; selezione UTM e TM riusate. Grammatica ASCII
  maiuscola senza spazi, zona UTM a due cifre, limite 15 byte.
  USNG e trasformazioni di datum non sono esposti da questa API.

- API scalare `coordinate_utm` WGS84 conforme a GOP-023: `to_utm`,
  `from_utm`, `format_utm`, `parse_utm`, `UtmCoordinate` e `UtmError`.
  Testo canonico `zone hemisphere easting northing`, emisfero N/S,
  risoluzione esplicita 1 m e arrotondamento half-even. Dominio 80S..84N
  con eccezioni Norvegia/Svalbard; fuori dominio errore tipizzato.
  Il prototipo non rilasciato con banda e UPS e' sostituito: nessun
  ingresso con banda, formato UPS pubblico o riconoscimento implicito.

- Migrazione DE-9IM al noder razionale comune: eventi senza grafo,
  sweep con filtri inclusivi X/Y e oracolo esaustivo della 1a nei test.
  La 6.1.0 non e' rilasciata: `RelateLimits` conta ora i test esatti dopo
  entrambi i filtri, le capacita' strutturali vive e aggiunge `max_nodes`
  (duplicati intermedi inclusi, default 10 milioni) e `deadline` (default
  nessuna). Errore distinto `DeadlineExceeded`, categoria ResourceLimit
  nella pipeline. La semantica precedente dei limiti 1a non e' mantenuta.
  La tranche 1b resta incompleta: buffer BigInt ancora fuori dal bilancio,
  nuovi ingressi controllati split_line/polygonize mancanti.
  Le API pubblicate `evaluate`/`evaluate_validated` conservano backend,
  risultati e default 6.0.0; differenziale con snapshot del tag locale.
- API Rust `predicates::relate_matrix`, `De9imMatrix` e `De9imPattern`:
  accesso alle nove celle, confronto di pattern OGC `TF*012` validati per
  intero e valutazione dei predicati nominati sulla stessa matrice.
  Matrice razionale esatta anche per unioni di collezioni, con precedenza
  dimensionale dichiarata e confine lineare Mod-2. Variante con limiti
  effettivi, prenotazioni strutturali e cancellazione cooperativa.
  I predicati su `De9imMatrix` leggono la matrice esatta anche sui subnormali;
  le due API nominate della 6.0.0 conservano il comportamento pubblicato.
  Nessuna modifica alle booleane o alle operazioni del piano.

## [6.0.0] - 2026-11-15

### Cambiamenti incompatibili

- Rimossi registro e risoluzione degli id storici: analizzatori e runner
  accettano solo gli id del catalogo corrente; ogni altro id e' sconosciuto.
- Rimossi note e istruzioni di migrazione delle release ritirate.
- Riferimenti CLI/SDK allineati al pin contratti
  `38d68b402389855edbcecb7ce57c31e91282cb4e`; delta e deviazioni adottati
  restano nel registro di adozione.

### Sicurezza

- Date e orari da file esterni senza panic ne' valori sbagliati: la CLI
  converte le scadenze chrono in `SystemTime` con confronto esatto e rifiuta
  i valori fuori dominio (`InvalidConfiguration`); il formatter record
  Parquet gestisce DATE e timestamp millis/micros nel dominio chrono e
  restituisce un errore senza valori invece di andare in panic; il rifiuto
  di INT96 avviene sullo schema del footer, prima della conversione Arrow e
  anche senza row group; TIME_MILLIS/TIME_MICROS negativi o oltre il giorno
  sono rifiutati invece di essere convertiti.

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
