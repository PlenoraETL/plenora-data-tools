# plenora-data-tools2

Kernel tabellari e geografici su Arrow `RecordBatch`, in Rust puro: tabelle
in ingresso, una trasformazione, tabelle in uscita.

Deriva da `plenora-data-tools` al commit `190c493` (fase 1 di un successore
semplificato). I nomi dei crate sono rimasti quelli, così le correzioni del
progetto d'origine si portano qui senza rinomine.

## Che cosa c'è

| crate | contenuto |
| --- | --- |
| `plenora-core` | re-export Arrow, `PlenoraError`, limiti, catalogo delle operazioni, contratti dati, contratto CRS fail-closed e riproiezione fra i CRS integrati ([«Riproiezione»](#riproiezione)), politica dei panici |
| `plenora-kernels-table` | kernel tabellari (filtri, ordinamenti, aggregazioni, join, espressioni, date, stringhe, qualità, spill) |
| `plenora-kernels-geo` | kernel geografici su `geo::Geometry` e adapter GeoArrow-WKB; `rust_backend` per `geo.make_valid`, `geo.polygonize` e `geo.split` senza GEOS, e i controlli di precisione della griglia degli overlay (`rust_backend::griglia`); `riproiezione` per `geo.reproject` senza PROJ |
| `plenora-pipeline` | runner minimo: piano SSA di operazioni tabellari, validazione senza dati, esecuzione su tabelle intere con byte vivi contati per allocazione e budget di memoria per passo ([«Runner»](#runner)) |
| `plenora-io` | tabelle da e verso file: Arrow IPC (file e stream), Parquet, GeoParquet 1.1; scrittura atomica; un piano da file a file ([«File»](#file)) |
| `vendor/` | `geo` (con il porting a `i_overlay` 9.0.0) e `wkt` con le patch di `patches/` (provenienza in `vendor/*/PROVENANCE*.md`) |

## Che cosa non c'è ancora

- **Operazioni geo nel runner**: [«Runner»](#runner) esegue solo le
  operazioni tabellari; le geo si chiamano ancora dai kernel.
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

**La garanzia vale per ingressi le cui parti distinte distano almeno la
precisione** (sotto, «Feature d'ingresso più vicine della precisione»):
coordinate che differiscono solo nelle ultime cifre, parti o distanze fra
parti sotto 1 cm sono fuori ambito, fusioni e scomparse lì sono accettate e
nessun controllo le cerca.

Il solo rifiuto legato alla precisione è lo **spostamento che il calcolo
introduce**, confrontato con la precisione: `PrecisionInsufficient`
("geometria troppo estesa per la precisione dichiarata"; `Unsupported`
dove attraversa il confine con `PlenoraError`: `RustBackendError`,
`ExtensionError::del_passo`). Lo misurano:

- **spaziatura delle coordinate** (all'ingresso di `polygonize`, quindi
  anche di `split` poligonale e dei passi di `make_valid` che lo usano,
  prima dello split lineare e prima di ogni overlay di `i_overlay`): se l'unità in ultima posizione del modulo
  massimo delle coordinate supera `p / 64` nessun punto calcolato potrebbe
  restare entro la precisione (a `2^52` un incrocio esatto `(B + 1.5, B +
  1.5)` torna a 70,7 cm da uno dei segmenti), e il kernel non calcola. In
  metri con 1 cm il limite è un modulo di circa `2^39` m, fuori da ogni
  dominio di un CRS reale;
- **noding di `polygonize`** (anche dentro `make_valid` e `split`): il punto d'incrocio
  calcolato in doppia-doppia e arrotondato in `f64` deve stare entro un
  quinto della precisione da entrambi i segmenti che divide (il noding si
  ripete al più cinque volte, e gli spostamenti si sommano). Oltre, il
  grafo non si costruisce. Con la spaziatura delle coordinate già
  controllata è un secondo livello di difesa;
- **griglia di `i_overlay`, a priori** (`rust_backend::griglia`): le
  booleane e il buffer di `geo` (vendorizzato, `i_overlay` 9.0.0 con il
  motore intero `i64`) portano le coordinate su interi con passo `g =
  2^(ceil(log2(r)) - 61)`, `r` il raggio del rettangolo d'ingombro degli
  operandi di **quella** chiamata dal suo centro (letto da
  `FloatPointAdapter::with_iter_conservative` di `i_float` 5.0.0). Un giro
  sposta un punto di al più `(1 + 3 sqrt(2) / 2) g` (arrotondamento dei
  vertici e degli incroci, primo aggancio, pulizia del risultato) più `12
  ulp(M)` per gli arrotondamenti dei `f64`: oltre `p / 2` (l'altra metà
  resta agli agganci successivi al primo giro, sotto) l'overlay non si
  esegue. Con la guardia di spaziatura soddisfatta il limite non scatta:
  **nessun limite d'estensione**, in metri con 1 cm 20.000 km hanno `g = 2^-37` m (con
  `i_overlay` 4.5 e `i32` il passo era `2^(round(log2(h)) - 29)` e
  passavano estensioni fino a circa 2.950 km); resta solo il modulo delle
  coordinate, circa `2^39` m;
- **overlay in catena, a priori**: un'operazione che passa il risultato
  di un overlay a un altro (`clip_to_mask`: maschera dissolta e poi
  intersecata; resti di `polygon_overlay` e rimozione delle sovrapposizioni
  di `clean_topology`: unione dei vicini e poi differenza; `subdivide`:
  fino a 32 livelli di tagli; il buffer: `Buffer` di `geo` e unione delle
  parti) divide `p / 2` fra i passi (`griglia::controlla_overlay_in_catena`):
  gli spostamenti sommati restano entro `p / 2`.

**Nessun controllo a posteriori.** Il risultato di un overlay o di un
buffer non è confrontato con gli ingressi dopo il calcolo (fino al
porting a `i_overlay` 9 tre controlli contro gli ingressi originali, e
per il buffer contro la definizione esatta, costavano il 90-99% di ogni
booleana e rifiutavano risultati corretti su dati ordinari: vedi
«Hazard»). La garanzia di 1 cm poggia sui limiti a priori sopra e sulla
correttezza di `i_overlay`; resta la validazione OGC dell'uscita.

**`make_valid` `STRUCTURE`** ha i propri overlay (`LINEWORK` non ne usa):
gli operandi sono normalizzati per asse su `[0, 1]^2`, dove il passo della
griglia `i64` è `2^-62` e lo spostamento lo fanno gli arrotondamenti dei
`f64`: al più `span * 2^-49 + 4 ulp(M)` per asse in coordinate originali,
di diagonale `d`; il bilancio di un vertice è il ritorno (al più `d`) più
l'aggancio al vertice d'ingresso vicino entro `d`, o al lato assiale che
gli passa accanto (al più `sqrt(2) * d`): se `(1 + sqrt(2)) d` supera la
precisione l'overlay non si esegue (in metri con 1 cm mai prima della
guardia di spaziatura; con `i_overlay` 4.5, passo `span * 2^-30`, oltre
circa 5.400 km di estensione su un solo asse). Gli overlay di una
riparazione sono in catena (unioni dei buchi e delle parti, differenza):
lo spostamento di ogni overlay eseguito si sottrae dalla precisione, e
l'overlay che la esaurirebbe non si esegue (un'unione di parti disgiunte
non passa da `i_overlay` e non conta).

**Buffer** (`rust_backend::buffer`). Il `Buffer` di `geo` con gli archi di
default approssimava con un passo di 0,2 rad (il cerchio di 10 m di un
punto aveva una freccia di 4,8 cm) e saltava senza errore una componente
che la griglia riduce a un punto anche quando il suo buffer era grande.
Ora:

- **archi dalla precisione**: `geo` espone `LineJoin::Round(a)` e
  `LineCap::Round(a)` di `i_overlay` (passo angolare `a`, portato in
  `[0.01 pi, 0.25 pi]`; in `i_overlay` 9 ogni intervallo angolare fra due
  punti consecutivi di un arco è al più `a`, in 4.5 arrivava a `1.5 a`). Si
  chiede `a = 2 acos(1 - f / |d|)` (meno una parte su un milione, per
  l'arrotondamento all'unità angolare intera) con `f = max(p / 2, 0.001
  |d|)`: fino a `|d| = 500 p` (5 m) freccia più griglia restano entro `p`;
  oltre vale la deviazione dichiarata sotto («Deviazione: archi del
  buffer»). Con il passo minimo la freccia è al più `1.24e-4 |d|`, sempre
  entro la tolleranza. Le giunzioni tonde non usano la soglia
  `miter_min_turn` di `i_overlay` 9 (vale solo per `Miter`). Estremità
  piatte e quadrate non hanno archi;
- **componenti sotto la griglia**: prima del calcolo una linea più corta di
  `2 g` diventa il suo primo punto, un poligono d'area sotto `4 g^2` o più
  sottile di `2 g` il suo anello esterno (e poi, se corto, un punto): così
  il loro buffer non sparisce (la linea di 0,4 mm accanto a una di 1.300
  km perdeva un disco di 10 m). Il buffer negativo di una collezione
  considera solo le parti areali;
- **griglia**: prima del calcolo `(3.5 + 2.5 sqrt(2)) g + 12 ulp(M)` per
  i due passaggi in catena (il `Buffer` di `geo` e l'unione delle parti)
  più il rientro degli offset dritti, entro `p / 2` sull'ingombro
  allargato di `3 |d|` (più del margine di
  `i_overlay` 9, `2.2 |d|`; vertici, distanza, punti degli archi e due
  overlay sulla griglia `i64`). Le direzioni intere di `i_float` 5 sono
  più corte del vero: una normale di al più `2^-30` (gli offset dritti
  rientrano di `2^-30 |d|`, nel bilancio: con 1 cm il buffer si rifiuta
  oltre `|d|` di circa 5.368 km), un punto d'arco di al più `4.1e-7 |d|`
  (tolto dalla freccia chiesta alle corde). Il buffer resta entro `p / 2` dal buffer
  esatto verso l'esterno ed entro `f + p / 2` verso l'interno lungo gli
  archi, senza controllo a posteriori contro la definizione.

`clean_topology` con la morfologia divide il bilancio: due buffer con
freccia `p / 8` (o lo 0,1% della tolleranza di chiusura, se maggiore:
la stessa deviazione) e griglia entro `p / 4`, e la rimozione delle
sovrapposizioni (unione dei vicini e differenza in catena) con griglia
entro `p / 4`: in tutto `p`. La rimozione non
accumula un'unione riga dopo riga (a ogni giro la griglia spostava
l'accumulatore: dopo 80 righe di 1.300 km una riga staccata larga 1,56 cm
spariva senza errore): ogni resto è la riga meno l'unione delle sole righe
precedenti che la toccano, prese dagli ingressi.

`dissolve` orienta gli ingressi prima di `unary_union` di `geo`, che
sceglie la regola di riempimento dal verso del primo anello: un poligono
valido di verso opposto spariva dall'unione.

**Ambito.** Tutte le operazioni geografiche. `geo.polygonize` e
`geo.split` con i controlli di spaziatura e noding; ogni operazione che
passa da `i_overlay` con la griglia a priori (e le catene di overlay):
`topology.rs` (`boolean_operation` e la variante `_validated`:
intersezione, unione, differenza, differenza simmetrica; `dissolve`,
`clip_to_mask`, `polygon_overlay`, `clean_valid_polygon_topology` con la
morfologia `buffer(±snap_tolerance)` e la rimozione delle sovrapposizioni),
`operations.rs` (`buffer`, `buffer_with_cap`), `extensions2.rs`
(`subdivide`, taglio dei poligoni), `extensions3.rs` (`coverage_validate`).
Tutte ricevono la precisione come argomento esplicito (`Precision`, nelle
unità delle coordinate): nessuna ha oggi un CRS risolto da cui ricavarla, e
il chiamante che lo ha usa `Precision::from_crs`.

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

**Hazard.**

- Una geometria più sottile di 1 cm (in tutto o in parte) può uscire fusa o
  vuota senza errore, per scelta.
- **Nessun controllo a posteriori** (decisione dell'utente). *Regola:* la
  garanzia di 1 cm degli overlay e del buffer poggia sui limiti a priori
  (griglia, arrotondamenti, catene) e sulla correttezza di `i_overlay`.
  *Ambito:* booleane, `dissolve`, `clip_to_mask`, `polygon_overlay`,
  `clean_topology`, `coverage_validate`, `subdivide`, `buffer`,
  `make_valid` `STRUCTURE`. *Hazard:* un difetto di `i_overlay` (una
  faccia persa o in più, come #87 in 4.5, corretto in 9.0) passerebbe
  senza errore; gli agganci di `i_overlay` dopo il primo giro (raggio `2^(k
  / 2) g` al giro `k`) non hanno un tetto a priori: per spostare un punto
  di `p / 2` servono circa `2 log2(p / g)` giri, 26 al limite della
  guardia di spaziatura e 60 a 20.000 km con 1 cm. I controlli rimossi
  costavano il 90-99% di ogni booleana (82% in `InteriorPoint` di `geo`
  per faccia) e rifiutavano risultati corretti (`completo` sulle
  differenze, sulle foglie di `subdivide` e sull'unione di due stelle da
  5.000 vertici, la cui area coincide con `|A| + |B| - |A ∩ B|` a `7e-7`
  m²), senza aver mai trovato un errore vero. *Condizione di rientro:* una
  verifica a posteriori a costo accettabile (campionata, o un oracolo
  esatto nei test su un corpus reale) che non rifiuti risultati corretti.
- **`coverage_validate`, decisione sull'area.** L'area di ogni
  sovrapposizione si confronta con la tolleranza sul risultato passato
  dalla griglia: una sovrapposizione più sottile della griglia può sparire
  (issue mancata) e vertici diversi su lati collineari possono lasciare una
  scheggia (issue spuria), entro la precisione per il perimetro della
  zona.
- **`subdivide`.** I tagli sono in catena (fino a 32 livelli): ognuno ha
  `1/32` di `p / 2`, e le foglie non sono confrontate con il poligono di
  partenza. Le parti non sono uniche: due versioni di `i_overlay` possono
  scegliere tagli diversi (una foglia sotto la soglia di vertici in una e
  sopra nell'altra), con la stessa area totale.
- **Costo, `i_overlay` 9 con `i64` e senza controlli a posteriori.**
  Mediane in release su Windows (7 ripetizioni), 1 cm, stelle rumorose
  in UTM; fra parentesi `i_overlay` 4.5.2 con i controlli a posteriori:
  booleane di due stelle da 5.000 vertici: intersezione 27,9 s (4,1 s,
  rifiutata), unione 0,46 s (5,0 s), differenza 0,42 s (3,9 s, rifiutata),
  xor 1,1 s (4,0 s, rifiutata) — l'intersezione, 6.872 parti, sono quasi
  tutti la validazione OGC dell'uscita (l'overlay di `geo` 164 ms, contro
  72-95 ms con `i32`); `dissolve` di 1.600 quadrati 6,4 ms (24 ms);
  `coverage_validate` di 400 celle 3,2 ms (22 ms); `clean_topology` di 100
  celle 116 ms (131 ms); `clip_inside_mask` di `bench_geo_perfcheck` 0,83
  s (19,4 s con i controlli e `i64`); buffer di una stella da 1.000
  vertici 10 / 14 / 42 / 261 ms a 1 / 10 / 100 / 1000 m (22 / 66 / 29 /
  172 ms), di una linea da 1.000 vertici 7 / 7 / 17 / 113 ms (43 / 24 /
  16 / 281 ms); `make_valid` di una stella da 1.000 vertici con 20 buchi
  che la attraversano 285 ms `STRUCTURE`, 486 ms `LINEWORK` (264 / 468
  ms). Il motore `i64` costa da 1,5 a 2 volte `i32` sull'overlay puro.
- **Deviazione: archi del buffer** (decisione dell'utente). *Regola:* gli
  archi del buffer (e della chiusura di `clean_topology`) hanno freccia al
  più `max(p / 2, 0.001 |d|)`, non `p / 2`. *Ambito:* `buffer`,
  `buffer_with_cap`, la morfologia di `clean_valid_polygon_topology`
  (`rust_backend::buffer::freccia_degli_archi`); la griglia resta entro
  `p / 2`. *Hazard:* oltre `|d| = 500 p`
  il buffer si scosta dal buffer esatto fino allo 0,1% della distanza (10
  cm a 100 m, 1 m a 1 km), sempre verso l'interno lungo gli archi (poligoni
  inscritti), senza errore: sopra la precisione di 1 cm. *Condizione di
  rientro:* archi entro `p / 2` a costo accettabile per ogni distanza
  (`i_overlay` non scende sotto un passo di `0.01 pi`), o un parametro
  esplicito di tolleranza nel piano.
- **Non applicabile.** Le parti di `subdivide` sotto la soglia di vertici
  escono invariate, senza overlay; i punti con estremità piatte e il buffer
  negativo senza parti areali sono vuoti per definizione, senza calcolo.
- Lo split lineare (`split_line`, sorgenti `LineString` di `geo.split`,
  codice precedente al porting) ammette un punto di taglio entro la
  tolleranza più un margine numerico proporzionale al modulo delle
  coordinate: l'adapter (`rust_backend::arrow::split_batches`) applica prima
  il controllo di spaziatura delle coordinate, così il margine resta sotto
  mezza precisione e un punto a più di 1 cm dalla linea, con tolleranza
  nulla, non taglia.

**Condizione di rientro.** Nessuna per la precisione, che è una scelta di
prodotto. Per l'assenza del controllo a posteriori, vedi l'hazard
«Nessun controllo a posteriori».

### `geo.reproject`: il cambio di datum vale quanto l'accuratezza accettata

**Regola.** La matematica della riproiezione resta entro la precisione
(proiezioni e trasformazioni entro 1e-8 m da PROJ, lati densificati entro
mezzo centimetro); il **cambio di datum** vale quanto l'accuratezza EPSG del
percorso fra i datum. Oltre 1 cm il cambio si rifiuta
(`REPROJECTION_ACCURACY_NOT_ACCEPTED`) salvo che la config dichiari
`accuratezza_accettata_m` almeno pari: è una garanzia indebolita per scelta
esplicita di chi scrive il piano, mai implicita.

**Ambito.** `geo.reproject` fra datum diversi non equivalenti per il
registro ([«La regola dell'accuratezza»](#la-regola-dellaccuratezza)).

**Hazard.** Il risultato può scostarsi dal vero fino all'accuratezza
accettata (metri per Monte Mario, ED50, OSGB36, NAD27 senza griglie), senza
errore; due geometrie vicine possono usare percorsi diversi e scostarsi fra
loro fino alla somma delle due accuratezze. WGS 84 e la famiglia ETRS89
sono equivalenti per convenzione (EPSG:1149 conta 0,
[«WGS 84 = ETRS89 per convenzione»](#wgs-84--etrs89-per-convenzione)):
oggi 50–80 cm reali in Europa, senza errore, salvo
`convenzione_wgs84_etrs89: false`. Gli altri limiti sono in
[«Limiti dichiarati della riproiezione»](#limiti-dichiarati-della-riproiezione).

**Condizione di rientro.** Nessuna: è l'accuratezza del registro. Con le
griglie NTv2 ufficiali (per esempio IGM per l'Italia) l'accuratezza scende a
quella della griglia.

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
`Polygon` si trovano sui rettangoli chiusi (scansione su `x`, e un R-tree di
`rstar` quando la scansione supererebbe 256 confronti per elemento): `relate`
non si chiama sulle coppie per cui renderebbe il suo ramo disgiunto (stessa
condizione `Rect::intersects` di `geo`, che lascia vuote le celle
Interno-Interno e Confine-Confine da cui nascono gli errori), le altre si
visitano nell'ordine `(i, j)` del doppio ciclo, quindi gli errori emessi e il
loro ordine non cambiano; con coordinate non finite, doppio ciclo. Su quelle
coppie `relate` riceve le parti come `PreparedGeometry` costruite una volta
per parte (grafo, R-tree dei segmenti e intersezioni fra anelli), non
ricostruite a ogni coppia: in `geo` 0.33.1 è la stessa `RelateOperation` su
una copia dello stesso grafo, e `geo` stesso usa le due forme
indifferentemente nella validazione dei buchi. La parte `i` si prepara dopo
la propria validazione e si libera alla fine del suo turno; come `j` resta
preparata solo una parte senza buchi (anche prima della propria
validazione), le altre restano `Polygon` e `relate` ne costruisce il grafo
per la sola coppia, come in `geo`. Sull'intersezione di due stelle da 5 000 vertici
(7 922 parti, una grande che tocca col rettangolo migliaia di piccole) la
validazione dell'uscita passa da 7,2 s a 57 ms (`bench_validazione_parti`).
L'oracolo è in `crates/plenora-kernels-geo/src/validazione_ogc/tests.rs`.

**Ambito.** `plenora-kernels-geo`, ogni validazione OGC che passa da
`ValidazioneProtetta`.

**Hazard.**

- il caso peggiore resta O(n²): con molti segmenti lunghi a rettangoli
  sovrapposti le coppie candidate sono quadratiche come nel doppio ciclo (il
  verdetto non cambia, il tempo sì). È il caso della stella a lati radiali:
  `geometry_from_wkb` di una stella da 2 000 vertici costa circa 2 ms, da
  10 000 circa 50 ms. Un R-tree dei segmenti non lo migliora (misurato: stessi
  tempi sulle stelle, da 5 a 10 volte più lento su cerchi e pettini), perché
  le coppie di rettangoli che si toccano sono già quadratiche: serve una
  scansione a linea mobile (Shamos-Hoey) con predicati esatti e le esclusioni
  di `geo` sugli estremi condivisi, non ancora scritta;
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
  una chiamata per coppia che si tocca;
- l'equivalenza fra `relate` su parti preparate e su parti ricostruite
  dipende da `PreparedGeometry` di `geo` 0.33.1 (copia profonda dello stesso
  grafo): a ogni aggiornamento di `geo` va riverificata, e l'oracolo la
  confronta col doppio ciclo letterale sulle forme che esercita;
- memoria delle parti preparate: un grafo preparato costa O(V + I), con `V`
  i vertici della parte e `I` le intersezioni fra i suoi anelli, quadratiche
  nel caso peggiore (una parte invalida con buchi a bande orizzontali e
  verticali che si incrociano). Una parte senza buchi ha un solo anello, su
  cui `geo` non cerca intersezioni (`I = 0`, grafo O(V)): solo queste restano
  preparate come `j`, dalla prima coppia fino alla fine del proprio turno
  come `i` (non subito dopo l'ultima coppia che le usa), nel caso peggiore
  tutte insieme, cioè qualche centinaio di byte per vertice. Una parte con
  buchi è preparata solo come `i`, una alla volta. Il picco ha due parti:
  la memoria **trattenuta** nella cache, O(somma dei vertici delle parti
  senza buchi) più O(V + I) della parte `i` di turno; e la memoria
  **temporanea** di ogni coppia, che copia il grafo di `i` e, se `j` ha
  buchi, costruisce e nodifica quello di `j`, Θ(Vj + Ij) con `Ij` fino a
  quadratico, anche prima della validazione di `j` (un rettangolo `i` contro
  una `j` invalida con molti buchi a bande incrociate). La temporanea è la
  stessa della `relate` letterale di `geo`, che per ogni coppia costruisce i
  due grafi: stesso ordine. Nella validazione dei buchi ogni buco è un
  anello solo, quindi sempre O(V).

**Condizione di rientro.** Una versione di `geo` con una ricerca delle
auto-intersezioni sub-quadratica a verdetto identico: la sequenza copiata si
toglie, la barriera torna a `check_validation` e l'oracolo resta come
regressione. Per le parti preparate: una `relate` di `geo` che riusi il
grafo di una geometria su più confronti con memoria dichiarata, o un budget
di memoria della validazione che le governi.

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

**Regola.** Le mappe di chiavi dei kernel tabellari usano un solo hash
deterministico senza seme, `KeyHasher`
(`crates/plenora-kernels-table/src/hashing.rs`): come `FastHasher` per i
valori nativi (interi, testi, valori e chiavi composte dei join, partizioni
delle finestre, blocchi di `fuzzy_join`, chiave Int64 singola di
`reconcile` e `assert_foreign_key`, valori pivot e celle di `pivot`) e come
`hash_chiave` (lunghezza, poi byte) per le chiavi binarie di riga (arena `KeyInterner` di
aggregate, distinct, set operation, assert_unique, table_diff, `reconcile`,
`assert_foreign_key` e dell'indice di `pivot`; mappe e scelta della partizione dello spill). L'uguaglianza delle chiavi si decide
sempre sui valori o sui byte: l'hash sceglie i candidati, mai il risultato,
e nessun output dipende dall'ordine di visita di una mappa (le mappe si
interrogano per chiave; dove si visitano, il risultato si ordina o si
riduce con operazioni commutative, e `fuzzy_join` sceglie il blocco peggiore
con uno spareggio sulla chiave).

**Ambito.** `plenora-kernels-table`: raggruppamenti, join, finestre,
set operation, qualità, spill, `fuzzy_join`.

**Hazard.**

- non è keyed: dati costruiti apposta per collidere degradano
  build e probe fino al quadratico entro i limiti di riga, che limitano `n`
  ma non il comportamento dentro `n`. Nessuna perdita di correttezza;
- ripiega i bit alti su quelli bassi dopo ogni blocco. Senza, il
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
  lock (5.0.0, motore `i64` di `i_overlay` 9.0.0), e un suo aggiornamento
  va riletto;
- il corpus applicativo reale (gate del laboratorio) non è mai stato eseguito:
  mancano WKB reali anonimizzati;
- la validazione interna dei kernel usa `check_validation` di `geo`,
  quadratica, non la scansione della voce precedente.

**Condizione di rientro.** La campagna differenziale del laboratorio
rieseguita contro questo `geo` vendorizzato e il corpus applicativo reale
verde, con le divergenze spiegate.

### `geo.delaunay` e `geo.voronoi`: triangolazione caricata in blocco

**Regola.** `extended_algorithms::delaunay` e `advanced::voronoi_cells` non
chiamano più `unconstrained_triangulation` e `voronoi_cells` di `geo`
0.33.1, che inseriscono i punti in `spade` uno alla volta: la
triangolazione si costruisce con `DelaunayTriangulation::bulk_load` di
`spade` 2.15.1 (la copia che `geo` usa già, stessi predicati esatti di
`robust` 1.2.0), in `crate::triangolazione`. Prima del caricamento i punti
si validano in ordine d'ingresso con `spade::validate_vertex` (stesso primo
errore dell'incrementale) e i duplicati si tolgono con l'uguaglianza di
`spade` (`==`, quindi `-0.0 == 0.0`); a ogni vertice restano il rango
(prima comparsa, l'indice che aveva nell'incrementale) e i bit che
l'incrementale gli lasciava (un duplicato successivo li sostituisce, tranne
quando i vertici distinti sono almeno due e ancora tutti collineari). Il
conteggio dei vertici dopo il caricamento si verifica: un punto perso o
fuso è un errore `Triangulation`/`Voronoi`, mai un'uscita.

- `delaunay`: stessi triangoli, antiorari, anelli chiusi `[a, b, c, a]`.
  L'ordine d'uscita (`DefinedOrder` nel catalogo) prima era quello interno
  delle facce di `spade`; ora è canonico e fa parte del contratto: ogni
  triangolo parte dal vertice comparso per primo, i triangoli sono in
  ordine lessicografico della prima comparsa dei tre vertici. Nel catalogo
  `geo.delaunay` passa a `semantic_version` 2 e `kernel_version` 2.
  Nessun controllo di precisione: i vertici d'uscita sono i punti
  d'ingresso con i loro bit e i predicati sono esatti, quindi nessuna
  coordinata calcolata può spostarsi.
- `voronoi_cells`: il corpo di `build_raw_voronoi_cells` e
  `voronoi_cells_with_params` di `geo` 0.33.1 è ricopiato (raggi,
  ordinamento angolare, ritaglio `Padded` con `intersection` di `geo`), le
  celle escono in ordine di rango e il rettangolo dei siti si accumula in
  quell'ordine, come nell'incrementale; associazione punto -> cella
  invariata. Il circocentro non è più `circumcenter` di `spade`, che
  dipende dal vertice da cui `spade` parte la faccia (la revisione ha
  trovato 12,5 cm fra incrementale e caricamento in blocco con siti a
  `10^15`): stessa formula con l'origine nel vertice di rango minimo,
  indipendente dalla rotazione, con un maggiorante del suo errore
  d'arrotondamento. La funzione riceve la precisione (`Precision`) e
  rifiuta con errore esplicito: `PrecisionInsufficient` se la spaziatura
  dei `f64` supera `p / 64` al modulo dei siti più la distanza dei punti
  lontani dei raggi, o al modulo dei vertici delle celle grezze (lo stesso
  `coordinate_abbastanza_fitte` degli overlay); `VerticeMalCondizionato`
  se il maggiorante di un circocentro supera `p / 4`; `PrecisionInsufficient`
  anche se la griglia del ritaglio delle celle di bordo (`intersection` di
  `geo`, `i_overlay`) supererebbe `p / 2`, lo stesso controllo a priori
  delle booleane (`griglia::controlla_overlay`). Nel catalogo
  `geo.voronoi` passa a `semantic_version` 2 e `kernel_version` 2.

`bulk_load` e non `bulk_load_stable`: la variante stabile reinserisce i
vertici saltati iterando un `HashSet` a seme casuale (`try_bulk_load_cdt`
di `spade` 2.15.1), quindi su griglie e punti cocircolari la
triangolazione cambierebbe da processo a processo; `bulk_load` li tiene in
un `Vec`. L'ordine d'ingresso che la variante stabile conserverebbe si
ricostruisce con il rango.

Gli oracoli ricopiano alla lettera le due funzioni di prima
(`extended_algorithms::oracolo_delaunay`, `advanced::tests`,
`oracolo_costruzione_*`): stessi errori per variante, messaggio e indice;
su punti casuali continui e UTM al centimetro stessi triangoli bit per bit;
su griglie, punti interi cocircolari e reticoli con duplicati un controllo
esatto in interi (`i128`) che l'uscita sia di Delaunay, anche con le
coordinate scalate a `2^-142` e `2^190`; celle Voronoi identiche bit per
bit o entro `10^-6` in Hausdorff (misurato: 70 % identiche, scarto massimo
`4,7e-10` in coordinate UTM). Il maggiorante del circocentro è provato
contro lo scarto fra le tre origini possibili su 20.000 triangoli, anche
sottili; sul dominio realistico (siti UTM al centimetro o continui, 1 cm)
il maggiorante misurato resta sotto `3,4e-4 p` fino a 30 km di lato.

Misure (release, mediane di 7 esecuzioni alternate prima/dopo, Windows,
punti casuali UTM al centimetro; prima -> dopo): `delaunay` 10k 27,5 ->
15,6 ms, 100k 1.215 -> 169 ms, 1M 97,7 -> 1,35 s; `voronoi` 10k 43 ->
28 ms, 100k 1.221 -> 253 ms, 1M 86,5 -> 3,9 s (a 1M il prima è una sola
esecuzione; `examples/bench_delaunay_voronoi.rs`).

**Ambito.** `plenora-kernels-geo`, `geo.delaunay` e `geo.voronoi`.

**Hazard.**

- **ingressi degeneri** (quattro o più punti cocircolari: griglie,
  reticoli, circonferenze): la triangolazione di Delaunay non è unica e
  quella caricata in blocco è un'altra triangolazione valida, con le stesse
  facce altrove (su una griglia 100x100, 9.802 triangoli su 19.602 in
  comune). Stesso numero di triangoli e stessa area totale, diagonali
  diverse. Il diagramma di Voronoi è unico, ma i suoi vertici si calcolano
  dai circocentri di triangoli diversi: dove il circocentro è mal
  condizionato (triangoli sottili su circonferenze grandi) lo scarto dal
  risultato di prima è quello dell'errore di arrotondamento del
  circocentro, lo stesso ordine dell'errore che il risultato di prima aveva
  già rispetto all'esatto;
- **circocentri**: l'origine nel vertice di rango minimo non è quella
  dell'incrementale, quindi un vertice Voronoi può differire di qualche
  `ulp` da prima anche su ingressi non degeneri; parità di angolo
  nell'ordinamento attorno al sito (rarissime) seguono l'ordine dei lati
  di `spade`, diverso da quello dell'incrementale;
- **rifiuti di `VerticeMalCondizionato`**: il maggiorante è al primo
  ordine, raddoppiato, non dimostrato in forma chiusa. Cresce con
  `R L^2 / A` (raggio circoscritto, lato, area): i triangoli sottili sul
  bordo dell'inviluppo lo fanno salire con l'estensione dei dati. Misurato
  con siti UTM al centimetro: sotto `3,4e-4 p` fino a 30 km, ma a 1.000 km
  di lato con 200.000 siti un triangolo arriva a `1,17 p` e l'operazione si
  rifiuta. Il vertice lontano di un triangolo sottile cade di solito fuori
  dal ritaglio, e il rifiuto è quindi prudente, non necessario, in quei
  casi;
- `intersection` di `geo` nel ritaglio delle celle di bordo non passa dal
  controllo di griglia di `rust_backend::griglia` (come prima di questa
  modifica): lo spostamento della griglia di `i_overlay` non è confrontato
  con la precisione;
- **predicati di `robust` 1.2.0**: `spade` accetta solo coordinate zero o
  di modulo in `[2^-142, 2^201]`, il dominio in cui Shewchuk dichiara che
  `orient2d` e `incircle` non vanno in underflow né in overflow; fuori,
  l'errore è esplicito (`SpadeError(TooSmall)`/`TooLarge`, come prima). I
  220 fallimenti di `robust` 1.2.0 trovati in `plenora-memory-lab`
  (`orient2d` con coordinate fino a `5e-324`, 1.266 confronti con
  l'oracolo razionale) cadono tutti fuori dal dominio: le 30 righe dentro
  sono corrette. Su 20.004 triple aggiuntive dentro il dominio, quasi
  collineari o collineari esatte (6.780) a esponenti estremi e misti,
  `orient2d` concorda con l'oracolo razionale in tutte. `incircle` non ha
  un oracolo razionale proprio: lo coprono il controllo esatto in interi
  sugli ingressi degeneri scalati agli estremi e la dichiarazione di
  Shewchuk;
- `spade` 2.15.1 è letto dai sorgenti (dati dei duplicati, `HashSet` di
  `bulk_load_stable`, ordine dei lati): un aggiornamento va riletto, e gli
  oracoli vedono una divergenza solo sulle forme che esercitano;
- il caso peggiore di `bulk_load` resta quadratico su ingressi molto
  degeneri (punti quasi tutti allineati), come l'incrementale.

**Condizione di rientro.** Una versione di `geo` che costruisca
triangolazione e celle Voronoi con il caricamento in blocco, un ordine
deterministico e circocentri con un limite d'errore dichiarato: le due
funzioni tornano a chiamare `geo`, e gli oracoli restano come regressione.
Per i rifiuti di `VerticeMalCondizionato`: un circocentro calcolato in
aritmetica estesa (differenze esatte, prodotti in doppia-doppia), che
riduce il maggiorante di circa `2^-53`.

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
- **Campo geometria dell'uscita.** `polygonize_batches` e `split_batches`
  non prendono più `output_crs`, e `make_valid_batches` non ripete più lo
  schema d'ingresso tale e quale: il campo geometria dell'uscita è quello
  dell'ingresso con tutti i suoi metadati (CRS, dimensioni, encoding,
  lineage, R2.4), senza la dichiarazione dei tipi che l'operazione
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
tipo che `parquet` deduce. Si rifiutano anche, prima di decodificare: le
colonne `INT96` (timestamp legacy di Impala e Spark, che `parquet` converte
con aritmetica che avvolge), una chiave ripetuta nei metadati chiave-valore
del file (`parquet` terrebbe l'ultima) e una chiave del file che contraddice
i metadati dello schema incorporato. In scrittura ogni decimale deve stare
nella precisione del suo tipo, a ogni profondità: `parquet` restringe il
valore alla larghezza fisica e un decimale fuori precisione tornerebbe un
altro numero.

**Scrittura deterministica.** `created_by` costante (`parquet-rs version
59.2.0`), pagine formato 1.0, row group di al più 1 048 576 righe,
statistiche di pagina, niente bloom filter, metadati JSON con chiavi in
ordine: la stessa tabella dà gli stessi byte, in Parquet e in Arrow IPC
(provato). Dopo ogni scrittura si rilegge il footer: per Parquet lo schema
incorporato deve essere quello scritto e applicarsi senza cambiare, per
Arrow IPC lo schema del file deve essere quello della tabella.

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
| altre chiavi di primo livello | ignorate, come chiede la specifica |

Ogni colonna diventa il campo che il contratto accetta: `Binary`
(`LargeBinary` si converte), `ARROW:extension:name = geoarrow.wkb`,
metadato di campo `geo` con `crs`, `encoding` e `dimensions` (solo se
`geometry_types` la decide: con un elenco vuoto resta quella delle chiavi
canoniche del campo, o `unknown`); poi
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
integrati) o `null` per un CRS mancante, `bbox` il riquadro XY di tutte le
coordinate quando le geometrie sono 2D. `geometry_types` porta insieme tipi
e dimensionalità, e si scrive solo ciò che il contratto decide, così la
rilettura ridà lo stesso contratto:

| contratto | `geometry_types` |
| --- | --- |
| dimensionalità `xy`/`xyz`, tipi dichiarati con elenco | l'elenco dichiarato (anche se i dati ne usano una parte), con ` Z` per `xyz` |
| dimensionalità `xy`/`xyz`, tipi non dichiarati | i tipi trovati nei dati (la rilettura li dichiara `exact`) |
| dimensionalità `unknown`, o dichiarazione senza elenco (`unresolved`) | `[]` |

Tipi e dimensionalità dei dati si verificano comunque contro il contratto. Il `geo` di campo non entra nello schema incorporato: lo porta il
metadato di file. Si rifiutano: EWKB, coordinate M, curve, un ordine degli
assi dichiarato diverso da `lon_lat`/`easting_northing`, un CRS dichiarato
ma non risolto, geometrie di un tipo o di una dimensionalità diversi dal
contratto.

La camminata delle celle (`plenora_io::wkb`) è una validazione strutturale:
WKB ISO dei sette tipi, 2D o 3D, byte order per geometria, conteggi limitati
dai byte rimasti, figli coerenti con la multi-geometria, profondità 64,
nessun byte in eccesso, coordinate finite (il punto vuoto, tutto NaN, è
ammesso e resta fuori dal riquadro). Non verifica la chiusura degli anelli e
ammette anelli vuoti: la validità geometrica resta ai kernel. Un proptest la confronta con la
codifica dei kernel e con le coordinate di `geo`.

Fixture: `crates/plenora-io/tests/dati/` contiene file scritti da pyarrow
(Parquet C++): due GeoParquet nella forma di GeoPandas, con il PROJJSON di
PROJ, e uno con timestamp `INT96` (`scripts/genera_fixture_geoparquet.py`).

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
| scrittura Arrow IPC | due volte il blocco più grande (circa 8 MiB, di più con righe molto più grandi della media), misurato sui blocchi veri con i dizionari interi, più 1 MiB | picco sotto 3 MiB |
| scrittura Parquet | 4 volte i byte del row group più grande, misurato sulle fette vere, più 8 MiB | picco fino a 45 MiB (liste, 5 milioni di righe) |

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
  (provato su ogni combinazione di dimensionalità, dati e dichiarazione dei
  tipi), i byte dei metadati no. Due cambi voluti: con dimensionalità nota
  e tipi non dichiarati la rilettura dichiara `exact` i tipi dei dati; senza
  blocco canonico e senza geometrie (zero righe o tutte nulle) la
  dimensionalità `xy`/`xyz` non si rappresenta (`geometry_types` vuoto) e si
  rilegge `unknown`.
- **Interi stretti da file malformati.**
  *Regola*: `parquet` legge `Int8`, `Int16`, `UInt8`, `UInt16` da colonne
  fisiche `INT32` con un cast che tronca.
  *Ambito*: `parquet_io::leggi` su file di altri scrittori.
  *Hazard*: un file malformato con un valore `INT32` fuori dal dominio del
  tipo logico (o dello schema incorporato) torna un altro valore, senza
  errore; gli scrittori conformi (e `plenora-io`) non producono questi file.
  *Rientro*: rilettura di quelle colonne come `Int32` con un cast
  verificato.
- **Permessi dei file su Unix**: `tempfile` crea il temporaneo con modo
  `0600` e la rinomina lo conserva, quindi i file scritti sono leggibili solo
  dal proprietario, anche quando sostituiscono un file con altri permessi.
  Su Windows valgono le ACL ereditate dalla directory.
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

### Suite lunga

Gli oracoli di massa (differenziali della validazione OGC, proptest con
migliaia di casi, soglie e configurazioni esplorate una per una) hanno due
misure. Senza variabili `cargo test` gira un sottoinsieme deterministico
degli stessi casi, scelto perché ogni ramo resti coperto; con
`PLENORA_TEST_LUNGHI=1` gira tutto:

```sh
PLENORA_TEST_LUNGHI=1 cargo test --workspace --locked
```

La suite lunga è il gate prima del merge; quella di default serve mentre si
lavora. Un valore diverso da `0` e `1` ferma i test che la leggono.

I gate completi prima di un commit sono in [`AGENTS.md`](AGENTS.md).

## CRS integrati

Senza PROJ, `plenora_core::crs::resolve_crs` risolve gli identificatori
d'autorità di una tabella integrata, generata dal registro EPSG. Descrive i
CRS (tipo, unità, assi, area d'uso, dominio di validità, ellissoide); la
riproiezione fra questi CRS è in [«Riproiezione»](#riproiezione). Per gli
identificatori in tabella la risoluzione è qui; per ogni altra definizione
vale la voce
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

**Condizione di rientro.** Una proiezione inversa verificata che riporti il
punto in lon/lat e lo confronti con la regione esatta: `geo.reproject` lo fa
già ([«La catena»](#la-catena)); `validate_geometry_domain` resta sul
rettangolo.

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
   `crates/plenora-core/src/crs/integrati/tests.rs`;
5. rigenerare i parametri di riproiezione e l'oracolo
   (`scripts/genera_riproiezione.py`, poi
   `scripts/genera_oracolo_riproiezione.py`, stesso ambiente) e rieseguire i
   test: l'oracolo pretende ogni CRS proiettato e ogni trasformazione senza
   griglia della tabella.

Il generatore rifiuta con un errore esplicito ciò che non sa descrivere: CRS
deprecati, non bidimensionali, con meridiano diverso da Greenwich, unità
diverse da gradi o metri, assi non nord/est, metodi di proiezione senza una
regola di dominio. Una nuova regola di dominio è una decisione da prendere in
PR, non un default.

## Riproiezione

`geo.reproject` riproietta una colonna geometria fra due CRS della tabella
integrata ([«CRS integrati»](#crs-integrati)), tutti e 169, in Rust puro:
nessun PROJ, nessuna griglia scaricata. La matematica è in
`plenora_core::crs::riproiezione`, il kernel su `geo::Geometry` e su
`RecordBatch` in `plenora_kernels_geo::riproiezione` (`reproject_batches`),
l'analisi del contratto in `analyze_reproject`.

```json
{"out": "rdn", "op": "geo.reproject", "in": ["catasto"],
 "config": {"target_crs": "EPSG:7791",
            "accuratezza_accettata_m": 0.1,
            "griglie": [{"trasformazione": 9734,
                         "file": "C:/griglie/35160622_47161840_R40_F00.gsb"}]}}
```

- `target_crs` (obbligatorio): un identificatore della tabella integrata;
- `accuratezza_accettata_m` (facoltativo): l'accuratezza, in metri, che si
  accetta per il cambio di datum ([«La regola dell'accuratezza»](#la-regola-dellaccuratezza));
- `trasformazioni` (facoltativo): codici EPSG delle trasformazioni da usare,
  nell'ordine; il percorso fra i datum è esattamente quello;
- `griglie` (facoltativo): griglie NTv2 fornite dall'utente, ognuna con il
  codice EPSG della trasformazione a griglia e il percorso del file;
- `convenzione_wgs84_etrs89` (facoltativo, predefinito `true`): WGS 84 e la
  famiglia ETRS89 equivalenti per convenzione
  ([«WGS 84 = ETRS89 per convenzione»](#wgs-84--etrs89-per-convenzione)).

Contratto: schema, righe, tipi geometrici e `FieldId` invariati; il CRS del
contratto e il metadato `geo` del campo diventano il target, le chiavi
canoniche CRS della sorgente si sostituiscono, `axis_order` diventa quello
GIS normalizzato del target. Le coordinate si leggono e si scrivono
nell'ordine GIS normalizzato (longitudine o easting prima, anche per 4326,
6707–6709, 6875, 3035): una colonna che dichiara un altro ordine, anche
`unknown`, si rifiuta, come a 190c493. Un CRS risolto dal chiamante (non
della tabella) si rifiuta con `CRS_NOT_BUILTIN`.

### La catena

Per ogni punto: dominio di validità del CRS sorgente (rettangolo proiettato
o mondo lon/lat), proiezione inversa, **regione lon/lat** del dominio
(Transverse Mercator: ±15° dal meridiano centrale e le latitudini del fuso;
Mercator: ±85,06°), cambio di datum lungo un **percorso** di trasformazioni
EPSG, regione lon/lat e proiezione diretta del target, dominio di validità
del target. Ogni uscita da un dominio o da una regione è
`COORDINATE_OUT_OF_CRS_DOMAIN`, senza coordinate nel messaggio. Con la
regione lon/lat la riproiezione chiude, per sé, il limite «il rettangolo è
più largo della regione» dei [CRS integrati](#limiti-dei-crs-integrati):
un punto UTM nel rettangolo ma oltre i 15° dal meridiano si rifiuta.

Stesso CRS, o CRS che differiscono solo per l'ordine d'autorità degli assi
(6707 e 7791, 4326 e `OGC:CRS84`): coordinate invariate al bit, dopo i
controlli di dominio.

### Metodi di proiezione

I parametri li genera `scripts/genera_riproiezione.py` dal registro EPSG
v11.022 (PROJ 9.5.1, pyproj 3.7.2, stesso ambiente vincolato di
`genera_crs_integrati.py`) in `crates/plenora-core/src/crs/riproiezione/epsg.rs`,
che riporta la fonte e non si modifica a mano.

| metodo EPSG | CRS | formule | scarto massimo da PROJ (oracolo) |
| --- | --- | --- | --- |
| Transverse Mercator (9807) | 148: UTM WGS 84 ed ETRS89, IGM95, RDN2008, ED50, Gauss-Boaga 3003/3004, 6875/7794, 27700, 31467, 2193 | Krüger al sesto ordine nella forma di Karney (2011) | 7e-9 m |
| Mercator (variant A) (9804) | 3395 | isometrica esatta, inversa per Newton | 4e-9 m |
| Popular Visualisation Pseudo Mercator (1024) | 3857 | sferica con raggio `a` sulle coordinate geodetiche | 4e-9 m |
| Lambert Conic Conformal (2SP) (9802) | 2154 | EPSG 7-2, latitudine per Newton | 4e-9 m |
| Lambert Azimuthal Equal Area (9820) | 3035 | EPSG 7-2, latitudine autalica inversa per Newton | 1e-8 m |
| Oblique Stereographic (9809) | 28992 | sfera conforme di Gauss e stereografica | 7e-9 m |
| Hotine Oblique Mercator (variant B) (9815) | 2056 | Swiss Oblique Mercator, come PROJ (`somerc`) | 9e-9 m |

Scarti su punti che coprono la regione del dominio (per i TM fino a 14,5°
dal meridiano centrale), avanti e indietro; andata e ritorno entro 9e-9 m.

### Cambi di datum

Il datum di un CRS è il suo CRS geografico di base (18 datum). Le
trasformazioni sono quelle EPSG fra due datum della tabella, 156 in tutto:

- **senza griglia** (123): traslazioni geocentriche (9603), Position Vector
  (9606) e Coordinate Frame (9607), con le formule EPSG linearizzate, via
  coordinate geocentriche con altezza nulla (come PROJ per i CRS 2D);
- **a griglia NTv2** (33): entrano solo se l'utente fornisce il file
  ([«Griglie NTv2»](#griglie-ntv2)).

Restano fuori, e il generatore lo scrive nell'intestazione del file: le
operazioni concatenate del registro (il percorso lo compone il codice), le
griglie in altri formati (NADCON, NADCON5, …), le trasformazioni sostituite
(`supersession`) da un'altra inclusa, come fa PROJ. Molodensky-Badekas
(9636) fra questi datum compare solo in trasformazioni sostituite; nessuna
dipende dal tempo. Una trasformazione nuova con un metodo non supportato,
dipendente dal tempo o senza accuratezza fa rifiutare il generatore.

Un **percorso** è una catena di al più tre trasformazioni, ognuna in un
verso, che non ripassa per lo stesso datum. L'accuratezza del percorso è la
**somma** delle accuratezze EPSG dei passi. I percorsi si provano in un
ordine fisso: accuratezza, numero di passi, area d'uso più piccola (a parità
di accuratezza vince la trasformazione più specifica), codici EPSG, verso.
Per ogni geometria si usa il **primo percorso la cui area d'uso contiene
tutti i suoi punti** (vertici e punti aggiunti dalla densificazione): una
geometria non mescola mai due percorsi, e una che nessun percorso ammesso
copre è un errore (`REPROJECTION_OUTSIDE_TRANSFORMATION_AREA`), mai un
ripiego. Se poi un punto trasformato (vertice, campione o punto di
densificazione) sarebbe coperto da solo da un percorso che viene prima, o un
lato attraversa il riquadro d'uso di un percorso precedente (per esempio il
tratto sardo di una linea lungo il parallelo 40 da 7 a 11 E, che il riquadro
continentale contiene ma per cui vale la trasformazione della Sardegna: i
parametri continentali lo sposterebbero di circa 6 m), la geometria si
rifiuta (`REPROJECTION_MIXED_TRANSFORMATION_AREAS`), con o senza vertici
intermedi: l'esito non dipende da come l'ingresso è segmentato. Va divisa, o
il percorso fissato con `trasformazioni`, che vale per tutte le geometrie.

Esempi della scelta, per punti tipici e senza griglie:

| coppia | punto | percorso scelto | accuratezza |
| --- | --- | --- | --- |
| RDN2008 ↔ ETRS89 (7791 ↔ 25832) | Italia | RDN2008 to ETRS89 (1), EPSG:6710 | 0 m (equivalenti) |
| GDA94 ↔ GDA2020 (4283 ↔ 7844) | Canberra | GDA94 to GDA2020 (1), EPSG:8048 | 0,01 m (entro la precisione) |
| Monte Mario ↔ RDN2008 (Gauss-Boaga 3003 ↔ 7791) | Roma | EPSG:1659 + 6710 inversa | 4 m |
| | Cagliari | EPSG:1661 (Sardegna) + 6710 inversa | 4 m |
| | con la griglia IGM EPSG:9734 | EPSG:9734 | 0,1 m |
| IGM95 ↔ RDN2008 (3064 ↔ 7791) | Italia | EPSG:1098 + 6710 inversa | 0,5 m |
| ED50 ↔ ETRS89 (23032 ↔ 25832) | Roma | ED50 to WGS 84 (1) EPSG:1133 + 1149 inversa | 10 m (1149 conta 0) |
| | Copenaghen | ED50 to ETRS89 (4), EPSG:1626 | 1 m |
| OSGB36 ↔ WGS 84 (27700 ↔ 4326) | Londra | OSGB36 to WGS 84 (6), EPSG:1314 | 2 m |
| Amersfoort ↔ ETRS89 (28992 ↔ 4258) | Utrecht | Amersfoort to ETRS89 (8), EPSG:9281 | 0,25 m |
| CH1903+ ↔ ETRS89 (2056 ↔ 4258) | Berna | CH1903+ to ETRS89 (1), EPSG:1647 | 0,1 m |
| NAD27 ↔ NAD83 (4267 ↔ 4269) | Kansas | NAD27 to WGS 84 (6) EPSG:1175 + 1188 inversa | 11 m |
| DHDN ↔ ETRS89 (31467 ↔ 25832) | Stoccarda | DHDN to ETRS89 (3), EPSG:1778 | 1 m |
| RGF93 v1 ↔ ETRS89 (2154 ↔ 4258) | Parigi | EPSG:1591 | 0,1 m |
| ETRS89, RDN2008 ↔ WGS 84 (25832, 3035, 7791 ↔ 4326, 32632) | Europa | ETRS89 to WGS 84 (1), EPSG:1149 (con 6710 per RDN2008) | 0 per convenzione (1 m con `convenzione_wgs84_etrs89: false`) |
| NZGD2000, SIRGAS 2000 ↔ WGS 84 | | EPSG:1565, EPSG:15894 | 1 m |
| CGCS2000 ↔ altri datum | | nessuno (`REPROJECTION_PATH_UNAVAILABLE`) | — |

Stesso datum (per esempio 4326 ↔ 3857 ↔ 32632, 4258 ↔ 3035 ↔ 25832):
nessuna trasformazione, accuratezza 0.

### WGS 84 = ETRS89 per convenzione

**Regola** (decisione dell'utente). Come la maggior parte dei GIS, e come
PROJ quando applica EPSG:1149 (traslazioni nulle), WGS 84 e la famiglia
ETRS89 (ETRS89 e i CRS su di esso: 4258, 25828–25837, 3035; RDN2008 e i
suoi, 6706–6709, 7791–7794, 6875, equivalente a ETRS89 per il registro con
accuratezza 0) sono **equivalenti per convenzione**: il passo ETRS89 to WGS
84 (1), EPSG:1149, conta accuratezza 0 invece di 1 m. WGS 84 → RDN2008 /
UTM 32N non chiede `accuratezza_accettata_m`.

**Ambito.** Il solo passo EPSG:1149, anche dentro una catena: ED50 → WGS 84
→ ETRS89 conta l'accuratezza del passo ED50 e 0 per il resto. Ogni altro
cambio di datum resta sotto la regola dell'accuratezza.

**Hazard.** La differenza reale fra WGS 84 (G2139) ed ETRS89 in Europa è
oggi di circa 50–80 cm e cresce di circa 2,5 cm all'anno (deriva della
placca euroasiatica): il risultato si scosta dal vero di tanto, senza errore.

**Condizione di rientro.** `convenzione_wgs84_etrs89: false` nella config:
EPSG:1149 torna a 1 m e la regola dell'accuratezza lo chiede esplicitamente.
Scritta su una coppia che non passa da EPSG:1149 si rifiuta (senza effetto).

### La regola dell'accuratezza

**Regola.** Un percorso la cui accuratezza sta entro la precisione di 1 cm
è sempre ammesso: lo stesso datum, i datum equivalenti per il registro
(RDN2008 ed ETRS89, EPSG:6710 con accuratezza 0), WGS 84 ed ETRS89 per
convenzione, GDA94 → GDA2020 (1 cm).
Oltre, solo se `accuratezza_accettata_m` è almeno pari all'accuratezza del
percorso; altrimenti l'analisi rifiuta con
`REPROJECTION_ACCURACY_NOT_ACCEPTED`, che riporta l'accuratezza del percorso
migliore. Dichiarata l'accuratezza, **il risultato vale solo entro quella
accuratezza**: la matematica aggiunge al più mezzo centimetro (sotto),
l'errore del cambio di datum è quello che il registro dichiara. Sono ammessi
tutti i percorsi entro l'accuratezza accettata, e ogni geometria prende il
primo che la copre: il risultato non è mai peggiore di quanto dichiarato.

`accuratezza_accettata_m` senza effetto (ogni percorso ammesso è già entro 1
cm), non finita o negativa si rifiuta, come ogni parametro scritto e senza
effetto del runner; così una griglia che nessun percorso ammesso usa.

### Griglie NTv2

Le trasformazioni EPSG a griglia NTv2 fra i datum della tabella (33, fra cui
le griglie IGM 9732–9737 per Monte Mario, ED50, IGM95 e RDN2008, OSTN15
7709/7710, rdtrans2018 9282, BeTA2007 15948/15949, le GDA 8444–8447) entrano
nei percorsi solo se l'utente fornisce il file in `griglie`, con il codice
EPSG della trasformazione: da lì vengono datum, verso e **accuratezza** (quella
del registro, per esempio 0,1 m per EPSG:9734). Il file si legge con la sola
libreria standard, al più 256 MiB, record per record con ogni conteggio
verificato (intestazioni NTv2, `SECONDS`, estensioni multiple del passo,
`GS_COUNT`, valori finiti, gerarchia delle sottogriglie ad albero,
endianness da `NUM_OREC`); interpolazione bilineare sulla sottogriglia più
fine che contiene il punto, inversa iterativa come PROJ (1e-12 radianti, al
più 20 passi, altrimenti `REPROJECTION_NOT_CONVERGED`). Un punto fuori dalla
griglia rende il percorso non applicabile alla geometria. Nessun
download: l'analisi verifica la forma della config, il file si legge
all'esecuzione (`NTV2_GRID_UNREADABLE`, `NTV2_GRID_INVALID`).

### Densificazione dei lati

Un lato dritto nel CRS sorgente non è dritto nel target (6° lungo il
parallelo 45 in UTM 32N: la corda si scosta di metri dalla curva). Ogni
lato si prova nei punti a 1/4, 1/2 e 3/4: l'immagine esatta deve stare entro
**metà della precisione del target** (5 mm a terra; per un target
geografico metà di 1 cm in gradi all'equatore, più severo altrove) dal lato
d'uscita, i punti del lato d'uscita entro la stessa distanza dalla spezzata
delle immagini, e nessuna metà del lato può avere un'immagine più lunga di
3/4 dell'intero (continuità). Con una griglia NTv2 nel percorso, in più,
estremi e campioni devono stare nella **stessa cella** di ogni griglia (il
campo di spostamenti è bilineare a pezzi: dentro una cella è quadratico
lungo il lato e i campioni ne misurano lo scarto; un rilievo di un nodo fra
due campioni non si perde), oppure l'immagine del lato deve essere più
corta della tolleranza (il pezzo che attraversa un bordo di cella, ridotto
per bisezione). Altrimenti il lato si divide a metà nel CRS sorgente, fino a
48 livelli e a `MAX_CELL_COORDINATES` coordinate per cella:
oltre, `REPROJECTION_EDGE_NOT_CONVERGED` (il caso tipico: un lato che nel
target attraversa l'antimeridiano) o `ResourceLimit`. Le rette del target
(meridiani in Mercator, paralleli in lon/lat) non ricevono punti. L'uscita
ha lo stesso tipo e la stessa struttura dell'ingresso, anelli chiusi, e deve
essere valida OGC: una riproiezione che la rendesse non valida è un errore.
Il calcolo gira dietro la barriera `calcolo_protetto`.

### Oracolo

`scripts/genera_oracolo_riproiezione.py` (stesso ambiente vincolato) scrive
in `crates/plenora-core/tests/fixtures/riproiezione/` i risultati di PROJ
9.5.1 obbligato alla **stessa operazione EPSG**, senza rete e senza scelta
automatica: ogni CRS proiettato della tabella (3.780 punti), ogni
trasformazione senza griglia avanti e inversa (2.214), ogni CRS della tabella
da e verso WGS 84 e le coppie rappresentative (Gauss-Boaga ↔ RDN2008 UTM
32N, ED50 UTM ↔ ETRS89 UTM, OSGB36 ↔ WGS 84 ed ETRS89, RD New ↔ ETRS89,
LV95 ↔ ETRS89, NAD27 ↔ NAD83, GDA94 ↔ GDA2020, DHDN, Lambert-93, LAEA; 1.660
catene), e una griglia NTv2 sintetica a due livelli con il suo
`hgridshift` (148 punti, e in catena come EPSG:9734). La prova
(`crs::riproiezione::oracolo`) chiede **1 mm** ovunque e stampa gli scarti
massimi (`cargo test -p plenora-core oracolo -- --nocapture`):

| famiglia | scarto massimo da PROJ |
| --- | --- |
| proiezioni, avanti e indietro | 1,1e-8 m (tabella sopra) |
| traslazioni geocentriche, avanti / inversa | 2,2e-9 m / 3,1e-9 m |
| Position Vector, avanti / inversa | 1,6e-9 m / 3,2e-9 m |
| Coordinate Frame, avanti / inversa | 1,6e-9 m / 3,1e-9 m |
| griglia NTv2, avanti / inversa | 4,8e-6 m / 4,8e-6 m |
| catene con cambio di datum | 6,7e-4 m (3035 → 4326: l'inversa LAEA di PROJ usa una serie troncata per la latitudine autalica; la nostra, per Newton, torna al punto entro 1e-8 m) |
| catene nello stesso datum / con la griglia | 6,7e-9 m / 4,1e-6 m |

**Vicino alle singolarità** (poli, bordi dei domini, limiti di Mercator)
`scripts/genera_riferimenti_singolari.py` scrive `singolari.csv` con le
formule chiuse EPSG a 60 cifre (mpmath 1.3.0, strumento di sviluppo, non
dipendenza): LAEA a 5 cm e a 1 mm dal polo, Mercator e Pseudo Mercator a
±85,05°, LCC, stereografica e svizzera ai bordi dei domini. Scarto massimo
7,5e-9 m avanti e indietro. L'inversa e la diretta di LAEA calcolano `1 -
sin(beta)` dalla colatitudine: prima `sin(phi)` arrotondava a 1 a pochi
millimetri dal polo e il punto finiva sul polo (5,6 cm a terra). Transverse
Mercator resta fuori (la forma di Karney è stabile fino al polo).

Rigenerare: `PYTHONPATH=<dir> python -B scripts/genera_riproiezione.py`,
poi `PYTHONPATH=<dir> python -B scripts/genera_oracolo_riproiezione.py`,
`cargo fmt --all` e i test. I generatori controllano la terna
pyproj/PROJ/EPSG come `genera_crs_integrati.py`.

### Limiti dichiarati della riproiezione

- **Aree d'uso come riquadri.** *Regola:* un passo si applica a un punto se
  il punto sta nel riquadro lon/lat dell'area d'uso EPSG (nel datum
  d'ingresso del passo, anche nel verso inverso). *Ambito:* scelta del
  percorso per geometria. *Hazard:* il riquadro è più largo dell'area vera,
  come in PROJ: il riquadro di «Italy - mainland» contiene la Sardegna, e un
  punto sardo isolato prende la trasformazione sarda solo perché il suo
  riquadro, più piccolo, viene prima; una trasformazione di buona
  accuratezza su un'area offshore può coprire terraferma nel suo riquadro.
  Le geometrie con punti che preferiscono percorsi diversi si rifiutano; fra
  i punti trasformati il controllo dei lati usa la corda lon/lat sorgente
  contro il riquadro comune dei passi di ogni percorso precedente (per
  eccesso: può rifiutare un lato che sfiora un riquadro senza che quel
  percorso lo copra davvero, per esempio fuori dalla sua griglia). L'accuratezza EPSG vale
  nell'area vera, non nel riquadro. *Rientro:* poligoni delle aree d'uso
  (non nel `proj.db` distribuito), o `trasformazioni` per fissare il
  percorso.
- **Accuratezza sommata.** La somma delle accuratezze EPSG dei passi è per
  eccesso rispetto alla somma quadratica; l'accuratezza di WGS 84 ed ETRS89
  come insiemi di realizzazioni (2 m e 0,1 m nel registro) non si aggiunge:
  vale quella delle trasformazioni, come in PROJ.
- **EPSG:2056 non segue alla lettera Hotine B.** *Regola:* con azimut e
  angolo del reticolo di 90° si usa la Swiss Oblique Mercator (formule
  swisstopo, `somerc` di PROJ). *Hazard:* le formule letterali di Hotine
  variant B (aposfera) differiscono fino a 9 cm ai bordi dell'area d'uso;
  qui si segue il riferimento nazionale e PROJ. Ogni altra Hotine B si
  rifiuta (nessuna nella tabella). *Rientro:* nessuno, è la scelta del
  riferimento.
- **Inversa di Helmert algebrica.** Il verso inverso di Position Vector e
  Coordinate Frame è l'inversa della formula (`+inv +proj=helmert` di PROJ),
  non il cambio di segno dei parametri che il registro indica come
  approssimazione: differenza di pochi millimetri, molto sotto
  l'accuratezza di ogni trasformazione fuori dalla precisione.
- **CRS 2D: l'altezza si scarta.** Come PROJ, il cambio di datum parte da
  altezza ellissoidica nulla e scarta quella d'arrivo: andata e ritorno
  attraverso un cambio di datum torna al punto entro 3,1 mm (oracolo), non
  al nanometro.
- **Traslazioni nulle come identità.** Con tre traslazioni nulle
  (NAD83, NZGD2000, RGF93, IGM95, SIRGAS, RDN2008 verso WGS 84 o ETRS89)
  lon/lat restano invariate anche fra GRS 1980 e WGS 84, come `+proj=noop`
  di PROJ; il passaggio geocentrico sposterebbe la latitudine di circa 0,1
  mm.
- **Densificazione a campioni.** Lo scarto di un lato si misura in tre punti
  e nei due versi, con la continuità delle metà: è una verifica, non una
  dimostrazione. Una curva che oscilli fra i campioni di un lato lungo
  potrebbe scostarsene di più (per uno scarto a S circa il 3% oltre il
  valore campionato, dentro il margine di mezza precisione); sulle
  proiezioni della tabella (lisce nei loro domini) il controllo a 2.000
  punti della prova resta entro la precisione. Con una griglia NTv2 la
  verifica è per cella (sopra). La copertura di una griglia si prova sui
  punti trasformati: un lato può uscire da una griglia e rientrarvi fra due
  punti di celle diverse solo se la sua immagine è sotto la tolleranza.
- **Griglie non verificate contro il registro.** Il file di `griglie` si
  lega al codice EPSG dichiarato dall'utente: che sia davvero la griglia di
  quella trasformazione (e quindi che valga la sua accuratezza) non si
  controlla (il file non porta il codice). Un file sbagliato ma ben formato
  dà spostamenti sbagliati senza errore.
- **Kernel non ancora nel runner.** Il runner rifiuta le operazioni geo
  ([«Runner»](#runner)): `reproject_batches` si chiama dal kernel. Un
  esecutore futuro che fonda le trasformazioni in place
  (`TransformInPlace`) deve rileggere il CRS dopo `geo.reproject`, che lo
  cambia a metà del gruppo.
- **Fuori ambito.** CRS fuori tabella, operazioni concatenate del registro,
  griglie non NTv2, percorsi di più di tre passi, CGCS2000 verso altri
  datum (nessuna trasformazione nel registro), coordinate Z/M.
