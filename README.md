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
| `plenora-kernels-geo` | kernel geografici su `geo::Geometry` e adapter GeoArrow-WKB; `rust_backend` per `geo.make_valid`, `geo.polygonize` e `geo.split` senza GEOS |
| `plenora-pipeline` | runner minimo: piano SSA di operazioni tabellari, validazione senza dati, esecuzione su tabelle intere con byte vivi contati per allocazione e budget di memoria per passo ([«Runner»](#runner)) |
| `plenora-io` | tabelle da e verso file: Arrow IPC (file e stream), Parquet, GeoParquet 1.1; scrittura atomica; un piano da file a file ([«File»](#file)) |
| `vendor/` | `geo`, `wkt`, `i_shape` con le patch di `patches/` (provenienza in `vendor/*/PROVENANCE*.md`) |

## Che cosa non c'è ancora

- **Operazioni geo nel runner**: [«Runner»](#runner) esegue solo le
  operazioni tabellari; le geo si chiamano ancora dai kernel.
- **`geo.reproject`**: richiedeva PROJ, è fuori dal catalogo.
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
precisione dichiarata"). Tre controlli lo misurano:

- **spaziatura delle coordinate** (all'ingresso di `polygonize`, quindi
  anche di `split` e dei passi di `make_valid` che lo usano, e prima di
  ogni overlay di `make_valid` `STRUCTURE`): se l'unità in ultima posizione del modulo
  massimo delle coordinate supera `p / 64` nessun punto calcolato potrebbe
  restare entro la precisione (a `2^52` un incrocio esatto `(B, B + 1.5)`
  torna a 27,7 cm), e il kernel non calcola. In metri con 1 cm il limite è
  un modulo di circa `2^39` m, fuori da ogni dominio di un CRS reale;
- **overlay di `make_valid` `STRUCTURE`** (`LINEWORK` non ne usa):
  `i_overlay` porta le coordinate su una
  griglia intera il cui passo `g`, letto dai sorgenti di `i_float` 1.16.0
  (`FloatPointAdapter::new`), è `2^(round(log2(h)) - 29)` con `h` la metà
  della dimensione maggiore del rettangolo d'ingombro degli operandi, cioè
  fra `2^-30.5` e `2^-29.5` di quella dimensione. In `make_valid` gli
  operandi sono normalizzati per asse su `[0, 1]^2`, dove il passo è
  esattamente `2^-30`: in coordinate originali `span * 2^-30` per asse, di
  diagonale `d`. Il bilancio di un vertice è l'arrotondamento alla griglia
  (al più `d / 2`) più l'aggancio al vertice d'ingresso **vicino** entro
  `d`, o al lato assiale d'ingresso che gli passa accanto (al più
  `sqrt(2) * d`): meno di `2 * d`. Se `2 * d` supera la precisione
  l'overlay non si esegue. In metri succede oltre circa 5.400 km di
  estensione su un solo asse, 3.800 km su entrambi. **Dopo** ogni overlay,
  qualunque cosa abbia fatto `i_overlay` dentro (i suoi agganci durante lo
  split dei segmenti, `split::snap_radius`, hanno un raggio di `2^(k/2)`
  passi al giro `k`, senza tetto a priori), ogni vertice dell'output deve
  stare entro la precisione da un lato d'ingresso dei due operandi
  (distanza punto-segmento con margine d'arrotondamento, lati in un
  `RTree`); altrimenti l'errore;
- **noding di `polygonize`** (anche dentro `make_valid` e `split`): il
  punto d'incrocio calcolato in doppia-doppia e arrotondato in `f64` deve
  stare entro un quinto della precisione da entrambi i segmenti che divide
  (il noding si ripete al più cinque volte, e gli spostamenti si sommano).
  Oltre, il grafo non si costruisce. Succede solo dove l'unità in ultima
  posizione delle coordinate si avvicina alla precisione (in metri, oltre
  circa `10^13` m).

**Ambito.** Tutte le operazioni geografiche. Il controllo della griglia e'
applicato oggi ai tre kernel portati (`geo.make_valid`, `geo.polygonize`,
`geo.split`; solo `make_valid` `STRUCTURE` usa l'overlay). Le altre operazioni
booleane passano dalla stessa griglia senza controllo, ed è il prossimo
passo: `topology.rs` (`boolean_operation`, `clip_to_mask`,
`polygon_overlay`, `dissolve`, `clean_valid_polygon_topology`),
`operations.rs` (`buffer_with_cap`, `Buffer` di `geo`), `extensions2.rs`
(`subdivide_polygon`), `extensions3.rs` (`coverage_validate_elements`).

Le verifiche a posteriori di `split` sono **locali**: la copertura del bordo
somma la lunghezza scoperta per anello sorgente (entro 1 cm), e l'area può
cambiare solo di 1 cm per la lunghezza dei lati di bordo con un estremo
calcolato dal noding. `make_valid` `LINEWORK` non ha verifiche a
posteriori di precisione perché non arrotonda nulla oltre il noding: ogni
passo dopo il noding è un'operazione esatta sull'insieme dei lati nodati
(vedi [«Differenze da GEOS»](#differenze-da-geos)).

**Feature d'ingresso più vicine della precisione.** La garanzia di 1 cm
vale per input le cui feature distinte (vertici, lati) distano almeno la
precisione l'una dall'altra, o coincidono esattamente. Feature distinte più
vicine di 1 cm (un vertice a pochi ULP da un lato, due lati quasi
coincidenti, coordinate che differiscono alla sedicesima cifra come `10` e
`10.000000000000002`) possono essere fuse o cambiare la topologia del
risultato, anche con aree diverse di molto più di perimetro per 1 cm, e
senza errore: su quegli input anche GEOS decide secondo il proprio
arrotondamento. Non è un caso da inseguire: dati così non hanno senso alla
precisione dichiarata. `make_valid` `LINEWORK` rifiuta
(`PrecisionInsufficient`) solo il caso che vede a costo trascurabile, un
incrocio non esatto del noding a meno di 1 cm da un altro vertice o da un
lato non incidente; due incroci arrotondati sullo stesso vertice o su un
vertice d'ingresso non sono riconosciuti.

**Hazard.** Una geometria più sottile di 1 cm (in tutto o in parte) può
uscire fusa o vuota senza errore, per scelta; feature distinte più vicine
di 1 cm possono cambiare la topologia (paragrafo precedente); sulle operazioni booleane non
ancora controllate una griglia più grossa di 1 cm (estensioni oltre circa
5.400 km in metri) non è rifiutata. Il controllo finale degli overlay di
`make_valid` `STRUCTURE` garantisce che **ogni vertice** dell'output stia entro 1 cm
dal linework degli operandi; non garantisce che stia vicino al lato
**giusto** (un vertice agganciato lungo un altro lato entro 1 cm passa), né
che un lato dell'output segua il linework fra i suoi due estremi, né la
topologia (quali facce sono piene): per questo restano la validazione OGC
dell'output e la campagna differenziale contro GEOS, non una prova. Lo split lineare
(`split_line`, sorgenti `LineString` di `geo.split`, codice precedente al
porting) ammette un punto di taglio entro la tolleranza più un margine
numerico proporzionale al modulo delle coordinate: l'adapter
(`rust_backend::arrow::split_batches`) applica prima lo stesso controllo di
spaziatura delle coordinate, così il margine resta sotto mezza precisione e
un punto a più di 1 cm dalla linea, con tolleranza nulla, non taglia.

**Condizione di rientro.** Nessuna per la precisione, che è una scelta di
prodotto; per il controllo della griglia, la sua estensione alle altre
operazioni booleane (con la spaziatura delle coordinate e il controllo
finale dei vertici); per i lati dell'output degli overlay, una verifica di
Hausdorff contro gli operandi, o un overlay con raggio d'aggancio costante
(`Precision::ABSOLUTE` di `i_overlay`, oggi non raggiungibile attraverso
`geo`).

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

### `geo.nearest`: lo scarto dell'R-tree si appoggia alla stima d'errore di `geo`

**Regola.** `nearest_matches` (e `_validated`) non confronta più ogni left
con ogni right: un R-tree dei rettangoli d'ingombro right sceglie i
candidati, e minimo, pari e distanze si calcolano sui soli candidati con la
stessa `Euclidean.distance` di prima, in ordine di indice right. Un right si
scarta solo se il suo rettangolo dista dal rettangolo left più di `d + S *
2^-40`, con `d` la distanza calcolata di un candidato e `S` il modulo
massimo delle coordinate: la distanza calcolata da `geo` 0.33.1 su geometrie
regolari non scende sotto quella vera di più di circa `64 * eps * S`
(differenze di coordinate, `hypot`, ramo di `line_segment_distance`,
tolleranza parametrica di `line_string_contains_point`, che dà zero a un
punto fuori dal rettangolo del segmento), quindi nessuno scartato è al
minimo. Le geometrie su cui la stima non vale non si scartano mai: parti
vuote o degeneri (linee di un punto, anelli aperti o sotto quattro vertici,
collezioni vuote, dove `geo` risponde zero o usa una tolleranza assoluta in
`f32`) e coordinate fuori da `{0} ∪ [2^-400, 2^400]` in modulo, NaN e
infiniti compresi: un right così è candidato di ogni riga, un left così li
prende tutti. Il limite `max_comparisons` resta `n * m` come prima: stesso
errore per gli stessi ingressi, anche se il lavoro vero è di solito molto
minore. L'oracolo, con la forza bruta copiata alla lettera e confrontata sui
bit, è in `crates/plenora-kernels-geo/src/analysis/nearest_oracolo.rs`.

**Ambito.** `plenora-kernels-geo`, `analysis::nearest_matches` e
`nearest_matches_validated`.

**Hazard.**

- la stima d'errore è letta dai sorgenti di `geo` 0.33.1 e `geo-types`
  0.7.19, non dimostrata in forma chiusa; il margine le lascia un fattore
  64 circa. Un aggiornamento che introducesse un'altra tolleranza assoluta o
  un altro «zero» per parti vuote renderebbe lo scarto sbagliato senza
  errore: l'oracolo lo vede solo sulle forme che esercita (fra queste il
  punto fuori dal rettangolo che `geo` dichiara sul segmento, che fallisce
  senza il margine);
- un panico di `geo` su una coppia (per esempio `nearest_neighbour_distance`
  con coordinate sotto `2^-400`) diventa `CalcoloNonConcluso` solo se la
  coppia è fra i candidati; quelle osservate coinvolgono sempre una
  geometria fuori dominio, che non si scarta, ma la forza bruta le valutava
  tutte;
- il caso peggiore resta O(n·m): con right equidistanti da molti left
  (una circonferenza, rettangoli grandi sovrapposti) i candidati sono tutti.

**Condizione di rientro.** Una distanza di `geo` con un limite d'errore
dichiarato, o un confronto esatto delle distanze: il margine si ricava da
lì e la stima letta dai sorgenti si toglie.

### Hash delle chiavi non keyed

**Regola.** Le mappe di chiavi dei kernel tabellari usano due hash
deterministici senza seme (`crates/plenora-kernels-table/src/hashing.rs`):
`KeyHasher` (`FastHasher`) per i valori nativi (interi, testi, valori e
chiavi composte dei join, partizioni delle finestre, blocchi di
`fuzzy_join`, chiave Int64 singola di `reconcile` e `assert_foreign_key`) e
`ChiaveHasher` per le chiavi binarie di riga (arena `KeyInterner` di
aggregate, distinct, set operation, assert_unique, table_diff, `reconcile`
e `assert_foreign_key`; mappe e scelta della partizione dello spill). L'uguaglianza delle chiavi si decide
sempre sui valori o sui byte: l'hash sceglie i candidati, mai il risultato,
e nessun output dipende dall'ordine di visita di una mappa (le mappe si
interrogano per chiave; dove si visitano, il risultato si ordina o si
riduce con operazioni commutative, e `fuzzy_join` sceglie il blocco peggiore
con uno spareggio sulla chiave).

**Ambito.** `plenora-kernels-table`: raggruppamenti, join, finestre,
set operation, qualità, spill, `fuzzy_join`.

**Hazard.**

- nessuno dei due è keyed: dati costruiti apposta per collidere degradano
  build e probe fino al quadratico entro i limiti di riga, che limitano `n`
  ma non il comportamento dentro `n`. Nessuna perdita di correttezza;
- entrambi ripiegano i bit alti su quelli bassi dopo ogni blocco. Senza, il
  passo di `KeyHasher` propagava le differenze solo verso i bit alti e su
  chiavi di più blocchi collideva anche senza avversario (un milione di
  codici `CUST-%08d` davano 960 000 hash, un milione di chiavi compatte
  Int64 32 768); i test di `hashing` e dei join fissano un milione di hash
  distinti su queste forme. Restano collisioni costruibili fra chiavi di
  lunghezza diversa (la lunghezza della coda entra nel digest, quella del
  testo intero no): costano tempo, non risultati.

**Condizione di rientro.** Un hasher con chiave per processo, verificato su
tutti gli usi.

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
distinta, fino a due `usize` per riga per l'assegnazione ai gruppi. Nel
runner il modello di costo ([«Budget di memoria»](#budget-di-memoria))
prevede queste strutture sul caso peggiore misurato (fixture `distinct`,
chiavi tutte distinte), senza contarle.

**Condizione di rientro.** Contabilità esplicita delle strutture di chiavi,
con errore `ResourceLimit` oltre il budget.

### `geo.make_valid`, `geo.polygonize`, `geo.split`: equivalenza a GEOS verificata, non dimostrata

**Regola.** Le tre operazioni girano sui kernel Rust del laboratorio
(`plenora_kernels_geo::rust_backend`), non su GEOS. L'equivalenza con GEOS
è semantica (stesse facce, stessi residui, stessa area) e poggia su una
campagna, non su una prova: 28.672 confronti differenziali e 861 casi curati
nel laboratorio, eseguiti su `geo` 0.33.1 **non patchato**. Qui `geo` ha
`orient2d` esatto: le prove indipendenti da GEOS sono rieseguite, quelle
differenziali no. Le differenze note sono in
[«Differenze da GEOS»](#differenze-da-geos).

**Ambito.** `geo.make_valid`, `geo.polygonize` e `geo.split` poligonale
(lo split lineare era già Rust puro).

**Hazard.**

- un input fuori dalle famiglie campionate può dare un risultato che GEOS
  darebbe diverso; area e copertura di `split` sono controllate a
  posteriori, le facce di `polygonize` e le riparazioni di `make_valid` solo
  dalla validazione OGC dell'output (in `LINEWORK` in più il bordo
  dell'area costruita deve tornare quello calcolato sui lati);
- `make_valid` `LINEWORK` ripete un polygonize per giro, e i giri sono al
  più i lati nodati: il caso peggiore (anelli concentrici collegati, che si
  sbucciano uno per giro) è quadratico nei lati per il logaritmo, entro il
  preflight di 10.000 segmenti ma senza un budget di tempo proprio, come in
  GEOS;
- dove il vecchio `orient2d` sbagliava il segno, o dove il laboratorio
  decideva da un'area in `f64` che ora è esatta, il comportamento
  verificato nel laboratorio e quello di qui possono divergere;
- le decisioni con tolleranza (elenco in `rust_backend/mod.rs`) seguono
  la precisione di 1 cm: sotto, il risultato può differire da GEOS; il
  passo di griglia è letto dai sorgenti di `i_float` nella versione del
  lock, e un suo aggiornamento va riletto;
- il corpus applicativo reale (gate del laboratorio) non è mai stato eseguito:
  mancano WKB reali anonimizzati;
- la validazione interna dei kernel usa `check_validation` di `geo`,
  quadratica, non la scansione della voce precedente.

**Condizione di rientro.** La campagna differenziale del laboratorio
rieseguita contro questo `geo` vendorizzato e il corpus applicativo reale
verde, con le divergenze spiegate.

## Operazioni topologiche in Rust puro

`geo.make_valid`, `geo.polygonize` e `geo.split` sono tornate nel catalogo
(145 operazioni, 74 geo) con lo stesso contratto pubblico di
`plenora-data-tools@190c493`: id, alias legacy, parametri, schema di output,
colonna `__class` (`polygon`, `cut_edge`, `dangle`, `invalid_ring`),
`__parent_index` di `split`, nomi delle varianti d'errore e attribuzione del
passo (`InvalidPlan`, `Internal` per ciò che è interno). Nel descrittore
cambiano solo i campi del backend: nessuna capability `geos`, maturità
`KernelValidated`, `kernel_version` 2.

| dove | che cosa |
| --- | --- |
| `crates/plenora-kernels-geo/src/rust_backend/{polygonize,split,make_valid}.rs` | i kernel del laboratorio, algoritmi invariati (modifiche elencate in `rust_backend/mod.rs`) |
| `crates/plenora-kernels-geo/src/rust_backend/mod.rs` | le firme di `geos_backend@190c493`: `make_valid_wkb`, `make_valid_geometry`, `polygonize_linework`, `split_polygon_by_linework` (le due riparazioni e lo split con in più la precisione) |
| `crates/plenora-kernels-geo/src/rust_backend/precision.rs` | la precisione dichiarata, 1 cm a terra nelle unità del CRS |
| `crates/plenora-kernels-geo/src/rust_backend/arrow.rs` | il trasporto Arrow di `190c493`: `make_valid_batches`, `polygonize_batches`, `split_batches` |
| `crates/plenora-kernels-geo/src/rust_backend/wkb.rs` | WKB dell'output con `POLYGON EMPTY` a zero anelli, come GEOS |

Provenienza: `plenora-memory-lab/operations/geo_rust`, sorgenti con gli
SHA-256 registrati in `results/geo-rust/fuzz-provenance.json`. Nessuna
dipendenza nuova: `geo`, `geozero`, `thiserror` erano già nel lock.

### Che cosa è provato qui

- i 18 test unitari dei tre kernel (uno con l'attesa aggiornata ai segni
  esatti, vedi sotto), fra cui il caso che perdeva una faccia di
  area 3,5 (`seed=2147483647`, indice 227): `tests/geo_rust_regressioni.rs` lo
  rigenera col generatore della campagna e lo confronta con l'esito GEOS
  registrato (8 poligoni, 0 cut edge, 3 dangle);
- la campagna di assurance indipendente da GEOS, 1.097 controlli in sei
  categorie (`tests/geo_rust_assurance.rs`);
- i test che a `190c493` coprivano le tre operazioni senza dipendere dal
  processo GEOS (`geos_backend`, parte GEOS di `geo_adversarial`, trasporto
  Arrow, analisi, catalogo), sulle stesse attese;
- segni esatti: il quadrato unitario in `(2^30, 2^30)` e varianti (offset
  grandi di segno diverso, aree minuscole, entrambi i versi, ogni vertice
  iniziale) dall'API e dall'adapter Arrow (`tests/geo_rust_segni_esatti.rs`),
  più i test d'unità di `rust_backend::exact`;
- la politica del centimetro (`tests/geo_rust_overlay_controllato.rs`, in
  metri): cornici di 10 m con margine da qualche centimetro in su escono con
  il buco e l'area entro perimetro per 1 cm, sotto il centimetro qualunque
  esito tranne un panico; la cornice valida con `L = 2^500` resta
  `NumericRange`; una geometria da riparare di 20.000 km e'
  `PrecisionInsufficient` in `STRUCTURE` (overlay), una di 1.300 km no,
  mentre `LINEWORK` le ripara entrambe con l'area di GEOS; i controesempi della
  revisione (buco largo `2^-40` m, cornice accanto a un triangolo di 1 km)
  riescono entro la precisione; la scala di `epsilon` del laboratorio
  riesce entro la precisione; le linee di un buco a `2^30` m, sopra la
  precisione, restano come in GEOS (la vecchia tolleranza le scartava);
- un buco a un ULP fuori dalla shell, che sulle coordinate normalizzate la
  tocca, promosso a poligono da `STRUCTURE`;
- l'ordine di anelli e parti (`tests/geo_rust_make_valid_ordine.rs`): i
  controesempi della sesta revisione (un buco che condivide un lato con la
  shell e ne sporge; parti sovrapposte di un `MultiPolygon`) in ogni ordine
  danno la stessa geometria, con area e linee dell'output GEOS 3.14 (112 e
  96: non l'area pari-dispari sugli anelli, 52 e 56); otto casi della
  classe B della campagna `p5` con l'output GEOS come attesa; una
  proprietà su input casuali (rettangoli, triangoli, bow-tie; fino a tre
  parti e quattro buchi) che permuta anelli e parti, e per `LINEWORK` ruota
  e inverte gli anelli, e vuole la stessa geometria con entrambi i metodi.
  Sul kernel precedente la proprietà e i controesempi falliscono;
- anelli e parti collassate (`tests/geo_rust_make_valid_anelli.rs`, attese
  dall'output GEOS 3.14): `STRUCTURE` su anelli che girano dentro se stessi
  (uno e due livelli, nei due versi), che si toccano in un vertice, a otto,
  a spirale, come shell e come buco; parti collassate unite come linee e
  punti; `LINEWORK` che rifiuta un incrocio arrotondato a `3e-16` da un
  vertice. Sul kernel precedente i tre test falliscono. In più
  un buco che tocca gli altri in due vertici, che il polygonize perdeva in
  silenzio (catene aperte scartate): ora resta buco, come in GEOS;
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
- **861 casi curati e di matrice** (138 `polygonize`, 227 `split`, 496
  `make_valid`) confrontati con GEOS: ordine e orientamento inversi, scale e
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
  d'ingresso. `make_valid` non ha queste eccezioni: `LINEWORK` è una
  funzione dell'insieme dei lati d'ingresso (identica su anelli e parti
  permutati, ruotati o invertiti), `STRUCTURE` scorre buchi e parti in un
  ordine canonico (identica su buchi e parti permutati).
- **Forma delle geometrie.** Punto iniziale e verso degli anelli (esterni
  antiorari in `polygonize`), e la scelta `Polygon`/`MultiPolygon`/
  `GeometryCollection` di una riparazione, possono differire da GEOS a parità
  di geometria.
- **`max_noding_work`** conta le coppie di segmenti esaminate durante il
  noding e la sua validazione, addebitate mentre accadono; GEOS stimava prima
  il quadrato dei segmenti. `make_valid` usa invece il quadrato dei segmenti
  (preflight del laboratorio).
- **Limiti che GEOS non aveva.** `make_valid` su input invalido oltre 10.000
  segmenti fallisce con `WorkLimit` (l'input valido passa invariato a ogni
  dimensione); i limiti di output di `polygonize` e `split` valgono anche
  sulle facce intermedie; in `split` il budget di parti e coordinate conta
  tutto l'output del polygonize interno, cioè anche le facce fuori dalla
  sorgente e i residui scartati (dangle e cut edge di una lama che sporge),
  non solo le parti tenute;
  in `split` il limite di coordinate vale per ciascun input e per la somma;
  un limite a `u64::MAX` è rifiutato (`UnboundedLimits`).
- **Precisione in più nelle firme.** `make_valid_wkb`,
  `make_valid_wkb_with_limits`, `make_valid_geometry`,
  `polygonize_linework`, `split_polygon_by_linework`, `make_valid_batches`,
  `polygonize_batches` e `split_batches` prendono la precisione dichiarata
  (1 cm a terra, `Precision::from_crs`): incompatibilità di firma con
  `190c493`, voluta. I kernel del laboratorio
  (`make_valid_geometry_rust*`, `split_polygon_by_linework_rust*`, e
  `PolygonizeOptions::precision`) la prendono in unità delle coordinate.
- **Errori nuovi.** Noding non convergente (`Unsupported`), segno o
  confronto d'area non decidibile su coordinate fuori dal dominio
  dell'aritmetica esatta, cioè con modulo fuori da `[2^-450, 2^450]`
  (`NumericRange`, `Unsupported`, anche da `make_valid`, che prima in
  quel caso avviava la riparazione di un poligono valido), bilancio di
  spostamento di un overlay di `make_valid` `STRUCTURE`, punto di noding
  arrotondato oltre 1 cm, o in `make_valid` `LINEWORK` a meno di 1 cm da
  un'altra feature (`PrecisionInsufficient`, `Unsupported`),
  precisione non
  finita o CRS senza precisione (`ResolvedCrs::precisione_coordinate`
  `None`: `InvalidPrecision`, `InvalidPlan`), memoria non prenotabile
  (`ResourceLimit`),
  panico di `geo`/`i_overlay` dentro il kernel (`Internal`, solo la forma
  del payload).
- **Segni esatti, anche dove GEOS non lo è.** Orientamento delle facce,
  annidamento per area, lato del punto nello split e area positiva del
  passthrough di `make_valid` sono decisi in modo esatto. Una faccia
  degenere solo per la precisione di GEOS resta un poligono: nel caso del
  laboratorio `iterated_noding_matches_geos_on_near_coincident_crossings` un
  triangolo di area circa 3,45e-31, che GEOS dà come anello invalido, esce
  come nono poligono (area totale e dangle invariati).
- **`make_valid` `LINEWORK` sui lati, senza overlay.** La regola è quella
  di `MakeValid` di GEOS (`MakeValidPoly`): nodare il bordo di tutto il
  poligono o multipoligono fondendo i lati ripetuti, poi a giri
  `BuildArea` sui lati rimasti, area := area XOR area nuova, lati := lati
  meno il bordo dell'area nuova; escono l'area, i lati rimasti e i vertici
  d'ingresso che il noding ha perso. Non è il pari-dispari su tutti gli
  anelli: un buco che condivide un lato con la shell e ne sporge, o parti
  di un multipoligono che si sovrappongono, danno aree diverse (e GEOS
  segue la sua regola, non il pari-dispari). Qui i giri sono operazioni
  esatte sull'insieme dei lati nodati: le aree costruite sono unioni di
  facce, il bordo di uno XOR è la somma modulo 2 dei bordi, e l'area finale
  è la regione il cui bordo è quella somma, colorata per adiacenza e
  riverificata. Dove GEOS arrotonda nei suoi overlay, qui non si arrotonda
  nulla dopo il noding. Il genitore di una faccia in `BuildArea` è la
  faccia che ha un buco uguale al suo guscio, senza l'ordine per area
  dell'inviluppo di GEOS (le due regole coincidono salvo inviluppi di area
  uguale); le linee escono come segmenti, non fuse fra nodi di grado 2.
- **`make_valid` `LINEWORK` e gli incroci arrotondati.** Dove un incrocio
  del noding non è esatto e cade a meno di 1 cm da un altro vertice o lato,
  la gerarchia delle facce (e con essa l'area di regioni intere) dipende da
  un ULP, e GEOS, che arrotonda a modo suo, può decidere diversamente
  (campagna degli anelli annidati: 52 m² da un incrocio a `3e-16` da un
  vertice). Qui è `PrecisionInsufficient`, dove GEOS risponde. Con incroci
  esatti (anche a `1e-15` da un'altra feature) non c'è rifiuto; sulle
  campagne i rifiuti in più sono i casi anisotropi alti micrometri e i
  ponti che passano per un vertice solo in aritmetica esatta.
- **Tipi d'ingresso di `make_valid_geometry`.** `Line` diventa
  `LineString`, `Rect` e `Triangle` diventano il `Polygon` di `to_polygon`,
  come faceva geozero a `190c493`.
- **Tolleranze dalla precisione.** Dove il laboratorio usava tolleranze
  fisse o proporzionali alla coordinata (`64 * EPSILON * |x|` e `1e-12` in
  `LINEWORK`, `1e-9` nei controlli di `split`), le decisioni seguono la
  precisione di 1 cm o sono esatte: `LINEWORK` non ha più tolleranze;
  lo split accetta area e copertura entro la precisione (prima
  rifiutava circa 600 casi traslati di `2^30` che GEOS risolve esattamente).
- **Validità.** «Valido» è la validazione OGC del workspace (quella di `geo`
  più il controllo degli anelli con punta), non `IsValid` di GEOS: dove le
  due divergono, `make_valid` può riparare un input che GEOS restituiva
  invariato, o viceversa. La scelta delle facce di `split` usa il campione
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
`Unsupported`); config tipizzate una volta; contratti di output passo per
passo con `analyze_table_contract` e i limiti con cui i kernel
eseguiranno, un solo `FieldAllocator`, provenance delle diagnostiche per
riga; colonne di ogni input e di ogni contratto contro `max_columns`.
`table.pivot` e `table.transpose` si rifiutano: il loro schema d'uscita
dipende dai dati.

**Ogni regola sulla config sta in un posto solo: l'analisi dei kernel.**
`analyze_table_contract(op, inputs, config, fields, limits)` riceve i limiti
del chiamante e rifiuta, per chiunque chiami i kernel e non solo per il
runner: nomi, testi, regex e conteggi oltre i limiti; nomi ripetuti e liste
vuote dove non hanno senso; chiavi di join, semi/anti, asof, `table_diff`,
FK e reconcile non leggibili come testo, o di lunghezza diversa fra i lati;
la riga intera di `distinct` senza `subset` con colonne che non sono testo;
gli operatori di `filter` e `conditional` sui tipi che il kernel non sa
valutare (testuali su colonne non testuali, ordinati fuori da
`scalar_compare_supported`, `==`/`!=` numerici con un valore non numerico);
i parametri che il kernel ignorerebbe (`date_format`, `precision`, `scale`,
`timezone` di `type_cast` su target che non li usano; `ascending` di
`add_row_number` e `dedup_advanced` senza `order_column`;
`inclusive_min`/`inclusive_max` di `assert_range` senza l'estremo;
`quantile`, `separator`, `distinct`, `skip_null` e `ddof` di `aggregate`
su funzioni che non li usano; `chars_start`, `chars_end` e `mask_char` di
`mask_data` fuori da `mask_type=custom`; `value` di `fill_na` con
`ffill`/`bfill`; `offset` di `window_function` fuori da `lag`/`lead`;
`ddof` di `rolling_window` fuori da `stddev`; `output_column` ed
`extract_all` di `string_extract` con gruppi con nome). Un parametro
assente prende il suo default; uno scritto e senza effetto si rifiuta,
nell'analisi e nel kernel, con la stessa funzione (`verifica_parametri`,
`verifica_offset`, `verifica_ascending`, `verifica_gruppi_con_nome`). Le
asserzioni vacue (`assert_not_null`, `assert_unique`, `assert_schema` senza
colonne, `assert_range` senza estremi, `assert_cardinality` senza vincoli,
`assert_metadata` senza chiavi, `conditional` senza condizioni, `sha256_hash`
e `stable_fingerprint` senza colonne); `melt` con variabile e valore
omonimi, `rename` con una sorgente ripetuta, `explode` con
`empty_policy=drop`; formati di data vuoti; `flatten_json` oltre
`max_columns`; `amount` di `date_add` che nessuna data sopporta, secondo
intercalare dell'ultimo giorno compreso (`dates::verifica_amount`); nomi
delle regole di `validate_rules` oltre 1024 byte; in `expression`, arietà
delle funzioni, pattern letterali di `regex_replace` e indici letterali
negativi di `substring`, questi ultimi solo dove la valutazione li
guarderebbe (nessun argomento che li precede, o la sostituzione, solo
null).

Il runner tiene un solo controllo proprio, perché non riguarda la config ma
l'ambiente del processo: la variabile `key_env` di `table.hmac_sha256`
deve esistere e non essere vuota.

### Esecuzione

Dopo ogni passo l'output del kernel deve avere nomi, tipi e metadati (di
campo e di schema) del contratto inferito (altrimenti `Internal`) e riceve lo schema del contratto; righe
per arco, colonne, nomi ripetuti e fattore di espansione si controllano
sui dati. Ogni tabella si libera appena ha girato il suo ultimo
consumatore; un'uscita che nessuno usa si libera subito, un input mai usato
prima del primo passo.

Il resoconto dà per passo operazione, righe in ingresso e in uscita,
variante del kernel, picco previsto, margine passato al kernel, tabelle
sfrattate e rilette, byte nuovi dell'output (allocazioni che nessuna
tabella residente raggiungeva prima del passo), byte vivi con l'output e
dopo i rilasci. `byte_vivi` (`plenora_core::memoria`) somma le allocazioni
Arrow delle tabelle residenti una volta ciascuna, per inizio
dell'allocazione e capacità, figli compresi: una slice, una rinomina o le
colonne di un batch letto da Arrow IPC non aggiungono nulla. È la stessa
misura con cui i kernel stimano i byte di un batch
(`spill::estimated_batch_bytes`).

### Budget di memoria

`limits.max_governed_memory_bytes` del piano è il budget del runner. Prima
di ogni passo deve valere

```text
byte_vivi(residenti) + riletture + picco_previsto(passo) <= budget
```

con `picco_previsto = S * (a + max(r * righe_in, c * byte_in, p * coppie))`
per operazione e variante, `S = 1.5`. I coefficienti vengono dalle misure
Windows (`PeakWorkingSet64`, profili wide, narrow, distinct avversario e
spilled, fino a 5 milioni di righe) in
`data/misure/catalogo-memoria-tabellare-v3.json`; li genera
`python scripts/genera_costi_operazioni.py` in
`crates/plenora-pipeline/src/costi_operazioni.rs`, con la formula
nell'intestazione e lo SHA-256 del catalogo (`--verifica` rigenera e
confronta; un test confronta l'impronta). Un'operazione senza modello si
rifiuta in validazione (`Unsupported`).

Quando il passo non sta, nell'ordine:

1. **sfratto** delle tabelle residenti che il passo non usa su file Arrow
   IPC temporanei, prima quella il cui prossimo uso è più lontano (Belady;
   a parità, per nome): il più corto prefisso di quell'ordine che fa stare
   il passo, poi si tengono in memoria quelle che non servono (anche quelle
   che non liberano nulla), e solo sulla scelta ridotta si verificano la
   quota su disco e il transitorio di ogni scrittura. Quel transitorio è
   calcolato prima di scrivere e sta nel budget insieme alle tabelle ancora
   residenti: Arrow IPC codifica ogni blocco in un vettore in memoria (fino
   al doppio del blocco) e i valori dei dizionari, che non si affettano con
   le righe, interi nel primo blocco; il limite superiore usa
   `get_slice_memory_size` di ogni blocco (per eccesso sui figli delle
   liste). La quota su disco si verifica sul limite superiore del file
   prima di codificare, poi sui byte veri. Il guadagno
   si misura come byte vivi che spariscono davvero (le allocazioni condivise
   con tabelle residenti restano), non come dimensione della tabella. Una
   tabella si rilegge prima del suo consumatore, e la rilettura (due volte i
   byte stimati: blocchi letti e tabella ricomposta) conta nel controllo di
   quel passo; gli output del piano sfrattati si rileggono alla fine, uno
   alla volta, nel budget. I file stanno sotto la quota `max_temp_bytes`;
2. **variante spilled** dove il kernel la ha (`sort`, `distinct`,
   `aggregate`, `union_distinct`, `intersect`, `except`), con il suo
   modello: lo spill riduce il transitorio, non l'output, che resta intero
   in memoria e che il modello comprende;
3. altrimenti **`ResourceLimit` prima di eseguire**, con il nome del passo
   e dell'operazione, senza valori dei dati.

Anche lo stato iniziale (gli input residenti) e quello finale (gli output
insieme) sono confini: oltre il budget sono un `ResourceLimit`, anche in un
piano senza passi.

Il kernel riceve come `max_governed_memory_bytes` il margine vero, budget
meno byte vivi, così i suoi preflight usano lo spazio che c'è. La
variante spilled tiene in memoria fino al proprio budget di batch riletti
da una partizione, e `concat_batches` li copia: la sua previsione aggiunge
al modello una riserva di due volte il margine richiesto (due partizioni
medie, `budget::riserva_spill`, al più 64 MiB), e il kernel riceve metà di
ciò che resta oltre il picco del modello, al più i 64 MiB con cui è stata
misurata. Come `max_temp_bytes`
riceve la quota che gli sfratti non occupano; lo sfratto stesso sceglie
solo tabelle la cui copia sta nella quota rimasta.

`byte_in` del modello è il maggiore fra i byte vivi degli input e il costo
di una loro copia (`plenora_core::memoria::byte_dati`: colonne che sono lo
stesso array contano ciascuna, perché i kernel le copiano ciascuna); la
stessa regola vale per `spill::estimated_batch_bytes`. Dopo il passo, byte
vivi con l'output oltre il budget sono un `ResourceLimit` esplicito.

Dalle misure: lo spill di `sort` costa quanto il sort in memoria (input e
output devono coesistere); il modello lo sceglie solo dove il termine per
byte della variante spilled è più basso, cioè su righe larghe, e mai sotto
circa 2,5 volte l'input. Le set operation spilled costano molto meno di
quelle in memoria (`intersect`: `c` da 5,9 a 1,0).

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
  copia delle tabelle esterne in memoria Rust all'ingresso (non fatta in
  F3: il budget conta le tabelle esterne come le vede `byte_vivi`).
- **Budget di memoria: byte Arrow più transitorio previsto, non RSS.**
  *Regola*: il budget garantisce che, a ogni confine di passo, i byte vivi
  delle tabelle residenti stiano sotto `max_governed_memory_bytes`, e che
  prima di ogni passo i byte vivi più il picco previsto dal modello ci
  stiano; l'output si controlla dopo con i byte esatti.
  *Ambito*: `PipelineValidata::run`, modello in
  `crates/plenora-pipeline/src/costi_operazioni.rs`.
  *Hazard*:
  - non è un tetto duro sulla memoria del processo: il transitorio dentro
    il kernel è una previsione empirica (misure Windows di
    `PeakWorkingSet64`, fattore 1,5), non una misura; un input fuori dalle
    fixture misurate (più righe di 5 milioni, 1000 per lato per
    `cross_join` e `fuzzy_join`, distribuzioni di chiavi o config diverse
    da quelle misurate: una `type_cast` di tutte le colonne invece di una)
    può superarla senza errore;
  - le varianti spilled sono misurate solo sui profili wide e distinct, non
    su narrow (righe strette, dove il costo per byte è più alto), e a
    `a59131f`, prima della correzione di `estimated_batch_bytes`: allora
    `read_partition` rifiutava partizioni circa dieci volte più piccole
    (`aggregate` spilled a 1 e 5 milioni di righe, rifiutato), quindi il
    working set delle partizioni misurato è più piccolo di quello di oggi.
    Il runner lo compensa con la riserva e passando al kernel spilled al
    più metà del margine oltre il picco del modello; con chiavi più
    sbilanciate di due partizioni medie il kernel rifiuta con
    `ResourceLimit`. I punti rifiutati non entrano nel modello;
  - `cross_join` e `fuzzy_join` hanno solo il termine per coppia,
    calibrato sulle larghezze di riga misurate: con righe più larghe il
    picco previsto è basso, e la difesa è il preflight dell'output del
    kernel con il margine passato;
  - per `date_extract` e `string_length` (solo profilo wide) la crescita per
    riga fra gli ultimi due campioni supera 1,5: il modello resta lineare
    con l'inviluppo sul campione più grande;
  - esclusi: overhead dell'allocatore e strutture Rust; i buffer di I/O
    dello spill dei kernel; la memoria esterna (FFI) contata
    come la vede `byte_vivi` (limite sopra);
  - per le operazioni che dipendono dai dati (join, `cross_join`,
    `fuzzy_join`, `pivot`, `transpose`, `explode`, `unnest`, `melt`,
    `aggregate`, `dedup_advanced`, finestre, `flatten_json`) il modello
    copre il transitorio del caso peggiore misurato; l'output lo limitano i
    preflight dei kernel con il margine passato e `max_rows`, e il
    controllo esatto dopo il passo, cioè dopo che è stato allocato;
  - uno sfratto libera memoria solo se nessun altro tiene l'allocazione:
    un clone tenuto dal chiamante la tiene viva, e il resoconto non lo vede.

  *Rientro*: contabilità esplicita delle strutture di chiavi nei kernel
  (limite «Memoria delle chiavi dei kernel in memoria non governata»),
  misure del profilo narrow per le varianti spilled e dei casi oltre il
  dominio misurato, e un allocatore contato per il processo (un tetto
  vero, non una previsione).
- **Solo operazioni tabellari**: le geo si rifiutano in validazione.
>>>>

- **Errori che dipendono dai valori delle celle, in esecuzione.**
  *Regola*: la validazione rifiuta ciò che config, schema e limiti rendono
  prevedibile; ciò che dipende dal valore di una cella fallisce, con un
  errore esplicito, quando il kernel la legge.
  *Ambito*: testo non numerico in una colonna Utf8 letta come numero
  (confronti ordinati, aggregazioni, statistiche, `assert_range`); valori
  che non si convertono nel tipo chiesto (`type_cast`, parse delle date);
  un `amount` di `date_add` che alcune date sopportano e quelle dei dati no;
  in `expression`, regex e indici di `substring` calcolati dalle colonne,
  divisori non letterali nulli; le asserzioni violate dai dati.
  *Hazard*: i passi a monte hanno già girato quando l'errore arriva.
  *Rientro*: nessuno previsto, è la natura del dato. L'oracolo
  `crates/plenora-pipeline/tests/oracolo_config.rs` esegue ogni config che
  l'analisi accetta (varianti di ogni operazione del catalogo) e ammette in
  esecuzione solo queste classi, elencate con il motivo; per le config che
  l'analisi rifiuta con una regola «il kernel fallirebbe», chiama il kernel
  direttamente e verifica che fallisca davvero (nessun rifiuto falso).
  Oltre i dati delle fixture non prova: il confine di `verifica_amount` sul
  secondo intercalare ha un test a parte.
- **Parametri ignorati: censimento per ispezione.**
  *Regola*: nessun parametro scritto si ignora; si rifiuta in analisi e nel
  kernel.
  *Ambito*: i parametri elencati in «Validazione», trovati leggendo i
  kernel tabellari; l'oracolo `oracolo_config.rs` li prova contro i kernel.
  *Hazard*: un parametro che il censimento non ha visto resterebbe ignorato
  senza errore. Restano fuori, di proposito, i parametri che hanno effetto
  ma non cambiano il risultato su certi dati (`distinct` con `min`/`max`).
  `fill_na` con `method=value` e senza `value` riempie con null, cioè non
  cambia niente: è accettato.
  *Rientro*: un parametro nuovo di un kernel entra con la sua regola in
  `verifica_parametri` e un caso nell'oracolo.
- **Chiave HMAC controllata in validazione, dal runner**: è ambiente, non
  config, quindi non sta nell'analisi dei kernel; la variabile d'ambiente
  può cambiare fra `validate` e `run`, e in quel caso l'errore arriva al
  passo.
- **Nome del passo negli errori**: aggiunto al messaggio conservando la
  categoria; gli errori con diagnostica per riga o già strutturati restano
  quelli del kernel.

## File

`plenora-io` legge e scrive le tabelle del runner. Ogni tabella è un solo
`RecordBatch` in memoria (decisione del maintainer: niente streaming, dati
tipici sotto i 10 milioni di righe).

```rust
let tabella = leggi_tabella(Path::new("ordini.parquet"), None, u64::MAX)?;
scrivi_tabella(&tabella, Path::new("ordini.arrow"), &OpzioniScrittura::default())?;
let report = esegui_da_file(&piano, &ingressi, &uscite, &OpzioniScrittura::default())?;
```

### Formati

| formato | estensioni | lettura | scrittura |
| --- | --- | --- | --- |
| Arrow IPC | `.arrow`, `.feather`, `.ipc`, `.arrows` | file (Feather v2) o stream, riconosciuti dal contenuto; tutti i blocchi ricomposti in uno | file, a blocchi di circa 8 MiB, senza compressione |
| Parquet | `.parquet` | un batch grande quanto il file, anche con più row group | proprietà fisse, `ZSTD` livello 3 (o `SNAPPY`, o nessuna) |
| GeoParquet 1.1 | `.parquet` | quando c'è il metadato di file `geo` | quando lo schema ha una colonna geometrica |

Il formato viene dall'estensione (senza distinzione di maiuscole) o da
`Formato` esplicito; un'estensione diversa è `Unsupported`, mai un formato
indovinato. Feather v1 si rifiuta.

**Arrow IPC** conserva schema, metadati di schema e di campo, tipi e bit
(NaN, `-0.0`) esattamente. **Parquet** anche, per i tipi provati in
`crates/plenora-io/tests/round_trip.rs` (interi con e senza segno, float con
NaN e `-0.0`, booleani, `Utf8`, `LargeUtf8`, `Utf8View`, binari, dizionari,
date, timestamp con fuso, ore, durate, intervalli anno-mese e giorno-ora,
decimali 32/64/128/256, liste, `LargeList`, `FixedSizeList`, strutture,
mappe, `Null`, tabelle vuote): lo schema Arrow incorporato (`ARROW:schema`)
deve coincidere campo per campo con quello che `parquet` applica, altrimenti
la lettura si rifiuta (`Schema`), perché `parquet` ricadrebbe in silenzio sul
tipo Parquet. Senza schema incorporato (file di scrittori non Arrow) vale il
tipo che `parquet` deduce.

**Scrittura deterministica.** `created_by` costante (`parquet-rs version
59.2.0`), pagine formato 1.0, row group di al più 1 048 576 righe,
statistiche di pagina, niente bloom filter, metadati JSON con chiavi in
ordine: la stessa tabella dà gli stessi byte, in Parquet e in Arrow IPC
(provato). Dopo ogni scrittura Parquet si rilegge il footer e se ne verifica
lo schema incorporato.

### GeoParquet

In lettura il metadato di file `geo` è l'autorità:

| chiave | lettura |
| --- | --- |
| `version` | `1.0.0` o `1.1.0`, altrimenti `Unsupported` |
| `primary_column` | deve essere fra le `columns` e la prima colonna geometrica dello schema |
| `encoding` | solo `WKB`; le codifiche GeoArrow native (`point`, …) sono `Unsupported` |
| `geometry_types` | tipi dichiarati (`exact`) e dimensionalità: tutti ` Z` → `xyz`, nessuno → `xy`, misti o elenco vuoto → `unknown`; ogni cella si verifica, un tipo non dichiarato è `DataMapping` |
| `crs` | assente → `OGC:CRS84`; `null` → CRS mancante; PROJJSON → identificatore integrato per `id` (sotto) |
| `edges` | assente o `planar`; `spherical` è `Unsupported` |
| `epoch` | `Unsupported` (il contratto non ha epoche) |
| `orientation`, `bbox`, `covering` | validati nella forma e lasciati cadere |
| altre chiavi di colonna | `Unsupported` |

Ogni colonna diventa il campo che il contratto accetta: `Binary`
(`LargeBinary` si converte), `ARROW:extension:name = geoarrow.wkb`,
metadato di campo `geo` con `crs`, `dimensions`, `encoding`; poi
`contract_from_arrow_schema` e `arrow_schema_from_contract` aggiungono il
blocco canonico `plenora.geometry.*` e `plenora.contract.version`, e
rifiutano chiavi canoniche già presenti in conflitto. Il metadato `geo` di
schema si toglie, `ARROW:extension:metadata` si sostituisce.

**CRS.** Un PROJJSON si riconduce alla tabella integrata solo per il suo
`id` (o un `ids` di un elemento): `EPSG` con codice numerico → `EPSG:<n>`,
`OGC`/`CRS84` → `OGC:CRS84`; il `type` del documento deve essere quello del
CRS integrato. Ogni altra cosa (senza `id`, altra autorità, codice fuori
tabella, `BoundCRS`, CRS 3D) è `CRS_NOT_BUILTIN`, mai un CRS indovinato. Le
coordinate WKB di GeoParquet sono sempre x = est/longitudine, come l'ordine
GIS normalizzato del contratto: `EPSG:4326` si legge `lon_lat`.

In scrittura, dal contratto: `version` `1.1.0`, `primary_column` la
geometria attiva, `encoding` `WKB`, `crs` il PROJJSON **completo** del CRS
integrato (`crates/plenora-io/data/projjson_integrati.json`, generato con
PROJ 9.5.1 da `scripts/genera_projjson_integrati.py` con la terna di
`genera_crs_integrati.py`; un test verifica che copra esattamente i CRS
integrati) o `null` per un CRS mancante, `geometry_types` i tipi dichiarati
dal contratto (verificati sui dati) o, senza dichiarazione, quelli trovati
nei dati, `bbox` il riquadro XY di tutte le coordinate quando le geometrie
sono 2D. Il `geo` di campo non entra nello schema incorporato: lo porta il
metadato di file. Si rifiutano: EWKB, coordinate M, curve, un ordine degli
assi dichiarato diverso da `lon_lat`/`easting_northing`, un CRS dichiarato
ma non risolto, geometrie di un tipo o di una dimensionalità diversi dal
contratto.

La camminata delle celle (`plenora_io::wkb`) è una validazione strutturale:
WKB ISO dei sette tipi, 2D o 3D, byte order per geometria, conteggi limitati
dai byte rimasti, figli coerenti con la multi-geometria, profondità 64,
nessun byte in eccesso, coordinate finite (il punto vuoto, tutto NaN, è
ammesso e resta fuori dal riquadro). Un proptest la confronta con la
codifica dei kernel e con le coordinate di `geo`.

Fixture: `crates/plenora-io/tests/dati/` contiene due file scritti da
pyarrow (Parquet C++) nella forma di GeoPandas, con il PROJJSON di PROJ
(`scripts/genera_fixture_geoparquet.py`).

### Scrittura atomica

Il contenuto va in un temporaneo `.plenora-io-*.tmp` nella directory della
destinazione, si porta su disco (`sync_all`), si verifica, e solo allora si
rinomina. Un errore prima della rinomina cancella il temporaneo: la
destinazione non vede mai un file parziale. Una destinazione esistente è
`Conflict` senza `OpzioniScrittura::sovrascrivi`, anche se compare durante la
scrittura (`persist_noclobber`); con la sovrascrittura la rinomina la
sostituisce.

### Un piano da file a file

`esegui_da_file` controlla prima di leggere che ogni output del piano abbia
un solo percorso, che i percorsi d'uscita siano distinti fra loro e dagli
ingressi (per testo e, per i file esistenti, per percorso canonico) e
scrivibili; poi carica gli input nell'ordine dato, esegue `validate` e `run`,
e scrive gli output nell'ordine del piano, liberando ognuno appena scritto.

### Memoria

Le tabelle caricate contano nel budget del piano
(`max_governed_memory_bytes`) come in `run`: ogni input si legge con il
budget residuo, e dopo la lettura i byte vivi esatti devono starci.

| passo | controllo prima | misura (Windows, allocazioni contate, 1 000–5 000 000 righe) |
| --- | --- | --- |
| lettura Arrow IPC | dimensione del file, il doppio se i blocchi sono più di uno (ricomposizione) | picco 1,0 volte la tabella con un blocco, 2,0 con più blocchi |
| lettura Parquet | 5 volte la stima dal footer, più 1 MiB | picco fino a 2,8 volte la tabella da 20 000 righe in su (liste, interi e float con null; 4 volte a 1 000 righe, per i buffer fissi), sempre sotto il 75% della previsione |
| scrittura Arrow IPC | due volte il blocco (≤ 8 MiB) e i dizionari, più 1 MiB | picco sotto 3 MiB |
| scrittura Parquet | 4 volte i byte del row group più grande, più 8 MiB | picco fino a 45 MiB (liste, 5 milioni di righe) |

La **stima dal footer** Parquet è il maggiore fra i byte non compressi dei
column chunk e i valori per la larghezza fisica di ogni foglia, più i byte
decodificati dei `BYTE_ARRAY` quando il file li dichiara
(`unencoded_byte_array_data_bytes`). Le misure si rifanno con un
allocatore che conta, fuori dal workspace (niente `unsafe` qui).

### Limiti dichiarati

- **Transitori di file previsti, non misurati.**
  *Regola*: prima di leggere un Parquet e prima di scrivere si verifica un
  picco previsto; dopo la lettura, i byte vivi esatti.
  *Ambito*: `parquet_io::leggi`, `ipc::leggi`, `esegui_da_file`.
  *Hazard*: la previsione Parquet viene dalle misure dei profili sopra; un
  file senza `unencoded_byte_array_data_bytes` con stringhe lunghe
  codificate a dizionario si decodifica in molti più byte di quanti il
  footer ne mostri, e la decodifica può superare il budget prima del
  controllo esatto, che allora fallisce con `ResourceLimit` dopo il picco.
  Come il budget del runner, non è un tetto duro sulla memoria del processo.
  *Rientro*: un allocatore contato per il processo.
- **Tipi Parquet rifiutati in scrittura**: `Interval(MonthDayNano)` e
  `RunEndEncoded` (errore di `parquet`, nessun file); un test ne tiene
  l'elenco. Una tabella senza colonne non si scrive in Parquet (il numero di
  righe andrebbe perso): si usa Arrow IPC.
- **Codec Parquet**: solo `UNCOMPRESSED`, `SNAPPY`, `ZSTD` sono compilati;
  `GZIP`, `BROTLI`, `LZ4`, `LZ4_RAW`, `LZO` si rifiutano prima di decodificare
  (`Unsupported`). Arrow IPC compresso (LZ4/ZSTD) si rifiuta con l'errore di
  Arrow.
- **GeoParquet, ciò che il contratto non porta**: `orientation`, `bbox` e
  `covering` letti si validano e si perdono; `epoch` e `edges: spherical` si
  rifiutano; il contratto ammette una sola colonna geometrica (D16), quindi
  un file con più colonne geometriche si rifiuta (`Schema`). Il PROJJSON
  letto si identifica per `id` e `type`: il resto del documento non si
  confronta con la tabella, e un documento che dichiara un `id` EPSG con
  parametri diversi da quelli del registro passa come quel codice.
- **Metadati geometrici normalizzati**: dopo GeoParquet il campo porta il
  `geo` di campo e il blocco canonico nella forma del contratto, non i byte
  di metadati che aveva prima della scrittura; il contratto si conserva
  (provato), i byte dei metadati no.
- **Più output, non una transazione**: ogni file è atomico, l'insieme degli
  output no; un errore sul secondo lascia scritto il primo. La directory non
  si sincronizza dopo la rinomina (su un crash del sistema la rinomina può
  non essere durevole).
- **Errori di `parquet` come codici**: il testo della dipendenza non entra
  nei messaggi (può contenere valori), solo la variante (`parquet error:
  general`, …), come per Arrow.

## Costruire e provare

Serve `rustup`: la toolchain (1.98.0) è fissata in `rust-toolchain.toml`.
L'unico codice nativo è libzstd, che `zstd-sys` compila con `cc` (niente
cmake): basta il compilatore C che il linker del target già richiede
(MSVC su Windows, `cc` su Linux).

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
