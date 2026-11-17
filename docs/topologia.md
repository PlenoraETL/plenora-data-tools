# Operazioni topologiche in Rust puro

`geo.make_valid`, `geo.polygonize` e `geo.split` sono tornate nel catalogo
(146 operazioni, 75 geo) con lo stesso contratto pubblico di
`plenora-data-tools@190c493`: id canonici, parametri, schema di output,
colonna `__class` (`polygon`, `cut_edge`, `dangle`, `invalid_ring`),
`__parent_index` di `split`, nomi delle varianti d'errore e attribuzione del
passo (`InvalidPlan`, `Internal` per ciò che è interno; dal ciclo dei
difetti geo i limiti di lavoro e d'uscita superati sono `ResourceLimit`,
[«Operazioni geo»](runner.md#operazioni-geo)). Nel descrittore
cambiano solo i campi del backend: nessuna capability `geos`, maturità
`KernelValidated`, `kernel_version` 2.

| dove | che cosa |
| --- | --- |
| `crates/plenora-kernels-geo/src/rust_backend/{polygonize,split,make_valid}.rs` | i kernel del laboratorio, algoritmi invariati (modifiche elencate in `rust_backend/mod.rs`) |
| `crates/plenora-kernels-geo/src/rust_backend/mod.rs` | le firme di `geos_backend@190c493`: `make_valid_wkb`, `make_valid_geometry`, `polygonize_linework`, `split_polygon_by_linework` (le due riparazioni e lo split con in più la precisione) |
| `crates/plenora-kernels-geo/src/rust_backend/precision.rs` | la precisione dichiarata, 1 cm a terra nelle unità del CRS |
| `crates/plenora-kernels-geo/src/rust_backend/arrow.rs` | il trasporto Arrow di `190c493`: `make_valid_batches`, `polygonize_batches`, `split_batches` |
| `crates/plenora-kernels-geo/src/rust_backend/wkb.rs` | WKB dell'output con `POLYGON EMPTY` a zero anelli, come GEOS; è l'encoder di ogni cella geometria d'uscita (`arrow_adapter::encode_geometry`), non solo di questi kernel |

Provenienza: `plenora-memory-lab/operations/geo_rust`, sorgenti con gli
SHA-256 registrati in `results/geo-rust/fuzz-provenance.json`. Nessuna
dipendenza nuova: `geo`, `geozero`, `thiserror` erano già nel lock.

## Che cosa è provato qui

### Noding comune: migrazione DE-9IM, tranche 1b incompleta

`relate_matrix` e la variante con limiti usano ora
`noding.rs` attraverso `predicates/arrangement_indexed.rs`. Il motore
consuma eventi senza costruire il grafo dei segmenti: gli estremi e le
intersezioni esatte forniscono tutte le ascisse critiche. L'arrangement
esaustivo della 1a resta compilato nei test come oracolo differenziale.

La 6.1.0 non e' rilasciata: i limiti della 1a non sono una compatibilita'
storica. Il nuovo conteggio distingue visite bbox e test esatti dopo i
filtri inclusivi X/Y. I limiti sui nodi precedono la costruzione, includono
copie e incidenze duplicate prima di dedup. La deadline monotona viene
controllata accanto alla cancellazione e prima di pubblicare la matrice.

Le prenotazioni dei vettori seguono ora le capacita' vive, anche durante
l'iterazione che consuma il contenitore. Restano esclusi i buffer BigInt:
la prova algebrica per oggetto (6299 bit di numeratore e 5273 bit di
denominatore per una coordinata d'intersezione ridotta) e il corpus estremo
sono in `noding/bounds.rs`, ma non sono ancora ricevute di prenotazione
per tutti gli intermedi aritmetici. Non e' dichiarata conformita' GOP-027.
Anche la possibile sovraprenotazione interna dell'allocatore resta fuori
dal bilancio; una capacita' Vec diversa dalla richiesta e' un errore esplicito.

Mancano ancora i nuovi ingressi controllati `split_line` e `polygonize`,
e la loro qualifica C1. Per O-M4, `evaluate` ed `evaluate_validated`
conservano `geo::Relate` della 6.0.0: il test differenziale usa uno snapshot
del sorgente pubblicato, compresi errori e casi estremi. I sorgenti storici
di split_line e polygonize sono identici al tag locale `v6.0.0`.
I reticoli, pettini, strisce e il caso sparso da 20.002 segmenti provano
il noder comune; non sostituiscono prove sui nuovi ingressi mancanti.

### API diretta DE-9IM

`plenora_kernels_geo::predicates` espone la matrice generale esatta:

| API | Contratto |
| --- | --- |
| `relate_matrix(left, right)` | Matrice con limiti predefiniti dichiarati |
| `relate_matrix_with_limits(left, right, limits, cancelled)` | Limiti espliciti e flag `AtomicBool` opzionale |
| `De9imMatrix::cells()` | Nove byte `F012`, ordine `II IB IE BI BB BE EI EB EE` |
| `De9imMatrix::matches(pattern)` | Confronto con pattern gia' interamente validato |
| `De9imMatrix::evaluate(predicate)` | Predicato nominato senza ripetere il calcolo |
| `Display` di `De9imMatrix` | Nove caratteri senza separatori |
| `FromStr` di `De9imPattern` | Esattamente nove byte ASCII `TF*012`; errore senza testo ricevuto |

Coordinate XY planari, stesso CRS e ordine degli assi a carico del chiamante.
La decomposizione verticale usa razionali esatti, anche agli esponenti
estremi di `f64`: non sposta vertici o intersezioni e non richiede griglia.
Si visitano celle di dimensione zero, uno e due. I membri poligonali sono
uniti come insiemi: lati condivisi interni e sovrapposizioni non restano
confini. Le collezioni annidate vengono appiattite.

Per le collezioni miste si dichiara la seguente scelta dimensionale:
precedenza alla posizione rispetto all'unione delle aree; fuori dalle aree,
alle linee; fuori da entrambe, ai punti residui. Un punto o una linea sul
bordo di un'area non ne cambia la classificazione. Le componenti lineari
seguono Mod-2 degli estremi dei membri. Questa parita' dipende dalla
rappresentazione: [0,2] e [1,3] hanno estremi dispari anche in 1 e 2,
mentre la singola linea [0,3] no. Le linee chiuse hanno confine vuoto.

La fonte e' [OGC SFA 1.2.1, 06-103r4](https://docs.ogc.org/is/06-103r4/06-103r4.pdf):
6.1.3.1 per le collezioni, 6.1.6.1 e 6.1.8.1 per le curve, 6.1.11.1
per i poligoni, 6.1.15.2 per celle e pattern. Le prove C1 riportano
sezione e costruzione razionale delle attese: contatti, sovrapposizioni,
buchi riempiti, settori incidenti, incroci a 1/3 e coordinate estreme.
Il corpus analitico confronta anche gli undici predicati sulla nuova matrice,
in entrambi i percorsi validato e con gate del motore esatto. Il matcher rifiuta matrici
sbagliate e controlla anche simboli invalidi dopo celle non corrispondenti.

**Limiti:** default di 10 milioni di coppie effettivamente esaminate,
100 milioni di passi di visita/classificazione/ordinamento, 10 milioni di
nodi e 256 MiB di richieste di capacita' strutturale ancora vive. Il drop
rilascia il budget; pop e dedup mantengono la capacita'. Le richieste non
includono i buffer aritmetici interni di `BigInt` ne' l'overhead allocatore:
non costituiscono un tetto all'heap totale. `RelateLimits::deadline` e'
opzionale (default `None`) e produce `PredicateError::DeadlineExceeded`;
se anche il token e' alzato prevale `Cancelled`.
Nessun maggiorante anticipato decide il rifiuto. Validazione e calcolo
condividono i contatori; cancellazione, scadenza, lavoro esaurito e prenotazione
esaurita producono varianti tipizzate di `PredicateError`.
La decomposizione privilegia l'esattezza: nel caso peggiore il numero di
celle e di classificazioni cresce polinomialmente fino al limite esplicito.
La validazione rifiuta anche interni poligonali disconnessi; sovrapposizioni
fra membri di una GeometryCollection sono ammesse, quelle di MultiPolygon no.

I predicati di `De9imMatrix::evaluate` leggono il nuovo motore esatto.
Le API pubblicate `evaluate` ed `evaluate_validated` mantengono invece
il backend `geo::Relate` della 6.0.0, rispettivamente con e senza gate OGC.
Su collezioni unite e incroci estremi i risultati possono differire: la
nuova matrice non modifica implicitamente il comportamento pubblicato.
Questa API Rust non aggiunge adapter Arrow, catalogo, CLI o SDK.

### Prove dei kernel topologici

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
  `NumericRange`; una geometria da riparare di 20.000 km passa in
  `STRUCTURE` (overlay `i64`; con `i_overlay` 4.5 era
  `PrecisionInsufficient`) come una di 1.300 km, la stessa a `2^45` m è
  rifiutata dalla guardia di spaziatura, e `LINEWORK` le ripara con l'area
  di GEOS; i controesempi della
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

## Che cosa resta nel laboratorio

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

## Differenze da GEOS

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
- **Campo geometria dell'uscita.** `polygonize_batches` e `split_batches`
  non prendono più `output_crs`, e `make_valid_batches` non ripete più lo
  schema d'ingresso tale e quale: il campo geometria dell'uscita è quello
  dell'ingresso con tutti i suoi metadati (CRS, dimensioni, encoding,
  lineage), senza la dichiarazione dei tipi che l'operazione
  riscrive, e con la nullability del contratto. È lo schema che l'analisi
  dichiara, verificato dall'oracolo `analyze::tests::kernel_crosscheck`
  (nomi, tipi, nullability, metadati di campo e di schema); a `190c493` il
  campo nasceva nuovo e perdeva i metadati dell'ingresso.
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
