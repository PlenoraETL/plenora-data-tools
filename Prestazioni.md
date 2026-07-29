# plenora-data-tools — Vincoli prestazionali e di memoria

Documento compagno di `Architetture.md`. Definisce i vincoli prestazionali e di
memoria come **criteri di accettazione** (decisione D26): hanno lo stesso
peso delle invarianti di sicurezza.

## 1. Scopo

`plenora-data-tools` deve essere progettata e valutata prima di tutto come una libreria:

- veloce;
- a basso consumo di memoria;
- prevedibile sotto carico;
- efficiente su pipeline tabellari e geografiche;
- capace di mantenere un picco di memoria limitato anche su input molto grandi.

Correttezza, determinismo, sicurezza fail-closed, migrazioni, fingerprint e publish atomico restano requisiti fondamentali, ma non devono introdurre overhead non necessario nel percorso critico di esecuzione.

Principio guida:

> La correttezza viene progettata staticamente; la velocità si ottiene mantenendo il runtime minimale.

---

## 2. Vincoli fondamentali

### V1 — Arrow come rappresentazione unica

Arrow resta l'unica rappresentazione tabellare nel percorso dati.

Non sono ammesse:

- serializzazioni intermedie non necessarie;
- conversioni verso strutture row-oriented per operazioni tabellari;
- copie complete di batch quando la semantica consente il riuso dei buffer;
- ricostruzioni ripetute dello schema durante l'esecuzione.

Le operazioni devono usare buffer Arrow esistenti in modalità zero-copy ogni volta che sia tecnicamente e semanticamente corretto.

---

### V2 — Hot path minimale

Tutto ciò che può essere risolto in `validate` o `prepare` non deve essere ripetuto durante l'esecuzione.

Devono essere risolti prima dell'hot path:

- parsing JSON;
- migrazione dei piani;
- deserializzazione delle configurazioni;
- risoluzione di alias;
- verifica delle capability;
- risoluzione CRS;
- inferenza dei contratti;
- assegnazione dei `FieldId`;
- risoluzione degli indici di colonna;
- scelta della strategia di esecuzione;
- calcolo del last consumer;
- configurazione delle metriche;
- policy di determinismo.

Durante l'esecuzione non devono essere eseguite ricerche per nome quando è disponibile un indice già risolto.

Esempio da evitare:

```rust
batch.column_by_name("geometry")
```

Esempio richiesto:

```rust
geometry_column_index: usize
```

---

### V3 — Streaming reale

Una pipeline classificata come streaming deve avere memoria limitata rispetto alla dimensione totale dell'input.

Una catena streaming non deve:

- conservare batch precedenti;
- accumulare tutto l'output;
- creare code non limitate;
- trattenere riferimenti nei sistemi di metriche;
- materializzare risultati intermedi;
- rimandare la scrittura dell'output alla fine dell'intero input.

Criterio di accettazione:

> Aumentando la dimensione dell'input di 100 volte, il picco di memoria di una pipeline realmente streaming deve restare quasi costante, salvo la dimensione del batch e le strutture fisse dell'engine.

---

### V4 — Segmenti lineari eseguiti direttamente

Una pipeline lineare deve essere compilata in un segmento fisico diretto:

```text
read batch
→ kernel 1
→ kernel 2
→ kernel 3
→ write batch
```

Non deve passare attraverso:

- una coda per ogni nodo;
- uno scheduler per ogni operazione;
- un task separato per ogni kernel;
- una materializzazione tra operazioni streaming;
- nuove reservation per trasformazioni banali già comprese nel lease del batch.

Il planner deve poter scegliere una modalità seriale e cache-friendly quando il parallelismo non produce benefici misurabili.

---

### V5 — Nessuna copia non richiesta

Non è ammessa una copia di buffer Arrow se non richiesta dalla semantica dell'operazione.

Le operazioni di:

- projection;
- rename;
- slice;
- selezione di colonne;
- pass-through di colonne;
- propagazione dei metadati;

devono riusare i buffer esistenti quando possibile.

Filter, take, sort, join e trasformazioni che producono nuovi valori possono allocare nuovi buffer, ma devono evitare duplicazioni ulteriori.

---

### V6 — Decode/encode geografico minimizzato

Le pipeline geografiche non devono pagare ripetutamente il costo:

```text
WKB → geo::Geometry → WKB
```

per ogni nodo della stessa catena.

Una catena come:

```text
buffer → simplify → centroid → area
```

deve poter diventare un segmento fisico geo con:

- un decode WKB all'ingresso;
- geometrie mantenute in forma decodificata tra i kernel;
- un encode WKB solo quando richiesto dall'output del segmento.

Obiettivo:

> Al massimo un decode e un encode WKB per segmento geo fuso, salvo operazioni che richiedano esplicitamente una diversa rappresentazione.

Il design dell'adapter Arrow deve permettere questa ottimizzazione fin dalla Fase 2, anche se l'implementazione completa viene introdotta progressivamente (Fase 2C).

**Stato (2026-07-29, ADR-0012 M1):** attuata per catene di kernel unari
`TransformInPlace` capability-gated (14 op), forma decodificata transiente
sul batch, semantica errori invariata dimostrata dall'oracolo esteso
(`tests/geo_fusion_oracle.rs`). Misura A/B engine-level
(`examples/bench_geo_fusion.rs`, 200k righe miste, buffer→simplify→
centroid, mediana di 5): **−14,6%** (0,611s → 0,522s, bande min/max non
sovrapposte), output byte-identici, zero fallback governor. Il −45% del
baseline kernel-level si riduce a livello engine per l'overhead fisso
condiviso (framing RecordBatch, lease, metriche). Cantieri successivi:
misure terminali (M2), reproject/make_valid ed estensioni (M3).

---

### V7 — Batch sizing controllato

Il batch size deve essere configurabile e preferibilmente adattivo.

Non è sufficiente ragionare solo in numero di righe. Devono essere considerati anche i byte:

```rust
target_batch_bytes
max_batch_bytes
```

Batch troppo piccoli aumentano:

- overhead di chiamata;
- scheduling;
- atomiche;
- costo delle code;
- metadati;
- inefficienza SIMD e cache.

Batch troppo grandi aumentano:

- picco di memoria;
- latenza;
- durata delle reservation;
- ritardo nello spill;
- latenza di cancellazione.

Per i dati geografici il limite in byte è prioritario rispetto al solo numero di righe.

---

### V8 — Parallelismo solo se conveniente

Il parallelismo è una scelta fisica, non un requisito.

L'`ExecutionPlan` deve poter scegliere tra:

```text
SerialFused
ParallelPerBatch
ParallelPerBranch
BlockingSingleTask
```

Il parallelismo non deve essere attivato automaticamente quando il costo di scheduling, sincronizzazione e code supera il beneficio del lavoro concorrente.

Pipeline piccole, batch ridotti e kernel economici devono poter essere eseguiti serialmente.

---

### V9 — Materializzazione minima

Ogni materializzazione deve essere esplicitamente prevista dall'`ExecutionPlan`.

Non sono ammesse materializzazioni implicite.

La materializzazione ai fan-out/fan-in nella v1 deve:

- condividere batch immutabili;
- evitare copie dei buffer;
- usare `MemoryLease` condivisi;
- rilasciare la memoria dopo l'ultimo consumer;
- applicare backpressure;
- spillare in modo controllato quando necessario.

Il fan-out non deve moltiplicare il consumo di memoria solo perché esistono più consumer.

---

### V10 — Rilascio al last consumer

Il planner deve calcolare il last consumer di ogni arco e di ogni risorsa intermedia.

Dopo l'ultimo utilizzo devono essere rilasciati immediatamente:

- batch;
- `MemoryLease`;
- geometrie decodificate;
- indici spaziali;
- hash table;
- buffer temporanei;
- file di spill non più necessari;
- cache di segmento.

Il rilascio non deve essere rimandato alla fine del piano.

---

## 3. Vincoli sul consumo di memoria

### M1 — Budget principale in byte

Il `ResourceGovernor` deve operare principalmente su byte.

I limiti di righe proteggono da espansioni logiche, ma non rappresentano il consumo reale di memoria.

Devono essere contabilizzati o stimati:

- buffer Arrow;
- capacità allocata;
- batch in coda;
- dictionary condivisi;
- geometrie decodificate;
- hash table;
- indici spaziali;
- writer IPC;
- cache di segmento;
- memoria temporanea dei kernel;
- spill.

---

### M2 — Ownership contabile chiara

Ogni batch in memoria deve avere un'unica ownership contabile.

```rust
struct GovernedBatch {
    batch: RecordBatch,
    lease: Arc<MemoryLease>,
}
```

Il lease deve:

- essere condiviso al fan-out;
- restare valido fino all'ultimo consumer;
- evitare doppio conteggio;
- evitare rilascio anticipato;
- evitare perdita di quota in caso di cancellazione.

Il reference counting deve avvenire per batch o buffer, mai per riga.

---

### M3 — Overhead del governor limitato

Il `ResourceGovernor` non deve introdurre overhead significativo nel percorso per batch.

Non deve:

- percorrere ricorsivamente l'intero batch a ogni nodo;
- ricontare ripetutamente buffer già noti;
- usare lock globali per ogni riga;
- creare una nuova reservation per ogni operazione semplice;
- richiedere copie per rendere la memoria contabilizzabile.

L'overhead del governor deve essere misurato tramite benchmark dedicati.

---

### M4 — Spill selettivo

Lo spill deve essere introdotto prima per le operazioni blocking ad alto impatto:

- sort;
- hash aggregation;
- hash join;
- spatial join;
- distinct;
- set operations.

Non è necessario costruire fin dalla prima versione uno spill universale per qualunque nodo.

Lo spill deve:

- rispettare `max_temp_bytes`;
- usare file partizionati e cancellabili;
- essere attivato prima dell'esaurimento della quota;
- mantenere metriche sui byte scritti e letti;
- non bloccare il pool CPU durante I/O.

---

### M5 — Reservation adattive

Le operazioni con memoria stimabile devono acquisire la reservation completa prima di iniziare.

Le operazioni a crescita imprevedibile devono usare un protocollo chunked:

- reservation minima iniziale;
- crescita per chunk;
- spill o rilascio prima di attendere nuova quota;
- limite di espansione;
- fail-fast quando non esiste una strategia sicura;
- nessuna attesa indefinita mantenendo risorse che bloccano altri rami.

---

## 4. Vincoli sul percorso geografico

### G1 — WKB come confine, non come obbligo interno per ogni nodo

`geoarrow.wkb` resta il contratto esterno e degli archi osservabili.

All'interno di un segmento fisico geo è ammessa una rappresentazione decodificata temporanea.

L'engine deve evitare di considerare WKB come obbligo di serializzazione tra ogni kernel logico.

---

### G2 — Decode cache limitata al segmento

La cache di geometrie decodificate deve:

- avere lifetime limitato al segmento;
- essere contabilizzata dal governor;
- essere rilasciata al last consumer;
- non trasformarsi in cache globale;
- rispettare il batch sizing;
- poter essere disattivata se il benchmark mostra un peggioramento.

---

### G3 — Benchmark su geometrie reali

Le prestazioni devono essere misurate su:

- punti;
- linee;
- poligoni semplici;
- multipoligoni;
- geometrie molto grandi;
- WKB eterogeneo;
- geometrie con molti componenti;
- spatial join con espansione elevata.

Il numero di righe non è una misura sufficiente della complessità geografica.

---

## 5. Vincoli dell'ExecutionPlan

### E1 — Configurazioni preparate

Ogni kernel fisico deve ricevere una configurazione già:

- deserializzata;
- validata;
- tipizzata;
- normalizzata;
- risolta rispetto agli indici di colonna;
- verificata rispetto al CRS;
- verificata rispetto alle capability.

Nessun kernel deve ricevere JSON o mappe dinamiche nel loop di esecuzione.

---

### E2 — Modalità fisiche esplicite

Una possibile struttura:

```rust
struct PhysicalSegment {
    kernels: Box<[PreparedKernel]>,
    mode: SegmentMode,
    output_contract: Arc<DataContract>,
}

enum SegmentMode {
    LinearStreaming,
    GeoFused,
    Blocking,
    BinaryBlocking,
}
```

Ogni segmento deve conoscere:

- input e output;
- batch size;
- strategia di parallelismo;
- last consumer;
- budget;
- possibilità di spill;
- policy di cancellazione;
- metriche da raccogliere.

---

### E3 — Fusione senza perdita di osservabilità

Quando più nodi logici vengono fusi in un segmento fisico, devono restare disponibili:

- attribuzione degli errori al nodo logico corretto;
- metriche per nodo;
- limiti per arco;
- cancellazione;
- proprietà intermedie nei test;
- determinismo;
- tracciamento dei decode/encode WKB.

---

## 6. Invarianti prestazionali

Questi vincoli sono criteri di accettazione verificabili, complementari alle invarianti di sicurezza (§10 di `Architetture.md`).

1. Una pipeline streaming non cresce linearmente in memoria con la dimensione totale dell'input.
2. Nessun buffer Arrow viene copiato se la semantica consente il riuso.
3. Nessun parsing JSON avviene nel percorso dati.
4. Nessuna ricerca per nome avviene nel loop per batch quando è disponibile un indice risolto.
5. Nessuna coda non limitata è ammessa.
6. Nessuna materializzazione avviene fuori dall'`ExecutionPlan`.
7. Le risorse intermedie vengono rilasciate al last consumer.
8. Il fan-out condivide i buffer invece di copiarli.
9. Un segmento geo fuso esegue al massimo un decode e un encode WKB, salvo necessità semantiche.
10. Il governor non opera per riga.
11. Il parallelismo viene usato solo quando produce un miglioramento misurato.
12. Lo spill non usa worker CPU per attese I/O.
13. Ogni regressione significativa rispetto ai kernel originari blocca il rilascio.
14. Il picco di memoria deve essere misurato e riportato per ogni benchmark principale.

---

## 7. Metriche obbligatorie

Ogni benchmark deve raccogliere almeno:

```text
rows/s
MB/s
wall time
CPU time
peak RSS
bytes allocated
allocation count
bytes copied
peak governed memory
spill bytes written
spill bytes read
WKB decode count
WKB encode count
queue high-water mark
average batch bytes
max batch bytes
lease age
oldest live lease
```

Per i kernel geografici devono essere raccolte anche:

```text
geometries/s
coordinates/s
average WKB bytes
decoded geometry bytes
spatial index bytes
candidate pairs
matched pairs
expansion factor
```

---

## 8. Benchmark gate

### 8.1 Tabellare

Benchmark obbligatori:

- filter su 1, 10 e 100 milioni di righe;
- projection e rename zero-copy;
- select e slice;
- string transform;
- aggregate;
- hash join;
- sort in-memory;
- sort con spill;
- distinct;
- explode;
- pipeline lineare con più kernel.

---

### 8.2 Geografico

Benchmark obbligatori:

- decode WKB;
- encode WKB;
- buffer;
- centroid;
- simplify;
- area;
- intersects;
- catena geo con un kernel;
- catena geo con più kernel;
- spatial join;
- geometrie semplici;
- geometrie molto grandi;
- dati eterogenei.

---

### 8.3 Memoria

Benchmark obbligatori:

- pipeline streaming su input crescente;
- fan-out con due consumer;
- fan-out con consumer lento;
- join sotto budget stretto;
- spatial join con forte espansione;
- cancellazione con code piene;
- lease condivisi;
- cache geo decodificata;
- spill e rilettura;
- rilascio al last consumer.

---

## 9. Budget di regressione

Ogni rilascio deve confrontarsi con:

- i kernel originari dei due progetti;
- la release precedente;
- una baseline archiviata.

Devono essere definite soglie esplicite, per esempio:

- nessuna regressione superiore al 5% sul throughput dei kernel principali;
- nessuna regressione superiore al 10% sui kernel complessi senza motivazione approvata;
- nessun aumento del picco RSS superiore al 5% sulle pipeline streaming;
- nessuna copia aggiuntiva di buffer nei casi dichiarati zero-copy;
- nessun aumento del numero di decode/encode WKB nei segmenti geo fusi.

Le soglie definitive devono essere stabilite dopo la baseline della Fase 1.

---

## 10. Roadmap prestazionale

Allineata alle fasi di `Architetture.md` §8.

### Fase 1 — Baseline

- trasloco meccanico;
- benchmark dei due progetti originari;
- raccolta di throughput, allocazioni e picco memoria;
- dataset sintetici e reali;
- baseline archiviata in CI.

### Fase 2A — Executor minimo

- DAG validato;
- segmenti lineari;
- streaming reale;
- blocking essenziale;
- metriche;
- nessuna ottimizzazione non misurata.

### Fase 2B — Concorrenza e memoria

- fan-out;
- `MemoryLease`;
- code bounded;
- governor;
- reservation adattive;
- spill selettivo;
- spatial join;
- test con budget bassi.

### Fase 2C — Ottimizzazioni fondamentali

- fusione dei segmenti;
- cache di decode geo;
- batch sizing adattivo;
- rilascio al last consumer;
- riduzione delle allocazioni;
- scelta dinamica seriale/parallela.

### Fase 3 — Ottimizzazioni avanzate

- ulteriori fusioni;
- encoding GeoArrow nativo, solo dietro benchmark;
- strategie alternative di fan-out;
- IPC canonico opzionale;
- parallelismo aggiuntivo nei kernel tabellari;
- ottimizzazioni specifiche per CPU e cache.

---

## 11. Criterio di successo

La libreria è considerata conforme ai propri obiettivi solo se:

- mantiene throughput competitivo rispetto ai kernel originari;
- riduce o mantiene il picco di memoria;
- esegue pipeline streaming con memoria quasi costante;
- evita copie e serializzazioni intermedie non necessarie;
- limita decode/encode geografici;
- controlla espansioni, join e spill senza deadlock;
- mantiene l'overhead del runtime inferiore al beneficio dell'orchestrazione;
- dimostra i risultati tramite benchmark riproducibili.

Principio finale:

> `plenora-data-tools` non deve essere soltanto corretta e sicura: deve dimostrare, con benchmark e limiti verificabili, di essere veloce e memory-efficient.
