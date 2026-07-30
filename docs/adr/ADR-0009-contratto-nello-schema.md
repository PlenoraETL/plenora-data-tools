# ADR 9 — Contratto nello schema Arrow: chiavi canoniche, lineage, assi d'errore (ICD §2, §3, §9)

- **Stato**: accettato, attuazione in corso (milestone A, B, D attuate; C
  parziale per vincolo sui monoliti)
- **Fonte normativa**: `plenora-contracts`, tag `v2.0-rc10` (revisione
  `3598259bbe07d1c853453ff34ca2c1d1d28a0272`) — §2 (chiavi dei metadati
  Arrow), §3 (modello geometrico), §9 (modello di errore) sono **proposte
  in attesa di ratifica**: l'implementazione segue la proposta come scelta
  progettuale dichiarata, non come obbligo ratificato (§16 R16.3: ratifica
  e implementazione sono atti distinti). Fino alla ratifica questo ADR e'
  l'autorita' locale; alla ratifica si allinea. L'emissione delle chiavi
  §2 prima della ratifica e' registrata come deroga (§15.4 emendata rc5 /
  DER-ICD-002) in `docs/deroghe.md` DER-002. (I commenti nel codice delle
  milestone A/B/D citano `v2.0-rc3`: normativamente identica. R4.6 e'
  entrata in rc9; la disambiguazione `crs_missing` di R4.6.3 in rc10.)
- **Decisioni collegate**: D16 (FieldId), ADR 3 (failure), ADR 7 (publish),
  ADR 8 (modello dimensionale)
- **Riferimenti**: `docs/deroghe.md` (registro deroghe, R16.2)

## Contesto

Il `DataContract` viveva solo in memoria e moriva al confine di processo:
ogni consumatore a valle riscopriva le proprieta' geometriche dagli header
GeoArrow (CRS, dimensionalita') o non le riscopriva affatto (tipi,
dichiarazione dei tipi, versione di protocollo). L'ICD §2 chiede che le
proprieta' viaggino nello schema Arrow come **chiavi separate** (R2.2: mai
blob serializzato — un consumatore che ne ignora una non perde le altre),
con versione di protocollo dichiarata a livello di schema (R2.5), coerenza
obbligatoria con le chiavi standard GeoArrow (R2.6) e precedenza come
completamento (R2.7). §9 chiede che l'errore porti quattro assi
indipendenti: categoria, fase, effetto remoto, ritentativo.

## Decisione

1. **Chiavi canoniche, mai blob.** Le proprieta' geometriche viaggiano come
   chiavi separate `plenora.geometry.*` nei metadati di campo
   (`Field::metadata`), secondo la tabella §2 dell'ICD; `plenora.contract.version`
   (= `1`) vive in `Schema::metadata`, mai ripetuta per campo (R2.5). Le
   chiavi opzionali assenti restano assenti (R5.2: mai un default al posto
   dell'assente). Protocollo implementato in
   `plenora-kernels-geo/src/arrow_adapter.rs` (milestone B): emissione da
   `GeometryColumnContract`, lettura fail-closed per chiave, gate di
   versione, coerenza R2.6, completamento R2.7.
2. **Tipi geometrici come dichiarazione, non come fatto.** `GeometryType`
   (16 valori canonici §3.1, serializzazione minuscola senza separatore) e
   `TypesDeclaration` (`exact`/`mixed`/`unresolved`, R3.4.1) modellano i
   tre stati possibili. Le coerenze sono imposte per costruzione (campi
   privati di `GeometryTypesProperty`): `exact` richiede elenco non vuoto,
   `unresolved` lo vieta, lista canonica unica (valori unici, ordine §3.1,
   senza spazi). Un ingresso legacy privo delle chiavi NON e' `unresolved`:
   e' «proprieta' non dichiarata» (confidence `Unknown`) — le due forme non
   sono convertibili (R3.4.1).
3. **`FieldId` non viaggia — e non si inventa.** L'identita' di colonna
   fuori dal processo e' per **nome**: `FieldId` appartiene al namespace
   del grafo che lo ha assegnato (D16) e perde significato al confine. La
   tabella §2 dichiara `plenora.field_id` **opzionale**: NON emettere il
   valore di grafo e' lecito (e' la scelta attuata — l'emissione iniziale
   e' stata rimossa perche' presentava come identita' un numero privo di
   significato cross-processo); una chiave `plenora.field_id` RICEVUTA e'
   invece propagata INVARIATA per R2.4 (chiave canonica non interpretata),
   mai sovrascritta dal valore di grafo. Il chiarimento da proporre
   all'owner ICD riguarda la semantica cross-processo di un `field_id`
   assegnato da un altro processo, non la liceita' dell'omissione.
4. **Lineage R2.4 come politica esplicita.** Propagazione delle chiavi non
   interpretate per lineage del campo: identity-preserving → copia
   invariata; type-preserving → copia selettiva; campo derivato →
   ricostruzione, mai eredita'; conflitto fra sorgenti → errore o
   `LossReport`, mai precedenza implicita. Attuazione per tappe (vedi
   "Stato di attuazione"): prima i confini non-monolitici (discovery CLI,
   output v4), poi i monoliti.
5. **Errore a quattro assi.** `ErrorPhase` (10 valori canonici),
   `RemoteEffect` (5 valori canonici R9.6) e `RetryDisposition` (5 valori
   canonici R9.7) come assi **derivati** per variante di `PlenoraError`
   (stesso stampo di `category()`); R9.5 vieta valori propri. R9.7
   sostituisce il booleano `retryable()` di M1d (rimosso): la disposizione
   e' calcolata da fase, effetto e idempotenza, mai dalla sola categoria.
   Il tagging di fase ai confini (`step_error`, `at_input`, publish) resta
   follow-up: raffinera' la fase, non la disposizione — effetto `None` per
   costruzione e idempotenza della riesecuzione valgono a qualunque fase
   raffinata, quindi il mapping di `retry_disposition()` non cambia.
6. **`PublishOutcome` mappato sull'asse effetto, non duplicato.**
   `PublishedButDurabilityUnconfirmed` (ADR 7) non e' un errore (R9.3):
   mappa su `RemoteEffect::Committed` — l'effetto esiste ed e'
   osservabile; cio' che manca e' la conferma di durabilita', non
   l'esistenza dell'effetto.
7. **CRS mancante nel contratto, requisito condizionato alle op (R4.6.3,
   rc9/rc10).** `GeometryColumnContract.crs` e' `ContractCrs`
   (`Resolved(ResolvedCrs)` | `Missing`), non piu' un `ResolvedCrs`
   obbligatorio: la discovery NON pretende un CRS risolvibile per
   operazioni che non lo richiedono — un filtro tabellare su una colonna
   non geometrica non ha bisogno di alcun CRS, e rifiutarlo e' piu'
   restrittivo del ruolo (rc10). `Missing` non porta dati (R4.4 intatta:
   mai un CRS inventato) ed e' distinto da `Resolved` nel modello interno
   (R4.1); la forma a enum (non `Option`) e' scelta perche' lo stato sia
   nominato e l'aggiunta futura di `DeclaredUnresolved` sia guidata dal
   compilatore. Il fallimento si sposta dalla discovery al punto in cui
   un'op con `CrsRequirement` tocca la colonna — `analyze_contract` delle
   op geo, a compile-plan (deterministico, mai a meta' stream) — con
   categoria `Crs` e messaggio che dichiara la causa («nessun CRS
   dichiarato in alcuna rappresentazione accettata»). Propagazione
   (R4.6.4): lo stato attraversa invariato i contratti di output e le
   chiavi §2 (`crs_resolution = missing`, nessuna chiave
   `crs_id`/`crs_definition`/`axis_order`/`srid` — coerenza R2.2).
   Fingerprint (ADR 4): lo stato ENTRA — risolto e mancante non sono lo
   stesso contratto; escluderlo farebbe accettare a un piano con op geo,
   in riesecuzione, un input senza CRS senza rivalidazione (fallimento
   spostato a runtime); per un filtro tabellare il mismatch costa una
   rivalidazione che passa (conservativo, dichiarato). `DeclaredUnresolved`
   NON e' modellato: la discovery non lo produce (una definizione
   dichiarata ma non risolvibile resta errore di risoluzione) e risolvere
   un'incoerenza dichiarata richiede una decisione esplicita nel piano
   (R4.6.3) — follow-up dichiarato. Una `crs_resolution` valorizzata
   (`resolved`/`declared_unresolved`) senza alcuna rappresentazione e'
   contraddittoria e resta errore di discovery (R4.1: mai collassarla su
   `missing`); un metadato `geo` malformato resta errore (R5.1:
   «illeggibile» non e' «assente»).

## Forzature note (dichiarate)

- **`Step` → `ErrorPhase::Write`.** L'enumerazione canonica delle fasi non
  ne ha una per l'esecuzione di trasformazioni: §9 e' nata sul modello di
  database-tools, che e' un bordo di I/O (connect, probe, read, write,
  commit); il centro non ha una fase propria — **difetto del contratto, non
  dell'implementazione**. Un `Step` nasce solo mentre un nodo produce il
  proprio stream (gli errori di lettura input emergono come `Io`/`Arrow`/
  `Schema` prima del DAG), quindi la fase piu' vicina al momento in cui
  l'errore nasce e' `Write`. Se l'emendamento §9 proposto all'owner passa
  (fase dedicata alla trasformazione), l'allineamento e' una costante; se
  non passa, questa forzatura resta la decisione documentata.
- **Fasi ambigue per varianti multi-sito**: `Io`/`Arrow` (lettura e
  scrittura) dichiarate `Write` (il lato con possibile effetto sul
  supporto, il solo rilevante per R9.7); `Contract` del governor (scatta a
  runtime) dichiarata `Validate`; `UnsupportedPublishTarget` → `Probe`
  (ispezione preliminare della destinazione, ADR 7). Quando R9.7 rendera'
  la fase operativa per il retry, queste approssimazioni richiederanno
  override ai confini (`step_error`, `at_input`, publish).
- **SRID 0 accettato** in lettura (lettera della norma: "intero senza
  segno"); database-tools lo rifiuta per i propri piani — irrigidimento di
  dominio suo, non del protocollo.
- **Versione 0 accettata**: R2.5 impone il fallimento solo per versioni
  successive alla nota.
- **CRS WKT come `crs_id`**: `ResolvedCrs` non porta hint di formato; una
  definizione non-JSON e' emessa come `crs_id` (stessa euristica del
  legacy `geo.crs`). Se serviranno definizioni WKT, `ResolvedCrs` dovra'
  portare il formato.

## Conseguenze

- Il gate R2.5 rende ogni evoluzione del protocollo un fallimento esplicito
  e immediato (versione > 1 → `Unsupported`), mai un'interpretazione
  parziale: la migrazione a una versione 2 sara' un atto visibile.
- La coerenza R2.6 fallisce su divergenza canonica/legacy: un produttore
  esterno incoerente viene rifiutato all'ingresso, non arbitrato.
- Le approssimazioni di fase sono concentrate in `error.rs` e isolate dal
  resto del sistema: l'emendamento §9 le risolve con una modifica locale.
- Fingerprint ADR 4: le nuove chiavi emesse nello schema entrano in
  `sorted_metadata`; i fingerprint dei contratti di input cambiano per gli
  schemi che portano chiavi canoniche — atteso e dichiarato (la v1 del
  fingerprint escludeva gia' FieldId e proprieta').

## Stato di attuazione

- **Milestone A (attuata, `1962405`)**: `GeometryType`, `TypesDeclaration`,
  `GeometryTypesProperty`, enum delle chiavi R2.2 in `plenora-core`;
  `GeometryColumnContract.types`; propagazione identity-preserving in
  `propagate_geometry`.
- **Milestone B (attuata, `babc7bf`)**: protocollo chiavi canoniche in
  `arrow_adapter.rs` (emissione, lettura fail-closed, gate versione,
  coerenza R2.6, completamento R2.7).
- **Milestone D (attuata, `40771de`)**: `ErrorPhase`, `RemoteEffect`,
  `phase()`/`remote_effect()` derivati, `PublishOutcome::remote_effect()`.
- **Milestone C (attuata nel perimetro non-monoliti)**: wiring della
  lineage ai confini — discovery CLI v4 (`discover_input_contract_from_schema`:
  gate R2.5, `read_geometry_contract_keys` come sorgente primaria,
  riconoscimento autosufficiente di colonne con sole chiavi canoniche,
  `types` come `Declared`/`Schema` solo se la coppia e' presente) ed
  emissione centralizzata nel percorso di output v4
  (`canonical_output_schema` in `executor.rs`: fusione fail-closed delle
  chiavi canoniche nei campi geometria, versione R2.5 sullo schema,
  rivestimento dei batch in scrittura — vedi "Cambi di comportamento").
- **Lineage nei monoliti `analyze.rs` (attuata, 2026-07-28)**: la policy
  R2.4 e' ora applicata nei due `analyze.rs`:
  - metadati di SCHEMA conservati ovunque (`Schema::new_with_metadata`
    al posto di `Schema::new` in tutti i siti identity/type-preserving e
    derivati-dataset: le chiavi sconosciute non sono giudicabili dal
    centro, la perdita rompe i round-trip);
  - multi-sorgente (join e varianti, `set_operation` UnionDistinct,
    `concat`/`concat_by_name` N-ario, op binarie geo): MERGE dei
    metadati di schema — chiave in una sola sorgente copiata, uguale
    copiata, **valori diversi = errore `Contract` che nomina solo la
    chiave** (mai i valori; primo conflitto deterministico per ADR-0001:
    sorgenti in ordine di dichiarazione, chiavi lessicografiche). La
    precedenza implicita "vince left/primo" e' eliminata;
  - metadati di CAMPO: identity-preserving copiati invariati (incluse
    le chiavi canoniche su colonne geometriche sopravvissute),
    type-preserving copiati (fill_na/replace), campi derivati NON
    ereditano (table_diff, colonne sintetiche; le canoniche le emette
    `canonical_output_schema` a valle);
  - deroghe documentate nel codice: `reconcile` e `validate_rules`
    Summary restano `Schema::new` (dataset derivati puri: ereditare
    descriverebbe il risultato con le proprieta' dell'ingresso, R5.1).
  - **Conseguenza fingerprint (ADR 4)**: i metadati ora propagati
    entrano in `sorted_metadata` del fingerprint dei contratti per i
    piani i cui input li portano — cambio atteso, dichiarato qui e
    gia' coperto da test di regressione dedicati nei due file.
  - **Percorso legacy `geo_transport/transport.rs` (analizzato
    2026-07-28)**: i suoi output senza metadati di schema sono dataset
    DERIVATI (indici di coppia, pezzi overlay, misure) — la classe R2.4
    corretta e' "non si eredita" e i siti sono gia' conformi; le deroghe
    sono ora documentate inline (`lineage_schema`, output overlay). Resta
    una decisione di design, non un bug: se anche l'output legacy debba
    emettere le chiavi canoniche (oggi solo GeoArrow) o restare
    GeoArrow-only fino al ritiro del percorso.
- **Follow-up dichiarati**: tagging di fase ai confini (`step_error`,
  `at_input`, publish — raffina le approssimazioni di fase, non la
  disposizione R9.7, gia' attuata); chiave `plenora.field_id` (decisione
  3) da proporre all'owner ICD; test di catena completa bordo-centro-bordo
  con gli altri due componenti.
- **R4.6.3 CRS condizionato alle op (attuata, 2026-07-29)**: decisione 7 —
  `ContractCrs` in `plenora-core`; discovery CLI che porta `Missing` invece
  di fallire (metadato `geo` malformato e dichiarazioni contraddittorie
  restano errori); gate `require_resolved_crs` nelle analyze geo
  (`dispatch.rs`, `producers.rs` — punto di validazione: `analyze_contract`,
  compile-plan); emissione `crs_resolution = missing` senza chiavi CRS in
  `canonical_geometry_metadata`; stato `missing` nel fingerprint ADR 4
  (forma risolta invariata). Test: unit (discovery, analyze, catalogo,
  fingerprint) e integrazione CLI (filtro che passa e propaga `missing`
  con round-trip, op geo che fallisce con la causa). Nessuna deroga: R4.4
  intatta.
- **Rinomina categorie §9 (attuata, 2026-07-29)**: varianti allineate
  all'enumerazione canonica (Appendice C): `Contract` → `InvalidPlan`,
  `Step` → `Execution`, `UnsupportedPublishTarget` → `Unsupported`,
  `Json`/`Arrow` → `DataMapping`; `ErrorCategory` e' ora l'enumerazione
  canonica completa a 18 valori (R9.5). **I testi `Display` sono
  invariati** ("contract violation", "step failed at node", "arrow
  error", ...): la rinomina e' a livello di variante e categoria
  machine-readable, non di messaggio — nessun consumatore testuale si
  rompe. Approssimazioni dichiarate nel codice: la fusione
  `Json`+`Arrow` in `DataMapping` perde la sorgente tipizzata (resta
  nel testo) e la distinzione di fase parse/I-O (`DataMapping` →
  `Write`); `Unsupported` assorbe la destinazione di publish non
  supportata (prima `Probe`, ora `Validate`) — entrambe da raffinare
  col tagging ai confini di R9.7.
- **Disposizione di retry R9.7 (attuata, 2026-07-29)**: enum
  `RetryDisposition` canonico a 5 valori (`never`, `safe`,
  `requires_idempotency_key`, `requires_recovery`, `after(durata)`) e
  `PlenoraError::retry_disposition()` calcolata da fase, effetto e
  idempotenza — mai dalla sola categoria; il booleano `retryable()` di
  M1d e' rimosso (R9.7 lo dichiara insufficiente e pericoloso). Mapping:
  `Safe` solo per `Io` (causa transitoria, effetto `None` per
  costruzione, riesecuzione idempotente), `Never` per cause
  deterministiche o volontarie; `RequiresIdempotencyKey`/
  `RequiresRecovery` mai prodotti (nessuno stato remoto), `After` mai
  prodotto (nessuna sorgente di backoff tipizzata — backoff e tentativi
  restano al chiamante).

## Cambi di comportamento (dichiarati)

- **Geometria senza CRS: da errore di discovery a stato `missing`
  (R4.6.3, decisione 7)**. Prima la discovery rifiutava OGNI colonna
  geometrica senza CRS risolvibile (`InvalidPlan` «senza metadato `geo`:
  impossibile determinare il CRS») — piu' restrittivo del ruolo: fermava
  anche piani puramente tabellari. Ora lo stato entra nel contratto come
  `ContractCrs::Missing` e solo le op con `CrsRequirement` falliscono, in
  analyze: variante `Crs` (come ogni requisito CRS non soddisfatto; prima
  `InvalidPlan` dalla discovery), messaggio che dichiara la causa («nessun
  CRS dichiarato in alcuna rappresentazione accettata») invece dell'ultimo
  tentativo di lettura fallito. R4.4 invariata: nessun CRS inventato, in
  nessun percorso.
- **Legacy `geo.dimensions` non canonico o non testuale: da `Unknown`
  silenzioso a errore** (lettura strict R5.1 nella discovery v4). Allineato
  alla regola 1 di AGENTS.md (niente failure silenziose); deviazione dal
  comportamento pre-C registrata qui perche' la discovery e' il punto in
  cui la strictness diventa effettiva.
- **Rivestimento dei batch in scrittura IPC v4**: ogni batch e' ricostruito
  con lo schema emesso (colonne condivise via Arc, costo zero sui buffer);
  un kernel che producesse batch con schema strutturalmente diverso dal
  contratto ora fallisce con `ArrowError` esplicito invece che in silenzio.
  Miglioramento fail-closed; comportamento nuovo, dichiarato.
- **Asimmetria nota**: `Output::schema()` porta il blocco canonico;
  `output_contract().schema` resta lo schema "plain" di validazione; i
  batch consegnati da `collect_batches`/`Iterator` restano con lo schema
  kernel. Solo il percorso IPC e' arricchito (perimetro della milestone).
