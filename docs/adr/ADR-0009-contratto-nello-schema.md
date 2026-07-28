# ADR 9 — Contratto nello schema Arrow: chiavi canoniche, lineage, assi d'errore (ICD §2, §3, §9)

- **Stato**: accettato, attuazione in corso (milestone A, B, D attuate; C
  parziale per vincolo sui monoliti)
- **Fonte normativa**: `plenora-contracts`, tag `v2.0-rc4` — §2 (chiavi dei
  metadati Arrow), §3 (modello geometrico), §9 (modello di errore) sono
  **proposte in attesa di ratifica**: l'implementazione segue la proposta
  come scelta progettuale dichiarata, non come obbligo ratificato (§16
  R16.3: ratifica e implementazione sono atti distinti). Fino alla ratifica
  questo ADR e' l'autorita' locale; alla ratifica si allinea. (I commenti
  nel codice delle milestone A/B/D citano `v2.0-rc3`: normativamente
  identica — rc4 non modifica requisiti, solo processo e fotografia di
  conformita'.)
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
3. **`FieldId` non viaggia.** L'identita' di colonna fuori dal processo e'
   per **nome**: `FieldId` appartiene al namespace del grafo che lo ha
   assegnato (D16) e perde significato al confine. Divergenza consapevole
   dalla tabella §2, che elenca `plenora.field_id` senza chiarirne la
   semantica cross-processo: la chiave e' letta se presente ma non e' usata
   per ricostruire identita'. Da proporre come chiarimento all'owner ICD.
4. **Lineage R2.4 come politica esplicita.** Propagazione delle chiavi non
   interpretate per lineage del campo: identity-preserving → copia
   invariata; type-preserving → copia selettiva; campo derivato →
   ricostruzione, mai eredita'; conflitto fra sorgenti → errore o
   `LossReport`, mai precedenza implicita. Attuazione per tappe (vedi
   "Stato di attuazione"): prima i confini non-monolitici (discovery CLI,
   output v4), poi i monoliti.
5. **Errore a quattro assi.** `ErrorPhase` (10 valori canonici) e
   `RemoteEffect` (5 valori canonici R9.6) come assi **derivati** per
   variante di `PlenoraError` (stesso stampo di `category()`); R9.5 vieta
   valori propri. Categoria (M1d) e `retryable()` invariati; la
   disposizione di retry di R9.7 (sostituisce il booleano) e' follow-up
   dichiarato — richiede la fase operativa ai confini, che questa
   attuazione prepara ma non ancora annota.
6. **`PublishOutcome` mappato sull'asse effetto, non duplicato.**
   `PublishedButDurabilityUnconfirmed` (ADR 7) non e' un errore (R9.3):
   mappa su `RemoteEffect::Committed` — l'effetto esiste ed e'
   osservabile; cio' che manca e' la conferma di durabilita', non
   l'esistenza dell'effetto.

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
  **Restano fuori, come follow-up esplicito** (vincolo maintainer sui
  monoliti):
  - `plenora-kernels-geo/src/analyze.rs`: `geometry_field` (:692),
    `new_geometry_field` (:711), `set_geometry_crs` (:730) e gli
    `Schema::new*` di :680, :844, :898, :927, :1001, :1079, :1252, :1304,
    :1360 — i campi derivati escono con sole chiavi GeoArrow.
  - `plenora-kernels-table/src/analyze.rs`: i `Field::new` di :324, :1479,
    :1599, :3067 perdono TUTTI i metadati di campo (violazione R2.4
    identity/type-preserving per le chiavi non interpretate) e gli
    `Schema::new*` di :500, :1237, :1293, :1334, :1412, :1499, :1601,
    :1623, :1862, :2630, :2793, :2882, :3017, :3471, :3617, :3694 quelli di
    schema.
  - Join: `combine_horizontal_fields` (:3146-3205) e `merge_geometry` (:385)
    con precedenza implicita al ramo sinistro — R2.4 vuole errore o
    `LossReport` su conflitto; `Schema::new` di :3083, :3254, :3313, :3336,
    :3431.
  - `geo_transport/transport.rs`: wrapper legacy `geometry_output_field`
    (:1693), riscritture post-kernel (:2167, :2288, :2422, :2494, :2586,
    :2749, :2948) e gli `Schema::new*` di :2199, :2290, :2421, :2495,
    :2587, :2648, :2754, :2949, :3454, :3501, :3546, :3668, :3741, :3806,
    :4156 — le chiavi canoniche dell'input si perdono nel percorso legacy.
- **Follow-up dichiarati**: disposizione di retry R9.7 (sostituisce
  `retryable()`); rinomina delle categorie d'errore verso l'enumerazione
  canonica (tabella "Mappatura dai modelli attuali" §9); chiave
  `plenora.field_id` (decisione 3) da proporre all'owner ICD; test di
  catena completa (dataset Z/M, CRS irrisolto, `lat_lon` attraverso il
  centro — rc4 Appendice A, "cosa manca alla catena").

## Cambi di comportamento (dichiarati)

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
