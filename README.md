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
| `vendor/` | `geo`, `wkt`, `i_shape` con le patch di `patches/` (provenienza in `vendor/*/PROVENANCE*.md`) |

## Che cosa non c'è ancora

- **Runner di pipeline**: nessun motore che concateni le trasformazioni; si
  chiamano i kernel direttamente.
- **`geo.reproject`**: richiedeva PROJ, è fuori dal catalogo.
- **Risoluzione CRS**: senza PROJ `resolve_crs` fallisce sempre chiuso; un
  CRS entra solo già risolto dal chiamante.

Engine, CLI, isolamento e protocollo del progetto d'origine non sono stati
portati. I riferimenti a `docs/…` nei commenti rimandano alla documentazione
di `plenora-data-tools`.

## Limiti dichiarati

### Validazione OGC: la ricerca delle auto-intersezioni non è quella di `geo`, il verdetto sì

**Regola.** Per `Polygon`, `MultiPolygon`, `GeometryCollection` e `Geometry`
la barriera `ValidazioneProtetta` non chiama `check_validation` di `geo`:
esegue `validazione_ogc::ValidazioneOgc::valida_ogc_rapida`, che rifà con le
API pubbliche di `geo` la stessa sequenza di `visit_validation` di `geo`
0.33.1 (stessi controlli, ordine, errori, stessa `relate`) e cambia **solo**
come si trovano le coppie di segmenti candidate: una scansione sui rettangoli
d'ingombro al posto del doppio ciclo O(n²). Il predicato per coppia è quello
di `geo`; gli anelli con coordinate non finite passano dal doppio ciclo.
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
- i controlli `relate` fra anelli e fra i poligoni di un `MultiPolygon` restano
  quadratici nel numero di anelli e di poligoni, come in `geo`.

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
  dalla validazione OGC dell'output;
- dove il vecchio `orient2d` sbagliava il segno, o dove il laboratorio
  decideva da un'area in `f64` che ora è esatta, il comportamento
  verificato nel laboratorio e quello di qui possono divergere;
- restano decisioni con tolleranza, non esatte (elenco in
  `rust_backend/mod.rs`): punto medio dei lati nell'assemblaggio dei buchi,
  tolleranze `1e-9` dei controlli a posteriori di `split` (una scheggia
  sotto quella soglia li passa, e rifiutano circa 600 casi traslati di
  `2^30` che GEOS risolve esattamente), lo snap per asse dell'overlay di
  `make_valid` (topologia protetta dalla precondizione, coordinate
  d'incrocio non esatte); la
  precondizione degli overlay di `make_valid` si regge sul comportamento di
  `i_float`/`i_overlay` letto dai sorgenti delle versioni nel lock (passo
  di griglia, raggio di aggancio che cresce solo dopo un passo che ha
  arrotondato): un aggiornamento di quei crate va riletto;
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
| `crates/plenora-kernels-geo/src/rust_backend/mod.rs` | le firme di `geos_backend@190c493`: `make_valid_wkb`, `make_valid_geometry`, `polygonize_linework`, `split_polygon_by_linework` |
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
- nessuna perdita silenziosa in `make_valid`: la cornice valida con
  `L = 2^500` è un errore esplicito da ogni ingresso del kernel, e una cornice
  con margine da `2^-10` a `2^-50` accanto a una geometria da riparare esce
  con la cornice di area esattamente uguale o con `PrecisionInsufficient`,
  mai con una parte in meno; i controesempi della revisione (buco largo
  `2^-40`, cornice accanto a un triangolo con estensione 1024) sono
  rifiutati prima dell'overlay (`tests/geo_rust_overlay_controllato.rs`);
  sulla campagna di assurance la precondizione valuta 268 overlay e non ne
  rifiuta nessuno;
- le linee dei buchi di \`LINEWORK\` a \`2^30\` (campagna differenziale traslata,
  seme 1 caso 92): prima scartate da una tolleranza, ora conservate come in
  GEOS; e un buco a un ULP fuori dalla shell, che sulle coordinate
  normalizzate la tocca, promosso a poligono da \`STRUCTURE\`;
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
  d'ingresso.
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
- **Errori nuovi.** Noding non convergente (`Unsupported`), segno o
  confronto d'area non decidibile su coordinate fuori dal dominio
  dell'aritmetica esatta, cioè con modulo fuori da `[2^-450, 2^450]`
  (`NumericRange`, `Unsupported`, anche da `make_valid`, che prima in
  quel caso avviava la riparazione di un poligono valido), overlay di
  `make_valid` con feature sotto 8 passi della griglia intera di
  `i_overlay` (`PrecisionInsufficient`, `Unsupported`, prima dell'overlay:
  una cornice con margine `2^-25` dell'estensione accanto a una geometria
  da riparare ora fallisce, prima da `2^-28` ne perdeva metà),
  memoria non prenotabile (`ResourceLimit`),
  panico di `geo`/`i_overlay` dentro il kernel (`Internal`, solo la forma
  del payload).
- **Segni esatti, anche dove GEOS non lo è.** Orientamento delle facce,
  annidamento per area, lato del punto nello split e area positiva del
  passthrough di `make_valid` sono decisi in modo esatto. Una faccia
  degenere solo per la precisione di GEOS resta un poligono: nel caso del
  laboratorio `iterated_noding_matches_geos_on_near_coincident_crossings` un
  triangolo di area circa 3,45e-31, che GEOS dà come anello invalido, esce
  come nono poligono (area totale e dangle invariati).
- **Tipi d'ingresso di `make_valid_geometry`.** `Line` diventa
  `LineString`, `Rect` e `Triangle` diventano il `Polygon` di `to_polygon`,
  come faceva geozero a `190c493`.
- **Validità.** «Valido» è la validazione OGC del workspace (quella di `geo`
  più il controllo degli anelli con punta), non `IsValid` di GEOS: dove le
  due divergono, `make_valid` può riparare un input che GEOS restituiva
  invariato, o viceversa. La scelta delle facce di `split` usa il campione
  interno e un test pari-dispari del laboratorio, con area e copertura del
  bordo verificate dopo, non `point_on_surface` + `covers` e la differenza
  simmetrica di GEOS.
- **Fuori perimetro come per le altre operazioni**: diagnostica per riga,
  envelope e fusione dei segmenti vivevano nell'engine, che non è portato.

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
tabella questa sezione supera la voce «Risoluzione CRS» di
[«Che cosa non c'è ancora»](#che-cosa-non-cè-ancora); per ogni altra
definizione quella voce resta vera.

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
predicati, `point_wkb` di `line_locate_point`, `reference_wkb` di `snap`, e
l'`extent` di `generate_grid` con il CRS del produttore. I kernel non
ricevono un CRS: sulle colonne il controllo lo chiama il chiamante, con
`plenora_kernels_geo::crs::validate_geometry_domain`, dopo la decodifica.

**Precisione.** `ResolvedCrs::precisione_coordinate()` esprime 1 cm a terra
nelle unità del CRS: `0.01 / horizontal_unit_to_metre` per i proiettati,
`0.01 / 111 319,49` gradi per i geografici.

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
