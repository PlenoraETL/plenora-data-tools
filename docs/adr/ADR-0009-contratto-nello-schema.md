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
   Il tagging di fase ai confini (lettura input, publish — variante wrapper
   `PlenoraError::Tagged`, attuato 2026-07-30, vedi "Stato di attuazione")
   raffina la fase, NON la disposizione: effetto `None` per costruzione e
   idempotenza della riesecuzione valgono a qualunque fase raffinata,
   quindi il mapping di `retry_disposition()` non e' cambiato (e'
   verificato per delega: il wrapper la eredita dalla sorgente).
6. **`PublishOutcome` mappato sull'asse effetto, non duplicato.**
   `PublishedButDurabilityUnconfirmed` (ADR 7) non e' un errore (R9.3):
   mappa su `RemoteEffect::Committed` — l'effetto esiste ed e'
   osservabile; cio' che manca e' la conferma di durabilita', non
   l'esistenza dell'effetto.
7. **CRS mancante nel contratto, requisito condizionato alle op (R4.6.3,
   rc9/rc10).** `GeometryColumnContract.crs` e' `ContractCrs`
   (`Resolved(ResolvedCrs)` | `DeclaredUnresolved { .. }` | `Missing`), non
   piu' un `ResolvedCrs` obbligatorio: la discovery NON pretende un CRS
   risolvibile per operazioni che non lo richiedono — un filtro tabellare
   su una colonna non geometrica non ha bisogno di alcun CRS, e rifiutarlo
   e' piu' restrittivo del ruolo (rc10). `Missing` non porta dati (R4.4
   intatta: mai un CRS inventato) ed e' distinto da `Resolved` nel modello
   interno (R4.1); la forma a enum (non `Option`) e' scelta perche' lo
   stato sia nominato e l'aggiunta di `DeclaredUnresolved` sia guidata dal
   compilatore. Il fallimento si sposta dalla discovery al punto in cui
   un'op con `CrsRequirement` tocca la colonna — `analyze_contract` delle
   op geo, a compile-plan (deterministico, mai a meta' stream) — con
   categoria `Crs` e messaggio che dichiara la causa («nessun CRS
   dichiarato in alcuna rappresentazione accettata» per `Missing`;
   «incoerenza CRS dichiarata non risolta... decisione esplicita nel
   piano» per `DeclaredUnresolved`: la colonna dichiara un'incoerenza, non
   un'assenza). Propagazione (R4.6.4): lo stato attraversa invariato i
   contratti di output e le chiavi §2 (`crs_resolution = missing`, nessuna
   chiave `crs_id`/`crs_definition`/`axis_order`/`srid` — coerenza R2.2).
   Fingerprint (ADR 4): lo stato ENTRA — risolto e mancante non sono lo
   stesso contratto; escluderlo farebbe accettare a un piano con op geo,
   in riesecuzione, un input senza CRS senza rivalidazione (fallimento
   spostato a runtime); per un filtro tabellare il mismatch costa una
   rivalidazione che passa (conservativo, dichiarato). Una `crs_resolution`
   valorizzata (`resolved`/`declared_unresolved`) senza alcuna
   rappresentazione e' contraddittoria e resta errore di discovery (R4.1:
   mai collassarla su `missing`); un metadato `geo` malformato resta
   errore (R5.1: «illeggibile» non e' «assente»).
   **`DeclaredUnresolved` (attuato 2026-07-30, BLOCK-08 di
   `release/rc.json`)**: la variante porta le dichiarazioni originali
   (`crs_id`, `definition` con il suo formato — R2.4/R4.6.4/R4.3; lo
   `srid` resta alla lineage dei metadati di campo: non e' una definizione
   risolvibile dal centro). La discovery lo produce in due casi: (a) il
   produttore dichiara `crs_resolution = declared_unresolved` — preservato
   senza tentare alcuna risoluzione (R4.6.3: in assenza di una decisione
   esplicita nel piano il centro propaga, non risolve; nessuna chiamata al
   backend, quindi nessun `BackendUnavailable`); (b) conflitto DECIDIBILE
   senza backend — `crs_id` e `crs_definition` co-presenti (accordo non
   decidibile testualmente, R2.7: mai arbitrato) o `crs_id`
   `authority:code` con codice numerico discordante da `srid` (R4.3.1) —
   preservato con entrambe le dichiarazioni, mai un errore (il centro non
   e' il bordo di scrittura) e mai una scelta silenziosa. Un fallimento di
   risoluzione di una rappresentazione singola RESTA un errore `Crs` e non
   diventa `DeclaredUnresolved` (limite dichiarato: il produttore che non
   puo' garantire la risoluzione dichiara `declared_unresolved`
   esplicitamente, come nel corpus di conformita'). La **decisione
   esplicita nel piano** e' il campo v4 `crs_decisions: { "<input>":
   "<definizione>" }` (alternativa scartata: `inputs` come oggetti
   `{"name","crs"}` — breaking change del formato, e il runner di
   conformita' emette `inputs: ["main"]`; scartato anche un nodo dedicato:
   la decisione e' un fatto sul contratto di input, non una
   trasformazione sul flusso). Le chiavi devono essere input dichiarati
   (validazione strutturale); l'applicazione e' nella CLI, dove piano e
   contratti scoperti si incontrano: la definizione e' risolta (senza
   backend: `BackendUnavailable`, come il CRS di piano) e sostituisce lo
   stato — su `Missing` o `Resolved` la decisione e' un errore esplicito
   (su `Missing` sarebbe un CRS inventato, R4.4; su `Resolved` una
   contraddizione del piano), mai ignorata in silenzio. Il contratto
   distingue la sorgente della risoluzione: **`ResolvedByDecision`**, un
   CRS risolto a tutti gli effetti per i consumatori (gate delle op,
   fingerprint, riepiloghi) ma marcato per l'emissione — le dichiarazioni
   della sorgente (chiavi canoniche CRS e `geo.crs` legacy nei metadati di
   campo) sono SOSTITUITE dal CRS deciso nella fusione dello schema di
   output (`strip_decided_crs_declarations`), mai propagate accanto ad
   esso. Lo schema del contratto di input resta intatto: il check
   fail-closed dell'executor confronta i campi del file con il contratto
   validato, metadati inclusi (una prima versione del design ripuliva lo
   schema di input — scartata perche' rompeva quel check).
   **Eccezione R2.6 in emissione** (`canonical_output_schema`): una
   `crs_resolution = resolved` preesistente e' corretta in
   `declared_unresolved` quando il contratto porta un'incoerenza rilevata
   — R4.6.4 vieta di silenziarla propagando la dichiarazione `resolved`
   che l'incoerenza smentisce; unica sovrascrittura ammessa, una sola
   chiave, una sola direzione (la direzione opposta non passa di qui: con
   una decisione del piano le dichiarazioni sono gia' state sostituite
   dalla strip).
8. **Operazioni che riscrivono un fatto canonico: sostituzione a monte,
   guard R2.6 intatto (attuata 2026-07-30).** R2.6 riguarda descrizioni
   divergenti DELLO STESSO FATTO; alcune operazioni CAMBIANO il fatto —
   `geo.reproject` cambia il CRS della colonna, le trasformazioni
   geometriche possono cambiare il tipo (`geo.centroid` di un poligono
   produce un punto). Per queste chiavi il contratto validato e'
   l'autorita' e la chiave ereditata si SOSTITUISCE, non si fonde: la strip
   avviene a monte, nel contratto di output prodotto dall'analisi
   (`analyze_reproject` rimuove il blocco CRS canonico della sorgente —
   `crs_id`, `crs_definition`+formato, `srid`, `axis_order`,
   `crs_resolution` — con `strip_rewritten_crs_keys`, stesso meccanismo di
   `strip_decided_crs_declarations`; `with_geometry_types` rimuove
   `types`/`types_declaration` con `strip_rewritten_types_declarations`), e
   `canonical_output_schema` le ri-emette dal contratto come ogni altra
   chiave. Il guard R2.6 NON e' indebolito: su qualunque altra chiave
   divergente continua a fallire (test di regressione dedicati), e una
   chiave riscritta che arrivasse comunque divergente (contratto costruito
   a mano) fallirebbe come prima. Alternativa scartata: marcare nel
   contratto le chiavi riscritte e insegnare al guard a saltarle — la
   sostituzione diventava un'eccezione nel punto di controllo invece che un
   fatto del contratto, e il punto esatto del conflitto (il campo
   pass-through riusato dall'analisi) restava tale. **Mappa dei tipi di
   output** (`transform_output_types` in `analyze/dispatch.rs`, verificata
   contro i kernel): `centroid`/`point_on_surface`/
   `line_interpolate_point` → `exact [point]`; `convex_hull`/
   `concave_hull` → `exact [polygon]` (entrambi avvolgono il risultato in
   `Geometry::Polygon`: `concave_hull` NON e' tipo-preservato); `envelope`
   → `exact [point,linestring,polygon]` (degeneri); `line_substring` →
   `exact [point,linestring]`; `buffer` → `exact [multipolygon]` (forma
   unica del kernel); `boundary` → `exact [multipoint,multilinestring,
   geometrycollection]`; `make_valid` → `mixed` senza elenco (l'output
   dipende dalla riparazione GEOS cella-per-cella: la colonna ammette tipi
   diversi PER DICHIARAZIONE, R3.4.1 — non `unresolved`, che significherebbe
   «byte non ispezionati» e per costruzione vieta l'elenco); `voronoi` →
   `exact [polygon]`; `clean_topology` → `exact [polygon,multipolygon]`;
   le booleane binarie (`clip`/`intersection`/`union`/`difference`/
   `symmetric_difference`) → `exact [multipolygon]`. Tipo preservato,
   propagazione invariata: `simplify`, `translate`, `scale`, `rotate`,
   `affine_transform`, `densify`, `snap_to_grid` (coordinate trasformate,
   variante geometrica inalterata). **Residuo dichiarato della classe**:
   le op che aggregano o espandono la geometria (`dissolve`/`collect`/
   builder/`explode`/`delaunay`/`split`/`subdivide`/`overlay`/`polygonize`/
   `line_merge`) clonano ancora la dichiarazione tipi dell'input nel
   contratto di output — il loro insieme di output e' funzione
   dell'input (espansioni) o dell'aggregazione, non coperto da questo
   intervento; il meccanismo (`with_geometry_types`) e' pronto per
   l'estensione. **Identificazione della colonna geometria**: l'estensione
   `ARROW:extension:name = geoarrow.wkb` e' ammessa, non richiesta — la
   forma a sole chiavi canoniche (`plenora.geometry.encoding` +
   `plenora.geometry.dimensions` bastano; criterio: almeno una chiave
   `plenora.geometry.*`, come in discovery) identifica la colonna
   (`field_declares_wkb_geometry`, predicato UNICO condiviso da
   `arrow_adapter::geometry_column_index`, dal trasporto e dal check di
   analyze, cosi' accettazione a esecuzione e rifiuto a compile-plan non
   possono divergere); una colonna non identificabile e' rifiutata in
   ANALISI (`require_identifiable_geometry`, modello B1.3 di
   `require_xy_dimensions` — ADR-0008: mai a meta' esecuzione), col check
   del trasporto ridotto a difesa in profondita'.

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
- **Fasi ambigue per varianti multi-sito** — RAFFINATE dal tagging ai
  confini (2026-07-30): gli errori `Io`/`DataMapping`/`Schema` che nascono
  leggendo una sorgente sono taggati `Read` (costruttori
  `Input::read_ipc_*`, `Network::input_stream`, sonde IPC della CLI); al
  confine di publish il riconoscimento della destinazione e' taggato
  `Probe` (la destinazione non supportata torna alla fase che aveva prima
  della fusione §9), la creazione del tempfile `Write`, flush/sync
  `Finalize`, check no-clobber e rename `Commit`. Approssimazioni RESIDUE
  dichiarate: `Io`/`DataMapping` NON taggati (nati nei kernel o nei
  percorsi legacy, dove nessun confine dichiara il momento) restano
  `Write` (il lato con possibile effetto sul supporto, il solo rilevante
  per R9.7); `InvalidPlan` del governor (scatta a runtime) resta
  `Validate` per decisione confermata — non si tagga; il tee di fan-out
  (`StoredEdgeError`) declassa qualunque errore d'arco non
  `Execution`/`Cancelled` a `InvalidPlan` («arco interrotto») come prima
  del tagging — comportamento preesistente, invariato. La disposizione
  R9.7 non dipende da nessuno di questi raffinamenti.
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

- **Op che riscrivono fatti canonici (attuata, 2026-07-30 — decisione 8)**:
  mappa tipi di output per-op in `analyze/dispatch.rs`
  (`transform_output_types`) con strip delle chiavi ereditate
  (`with_geometry_types`, `strip_rewritten_types_declarations`),
  sostituzione delle chiavi CRS in `analyze_reproject`
  (`strip_rewritten_crs_keys`), predicato di identificazione condiviso
  (`field_declares_wkb_geometry`: estensione `geoarrow.wkb` o sole chiavi
  canoniche) e rifiuto in analyze (`require_identifiable_geometry`). Test:
  sweep su tutte le 75 op (dichiarazione attesa per op), sostituzione su
  input con tipi dichiarati, preservazione per le op a tipo preservato,
  strip CRS di `reproject`, rifiuto in analisi vs accettazione
  canonica-only, regressione R2.6 su chiave divergente, end-to-end
  (executor: centroid su input canonica-only; executor+CLI con
  `proj-backend`: `reproject` con chiavi canoniche di sorgente).

- **`DeclaredUnresolved` e decisione nel piano (attuati, 2026-07-30 —
  BLOCK-08 di `release/rc.json`, decisione 7)**: variante
  `ContractCrs::DeclaredUnresolved` in `plenora-core`; discovery CLI che
  preserva lo stato dichiarato e i conflitti decidibili
  (`contract_crs_from_keys`); campo v4 `crs_decisions` (validazione
  strutturale in `PlanV4`, applicazione in `apply_crs_decisions`, identita'
  ADR 4 nella forma canonica); gate `require_resolved_crs` con messaggio
  distinto per stato; emissione `declared_unresolved` con le dichiarazioni
  originali ed eccezione R2.6 dichiarata in `canonical_output_schema`;
  stato nel fingerprint dei contratti di input (tre stati, tre
  fingerprint). Test: unit (discovery, emissione, gate, piano, fingerprint,
  fusione output) e integrazione CLI (propagazione `declared_unresolved`,
  `conflicting_crs` preservato e dichiarato, gate geo, decisione su stato
  non applicabile, decisione che risolve — con `proj-backend`).

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
  - **Percorso legacy a parita' (attuato, 2026-07-30 — BLOCK-06 di
    `release/rc.json`, decisione owner: PARITA', non ritiro)**: anche gli
    output legacy emettono le chiavi canoniche §2 in doppia emissione con
    quelle GeoArrow (DER-002 estesa al legacy). Forma minima scelta per lo
    stato CRS: il trasporto legacy non ha un `DataContract` tipizzato e
    non legge i metadati dell'input (il CRS e' dichiarato nello schema
    operativo e risolto al livello comandi, `publish.rs`); il blocco e'
    quindi derivato dal metadato `geo` del campo di OUTPUT — la stessa
    fonte del GeoArrow legacy, coerenza R2.6 per costruzione — con
    [`canonical_geometry_metadata_for_resolved_definition`] (corpo CRS
    condiviso col braccio `Resolved` di `canonical_geometry_metadata`:
    byte identici a parita' di definizione). Regole: campo riscritto
    (`geometry_output_field`) → stato `resolved` dalla definizione
    dichiarata nell'operazione; campo propagato invariato (pass-through,
    es. `within`/`count`) con chiavi canoniche → propagate invariate
    (R2.4/R4.6.4: `missing` resta `missing`, `declared_unresolved` resta
    con le dichiarazioni originali); campo propagato senza dichiarazioni
    CRS → `crs_resolution = missing` (R4.6.3) e `dimensions = unknown`
    (R3.4: le celle non sono ricodificate). Punto UNICO di applicazione:
    post-processo centrale `canonical_legacy_output` sui due entry point
    `transform_arrow`/`pair_arrow`, prima di `encode_ipc`, con versione
    R2.5 sullo schema (fail-closed su versione divergente) e rivestimento
    dei batch. Output senza geometrie (lineage di coppie) invariati. I
    comandi `transform` (frame WKB v2) e `spatial-join` (protocollo
    coppie) non hanno uno schema Arrow in uscita: la doppia emissione non
    e' applicabile, dichiarato qui.
- **Follow-up dichiarati**: chiave `plenora.field_id` (decisione 3) da
  proporre all'owner ICD; test di catena completa bordo-centro-bordo con
  gli altri due componenti. (Il tagging di fase ai confini era in questa
  lista: attuato il 2026-07-30, vedi la voce dedicata sotto.)
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
- **Tagging di fase ai confini (attuato, 2026-07-30 — BLOCK-03 di
  `release/rc.json`)**: variante wrapper `PlenoraError::Tagged { phase,
  source }` in `plenora-core` — `Display` DELEGATO alla sorgente (testo
  byte-identico, nessun consumatore testuale si rompe), `category()`/
  `remote_effect()`/`retry_disposition()` delegate, solo `phase()` e'
  raffinato; costruzione via `with_phase` (il primo tag, il piu' vicino
  all'origine, vince e non si annida), lettura via `phase_tag()`/`untag()`.
  Alternativa scartata: campo fase opzionale sulle varianti — avrebbe
  reso strutturate le varianti tuple (`Io(#[from] std::io::Error)`,
  `DataMapping(String)`), rompendo la conversione `#[from]` e ogni
  costruzione/match esistente. Confini taggati: lettura input → `Read`
  (`Input::read_ipc_*`, `Network::input_stream` — errori della sorgente e
  coerenza per-batch dello schema — sonde `is_ipc_file_format`/
  `ipc_header_schema` della CLI); publish ADR 7 → `Probe` (riconoscimento
  destinazione, incluse directory inesistente e destinazione non
  supportata, che torna alla fase pre-fusione), `Write` (creazione
  tempfile), `Finalize` (flush/sync), `Commit` (check no-clobber «output
  gia' esistente» e rename atomico). NON taggati, per scelta: errori
  della closure di scrittura di publish (nascono nel chiamante; derivati
  gia' corretti), governor (resta `Validate`), `Execution`/`Cancelled`
  (restano `Write`, decisione progettuale invariata), `step_error`/
  `tag_execution`/`with_diagnostics`/`at_input`/`at_node` (attraversano il
  wrapper per delega di `Display`: testo identico, comportamento
  invariato). Match che vede attraverso il wrapper per contratto:
  `From<PlenoraError> for ArrowTransportError` (braccio esplicito, la
  conversione conserva la variante interna come senza tag). Cleanup:
  nessun errore prodotto (tempfile ripulito via `Drop`, infallibile).

## Cambi di comportamento (dichiarati)

- **`declared_unresolved` non si auto-risolve piu' (R4.6.3, decisione 7,
  BLOCK-08)**. Prima la discovery risolveva una definizione dichiarata
  `declared_unresolved` ma risolvibile ed emetteva `resolved` — scelta
  silenziosa contro il default propagate di R4.6.3. Ora lo stato entra nel
  contratto come `ContractCrs::DeclaredUnresolved` con le dichiarazioni
  originali, si propaga invariato attraverso le op senza `CrsRequirement`
  (R4.6.4) e ferma le op geo in analyze con categoria `Crs` e messaggio
  distinto da `missing`. Nessuna chiamata al backend per questo stato:
  il caso `crs_unresolved` del corpus di conformita' non richiede piu'
  `proj-backend`.
- **Conflitto decidibile fra rappresentazioni CRS: da scelta silenziosa a
  `DeclaredUnresolved` (R4.3.1/R4.6.3, decisione 7)**. Prima `crs_id` +
  `crs_definition` co-presenti risolvevano `crs_definition` (precedenza
  implicita) e uno `srid` discordante da `crs_id` era ignorato: il centro
  sceglieva per conto dell'utente. Ora il conflitto decidibile senza
  backend diventa `DeclaredUnresolved` con entrambe le dichiarazioni — non
  errore (R4.6.1: il centro non e' il bordo di scrittura; attesa del corpus
  `conflicting_crs`: `transformation_core = preserve`), non scelta. La
  risoluzione resta possibile solo con una decisione esplicita nel piano
  (`crs_decisions`).
- **Emissione con incoerenza rilevata: `crs_resolution` corretta a
  `declared_unresolved` (R4.6.4)**. Un produttore che dichiara `resolved`
  con rappresentazioni in conflitto e' smentito dalle sue stesse chiavi:
  il blocco canonico di output corregge lo stato (unica sovrascrittura
  ammessa sulla fusione R2.6, una sola direzione) e ri-emette le
  dichiarazioni originali invariate (`crs_id`, `srid` di lineage), perche'
  l'incoerenza arrivi al bordo di scrittura, dove R4.6.2 la ferma.
  Conseguenza misurabile: il runner di conformita' (diff meccanico prima/
  dopo) segnala `crs_resolution: resolved -> declared_unresolved` sul caso
  `conflicting_crs` — e' la dichiarazione dell'incoerenza, non una perdita
  (il runner stesso dichiara di verificare solo la preservazione, non
  l'obbligo di dichiarare).

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
- **`phase()` raffinata ai confini (tagging, 2026-07-30)**: gli errori
  nati leggendo un input riportano ora `Read` (prima `Write`), quelli del
  confine di publish `Probe`/`Write`/`Finalize`/`Commit` secondo il punto
  (prima `Validate`/`Write`). Cambia il SOLO asse fase: testi `Display`,
  categorie, effetti e disposizioni di retry sono byte-identici a prima
  (verificato per delega del wrapper e da test dedicati). I consumatori
  machine-readable della fase ricevono valori piu' precisi; quelli
  testuali non osservano alcuna differenza.
- **Tipi geometrici dichiarati dalle op che li cambiano (decisione 8,
  2026-07-30)**. Prima il contratto di output delle trasformazioni 1:1 era
  `input.clone()`: un `geo.centroid` su `types=polygon/exact` produceva
  byte `Point` con contratto `polygon/exact` (e la stessa forma ereditata
  faceva sparare il guard R2.6 in emissione). Ora il contratto dichiara i
  tipi dell'OUTPUT (mappa per-op verificata contro i kernel, decisione 8)
  e le chiavi `types`/`types_declaration` ereditate sono sostituite.
  Conseguenza misurabile: gli output IPC dei type-changer emettono ora
  `plenora.geometry.types`/`types_declaration` coerenti coi byte (prima:
  assenti o ereditati).
- **`geo.reproject` con chiavi canoniche CRS di sorgente: da rifiuto R2.6
  a sostituzione (decisione 8)**. Prima un campo con `crs_id` (e
  `srid`/`axis_order`) della sorgente faceva fallire l'emissione (il
  contratto dice il target). Ora le chiavi CRS ereditate sono sostituite
  nel contratto di output dell'analisi e ri-emesse dal contratto: il
  piano esegue e l'output dichiara il target. R2.6 e' invariato per ogni
  altra chiave.
- **Colonna geometria a sole chiavi canoniche: da rifiuto a esecuzione ad
  accettata (decisione 8)**. Prima il trasporto identificava la colonna
  solo via `ARROW:extension:name` e il rifiuto arrivava a meta'
  esecuzione; ora la forma canonica-only e' accettata ovunque e una
  colonna davvero non identificabile e' rifiutata in validazione del
  piano (analyze), mai a meta' stream (ADR-0008).
