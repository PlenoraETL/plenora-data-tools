# plenora-data-tools — Architettura

Engine unico **Arrow-in / Arrow-out** per pipeline dichiarative di trasformazioni
tabellari e geografiche. Nasce dalla fusione di due progetti esistenti:

- **plenora-nogeo-tools** — engine tabellare (62 operazioni, piani JSON
  `Plan{steps}`, validazione fail-closed, Arrow IPC).
- **plenora-geo-tools-arrow** — engine geografico (65 operazioni, kernel su
  `geo::Geometry`, trasporto Arrow IPC con geometrie GeoArrow-WKB, gestione CRS,
  backend GEOS/PROJ opzionali).

Principi ereditati da entrambi, non negoziabili — sono **criteri di
accettazione**, non solo principi documentali:

- `#![forbid(unsafe_code)]` ovunque.
- **Fail-closed**: validazione integrale dei contratti prima di toccare i dati,
  campi sconosciuti rifiutati, nessun output parziale, publish atomico
  (tempfile + persist no-clobber).
- Limiti di risorsa applicati prima delle allocazioni — incluse le risorse di
  planning (§5).
- Errori senza dati sensibili: contesto (nodo, operazione, motivo), mai valori.
- Nessuna riproiezione implicita: solo lo step esplicito `geo.reproject` cambia CRS.
- Governor, code, pool, spill, cancellazione e publish sono progettati come
  **un unico protocollo di esecuzione** (§6.4), non come sottosistemi
  indipendenti: è la difesa contro deadlock, doppio conteggio, starvation e
  garanzie di atomicità non sostenibili.
- **Prestazioni e memoria sono criteri di accettazione** (decisione D26):
  la correttezza è progettata staticamente, la velocità si ottiene mantenendo
  il runtime minimale — hot path senza JSON né ricerche per nome, zero-copy
  dove la semantica lo consente, streaming con memoria quasi costante,
  benchmark gate con budget di regressione. I vincoli completi (V1–V10,
  M1–M5, G1–G3, E1–E3, invarianti prestazionali, metriche, benchmark) sono in
  `Prestazioni.md`, documento compagno con lo stesso peso di questo.

---

## 1. Modello mentale

L'utente descrive **cosa** fare come un grafo di operazioni; la libreria decide
**come** eseguirlo. Nel piano non compaiono mai annotazioni esecutive
(`parallel`, `threads`, `materialize`): renderebbero i piani dipendenti
dall'implementazione e dall'hardware. Il planner deriva tutto dal DAG.

```
input ─┬─ A ─┐
       └─ B ─┴─ C ─┬─ D ─┐
                   └─ E ─┴─ F ─→ risultato
```

Ogni arco del grafo è un `RecordBatch` (o uno stream di batch) conforme a un
`DataContract` dichiarato. Le geometrie sono colonne Arrow come le altre: uno
step tabellare le filtra/sposta insieme alle altre colonne, uno step geografico
le trasforma.

L'API pubblica ha esattamente **due fasi**:

```
Fase 1:  validate(plan_json, input_contracts) -> Result<ValidatedGraph>
Fase 2:  execute(&ValidatedGraph, inputs)     -> Result<Output>
```

`execute` accetta solo il prodotto di `validate` (type-state): non esiste un
modo di eseguire un grafo non validato. Internamente `execute` costruisce un
`ExecutionPlan` specifico dell'esecuzione corrente (§6.3): il `ValidatedGraph`
contiene decisioni semantiche stabili, l'`ExecutionPlan` decisioni fisiche.

---

## 2. Rappresentazione dei dati (decisione D1)

**Arrow è l'unica rappresentazione. Le geometrie sono colonne `Binary` con
encoding GeoArrow-WKB.**

- Colonna `Binary`, una cella WKB (2D) per riga, metadati di estensione secondo
  lo spec GeoArrow:
  - `ARROW:extension:name = geoarrow.wkb`
  - metadato `geo` (JSON) con chiave `crs` — obbligatorio se la colonna esiste.
- Validazione strutturale WKB per cella (limiti: 64 MiB/cella, 100k componenti,
  profondità 64), ereditata da `plenora-geo-tools-arrow/src/lib.rs`.
- **Nessuna dipendenza dal crate `geoarrow`.** Motivazioni:
  - i kernel geo lavorano comunque su `geo::Geometry<f64>`: la conversione per
    cella esiste con qualunque encoding; il nativo la renderebbe solo più economica;
  - le operazioni tabellari (filter/take/concat/join) non devono capire il
    contenuto delle celle: trattarle come byte opachi è corretto;
  - WKB rappresenta colonne eterogenee (Point + Polygon misti) senza union
    encoding;
  - è già implementato, testato e fuzzato nel progetto geo;
  - `geoarrow.wkb` è una codifica ufficiale dello spec GeoArrow: pyarrow, GDAL,
    DuckDB la leggono.
- **Porte di uscita documentate** (Fase 2C/3, dietro benchmark):
  - se il parsing WKB diventa collo di bottiglia, encoding nativo dietro lo
    stesso adapter, senza toccare i contratti dei nodi;
  - **cache di decode a livello di segmento**: in una catena geo fusa
    (`buffer → simplify → centroid`) le geometrie restano decodificate tra uno
    step e l'altro — al massimo un decode e un encode WKB per segmento fuso.
    Il contratto esterno resta `geoarrow.wkb`; l'adapter Arrow va progettato
    fin dalla Fase 2 in modo da **non precludere** questa ottimizzazione
    (vincoli V6/G1/G2 di `Prestazioni.md`).

**Versioni (decisione D0)**: Arrow pinnato `=59.1.0` — versione comune già
adottata dai due progetti di origine, scelta e verificata al momento della
decisione. Sub-crate `arrow-array`, `arrow-schema`, `arrow-ipc`, `arrow-select`
(non il metacrate `arrow`), in un unico punto (`[workspace.dependencies]`).
Tutti i crate del workspace dipendono da Arrow solo tramite `plenora-core`.
Poiché l'engine è Arrow-in/Arrow-out, i tipi Arrow fanno parte del **contratto
pubblico** dell'API: un bump di Arrow è un cambio potenzialmente breaking per
gli utilizzatori Rust — accettato e documentato come tale, non nascosto.
Il valore del pin è una decisione tecnica con data e motivazione; non è
un'invariante del documento essere o meno l'ultima release. (Verifica fatta in
fase di design: il crate `geoarrow` 0.8.0 richiederebbe Arrow ^58; non usandolo,
si resta su 59.1.0.)

---

## 3. Struttura del workspace

```
plenora-data-tools/
├── Cargo.toml                     # workspace + [workspace.dependencies]
├── crates/
│   ├── plenora-core/              # fondamenta condivise
│   ├── plenora-kernels-table/     # kernel tabellari (da nogeo-tools)
│   ├── plenora-kernels-geo/       # kernel geografici (da geo-tools-arrow)
│   ├── plenora-engine/            # contratto piano, planner, executor
│   └── plenora-cli/               # binario sottile
├── fuzz/                          # target cargo-fuzz unificati
├── tests/                         # integration e matrice avversaria
└── reference/                     # differenziale vs Manipola Python
```

### 3.1 plenora-core

- Re-export unico di Arrow (versione pinnata nel workspace).
- `PlenoraError`: fusione di `EngineError` e `GeoEngineError` — varianti
  `Contract`, `Unsupported`, `Schema`, `Step{node, operation, reason}`, `Crs`,
  `Arrow`, `Json`, `Io`.
- `Limits` unificato, in tre famiglie semanticamente distinte (decisione D19):
  - **Dati/runtime**: `RowLimits` — i limiti principali della v1 sono
    `max_input_rows`, `max_output_rows`, `max_rows_per_edge`,
    `max_expansion_factor`; `max_total_rows_processed` resta una **metrica o
    limite avanzato**, perché il suo conteggio dipende dal piano fisico (due
    `ExecutionPlan` semanticamente equivalenti potrebbero conteggiarlo in modo
    diverso — semantica in ADR 6) — più `max_governed_memory_bytes`, `max_temp_bytes`,
    `spill_partitions`, `max_wkb_cell_bytes`, `max_payload_bytes`,
    `max_batches`, profondità geometrica, `max_parallelism`.
  - **Piano** (`PlanLimits`, §5): complessità del grafo e delle config.
  - **Stringhe**: `max_string_bytes`, `max_regex_bytes`.
- `catalog`: `OperationDescriptor` unificato — vedi §4.3.
- `crs`: contratto CRS (da `crs.rs` del progetto geo): `ResolvedCrs`,
  `CrsKind`, `resolve_crs`, `validate_requirement`, `validate_geometry_domain`.
  Senza backend PROJ: fallimento chiuso `CRS_BACKEND_UNAVAILABLE`.
- Convenzione colonna geometria: costanti dei metadati, verifica estensione,
  lettura/scrittura `geo.crs`.

### 3.2 plenora-kernels-table

Trasloco dei 17 moduli kernel di `plenora-nogeo-tools/src/kernels/`
(`columns`, `strings`, `cleansing`, `filtering`, `dates`, `utility`, `analysis`,
`aggregation`, `reshape`, `joins`, `setops`, `security`, `quality`,
`governance`, `formula`, `expressions`, `spill`) con i loro helper condivisi.
Funzioni pure `&RecordBatch -> Result<RecordBatch>`, senza I/O, senza stato
globale. Config serde con `deny_unknown_fields` invariate: i piani esistenti
continuano a funzionare.

### 3.3 plenora-kernels-geo

- Kernel su `geo::Geometry<f64>` (trasloco di `operations`, `analysis`,
  `topology`, `predicates`, `construction`, `extended`, `extended_algorithms`,
  `advanced`, `spatial_join`): indipendenti da Arrow, non conoscono IPC,
  `RecordBatch` né orchestrazione.
- **Adapter Arrow**: il confine unico in cui è definita la rappresentazione
  geometria-in-Arrow (individuazione colonna, verifica estensione, decode/encode
  WKB, metadati CRS, dispatch per shape del risultato). Progettato per ammettere
  la cache di decode per segmento (§2) senza modifiche ai contratti.
- Feature flag preservate: `geos-backend`, `proj-backend`,
  `full-backends = geos + proj`.
- Parallelismo intra-kernel con rayon, collect indicizzato (ordine
  deterministico).

### 3.4 plenora-engine

Il pezzo che oggi non esiste in nessuno dei due progetti. Tre componenti:

- **Planner** (`validate`): funzione pura. Prende piano + contratti di input e
  produce il `ValidatedGraph`: DAG validato, config deserializzate e migrate,
  contratti di ogni arco inferiti a secco, CRS risolti, decisioni **semantiche
  stabili** (struttura, tipi, CRS, ordini dichiarati).
- **Preparer** (`prepare`, interno a `execute`): produce l'`ExecutionPlan` per
  l'esecuzione corrente — segmenti streaming, punti di materializzazione,
  gruppi paralleli, quote di risorse.
- **Executor**: esegue l'`ExecutionPlan` secondo il protocollo di §6.4.

### 3.5 plenora-cli

- `validate --plan p.json --inputs i1.arrow i2.arrow ...` — solo fase 1,
  stampa il riepilogo del grafo validato (contratti degli archi, fingerprint).
- `run --plan p.json --inputs ... --output o.arrow` — validate + execute,
  publish atomico.
- `catalog` — catalogo machine-readable.
- Compatibilità protocollo PLNGEO3 (envelope checksummed) solo dove serve
  verso l'adapter Python esistente; I/O interno: Arrow IPC file format.

**Fuori scope (invariato)**: lettura/scrittura di formati file (CSV, XLSX,
SHP, GPKG, GeoJSON, …) resta a **plenora-IO-tools** (il `plenora-datafile`
previsto, in costruzione in parallelo: `C:\tmp\plenora-IO-tools`), che produce
e consuma `RecordBatch` secondo il contratto di §4.1 condiviso via
`plenora-core`.

---

## 4. I tre livelli di contratto

### 4.1 Contratto di input (bordo esterno)

Un input valido è:

1. Arrow IPC (file o stream). Nient'altro entra nel sistema.
2. Schema con tipi nel **set chiuso ammesso**: `Utf8`, `LargeUtf8`
   (normalizzato), `Int64`, `UInt64`, `Float64`, `Boolean`, `Date32`,
   `Timestamp(Millisecond, tz)`, `Decimal128`, `Binary`, `Dictionary`,
   `List`/`Struct` (per explode/unnest).
3. Colonne geometriche riconosciute **solo** tramite metadati di estensione
   `geoarrow.wkb` + `geo.crs` obbligatorio e risolvibile. Una colonna `Binary`
   senza metadati è una colonna di byte, non una geometria.
4. **Nella v1, al massimo una colonna geometrica per input** (decisione D16):
   input con più colonne `geoarrow.wkb` sono rifiutati in validazione.
5. Header, estensioni e CRS validati all'apertura (validazione statica);
   i contenuti delle celle sono validati incrementalmente durante la lettura
   (validazione dinamica, §6.2).

### 4.2 Contratto di arco (bordo interno)

Ogni arco del DAG trasporta `RecordBatch` conformi al `DataContract` inferito
dal planner. La conformità è verificata ai bordi dei nodi: sempre nei test,
mai in produzione (costo). Invariante di debug: un difetto è sempre
"il nodo X ha violato il suo contratto di output".

### 4.3 Contratto di operazione (catalogo)

Ogni operazione dichiara in modo machine-readable:

| Campo | Significato |
|---|---|
| `id` | namespaced: `table.*` / `geo.*` (alias legacy versionati, §7) |
| `family` | kernel family di appartenenza |
| `origin` | manipola-compat / estensione |
| `arity` | unaria / binaria ordinata (left,right) / N-aria |
| `execution_class` | `Streaming` (1:1) / `Blocking` / `BinaryBlocking` |
| `cancellation_behavior` | `Cooperative` / `BoundaryOnly` / `NonInterruptible` (decisione D24) |
| `result_shape` | da geo: `OneToOne`, `OneToMany`, `ManyToOne`, `Collective`, … |
| `crs_requirement` | solo op geo: `known` / `projected` / `geographic` / `same_projected` / `reprojection` |
| `required_capabilities` | backend/feature necessari (`geos`, `proj`) — verificati in validazione |
| `determinism` | politica di ordinamento per op con ordine non definito (union, concat di rami) |
| `semantic_version`, `config_schema_version`, `contract_analysis_version`, `kernel_version` | **versioni esplicite per componente** (decisione D17): ogni modifica incompatibile incrementa la versione pertinente; il fingerprint del catalogo deriva da queste, mai da hash del binario |
| `maturity` / `support_level` | pipeline di promozione |
| `analyze_contract(inputs, config) -> Result<DataContract>` | **inferenza a secco** — obbligatoria per ogni op |

**`DataContract`** (decisioni D6, D16, D25):

```rust
struct DataContract {
    schema: SchemaRef,
    geometries: Vec<GeometryColumnContract>,   // v1: len <= 1 (validato)
    active_geometry: Option<FieldId>,
    properties: ContractProperties,
}

struct GeometryColumnContract {
    field_id: FieldId,          // identità logica stabile nel grafo
    name: String,               // proprietà visibile: le rinomine cambiano name, non field_id
    crs: ResolvedCrs,
    dimensions: GeometryDimensions, // 5 varianti (xy..xyzm, unknown), ADR 8
    nullable: bool,
}
```

- Il modello è **estensibile** (più geometrie, geometria attiva), il
  comportamento v1 è **chiuso**: un contratto con più di una colonna geometrica
  non è validabile. Struttura aperta, comportamento chiuso.
- **`FieldId`: namespace globale del grafo** (decisione D16). Gli ID sono
  assegnati dal planner dopo aver letto tutti gli input — oppure rimappati
  all'ingresso del grafo — così due input non possono collidere (entrambi
  potrebbero arrivare con un ipotetico `FieldId(4)` assegnato
  indipendentemente). Regole di propagazione: una **rinomina preserva** il
  `FieldId`; una colonna **calcolata o derivata ne riceve uno nuovo**; un join
  eredita i `FieldId` globali dei rispettivi rami senza collisioni.
- Ogni proprietà porta **provenienza e scope**:

```rust
enum PropertyConfidence<T> { Declared(T), Proven(T), Estimated(T), Unknown }
enum PropertyScope { Schema, Batch, Stream, Dataset }
```

  Il planner usa come precondizioni semantiche solo proprietà `Proven`;
  le `Estimated` guidano esclusivamente scelte prestazionali correggibili a
  runtime. Lo scope conta: "ogni batch è ordinato" non implica "lo stream è
  ordinato". Le proprietà possono cambiare livello nel tempo: una `Declared`
  può diventare `Proven` tramite validazione dinamica, una `Estimated` può
  essere aggiornata dal runtime, e `analyze_contract` di ogni op deve
  **declassare o eliminare** le proprietà che l'operazione non garantisce più
  in output. Cardinalità, bounding box, unicità, validità topologica non sono
  dimostrabili dagli header: non possono mai essere `Proven` in fase 1.
  La rappresentazione concreta (`ContractProperty<T> { confidence, scope }` o
  tipi specifici per famiglia di proprietà) deve escludere a compile-time le
  combinazioni prive di senso — es. `Proven` + scope `Schema` per una
  proprietà che ha significato solo a livello dataset (dettaglio di type
  safety, ADR 5).
- Un'op senza `analyze_contract` non entra nel catalogo.

---

## 5. Formato del piano (DAG dichiarativo)

```json
{
  "schema_version": 5,
  "limits": { "max_rows_per_edge": 10000000, "max_governed_memory_bytes": 536870912, "max_parallelism": 8 },
  "crs": "EPSG:32632",
  "inputs":  ["main", "fiumi"],
  "nodes": [
    { "id": "a", "op": "table.filter_rows",  "in": ["main"],  "config": { } },
    { "id": "b", "op": "geo.buffer",         "in": ["fiumi"], "config": { "distance": 100 } },
    { "id": "c", "op": "geo.spatial_join",   "in": ["a", "b"], "config": { "predicate": "intersects" } },
    { "id": "d", "op": "table.aggregate",    "in": ["c"],     "config": { } }
  ],
  "output": "d"
}
```

Regole:

- Campi sconosciuti rifiutati a ogni livello (`deny_unknown_fields`).
- Grafo aciclico, un solo nodo di output, ogni `in` deve riferire un nodo o un
  input esistente.
- Il piano **lineare** (`Plan{steps}` di nogeo, schema_version ≤ 3) è il caso
  degenerato: ogni nodo ha un solo `in`, il precedente. Conversione tramite la
  pipeline di migrazione esplicita (§7), non dipendente dai default correnti,
  e con destinazione il canonico **v5**: non esiste una forma intermedia da
  cui ripartire.
- La `schema_version` canonica è **5** (ADR 15). Un piano `schema_version: 4`
  è accettato solo attraverso la migrazione esplicita, che ne traduce il nome
  del budget di memoria. **Non c'è alias**, e il rifiuto è simmetrico: un
  piano v5 che usa il nome della v4 è rifiutato, un piano v4 (o lineare v1)
  che usa il nome della v5 pure. Ogni formato conserva il proprio nome —
  `max_memory_bytes` nella v1 e nella v4, `max_governed_memory_bytes` nel
  canonico — e la traduzione avviene all'ingresso, mai lasciando che un nome
  funzioni in due formati.
- Nessuna annotazione di esecuzione nei nodi: il piano dichiara solo
  dipendenze e configurazioni.
- `max_parallelism` sta in `limits` (risorsa), non nei nodi.

**Limiti alla complessità del piano** (`PlanLimits`, decisione D19): un piano
ostile può consumare risorse già in parse/validazione/ordinamento topologico.
Si applicano **il prima possibile durante il parsing**, prima di qualsiasi
allocazione guidata dal contenuto:

```rust
struct PlanLimits {
    max_plan_json_bytes: usize,
    max_plan_nodes: usize,
    max_plan_edges: usize,
    max_plan_depth: usize,
    max_fan_out: usize,
    max_inputs: usize,
    max_config_bytes_per_node: usize,
    max_identifier_bytes: usize,
}
```

### Regole sulle colonne geometria nel grafo

- Gli step `table.*` trattano la colonna geometria come una colonna qualsiasi
  (filtrare/rinominare/proiettare la coinvolge come le altre); i metadati di
  estensione e `geo.crs` **seguono la colonna** anche attraverso rinomine (il
  `FieldId` resta, cambia solo il nome).
- Eliminare la colonna geometria rende il batch non-geografico: gli step
  `geo.*` successivi falliscono **in validazione** (grazie all'inferenza a
  secco), non a runtime.
- Solo `geo.reproject` modifica `geo.crs`; ogni altro step geo lo preserva.

---

## 6. Le due fasi in dettaglio

### 6.1 Fase 1 — `validate`

Input: piano JSON + contratti di input (schemi Arrow letti dagli header IPC,
nessuna riga di dati).

Passi, tutti a secco:

1. Applicazione dei `PlanLimits` durante il parsing.
2. Parse e validazione strutturale del grafo (aciclicità, riferimenti,
   output unico); migrazione di piani e config legacy (§7).
3. Deserializzazione e validazione di ogni `config` contro lo schema
   dell'operazione.
4. Risoluzione CRS e verifica `crs_requirement` per ogni nodo geo; verifica
   compatibilità CRS left/right per i nodi binari.
5. Verifica delle `required_capabilities` contro i backend compilati
   (GEOS/PROJ): un'op senza backend disponibile fallisce qui, non a metà
   esecuzione.
6. Inferenza dei `DataContract` arco per arco in ordine topologico
   (`analyze_contract` di ogni op), con assegnazione dei `FieldId` nel
   namespace globale del grafo.

Output: `ValidatedGraph` immutabile, contenente **solo decisioni semantiche
stabili** — struttura, tipi, CRS, ordini dichiarati. Nessuna decisione fisica:
input con lo stesso schema possono avere dimensioni e distribuzioni opposte
(cento righe o un miliardo), e la strategia migliore cambia.

**Identità del `ValidatedGraph`** (decisione D7):

```rust
struct ValidatedGraph {
    plan_hash: PlanHash,                          // hash canonico del piano migrato
    catalog_fingerprint: CatalogFingerprint,      // dalle versioni per-componente (D17)
    engine_version: EngineVersion,
    required_capabilities: CapabilitySet,
    input_contract_fingerprints: Vec<ContractFingerprint>,
    plan_format_version: u16,
    // grafo, contratti degli archi...
}
```

Il `catalog_fingerprint` deriva dai descrittori serializzati in ordine stabile
con le loro versioni esplicite (`semantic_version`, `config_schema_version`,
`contract_analysis_version`, `kernel_version`, capability, determinismo) —
**mai** da hash del binario o del codice compilato, che cambierebbe tra
compilatori, piattaforme e build senza alcun cambio semantico. Due cataloghi
con gli stessi nomi ma semantica diversa producono fingerprint diversi; la
disciplina di incremento delle versioni è parte della review e della CI
(ADR 4).

L'executor rifiuta un `ValidatedGraph` non più compatibile: catalogo cambiato,
versione Arrow diversa, backend diversi, **profilo di publish richiesto non
supportato** (il `required_capabilities` include anche
`AtomicPublish`/`DurableAtomicPublish`), input con contratto diverso → errore
di mismatch, mai procedere. Questo rende sicuro il caching dei grafi validati.

### 6.2 Validazione statica e validazione dinamica

La fase 1 legge solo header e metadati: **non può** verificare i contenuti
delle celle. La distinzione è esplicita (decisione D8):

- **Validazione statica** (fase 1): limiti del piano, struttura del grafo,
  config, schema Arrow, metadati geo, presenza e risolvibilità del CRS,
  compatibilità CRS, capability dei backend, inferenza dei contratti.
- **Validazione dinamica incrementale** (fase 2, durante la lettura):
  struttura WKB di ogni cella, limiti per cella, coordinate finite, profondità
  e numero di componenti, dominio geografico effettivo, dimensionalità e
  nullability della colonna geometrica contro il `GeometryColumnContract`,
  limiti di righe e payload. La validazione incrementale può promuovere
  proprietà a `Proven(Batch)` per il batch corrente; una proprietà dataset-wide
  diventa `Proven(Dataset)` solo al completamento dell'intero input — mai
  inferenze premature a metà stream.

Corollario onesto: gli errori di **contratto** sono scoperti prima della lettura
dei dati; gli errori di **contenuto** sono scoperti in streaming, prima che i
dati non validi raggiungano il nodo successivo — e comunque senza mai produrre
output parziale.

### 6.3 `prepare` ed `execute`

`execute(&ValidatedGraph, inputs)` svolge internamente due passi (decisione
D15):

1. **`prepare(&ValidatedGraph, runtime_context) -> ExecutionPlan`**: decisioni
   fisiche per *questa* esecuzione — segmenti streaming, punti di
   materializzazione (fan-out e fan-in), gruppi di rami paralleli, quote di
   risorse per nodo. Uno stesso `ValidatedGraph` produce `ExecutionPlan`
   diversi su dati di scala diversa: è il comportamento voluto.
2. **Esecuzione** dell'`ExecutionPlan` secondo il protocollo di §6.4.

Le statistiche di runtime sono trattate con la stessa disciplina delle
proprietà:

```rust
enum RuntimeStatistic<T> { Known(T), Estimated(T), Unknown }
```

Negli Arrow IPC **stream** il numero di batch e di righe non è noto prima della
lettura completa; nel file format può esserlo ma non copre tutte le statistiche
utili. Regola: `prepare` produce **sempre un piano valido anche con statistiche
completamente assenti** (`Unknown` → scelta conservativa), e le usa solo per
scelte migliorative quando `Known` o `Estimated`.

L'`ExecutionPlan` è composto da **segmenti fisici** con modalità esplicita
(vincoli E1/E2 di `Prestazioni.md`): ogni kernel riceve una configurazione già
deserializzata, validata, tipizzata, normalizzata e risolta rispetto agli
indici di colonna — niente JSON né mappe dinamiche nel loop di esecuzione,
niente ricerche per nome nel percorso per batch.

```rust
struct PhysicalSegment {
    kernels: Box<[PreparedKernel]>,
    mode: SegmentMode,               // LinearStreaming | GeoFused | Blocking | BinaryBlocking
    output_contract: Arc<DataContract>,
    // batch size, strategia di parallelismo (SerialFused | ParallelPerBatch |
    // ParallelPerBranch | BlockingSingleTask), last consumer, budget,
    // possibilità di spill, policy di cancellazione, metriche
}
```

Regole di esecuzione:

- **Segmenti streaming**: catene di nodi `Streaming` scorrono batch-per-batch
  senza materializzare l'intera tabella.
- **Materializzazione ai fan-out/fan-in** (decisione D9): strategia
  conservativa della v1; il fan-out resta una proprietà logica del DAG
  (decisione D5) e alternative fisiche future (rilettura di sorgenti seekable,
  broadcaster bounded, spill progressivo, materializzazione solo sotto
  pressione di memoria, condivisione reference-counted) non cambiano il
  formato del piano.
- **Determinismo** (decisione D10): due livelli —
  - **Semantico** (garantito sempre): stesse righe, valori, ordine dichiarato,
    null, geometrie nella precisione prevista, indipendentemente dallo
    schedule. Per le geometrie l'uguaglianza è definita in ADR 1 (confronto
    geometrico, non confronto dei byte WKB: orientamento anelli, punto
    iniziale, ordine componenti, tolleranza floating point, `NaN`/`-0.0`,
    garanzia limitata alla stessa versione dei backend).
  - **IPC canonico** (opzionale): anche stessi confini di batch, ordine
    metadati, dictionary layout, configurazione writer. Solo a parità di
    versione engine; per test, cache e hashing.
- **Publish atomico** (decisione D22): la garanzia è definita da una **matrice
  di supporto** e da **due profili distinti** — rename atomico e durabilità non
  sono la stessa cosa:
  - `AtomicPublish`: nessun output parziale mai visibile (tempfile +
    rename/persist no-clobber sullo stesso filesystem);
  - `DurableAtomicPublish`: in più, la durabilità dopo crash della macchina
    (`fsync` del file e della directory), a costo di prestazioni — intesa come
    **le più forti garanzie di durabilità offerte dal filesystem e dalla
    piattaforma supportata**, non una garanzia universale dopo qualunque
    guasto hardware (controller, cache disco e impostazioni di sistema restano
    fuori dal controllo dell'engine).
  L'ADR 7 decide il profilo di default. Matrice: filesystem locali supportati,
  requisito same-filesystem tra tempfile e destinazione, comportamento
  documentato su Windows (share lock, antivirus) e Linux, destinazioni
  remote/network fs **fuori scope nella v1**. Il persist avviene solo a grafo
  completato con successo.

### 6.4 Il protocollo di esecuzione

Governor, code, pool, spill, cancellazione e publish sono **un unico
protocollo** (decisione D18). Le regole:

**Resource accounting** (decisione D11):

```rust
struct ResourceGovernor { /* quota globale di piano */ }
struct MemoryLease { /* quota reference-counted, segue i batch */ }
struct TempReservation  { /* quota spill */ }
struct RowBudget        { /* budget righe secondo RowLimits */ }
```

- Garanzia riformulata onestamente: il governor contabilizza in modo
  **deterministico e generalmente preciso** le allocazioni governate
  dall'engine; le allocazioni condivise (buffer slice zero-copy, dictionary
  condivisi), esterne o native (GEOS) possono essere solo **stimate**. ADR 2
  definisce ownership contabile dei buffer, regole per slice e dictionary,
  accounting dei temporanei, margine di sicurezza configurabile e metriche
  (riservata/osservata/stimata).

**Prevenzione deadlock sulle reservation** (decisione D18, invariante I4):

> Nessun nodo può attendere indefinitamente una nuova reservation mantenendo
> risorse che impediscono agli altri nodi di progredire.

Le due categorie di operatori sono nettamente separate: le operazioni blocking
con memoria stimabile acquisiscono la **reservation completa** prima di
iniziare (divieto di attesa con reservation parziale, acquisizione in ordine
globale, fail-fast); le **operazioni adattive** — join, explode, alcune
operazioni geografiche, la cui memoria finale non è stimabile a priori —
seguono il protocollo chunked dedicato: reservation minima iniziale, crescita a
chunk, **nessuna attesa di nuova quota senza prima spillare o rilasciare
memoria revocabile**, rispetto di `max_expansion_factor` (la cui base di
confronto — output/input, per operazioni binarie rispetto a left, right o
somma — è **specifica per classe di operazione** e definita in ADR 6),
interruzione controllata prima che l'output intermedio diventi ingestibile.
Timeout solo come ultima protezione. Test deterministici obbligatori con quote
molto basse e almeno due rami concorrenti.

**Ownership dei batch nelle code** (decisione D18, invariante I5): batch e
quota di memoria attraversano le code come un'unica unità, e il fan-out
condivide la stessa quota tramite un lease reference-counted —

```rust
struct GovernedBatch {
    batch: RecordBatch,
    lease: Arc<MemoryLease>,
}
```

Il batch resta contabilizzato **una sola volta** fino al rilascio dell'ultimo
riferimento: niente doppio conteggio producer/consumer, niente attribuzione
arbitraria a un solo ramo del fan-out, niente quota persa alla cancellazione.
ADR 2 definisce le regole per slice zero-copy, drain, cleanup e batch condivisi
tra più consumer, e fissa i requisiti di **osservabilità dei lease** — un
consumer che conserva un riferimento mantiene la quota occupata (contabilmente
corretto, ma difficile da diagnosticare senza metriche): età del lease, nodo
proprietario originario, numero di riferimenti, byte trattenuti, lease più
vecchi, lease vivi durante la cancellazione.

**Thread pool** (decisione D13, invariante I3):

> Un worker CPU non deve mai attendere in modo bloccante una risorsa che può
> essere liberata solo da un altro worker dello stesso pool.

Pool CPU globale unico (rami DAG e kernel rayon, nessun pool annidato per
nodo); I/O (lettura, scrittura, spill) separato; semaforo globale
(`max_parallelism`); code bounded. **Prototipo obbligatorio** prima
dell'executor completo, con: `max_parallelism` = 1 e 2, due rami
CPU-intensive, kernel rayon invocato da un ramo, code piene, cancellazione
durante un'attesa, ramo con spill, producer/consumer molto sbilanciati, due
rami che competono per memoria insufficiente.

**ExecutionContext** (decisione D12): ogni nodo riceve un **handle condiviso**
allo stesso contesto — una sola quota globale per esecuzione:

```rust
struct ExecutionContext {
    execution_id: ExecutionId,
    cancellation: CancellationToken,
    resources: Arc<ResourceGovernor>,
    temp_store: Arc<TempStore>,
    metrics: Arc<dyn MetricsSink>,
}
```

**Failure e cancellazione**: un ramo che fallisce cancella immediatamente gli
altri (first-error wins). L'errore primario conserva diagnosi completa:
`execution_id`, nodo, operazione, categoria, source chain interna, disposizione
di retry (R9.7) se applicabile — mai valori o payload sensibili. Gli
errori secondari restano telemetria. Per il debugging operativo è prevista una
**modalità diagnostica esplicitamente opt-in** (solo per input fidati):
l'errore può includere nodo, indice batch, indice riga, colonna e tipo di
violazione (es. `node=buffer batch=12 row=941 field=geometry
reason=WKB_DEPTH_LIMIT`), con hash o descrizione strutturale del valore — mai
il valore stesso. I kernel lunghi controllano il token
secondo il loro `cancellation_behavior`. Per i `NonInterruptible` (alcune
chiamate GEOS/PROJ): si impediscono nuove attività dopo la cancellazione, la
latenza massima attesa è **documentata e osservabile nelle metriche**, le
reservation trattenute dal kernel sono visibili al governor, gli altri rami
possono attendere il suo completamento prima del cleanup finale — nella v1
questa attesa è accettata esplicitamente, non promettiamo cancellazione
immediata. L'isolamento in processo separato è valutato solo per backend
realmente instabili.

**Panic e crash** (decisioni D14, D21): i panic Rust intercettabili sono catturati
al confine dell'executor (`catch_unwind`), causano cancellazione globale e sono
convertiti in errore interno senza dati sensibili; i kernel non usano panic per
errori attesi; **nessun panic può portare al publish**. La garanzia è limitata
onestamente: `catch_unwind` non copre `panic = "abort"`, crash in GEOS/PROJ,
OOM killer, kill esterni. Difesa strutturale: directory temporanee isolate per
`execution_id`, marker di esecuzione attiva/completata con lock file (prova
principale locale — rilasciato dal sistema operativo dopo un crash), PID,
identificativo host e timestamp di heartbeat come segnali diagnostici
aggiuntivi. Regola dello **scavenging all'avvio**: elimina solo directory
**senza lock attivo e con heartbeat scaduto** — PID e host non sono prove
sufficienti (il PID può essere riutilizzato; una macchina sospesa o ibernata
può rendere vecchio il timestamp senza che l'esecuzione sia orfana). TTL
conservativo, test di riavvio con directory lasciate intenzionalmente
incomplete.

**Cleanup garantito**: a successo, errore, cancellazione o panic intercettabile —
cleanup di spill, chiusura di code e broadcaster, nessun publish se qualunque
ramo è fallito.

**Nessun side effect** (decisione D23, invariante I2): kernel e nodi sono privi
di side effect esterni osservabili (file, chiamate remote, database, eventi).
Solo il sink finale pubblica un risultato. Metriche e telemetria tecnica sono
fuori dalla transazione purché prive di dati sensibili. Operazioni con side
effect apparterranno a un futuro modello transazionale separato.

### 6.5 Conseguenze della separazione in due fasi

- **Riuso**: un `ValidatedGraph` si esegue N volte su input diversi (con
  contratti conformi): la validazione si paga una volta; ogni esecuzione ha il
  proprio `ExecutionPlan` adeguato alla scala dei dati.
- **Dry-run**: il `ValidatedGraph` è ispezionabile senza eseguire nulla.
- **Testabilità**: planner puro testabile con grafi e schemi sintetici;
  executor con property test "seriale vs parallelo, risultato semanticamente
  identico" e test deterministici del protocollo (quote basse, code piene,
  cancellazione, panic).

---

## 7. Compatibilità e migrazione

- **Id operazioni**: namespace `table.*` / `geo.*`.
- **Alias legacy versionati** (decisione D20): la risoluzione degli alias senza
  namespace usa una tabella versionata `(schema_version, legacy_alias) ->
  canonical_operation_id`, **immutabile per le versioni già pubblicate**: un
  alias univoco oggi non può cambiare significato domani quando arriva una
  nuova operazione potenzialmente in conflitto.
- **Pipeline di migrazione esplicita** (decisione D20): piano legacy → parse
  nella versione originaria → migrazione canonica del piano → migrazione
  versionata delle config → piano v5 canonico → validazione. Mai dipendere
  implicitamente dai default correnti delle configurazioni. La migrazione è
  **deterministica e idempotente**, coperta da **golden test per ogni versione
  supportata**: piano legacy → migrazione → serializzazione canonica →
  confronto con fixture, inclusi percorsi con più salti di versione.
  Il `plan_hash` è calcolato sul piano **canonico migrato**: le regole di
  canonicalizzazione sono parte dell'ADR 4 — ordine dei campi JSON
  irrilevante, numeri e default normalizzati, alias sostituiti dagli ID
  canonici, e regola esplicita sull'equivalenza tra config omessa e config con
  default esplicito (due piani legacy semanticamente equivalenti devono
  produrre lo stesso piano canonico, quindi lo stesso hash: è ciò che rende
  cache e riproducibilità affidabili).
- **Protocollo PLNGEO3**: preservato per l'adapter Python; geometrie
  `geoarrow.wkb` già conformi al contratto di input.
- **Test ereditati**: tutte le suite esistenti dei due progetti devono passare
  nel workspace senza modifiche di comportamento.

## 8. Fasi di lavoro

- **Fase 0 — Fondamenta**: workspace, `[workspace.dependencies]`, CI,
  quality-gate. Diff dei cataloghi e tabella alias versionata. (ADR: già
  scritti, §9 e `docs/adr/`.)
- **Fase 1 — Coesistenza + baseline**: trasloco meccanico nei 4 crate, errori e
  `Limits` unificati, catalogo unico con namespace + alias. Nessun cambio di
  comportamento: suite dei due progetti verdi nel workspace. Rigorosamente
  meccanica. In parallelo: **baseline prestazionale archiviata in CI** dei due
  progetti originari (throughput, allocazioni, picco RSS; dataset sintetici e
  reali) — è il riferimento del benchmark gate (D26).
- **Fase 2A — Executor minimo**: formato piano v4, `PlanLimits`, planner con
  inferenza a secco (`analyze_contract` per ogni op — il lavoro netto più
  grosso), `prepare`/`ExecutionPlan` con `PhysicalSegment`, segmenti lineari
  diretti, streaming reale, blocking essenziale, metriche. API a due fasi, CLI
  `validate`/`run`. Nessuna ottimizzazione non misurata.
- **Fase 2B — Concorrenza e memoria**: protocollo di esecuzione completo
  (governor, `GovernedBatch` con lease, code bounded, pool, reservation
  adattive, spill selettivo, cancellazione, scavenging, spatial join). **Tre
  prototipi critici in testa**:
  1. ownership condivisa di batch e lease al fan-out;
  2. reservation adattive senza deadlock per join ed explode;
  3. pool CPU con `max_parallelism` = 1 e code bounded piene.
  Poi gli altri prototipi: due rami in competizione per memoria insufficiente,
  coda piena durante cancellazione, panic intercettabile con cleanup completo,
  temporanei orfani dopo riavvio simulato, persist no-clobber su Windows e
  Linux, spatial join con espansione elevata e limiti di righe. Test: pipeline
  miste, grafi con fan-out/fan-in, matrice avversaria estesa, property test
  seriale/parallelo, preservazione errore primario, fingerprint mismatch,
  golden test di migrazione, validazione dinamica completa.
- **Fase 2C — Ottimizzazioni fondamentali** (solo dietro benchmark):
  fusione dei segmenti, cache di decode geo per segmento (§2), batch sizing
  adattivo (`target_batch_bytes`/`max_batch_bytes`), rilascio al last consumer,
  riduzione delle allocazioni, scelta dinamica seriale/parallela. Ogni segmento
  fisico fuso mantiene la **mappa verso i nodi logici originari**: attribuzione
  degli errori al nodo corretto, cancellazione, metriche per nodo, limiti e
  conteggi per arco, visibilità delle proprietà intermedie nei test,
  determinismo (E3).
- **Fase 3 — Ottimizzazioni avanzate**: ulteriori fusioni, strategie
  alternative di fan-out, IPC canonico opzionale, spill generalizzato,
  parallelismo aggiuntivo nei kernel table, ottimizzazioni CPU/cache, eventuale
  encoding GeoArrow nativo — solo dietro benchmark.
- **Fase 4 (separata)**: integrazione con **plenora-IO-tools** (già in
  costruzione come `plenora-datafile`): sorgenti del DAG lette via
  `LayerReader` invece che solo file Arrow IPC; più colonne geometriche attive
  (il modello è già pronto, D16); operazioni con side effect in un modello
  transazionale separato. Prerequisito: promozione del validatore WKB e della
  convenzione colonna geometria da `plenora-kernels-geo` a `plenora-core`,
  così IO-tools dipende solo da core.

## 9. ADR

Gli Architecture Decision Record sono scritti in `docs/adr/` e fissano i punti
più delicati (scrittura prevista in Fase 0, completata):

- **ADR 1 — Livelli di determinismo**: semantico vs IPC canonico; uguaglianza
  geometrica (tolleranza, normalizzazione topologica opzionale, ordine canonico
  dei componenti, `NaN`/`-0.0`, confronto geometrico ≠ confronto WKB);
  garanzie tra versioni e backend.
- **ADR 2 — Resource accounting e reservation protocol**: perimetro di
  `max_governed_memory_bytes`; memoria precisa/stimata; ownership contabile di buffer,
  slice e dictionary; regola anti-deadlock; **reservation adattive** per
  operatori con crescita imprevedibile (join, explode); **`MemoryLease` e
  fan-out** (condivisione reference-counted, regole di attribuzione);
  priorità tra rami; trigger spill; margine di sicurezza; metriche.
- **ADR 3 — Failure, cancellazione e crash**: first-error con diagnosi
  primaria; `cancellation_behavior` e **osservabilità dei `NonInterruptible`**
  (latenza attesa, reservation trattenute, attesa di cleanup nella v1); policy
  panic e limiti di `catch_unwind`; temp dir per `execution_id`, marker
  (PID + host + heartbeat + lock file), TTL dello scavenging; cleanup; publish
  solo a successo.
- **ADR 4 — Identità del `ValidatedGraph` e fingerprint del catalogo**: regola
  canonica basata sulle versioni per-componente; disciplina di incremento in
  review/CI; fingerprint dei contratti; **canonicalizzazione del piano** ai
  fini del `plan_hash` (ordine campi, normalizzazione numeri/default, alias →
  ID canonici, equivalenza config omessa/esplicita); serializzazione;
  invalidazione cache.
- **ADR 5 — Separazione `ValidatedGraph` / `ExecutionPlan`**: cosa è decisione
  semantica stabile e cosa decisione fisica; `RuntimeStatistic` e comportamento
  con statistiche assenti; proprietà `Estimated` nelle scelte fisiche.
- **ADR 6 — Semantica dei limiti**: `RowLimits` (per arco, input, output,
  totale, fattore di espansione), `PlanLimits`, interazione con
  explode/join/fan-out.
- **ADR 7 — Publish atomico per piattaforma**: matrice filesystem/piattaforma,
  same-filesystem, profili `AtomicPublish` / `DurableAtomicPublish` (fsync di
  file e directory, costo prestazionale, profilo di default), Windows vs Linux,
  remoto fuori scope.
- **ADR 8 — Segmenti fisici, fusione e hot path** *(da scrivere dopo i primi
  benchmark)*: criteri di fusione dei nodi, `SegmentMode`, zero-copy,
  decode/encode WKB per segmento, batch sizing, last consumer, mappa nodi
  logici ↔ segmenti fisici. Nel frattempo la guida provvisoria è ADR 5 +
  `Prestazioni.md`.

## 10. Invarianti

Criteri di accettazione verificabili in test:

1. Nessun piano non validato può essere eseguito.
2. Nessun nodo può produrre side effect esterni osservabili.
3. Nessun worker CPU attende una risorsa liberabile solo da un altro worker
   dello stesso pool.
4. Nessun nodo attende una reservation aggiuntiva mantenendo indefinitamente
   risorse che bloccano il progresso globale.
5. Ogni batch in memoria possiede una chiara ownership contabile.
6. Il fan-out è una proprietà logica del DAG, non una strategia fisica.
7. Gli errori di contratto precedono la lettura; gli errori di contenuto
   precedono il nodo successivo.
8. Nessun errore, cancellazione o panic intercettabile può pubblicare
   l'output finale.

A queste si aggiungono le **14 invarianti prestazionali** di `Prestazioni.md`
§6 (streaming a memoria quasi costante, zero-copy dove consentito, niente JSON
né ricerche per nome nel percorso dati, code sempre limitate, rilascio al last
consumer, un decode/encode WKB per segmento geo fuso, regressione oltre budget
= rilascio bloccato), con lo stesso status di criteri di accettazione.

## 11. Rischi e mitigazioni

| Rischio | Mitigazione |
|---|---|
| Conflitti id tra i due cataloghi "Manipola" | Tabella alias versionata e immutabile (D20) |
| Operazione senza inferenza registrata | `analyze_contract` obbligatoria: non registrabile in catalogo, piano non validabile |
| Proprietà stimate trattate come provate | `PropertyConfidence` + `PropertyScope`: precondizioni semantiche solo `Proven` (D25) |
| Collisioni di `FieldId` tra input o dopo join | Namespace globale del grafo, assegnazione/rimapping da planner (D16) |
| `ValidatedGraph` riusato su dati di scala diversa con strategia errata | `prepare`/`ExecutionPlan` per esecuzione, `RuntimeStatistic` con fallback conservativo (D15) |
| Deadlock tra reservation di rami concorrenti | Regola forte anti-deadlock + reservation adattive + test a quote basse (D18, I4, ADR 2) |
| Doppio conteggio/perdita di quota nelle code e al fan-out | `GovernedBatch` con `MemoryLease` reference-counted (D18, I5) |
| Starvation del pool (rami × kernel × attese) | Invariante I3 + prototipo pool (D13) |
| Memoria Arrow non contabilizzabile con precisione assoluta | Garanzia riformulata: precisa per allocazioni governate, stimata per condivise/esterne (ADR 2) |
| Crash non intercettabili (abort, GEOS, OOM, kill) | Temp dir per `execution_id` + scavenging all'avvio (D21) |
| Scavenging che cancella un'esecuzione viva ma lunga | Marker PID + host + heartbeat + lock file, TTL conservativo (ADR 3) |
| Publish "atomico" non tale su qualche filesystem | Matrice di supporto, profili `AtomicPublish`/`DurableAtomicPublish`, remoto fuori scope (D22, ADR 7) |
| Kernel monolitici non cancellabili | `cancellation_behavior`, latenza documentata e osservabile (D24) |
| Alias legacy che cambiano significato nel tempo | Tabella alias immutabile per `schema_version` (D20) |
| Piani legacy validi ma con config semanticamente diverse | Pipeline di migrazione esplicita e versionata + golden test (D20) |
| Falsi positivi/negativi nei test di determinismo geometrico | Uguaglianza geometrica definita in ADR 1, non confronto WKB |
| DoS in fase di validate con piano ostile | `PlanLimits` applicati durante il parsing (D19) |
| Fingerprint instabile o cieco a cambi semantici | Versioni esplicite per componente + disciplina CI (D17, ADR 4) |
| Regressione prestazioni geo dal trasloco | `benchmark_arrow_compare.py` come gate di Fase 1 |
| Side effect futuri incompatibili con atomicità | Invariante I2; side effect solo in modello transazionale futuro (D23) |
| Deriva delle invarianti di sicurezza | Invarianti come criteri di accettazione + fuzz unificati |

## 12. Decisioni registrate

Le decisioni sono raggruppate per fascia di stabilità; i numeri non vengono
mai riassegnati.

### 12.1 Decisioni fondamentali (modello e contratti — stabili, pubbliche)

- **D0 — Arrow `=59.1.0`, niente crate `geoarrow`.** Versione comune già
  adottata dai due progetti di origine, scelta e verificata al momento della
  decisione. Canone geometrie: `geoarrow.wkb` in colonna `Binary` + `geo.crs`.
  Porta di uscita verso l'encoding nativo documentata (§2).
- **D1 — DAG dichiarativo puro.** Nessuna annotazione di esecuzione nel piano;
  parallelismo derivato dal planner.
- **D2 — API a due fasi obbligatorie** (`validate` → `execute`), type-state:
  l'esecuzione richiede il `ValidatedGraph`.
- **D3 — Workspace multi-crate** (core / kernels-table / kernels-geo / engine
  / cli): GEOS/PROJ e rayon non si impongono a chi usa solo il tabellare.
- **D4 — Un solo output per piano** (v4). Output multipli rimandati.
- **D5 — Il DAG non prescrive la strategia di fan-out.** Un fan-out esprime
  esclusivamente dipendenze tra nodi; non implica una specifica modalità di
  buffering, rilettura o condivisione dei dati. Proprietà del modello.
- **D6 — `DataContract` invece di nudo `Schema`**, con API `analyze_contract`
  predisposta all'estensione (§4.3).
- **D7 — Il `ValidatedGraph` porta la propria identità** (hash piano,
  fingerprint catalogo/contratti, versione engine, capability): l'executor
  rifiuta grafi non più compatibili (§6.1).
- **D8 — Validazione statica (fase 1) e dinamica incrementale (fase 2) distinte
  e dichiarate** (§6.2).
- **D15 — Separazione `ValidatedGraph` (semantico, stabile) / `ExecutionPlan`
  (fisico, per-esecuzione, via `prepare` interno)**, con `RuntimeStatistic` e
  piano valido anche senza statistiche (§6.3, ADR 5).
- **D16 — Contratto geometrie estensibile, comportamento v1 chiuso**:
  `geometries: Vec` + `active_geometry` nel modello; la v1 rifiuta input e
  contratti con più di una colonna geometrica. `FieldId` in namespace globale
  del grafo: rinomina preserva, colonna derivata ne riceve uno nuovo, join
  eredita senza collisioni (§4.3).
- **D17 — Catalogo versionato per componenti** (`semantic_version`,
  `config_schema_version`, `contract_analysis_version`, `kernel_version`); il
  fingerprint deriva dalle versioni esplicite, mai da hash del binario (§4.3,
  §6.1, ADR 4).
- **D19 — Tre famiglie di limiti**: `RowLimits` semantici, `PlanLimits`
  applicati al parsing, limiti stringa (§3.1, §5, ADR 6).
- **D20 — Alias legacy versionati e immutabili + pipeline di migrazione
  esplicita** di piani e config, con golden test deterministici e idempotenti
  (§7).
- **D23 — Nodi privi di side effect esterni osservabili** (I2); operazioni con
  side effect solo in un futuro modello transazionale separato (§6.4).
- **D25 — `PropertyConfidence` + `PropertyScope` su ogni proprietà del
  `DataContract`**: solo `Proven` come precondizioni semantiche; `Estimated`
  solo per scelte fisiche correggibili; scope dichiarato (batch ≠ stream)
  (§4.3).
- **D26 — Prestazioni e memoria come criteri di accettazione**: hot path
  minimale (tutto ciò che è risolvibile in `validate`/`prepare` non si ripete
  in esecuzione), zero-copy dove la semantica lo consente, streaming reale,
  parallelismo solo se misurato conveniente, benchmark gate con budget di
  regressione rispetto alla baseline della Fase 1. Specifica completa in
  `Prestazioni.md` (§6.3, §8).

### 12.2 Decisioni dell'executor v1 (sostituibili senza cambiare il formato del piano)

- **D9 — Strategia fan-out dell'executor v1: materializzazione ai
  fan-out/fan-in.** Scelta conservativa dell'implementazione, sostituibile
  senza modificare il formato del piano (§6.3).
- **D11 — Resource accounting esplicito** con `ResourceGovernor`; preciso per
  allocazioni governate, stimato per condivise/esterne/native (§6.4).
- **D13 — Pool CPU globale unico + I/O separato + semaforo**, con invariante
  anti-starvation (I3) e prototipo obbligatorio (§6.4).
- **D18 — Un unico protocollo di esecuzione**: regola anti-deadlock sulle
  reservation (I4), reservation adattive per operatori a crescita
  imprevedibile, `GovernedBatch` con `MemoryLease` reference-counted per
  l'ownership nelle code e al fan-out (I5) (§6.4, ADR 2).
- **D21 — Crash oltre i panic**: temp dir isolate per `execution_id`, marker
  PID + host + heartbeat + lock file, scavenging dei temporanei orfani
  all'avvio con TTL conservativo (§6.4, ADR 3).
- **D22 — Publish atomico definito da matrice di supporto e due profili**
  (`AtomicPublish` / `DurableAtomicPublish`): filesystem locali,
  same-filesystem, Windows/Linux documentati, remoto fuori scope v1 (§6.3,
  ADR 7).
- **D24 — `cancellation_behavior` nel catalogo**: `Cooperative` /
  `BoundaryOnly` / `NonInterruptible`, con latenza documentata e osservabile e
  nessuna promessa di cancellazione immediata per i non interrompibili (§4.3,
  §6.4).

### 12.3 Decisioni trasversali specificate dagli ADR

Le scelte fini — regole esatte di fingerprinting e disciplina di versionamento
(ADR 4), perimetro e regole contabili della memoria (ADR 2), uguaglianza
geometrica e tolleranze (ADR 1), profilo di publish di default (ADR 7),
parametri dello scavenging (ADR 3), semantica piena dei `RowLimits` (ADR 6) —
sono vincolate dagli ADR, non duplicate qui.

- **D10 — Determinismo a due livelli**: semantico (sempre) e IPC canonico
  (opzionale, stessa versione engine). Dettagli in ADR 1 (§6.3).
- **D12 — `ExecutionContext` con handle condivisi**, cancellazione cooperativa,
  first-error wins con diagnosi completa dell'errore primario. Dettagli in
  ADR 3 (§6.4).
- **D14 — Policy panic**: `catch_unwind` al confine, cancellazione globale,
  errore interno senza dati sensibili, mai publish. Dettagli in ADR 3 (§6.4).
