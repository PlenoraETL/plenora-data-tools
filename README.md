# plenora-data-tools2

Kernel tabellari e geografici su Arrow `RecordBatch`, in Rust puro: tabelle
in ingresso, una trasformazione, tabelle in uscita.

Deriva da `plenora-data-tools` al commit `190c493` (fase 1 di un successore
semplificato). I nomi dei crate sono rimasti quelli, così le correzioni del
progetto d'origine si portano qui senza rinomine.

## Che cosa c'è

| crate | contenuto |
| --- | --- |
| `plenora-core` | re-export Arrow, `PlenoraError`, limiti, catalogo delle operazioni, contratti dati, contratto CRS fail-closed, politica dei panici |
| `plenora-kernels-table` | kernel tabellari (filtri, ordinamenti, aggregazioni, join, espressioni, date, stringhe, qualità, spill) |
| `plenora-kernels-geo` | kernel geografici su `geo::Geometry` e adapter GeoArrow-WKB; `rust_backend` per `geo.polygonize` e `geo.split` senza GEOS |
| `plenora-pipeline` | runner minimo: piano SSA di operazioni tabellari, validazione senza dati, esecuzione su tabelle intere con byte vivi contati per allocazione ([«Runner»](#runner)) |
| `vendor/` | `geo`, `wkt`, `i_shape` con le patch di `patches/` (provenienza in `vendor/*/PROVENANCE*.md`) |

## Che cosa non c'è ancora

- **Operazioni geo nel runner**: [«Runner»](#runner) esegue solo le
  operazioni tabellari; le geo si chiamano ancora dai kernel.
- **`geo.reproject`**: richiedeva PROJ, è fuori dal catalogo.
- **`geo.make_valid`**: fuori dal catalogo, dall'analisi e dall'adapter
  Arrow (l'analisi la rifiuta come operazione sconosciuta, `Unsupported`).
  Il kernel Rust del laboratorio non è ancora qualificato: in `LINEWORK`
  l'unione della sporgenza di un buco che tocca la shell non commuta con lo
  XOR degli altri buchi, e l'area del risultato dipende dall'ordine dei
  buchi (e dei poligoni di un `MultiPolygon`) ben oltre 1 cm, senza errore:
  ogni vertice resta su un lato degli operandi e l'output è OGC-valido,
  quindi nessun controllo lo intercetta. Il lavoro continua sul branch
  `geo-rust-puro`; l'operazione torna quando è qualificata contro GEOS.
- **Risoluzione CRS fuori tabella**: senza PROJ `resolve_crs` risolve solo
  gli identificatori d'autorità della tabella integrata
  ([«CRS integrati»](#crs-integrati)); un codice fuori tabella fallisce
  chiuso con `CRS_NOT_BUILTIN`, una definizione WKT, WKT2, PROJJSON o
  proj-string con `CRS_BACKEND_UNAVAILABLE`, e un CRS così entra solo già
  risolto dal chiamante.

Engine, CLI, isolamento e protocollo del progetto d'origine non sono stati
portati. I riferimenti a `docs/…` nei commenti rimandano alla documentazione
di `plenora-data-tools`.

## Limiti dichiarati

### Precisione delle operazioni geografiche: 1 cm a terra

**Regola.** Ogni operazione geografica è garantita entro **1 cm a terra**,
precisione fissa, l'analogo del modello a precisione fissa di GEOS o di
`gridSize = 0.01` di PostGIS in metri. Nelle unità delle coordinate, con
una sola funzione, `plenora_core::crs::ResolvedCrs::precisione_coordinate`
(a cui delega `rust_backend::precision::Precision::from_crs` dei kernel):

- CRS proiettato: `0.01 / horizontal_unit_to_metre`;
- CRS geografico: 1 cm in gradi all'equatore, il valore più severo,
  `0.01 / 111_319.49`, circa `8.98e-8` gradi.

Sotto la precisione un risultato può differire dall'esatto e la differenza
è accettata: vertici spostati, schegge e parti sottili fuse o sparite, aree
diverse di circa perimetro per 1 cm. Sopra la precisione ogni errore e'
esplicito, mai silenzioso. Le funzioni dei kernel chiamate senza CRS
ricevono la precisione come argomento esplicito, senza valore predefinito.

Il solo rifiuto legato alla precisione è lo **spostamento che il calcolo
introdurrebbe**, confrontato con la precisione prima di costruire il
risultato: `PrecisionInsufficient` ("geometria troppo estesa per la
precisione dichiarata"). Due controlli lo misurano:

- **spaziatura delle coordinate** (all'ingresso di `polygonize`, quindi
  anche di `split` poligonale, e prima dello split lineare): se l'unità in
  ultima posizione del modulo massimo delle coordinate supera `p / 64`
  nessun punto calcolato potrebbe restare entro la precisione (a `2^52` un
  incrocio esatto `(B + 1.5, B + 1.5)` torna a 70,7 cm da uno dei segmenti),
  e il kernel non calcola. In metri con 1 cm il limite è un modulo di circa
  `2^39` m, fuori da ogni dominio di un CRS reale;
- **noding di `polygonize`** (anche dentro `split`): il punto d'incrocio
  calcolato in doppia-doppia e arrotondato in `f64` deve stare entro un
  quinto della precisione da entrambi i segmenti che divide (il noding si
  ripete al più cinque volte, e gli spostamenti si sommano). Oltre, il
  grafo non si costruisce. Con la spaziatura delle coordinate già
  controllata è un secondo livello di difesa.

**Ambito.** Tutte le operazioni geografiche. I controlli sono applicati
oggi ai due kernel portati (`geo.polygonize`, `geo.split`). Le operazioni
booleane passano dalla griglia intera di `i_overlay` senza controllo, ed è
il prossimo passo: `i_overlay` porta le coordinate su una griglia il cui
passo, letto dai sorgenti di `i_float` 1.16.0 (`FloatPointAdapter::new`),
è `2^(round(log2(h)) - 29)` con `h` la metà della dimensione maggiore del
rettangolo d'ingombro degli operandi, e aggancia gli incroci con un raggio
che cresce a ogni giro. Riguarda `topology.rs` (`boolean_operation`,
`clip_to_mask`, `polygon_overlay`, `dissolve`,
`clean_valid_polygon_topology`), `operations.rs` (`buffer_with_cap`,
`Buffer` di `geo`), `extensions2.rs` (`subdivide_polygon`),
`extensions3.rs` (`coverage_validate_elements`).

Le verifiche a posteriori di `split` sono **locali**: la copertura del bordo
somma la lunghezza scoperta per anello sorgente (entro 1 cm), e l'area può
cambiare solo di 1 cm per la lunghezza dei lati di bordo con un estremo
calcolato dal noding.

**Hazard.** Una geometria più sottile di 1 cm (in tutto o in parte) può
uscire fusa o vuota senza errore, per scelta; sulle operazioni booleane non
ancora controllate lo spostamento della griglia e degli agganci di
`i_overlay` non è confrontato con la precisione. Lo split lineare
(`split_line`, sorgenti `LineString` di `geo.split`, codice precedente al
porting) ammette un punto di taglio entro la tolleranza più un margine
numerico proporzionale al modulo delle coordinate: l'adapter
(`rust_backend::arrow::split_batches`) applica prima il controllo di
spaziatura delle coordinate, così il margine resta sotto mezza precisione e
un punto a più di 1 cm dalla linea, con tolleranza nulla, non taglia.

**Condizione di rientro.** Nessuna per la precisione, che è una scelta di
prodotto; per le operazioni booleane, un controllo della griglia e dello
spostamento dei vertici dopo ogni overlay (lo sviluppa `geo.make_valid` sul
suo branch di lavoro).

### Validazione OGC: la ricerca delle auto-intersezioni non è quella di `geo`, il verdetto sì

**Regola.** Per `Polygon`, `MultiPolygon`, `GeometryCollection` e `Geometry`
la barriera `ValidazioneProtetta` non chiama `check_validation` di `geo`:
esegue `validazione_ogc::ValidazioneOgc::valida_ogc_rapida`, che rifà con le
API pubbliche di `geo` la stessa sequenza di `visit_validation` di `geo`
0.33.1 (stessi controlli, ordine, errori, stessa `relate`) e cambia **solo**
come si trovano le coppie di segmenti candidate: una scansione sui rettangoli
d'ingombro al posto del doppio ciclo O(n²). Il predicato per coppia è quello
di `geo`; gli anelli con coordinate non finite passano dal doppio ciclo.
Allo stesso modo le coppie di poligoni di un `MultiPolygon` e di buchi di un
`Polygon` si trovano con una scansione sui rettangoli chiusi: `relate` non
si chiama sulle coppie per cui renderebbe il suo ramo disgiunto (stessa
condizione `Rect::intersects` di `geo`, che lascia vuote le celle
Interno-Interno e Confine-Confine da cui nascono gli errori), le altre si
visitano nell'ordine `(i, j)` del doppio ciclo, quindi gli errori emessi e il
loro ordine non cambiano; con coordinate non finite, doppio ciclo.
L'oracolo è in `crates/plenora-kernels-geo/src/validazione_ogc/tests.rs`.

**Ambito.** `plenora-kernels-geo`, ogni validazione OGC che passa da
`ValidazioneProtetta`.

**Hazard.**

- il caso peggiore resta O(n²): con molti segmenti lunghi a rettangoli
  sovrapposti le coppie candidate sono quadratiche come nel doppio ciclo (il
  verdetto non cambia, il tempo sì);
- la sequenza è copiata da `geo` 0.33.1: a ogni aggiornamento di `geo` va
  riallineata a mano, e l'oracolo rileva una divergenza solo sulle forme che
  esercita;
- la correttezza dello scarto delle coppie dipende dal segno esatto di
  `orient2d` nel kernel di `geo`: con un kernel non esatto il filtro potrebbe
  scartare una coppia che il doppio ciclo dichiara intersecante;
- lo scarto delle coppie di poligoni e di buchi dipende dal ramo disgiunto
  di `RelateOperation` di `geo` 0.33.1: se `relate` smettesse di
  rispondere senza calcolo sui rettangoli disgiunti, la condizione copiata
  andrebbe riverificata;
- con molti poligoni o buchi a rettangoli sovrapposti (oltre 32 coppie per
  elemento) i confronti fra rettangoli tornano quadratici, e `relate` resta
  una chiamata per coppia che si tocca.

**Condizione di rientro.** Una versione di `geo` con una ricerca delle
auto-intersezioni sub-quadratica a verdetto identico: la sequenza copiata si
toglie, la barriera torna a `check_validation` e l'oracolo resta come
regressione.

### Hash delle chiavi non keyed

**Regola.** Le mappe di chiavi dei kernel tabellari usano due hash
deterministici senza seme (`crates/plenora-kernels-table/src/hashing.rs`):
`KeyHasher` per i valori nativi (interi, testi, valori di join) e
`ChiaveHasher` per le chiavi binarie di riga (arena `KeyInterner` di
aggregate, distinct, set operation, assert_unique e table_diff; mappe e
scelta della partizione dello spill). L'uguaglianza delle chiavi si decide
sempre sui valori o sui byte: l'hash sceglie i candidati, mai il risultato.

**Ambito.** `plenora-kernels-table`: raggruppamenti, join, set operation,
qualità, spill.

**Hazard.**

- nessuno dei due è keyed: dati costruiti apposta per collidere degradano
  build e probe fino al quadratico entro i limiti di riga, che limitano `n`
  ma non il comportamento dentro `n`. Nessuna perdita di correttezza;
- `KeyHasher` ha un passo per blocco che propaga le differenze solo verso i
  bit alti: su chiavi di più blocchi in cui i byte che variano stanno in
  cima a un blocco e nel blocco di coda collide anche senza avversario (un
  milione di chiavi compatte Int64 davano 32 768 hash). Per questo le chiavi
  binarie di riga e lo spill usano `ChiaveHasher`; le mappe di valori nativi
  su più blocchi (testi, chiavi composte dei join) usano ancora `KeyHasher`,
  e lì il rischio residuo è di tempo.

**Condizione di rientro.** Un hasher con chiave per processo, verificato su
tutti gli usi (nessun output deve dipendere dall'ordine di una mappa), e
`KeyHasher` corretto o sostituito sulle chiavi di più blocchi.

### Memoria delle chiavi dei kernel in memoria non governata

**Regola.** `aggregate`, `distinct`/`dedup_advanced`, le set operation,
`assert_unique` e `table_diff` in memoria non contabilizzano le proprie
strutture di chiavi (arena, indici, gruppi) su `max_governed_memory_bytes`:
il budget decide solo il passaggio allo spill, sulla stima dei byte
dell'input. Nelle varianti spilled la contabilità dipende dall'operatore:

- set operation: le chiavi distinte di **ciascuna partizione** (lunghezza
  della chiave più 64 byte per chiave), quindi più partizioni riducono il
  picco;
- `distinct`: la mappa delle statistiche è **globale**, una
  voce per chiave distinta di tutto l'input (lunghezza più 64 byte), e più
  partizioni non la riducono;
- `aggregate`: i batch Arrow letti di una partizione, **non** le strutture
  di chiavi e gruppi costruite su di essi.

**Ambito.** I kernel elencati, percorso in memoria; nelle varianti spilled,
le strutture di chiavi e gruppi di `aggregate`.

**Hazard.** Con molte chiavi distinte il picco reale supera la stima
dell'input: l'arena delle chiavi, due `usize` e una voce di mappa per chiave
distinta, fino a due `usize` per riga per l'assegnazione ai gruppi.

**Condizione di rientro.** Contabilità esplicita delle strutture di chiavi,
con errore `ResourceLimit` oltre il budget.

### `geo.polygonize`, `geo.split`: equivalenza a GEOS verificata, non dimostrata

**Regola.** Le due operazioni girano sui kernel Rust del laboratorio
(`plenora_kernels_geo::rust_backend`), non su GEOS. L'equivalenza con GEOS
è semantica (stesse facce, stessi residui, stessa area) e poggia su una
campagna, non su una prova: 28.672 confronti differenziali e 861 casi curati
nel laboratorio, eseguiti su `geo` 0.33.1 **non patchato**. Qui `geo` ha
`orient2d` esatto: le prove indipendenti da GEOS sono rieseguite, quelle
differenziali no. Le differenze note sono in
[«Differenze da GEOS»](#differenze-da-geos).

**Ambito.** `geo.polygonize` e `geo.split` poligonale (lo split lineare
era già Rust puro).

**Hazard.**

- un input fuori dalle famiglie campionate può dare un risultato che GEOS
  darebbe diverso; area e copertura di `split` sono controllate a
  posteriori, le facce di `polygonize` solo dalla validazione OGC
  dell'output;
- dove il vecchio `orient2d` sbagliava il segno, o dove il laboratorio
  decideva da un'area in `f64` che ora è esatta, il comportamento
  verificato nel laboratorio e quello di qui possono divergere;
- le decisioni con tolleranza (elenco in `rust_backend/mod.rs`) seguono
  la precisione di 1 cm: sotto, il risultato può differire da GEOS;
- il corpus applicativo reale (gate del laboratorio) non è mai stato eseguito:
  mancano WKB reali anonimizzati;
- la validazione interna dei kernel usa `check_validation` di `geo`,
  quadratica, non la scansione della voce precedente.

**Condizione di rientro.** La campagna differenziale del laboratorio
rieseguita contro questo `geo` vendorizzato e il corpus applicativo reale
verde, con le divergenze spiegate.

## Operazioni topologiche in Rust puro

`geo.polygonize` e `geo.split` sono tornate nel catalogo (144 operazioni,
73 geo; `geo.make_valid` non ancora, vedi
[«Che cosa non c'è ancora»](#che-cosa-non-cè-ancora)) con lo stesso
contratto pubblico di
`plenora-data-tools@190c493`: id, alias legacy, parametri, schema di output,
colonna `__class` (`polygon`, `cut_edge`, `dangle`, `invalid_ring`),
`__parent_index` di `split`, nomi delle varianti d'errore e attribuzione del
passo (`InvalidPlan`, `Internal` per ciò che è interno). Nel descrittore
cambiano solo i campi del backend: nessuna capability `geos`, maturità
`KernelValidated`, `kernel_version` 2.

| dove | che cosa |
| --- | --- |
| `crates/plenora-kernels-geo/src/rust_backend/{polygonize,split}.rs` | i kernel del laboratorio, algoritmi invariati (modifiche elencate in `rust_backend/mod.rs`) |
| `crates/plenora-kernels-geo/src/rust_backend/mod.rs` | le firme di `geos_backend@190c493`: `polygonize_linework`, `split_polygon_by_linework` (con in più la precisione) |
| `crates/plenora-kernels-geo/src/rust_backend/precision.rs` | la precisione dichiarata, 1 cm a terra nelle unità del CRS |
| `crates/plenora-kernels-geo/src/rust_backend/arrow.rs` | il trasporto Arrow di `190c493`: `polygonize_batches`, `split_batches` |

Provenienza: `plenora-memory-lab/operations/geo_rust`, sorgenti con gli
SHA-256 registrati in `results/geo-rust/fuzz-provenance.json`. Nessuna
dipendenza nuova: `geo`, `geozero`, `thiserror` erano già nel lock.

### Che cosa è provato qui

- i test unitari dei due kernel (uno con l'attesa aggiornata ai segni
  esatti, vedi sotto), fra cui il caso che perdeva una faccia di
  area 3,5 (`seed=2147483647`, indice 227): `tests/geo_rust_regressioni.rs` lo
  rigenera col generatore della campagna e lo confronta con l'esito GEOS
  registrato (8 poligoni, 0 cut edge, 3 dangle);
- la campagna di assurance indipendente da GEOS, i 557 dei 1.097
  controlli in sei categorie che non riguardano `make_valid`
  (`tests/geo_rust_assurance.rs`);
- i test che a `190c493` coprivano le due operazioni senza dipendere dal
  processo GEOS (`geos_backend`, parte GEOS di `geo_adversarial`, trasporto
  Arrow, analisi, catalogo), sulle stesse attese;
- segni esatti: il quadrato unitario in `(2^30, 2^30)` e varianti (offset
  grandi di segno diverso, aree minuscole, entrambi i versi, ogni vertice
  iniziale) dall'API e dall'adapter Arrow (`tests/geo_rust_segni_esatti.rs`),
  più i test d'unità di `rust_backend::exact`;
- la politica del centimetro in metri (`tests/geo_rust_precisione_locale.rs`
  e i test di `rust_backend::split`): noding arrotondato oltre 1 cm e
  coordinate troppo rade rifiutati, lo stesso incrocio esatto a `2^30` m
  conservato; un buco omesso dall'output di `split` rifiutato dalla
  copertura per anello; uno split lineare a `2^48` m rifiutato invece di
  tagliare con un punto a 25 cm dalla linea;
- determinismo: ogni operazione due volte byte per byte; input permutato o
  invertito byte per byte dove il contratto promette la forma canonica, per
  classe e area dove non la promette.

### Che cosa resta nel laboratorio

Serve GEOS in esecuzione, quindi non gira qui. Vive in
`plenora-memory-lab/operations/geo_rust` (`*_reference`,
`polygonize_matrix_probe`, `wkb_stress_*`, script `.ps1`) e in
`results/geo-rust/`:

- **fuzz differenziale**: 16 semi × 256 casi per operazione, 12.288
  configurazioni, 28.672 confronti Rust/GEOS verdi (`fuzz-summary.csv`);
- **861 casi curati e di matrice** (138 `polygonize`, 227 `split`, e 496
  di `make_valid`, non esposta qui) confrontati con GEOS: ordine e
  orientamento inversi, scale e
  traslazioni, anisotropia, intersezioni quasi coincidenti, punti ripetuti.
  Le attese sono calcolate da GEOS a ogni esecuzione, non salvate come dati:
  portarle qui richiederebbe rigenerarle con GEOS;
- matrice `polygonize` a sei scenari e stress WKB fino a un milione di anelli,
  con tempi e RAM contro GEOS;
- il runner del corpus applicativo reale, mai eseguito per assenza di dati.

### Differenze da GEOS

- **Ordine dell'output.** `polygonize`: poligoni nell'ordine di estrazione
  delle facce, residui per classe nell'ordine del grafo; `split`: parti
  nell'ordine delle facce. Il contenuto è equivalente, l'ordine no.
- **Forma canonica.** Su input permutato o invertito l'output è identico
  quando le intersezioni sono esattamente rappresentabili; altrimenti coincide
  per classe e area, ma i bit di un'intersezione propria, il segno di uno
  zero e, senza noding, l'ordine delle linee duplicate seguono l'ordine
  d'ingresso.
- **Forma delle geometrie.** Punto iniziale e verso degli anelli (esterni
  antiorari in `polygonize`) possono differire da GEOS a parità di
  geometria.
- **`max_noding_work`** conta le coppie di segmenti esaminate durante il
  noding e la sua validazione, addebitate mentre accadono; GEOS stimava prima
  il quadrato dei segmenti.
- **Limiti che GEOS non aveva.** I limiti di output di `polygonize` e
  `split` valgono anche
  sulle facce intermedie; in `split` il budget di parti e coordinate conta
  tutto l'output del polygonize interno, cioè anche le facce fuori dalla
  sorgente e i residui scartati (dangle e cut edge di una lama che sporge),
  non solo le parti tenute;
  in `split` il limite di coordinate vale per ciascun input e per la somma;
  un limite a `u64::MAX` è rifiutato (`UnboundedLimits`).
- **Precisione in più nelle firme.** `polygonize_linework`,
  `split_polygon_by_linework`, `polygonize_batches` e `split_batches`
  prendono la precisione dichiarata (1 cm a terra, `Precision::from_crs`):
  incompatibilità di firma con `190c493`, voluta. I kernel del laboratorio
  (`split_polygon_by_linework_rust*` e `PolygonizeOptions::precision`) la
  prendono in unità delle coordinate.
- **Errori nuovi.** Noding non convergente (`Unsupported`), segno o
  confronto d'area non decidibile su coordinate fuori dal dominio
  dell'aritmetica esatta, cioè con modulo fuori da `[2^-450, 2^450]`
  (`NumericRange`, `Unsupported`), coordinate troppo rade per la precisione
  o punto di noding arrotondato oltre 1 cm (`PrecisionInsufficient`,
  `Unsupported`), precisione non
  finita o CRS senza precisione (`ResolvedCrs::precisione_coordinate`
  `None`: `InvalidPrecision`, `InvalidPlan`), memoria non prenotabile
  (`ResourceLimit`),
  panico di `geo`/`i_overlay` dentro il kernel (`Internal`, solo la forma
  del payload).
- **Segni esatti, anche dove GEOS non lo è.** Orientamento delle facce,
  annidamento per area e lato del punto nello split sono decisi in modo
  esatto. Una faccia
  degenere solo per la precisione di GEOS resta un poligono: nel caso del
  laboratorio `iterated_noding_matches_geos_on_near_coincident_crossings` un
  triangolo di area circa 3,45e-31, che GEOS dà come anello invalido, esce
  come nono poligono (area totale e dangle invariati).
- **Tolleranze dalla precisione.** Dove il laboratorio usava tolleranze
  fisse (`1e-9` nei controlli di `split`), le decisioni seguono la
  precisione di 1 cm: lo split accetta area e copertura entro la precisione
  (prima rifiutava circa 600 casi traslati di `2^30` che GEOS risolve
  esattamente).
- **Validità.** «Valido» è la validazione OGC del workspace (quella di `geo`
  più il controllo degli anelli con punta), non `IsValid` di GEOS. La
  scelta delle facce di `split` usa il campione
  interno e un test pari-dispari del laboratorio, con area e copertura del
  bordo verificate dopo, non `point_on_surface` + `covers` e la differenza
  simmetrica di GEOS.
- **Fuori perimetro come per le altre operazioni**: diagnostica per riga,
  envelope e fusione dei segmenti vivevano nell'engine, che non è portato.

## Runner

`plenora-pipeline` concatena le operazioni **tabellari** del catalogo su
tabelle intere in memoria: un `RecordBatch` per nome, niente streaming.

```rust
let pipeline = Pipeline::from_json(testo)?;              // o costruita in Rust
let validata = pipeline.validate(&[("ordini", schema)])?; // senza dati
let esito = validata.run(vec![("ordini".into(), tabella)])?;
// esito.outputs: Vec<(String, RecordBatch)>; esito.report: un ReportPasso per passo
```

### Il piano

In Rust il piano si costruisce con le strutture `Pipeline` e `Passo`; dal
testo si legge solo con `Pipeline::from_json`, che rifiuta campi sconosciuti
e chiavi ripetute a ogni profondità, config comprese. `Pipeline` non
implementa `Deserialize`: una lettura serde diretta terrebbe in silenzio
l'ultima di due chiavi ripetute.

```json
{
  "version": 1,
  "inputs": ["ordini", "clienti"],
  "limits": {"max_rows_per_edge": 1000000},
  "steps": [
    {"out": "validi", "op": "table.filter", "in": ["ordini"],
     "config": {"column": "importo", "operator": ">", "value": 0}},
    {"out": "uniti", "op": "table.join", "in": ["validi", "clienti"],
     "config": {"left_keys": ["cliente"], "right_keys": ["id"], "how": "inner"}},
    {"out": "totali", "op": "table.aggregate", "in": ["uniti"],
     "config": {"group_by": ["regione"],
                "aggregations": [{"column": "importo", "function": "sum"}]}}
  ],
  "outputs": ["totali"]
}
```

- `version`: solo `1`.
- `inputs`, `steps[].out`: nomi in forma SSA, ognuno definito una volta;
  un passo usa solo nomi definiti prima (`in`, nell'ordine dell'operazione:
  left, right); `outputs` nomina tabelle definite, senza ripetizioni.
- `op`: id canonico del catalogo; gli alias legacy si rifiutano.
- `config`: la config del kernel; assente vale `{}`.
- `crs` (facoltativo): CRS di piano per i produttori geo, risolto in
  validazione.
- `limits` (facoltativo): sostituisce uno per uno i default di
  `Limits::default()` (`max_input_rows`, `max_output_rows`,
  `max_rows_per_edge`, `max_expansion_factor`, `max_governed_memory_bytes`,
  `max_temp_bytes`, `spill_partitions`, `max_string_bytes`,
  `max_regex_bytes`), poi `Limits::validate`. Gli altri limiti non sono
  dichiarabili, perché il runner non li applica.

### Validazione

**Regola**: ciò che schemi, config e limiti rendono prevedibile fallisce in
`validate`, mai dopo che qualche passo ha girato.

Prima di qualunque esecuzione, contro gli schemi degli input: nomi SSA;
limiti di complessità del piano (`PlanLimits::default()`: passi, input,
archi, fan-out, profondità, byte di config per passo, lunghezza dei nomi,
byte del testo JSON); operazione, arietà e dispatch (`table.concat` a più
di due input, le operazioni geo e quelle senza dispatch sono
`Unsupported`); config tipizzate una volta; controlli statici delle config
contro i limiti; contratti di output passo per passo con
`analyze_table_contract`, un solo `FieldAllocator`, provenance delle
diagnostiche per riga; colonne di ogni input e di ogni contratto contro
`max_columns`; controlli che dipendono da schema e config insieme e che
l'analisi dei kernel non ha ancora (chiavi di join, semi/anti, asof,
`table_diff`, FK e reconcile leggibili come testo; riga intera di `distinct`
senza `subset`; operatori testuali di `filter` e `conditional`; `date_format`
di `type_cast` su target che non lo usano; `explode` con `empty_policy=drop`;
`stable_fingerprint` senza colonne; `flatten_json` oltre `max_columns`;
chiave HMAC presente nell'ambiente). Ogni regola sta in un posto solo: quando
l'analisi la acquisisce si toglie dal runner (formati delle date,
`order_column` ordinabile e `group_by` testuale sono già passati di là). `table.pivot` e `table.transpose` si rifiutano: il loro
schema d'uscita dipende dai dati.

### Esecuzione

Dopo ogni passo l'output del kernel deve avere nomi, tipi e metadati (di
campo e di schema) del contratto inferito (altrimenti `Internal`) e riceve lo schema del contratto; righe
per arco, colonne, nomi ripetuti e fattore di espansione si controllano
sui dati. Ogni tabella si libera appena ha girato il suo ultimo
consumatore; un'uscita che nessuno usa si libera subito, un input mai usato
prima del primo passo.

Il resoconto dà per passo operazione, righe in ingresso e in uscita, byte
nuovi dell'output (allocazioni che nessuna tabella residente raggiungeva
prima del passo) e byte vivi dopo il passo. `byte_vivi` somma le
allocazioni Arrow delle tabelle residenti una volta ciascuna, per inizio
dell'allocazione e capacità, figli compresi: una slice o una rinomina non
aggiungono nulla.

### Limiti dichiarati del runner

- **Tabelle intere in memoria**: nessuno streaming, nessun batch parziale.
- **`byte_vivi` esatto solo per la memoria allocata da Rust.**
  *Regola*: si conta ogni allocazione una volta, per inizio
  (`Buffer::data_ptr`) e capacità (`Buffer::capacity`); escluse le
  strutture Rust (`ArrayData`, `Arc`, schemi) e l'overhead dell'allocatore.
  *Ambito*: `plenora_pipeline::byte_vivi` e i byte del resoconto.
  *Hazard*: per la memoria esterna (FFI, `bytes::Bytes`) Arrow dichiara come
  capacità la vista importata, non l'allocazione: viste con inizi diversi
  della stessa allocazione esterna si sommano, la parte fuori dalle viste
  non si conta, a parità di inizio vale la maggiore. Il conto può essere
  sbagliato in entrambe le direzioni, senza errore. Le tabelle passano a
  `run` per valore: un clone tenuto dal chiamante tiene vive allocazioni
  che il resoconto non vede.
  *Rientro*: un'API di Arrow che distingua la deallocazione `Custom`, o la
  copia delle tabelle esterne in memoria Rust all'ingresso (decisione di
  F3, con il budget).
- **Budget non applicato**: `max_governed_memory_bytes` arriva ai kernel che
  lo usano, ma il runner non limita i byte vivi e non attiva lo spill
  (percorso presente, selezionato solo dai test).
- **Solo operazioni tabellari**: le geo si rifiutano in validazione.
- **Controlli statici delle config nel runner**: sono il porting di
  `validate_step_contract` di `190c493`, perché l'analisi dei contratti non
  li ripete e senza di essi alcune config passerebbero con un significato
  diverso da quello scritto. Il loro posto è l'analisi dei kernel; lì
  rientrano quando ci sono.
- **Fallimenti prevedibili non ancora anticipati**: la regola della
  validazione non copre ancora gli operatori ordinati di `filter` e
  `conditional` su tipi che `scalar_compare` non accetta, `==`/`!=` su
  colonne numeriche con valore non numerico in `conditional`, `amount` di
  `date_add` fuori scala, arietà, regex letterali e `substring` negativi di
  `expression`. Falliscono con un errore esplicito, ma durante
  l'esecuzione. Rientro: predicati di tipo pubblici nei kernel, o gli stessi
  controlli nell'analisi.
- **Chiave HMAC controllata in validazione**: la variabile d'ambiente può
  cambiare fra `validate` e `run`; in quel caso l'errore arriva al passo.
- **Nome del passo negli errori**: aggiunto al messaggio conservando la
  categoria; gli errori con diagnostica per riga o già strutturati restano
  quelli del kernel.

## Costruire e provare

Serve solo `rustup`: la toolchain (1.98.0) è fissata in
`rust-toolchain.toml`. Niente dipendenze native.

```sh
cargo build --workspace --locked
cargo test --workspace --locked
```

I gate completi prima di un commit sono in [`AGENTS.md`](AGENTS.md).

## CRS integrati

Senza PROJ, `plenora_core::crs::resolve_crs` risolve gli identificatori
d'autorità di una tabella integrata, generata dal registro EPSG. Descrive i
CRS (tipo, unità, assi, area d'uso, dominio di validità, ellissoide) e **non
riproietta**: nessuna coordinata cambia sistema. Per gli identificatori in
tabella la risoluzione è qui; per ogni altra definizione vale la voce
«Risoluzione CRS fuori tabella» di
[«Che cosa non c'è ancora»](#che-cosa-non-cè-ancora).

**Fonte.** Registro EPSG v11.022 (2024-11-05) come distribuito con PROJ
9.5.1, letto con pyproj 3.7.2 da `scripts/genera_crs_integrati.py`, che
scrive `crates/plenora-core/src/crs/epsg_integrati.rs` (generato, non si
modifica a mano). Le costanti `BUILTIN_EPSG_VERSION`, `BUILTIN_EPSG_DATE` e
`BUILTIN_PROJ_VERSION` riportano la fonte nel codice.

**Che cosa c'è** (169 CRS, tutti bidimensionali, meridiano di Greenwich,
gradi o metri):

| gruppo | codici |
| --- | --- |
| Italia | 4265 Monte Mario; 3003, 3004 Monte Mario / Italy zone 1 e 2 (Gauss-Boaga); 4670 IGM95; 3064, 3065 IGM95 / UTM 32N, 33N; 6706 RDN2008; 6707, 6708, 6709 RDN2008 / UTM 32N, 33N, 34N **(N-E)**; 7791, 7792, 7793 RDN2008 / UTM 32N, 33N, 34N (E-N); 6875 RDN2008 / Italy zone (N-E); 7794 RDN2008 / Italy zone (E-N); 4230 ED50; 23032, 23033, 23034 ED50 / UTM 32N, 33N, 34N |
| mondo | 4326 WGS 84; `OGC:CRS84`; 3857 Pseudo-Mercator; 3395 World Mercator; 4258 ETRS89; 3035 ETRS89-extended / LAEA Europe; 4269 NAD83; 4267 NAD27; 4283 GDA94; 7844 GDA2020; 4171 RGF93 v1; 2154 RGF93 v1 / Lambert-93; 4277 OSGB36; 27700 British National Grid; 4674 SIRGAS 2000; 4490 CGCS2000; 2056 CH1903+ / LV95; 31467 DHDN / 3-degree Gauss-Kruger zone 3; 28992 Amersfoort / RD New; 2193 NZTM2000 |
| fusi UTM | WGS 84 32601–32660 e 32701–32760; ETRS89 25828–25837 |

Restano fuori, di proposito: 25838 (ETRS89 / UTM zone 38N, deprecato nel
registro), i CRS 3D (per esempio 4979) e i CRS con meridiano fondamentale
diverso da Greenwich (per esempio 4806, Monte Mario (Rome)). Attenzione ai
nomi: nel registro 6707–6709 sono le varianti **northing-first** e
7791–7793 quelle easting-first.

**Forme accettate.** `EPSG:<codice>` (autorità senza distinzione di
maiuscole, codice senza zeri iniziali),
`urn:ogc:def:crs:EPSG:<versione>:<codice>` (versione vuota o numerica),
`OGC:CRS84` e `urn:ogc:def:crs:OGC:<versione>:CRS84`. Due forme dello stesso
codice sono semanticamente uguali (il canonical dipende dal CRS, non dal
testo); `EPSG:4326` (lat/lon) e `OGC:CRS84` (lon/lat) no. Un identificatore
d'autorità fuori tabella fallisce con `CRS_NOT_BUILTIN`; WKT, WKT2, PROJJSON
e proj-string con `CRS_BACKEND_UNAVAILABLE`.

**Area d'uso e dominio di validità.** Sono due cose diverse:

- l'**area d'uso EPSG** (riquadro lon/lat del registro e, per i proiettati,
  il suo inviluppo proiettato) è un metadato e non rifiuta dati. I dati reali
  la superano di norma: ISTAT pubblica i confini di tutta l'Italia in UTM
  32N, fino a circa 9,5° dal meridiano centrale, fuori dall'area 6°–12° E del
  fuso;
- il **dominio di validità** è il controllo: `validate_geometry_domain`
  rifiuta con `COORDINATE_OUT_OF_CRS_DOMAIN` una coordinata che ne esce, senza
  riportarne il valore. È un controllo contro un CRS sbagliato o coordinate
  prive di senso, non una garanzia di precisione. La regola è fissa per
  famiglia di proiezione:
  - geografici: longitudine −180…180, latitudine −90…90;
  - Transverse Mercator (UTM, Gauss-Boaga, GK, British National Grid, NZTM,
    RDN2008 Italy zone): inviluppo della regione a ±15° di longitudine dal
    meridiano centrale (l'algoritmo di Krüger/Karney usato da PROJ resta
    accurato a pochi nanometri entro 3900 km dal meridiano centrale).
    Latitudini: 0…84 per gli UTM nord e −80…0 per i sud, estese quando l'area
    EPSG le supera (32629 e 25832 arrivano a 84,01); per i TM nazionali l'area
    EPSG allargata del 50% dell'estensione per lato;
  - Mercator (3857, 3395): longitudine ±180, latitudine ±85,06;
  - altri reticoli (Lambert-93, LAEA Europe, LV95, RD New): inviluppo
    proiettato dell'area EPSG allargato del 50% dell'estensione per lato.

  Gli inviluppi li calcola il generatore con PROJ (solo la conversione dal CRS
  geografico di base, bordi campionati e raffinati), spostati di un
  micrometro e arrotondati al millimetro verso l'esterno: un limite esatto
  come l'equatore degli UTM (northing 0) diventa −0,001. Il dominio contiene
  sempre l'area d'uso.

**Dove gira il controllo.** Nell'analisi dei contratti geo, sulle geometrie
che arrivano dalla config con il CRS dell'input: `other_wkb` di distanze e
predicati e la lama `other_wkb` di `geo.split`, `point_wkb` di
`line_locate_point`, `reference_wkb` di `snap`, e
l'`extent` di `generate_grid` con il CRS del produttore. I kernel non
ricevono un CRS: sulle colonne il controllo lo chiama il chiamante, con
`plenora_kernels_geo::crs::validate_geometry_domain`, dopo la decodifica.

**Precisione.** `ResolvedCrs::precisione_coordinate()` esprime 1 cm a terra
nelle unità del CRS: `0.01 / horizontal_unit_to_metre` per i proiettati,
`0.01 / 111 319,49` gradi per i geografici; `None` quando il quoziente non
è un `f64` normale e positivo. È la precisione dichiarata delle operazioni
geografiche ([«Limiti dichiarati»](#precisione-delle-operazioni-geografiche-1-cm-a-terra)):
`Precision::from_crs` dei kernel geo delega a questa funzione, e un `None` è
`InvalidPrecision`.

### Limiti dei CRS integrati

**Regola.** Il dominio di validità di un proiettato è un rettangolo in
easting/northing, non la regione esatta: senza una proiezione inversa il
punto non si riporta in lon/lat.

**Ambito.** `plenora_core::crs::validate_geometry_domain` sui CRS proiettati
della tabella integrata.

**Hazard.**

- il rettangolo è più largo della regione che descrive: vicino ai poli un
  punto UTM può stare nel rettangolo e oltre i 15° dal meridiano centrale;
- gradi dati per errore a un fuso UTM nord passano: sono un punto vicino
  all'equatore dentro il fuso, e le sole coordinate non lo distinguono (fusi
  sud, Gauss-Boaga e reticoli nazionali invece li rifiutano);
- un fuso UTM copre un solo emisfero: dati che attraversano l'equatore nello
  stesso fuso (northing negativi in un fuso nord, pratica comune in Kenya e
  Uganda) sono rifiutati, e vanno scritti nel fuso dell'altro emisfero;
- i reticoli nazionali non TM accettano solo il 50% dell'estensione oltre
  l'area EPSG per lato: dati lontani dal territorio (per esempio la
  piattaforma continentale olandese in RD New, verso 55,7° N) sono rifiutati;
- il canonical è un sottoinsieme del PROJJSON (tipo, nome, sistema di
  coordinate, `id`): è stabile qui, ma non è confrontabile con un canonical
  prodotto da PROJ in `plenora-data-tools`;
- le coordinate si leggono nell'ordine GIS normalizzato; un dato scritto
  nell'ordine d'autorità di un CRS northing-first (6707, 3035) va
  normalizzato prima del controllo.

**Condizione di rientro.** Una proiezione inversa verificata (o un backend
PROJ) che riporti il punto in lon/lat e lo confronti con la regione esatta.

### Aggiungere un codice

1. aggiungerlo a una delle liste di `scripts/genera_crs_integrati.py`
   (`ITALIA`, `MONDO` o i fusi UTM);
2. preparare fuori dal workspace l'ambiente che ha prodotto la tabella:
   pyproj 3.7.2 come ruota binaria che include PROJ 9.5.1 e il registro EPSG
   v11.022. Quella usata è `cp311-cp311-win_amd64` (CPython 3.11, Windows
   x64):
   `python -m pip install --only-binary=:all: --target <dir> pyproj==3.7.2`.
   Altre ruote o una build contro un PROJ di sistema possono portare un altro
   PROJ (per esempio 9.8.1 con EPSG v12.029) e limiti diversi: il generatore
   controlla la terna pyproj/PROJ/EPSG e si rifiuta di girare se non coincide.
   Cambiare versione è una decisione da prendere in PR, con il diff dei dati;
3. rigenerare con `PYTHONPATH=<dir> python scripts/genera_crs_integrati.py` e
   formattare con `cargo fmt --all`;
4. aggiornare l'elenco atteso in
   `crates/plenora-core/src/crs/integrati/tests.rs` e rieseguire i test.

Il generatore rifiuta con un errore esplicito ciò che non sa descrivere: CRS
deprecati, non bidimensionali, con meridiano diverso da Greenwich, unità
diverse da gradi o metri, assi non nord/est, metodi di proiezione senza una
regola di dominio. Una nuova regola di dominio è una decisione da prendere in
PR, non un default.
