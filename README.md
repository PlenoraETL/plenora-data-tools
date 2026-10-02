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
| `plenora-kernels-table` | kernel tabellari (filtri, ordinamenti, aggregazioni, join, espressioni, date, stringhe, qualità), tutti in memoria |
| `plenora-kernels-geo` | kernel geografici su `geo::Geometry` e adapter GeoArrow-WKB; `rust_backend` per `geo.make_valid`, `geo.polygonize` e `geo.split` senza GEOS, e i controlli di precisione della griglia degli overlay (`rust_backend::griglia`); `riproiezione` per `geo.reproject` senza PROJ |
| `plenora-pipeline` | runner minimo: piano SSA di operazioni tabellari e geo, validazione senza dati, esecuzione su tabelle intere con byte vivi contati per allocazione e budget di memoria per passo ([«Runner»](#runner)) |
| `plenora-io` | tabelle da e verso file: Arrow IPC (file e stream), Parquet, GeoParquet 1.1; scrittura atomica; un piano da file a file ([«File»](#file)) |
| `plenora-cli` | la CLI pubblica `plenora-data` (CLI 2.0 di `plenora-contracts`: `catalog`, `describe`, `validate`, `run`, `capabilities`) e la stessa superficie in Rust ([«CLI `plenora-data`»](#cli-plenora-data)) |
| `vendor/` | `geo` (con il porting a `i_overlay` 9.0.0) e `wkt` con le patch di `patches/` (provenienza in `vendor/*/PROVENANCE*.md`) |

## Che cosa non c'è ancora

- **Risoluzione CRS fuori tabella**: senza PROJ `resolve_crs` risolve solo
  gli identificatori d'autorità della tabella integrata
  ([«CRS integrati»](#crs-integrati)); un codice fuori tabella fallisce
  chiuso con `CRS_NOT_BUILTIN`, una definizione WKT, WKT2, PROJJSON o
  proj-string con `CRS_BACKEND_UNAVAILABLE`, e un CRS così entra solo già
  risolto dal chiamante.

Engine, CLI, isolamento e protocollo del progetto d'origine non sono stati
portati: la CLI `plenora-data` è nuova, scritta sui contratti pubblici
([«CLI `plenora-data`»](#cli-plenora-data)). Nemmeno, e oggi non sono in
programma: la verifica automatica (CI,
fuzzing, misura della copertura, mutation testing: i gate si eseguono a
mano, [`AGENTS.md`](AGENTS.md)), l'identità del piano (`plan_hash`,
fingerprint del catalogo) e l'interruzione di un kernel a metà: scadenza e
annullamento si controllano solo fra i passi
([«Scadenza e annullamento»](#scadenza-e-annullamento)).

## Le operazioni

Il riferimento delle operazioni del catalogo è
[`docs/operazioni.md`](docs/operazioni.md): una scheda per operazione con
parametri, schema d'uscita, semantica delle righe, ordine, errori,
complessità e un esempio. Questo README resta il documento delle regole e
dei limiti dichiarati; le schede li collegano senza ripeterli.

`docs/operazioni.md` è generato dalle schede `docs/schede/<id>.md` e dal
catalogo (`plenora_core::catalog`) da `crates/plenora-io/tests/operazioni_doc.rs`,
che esegue anche l'esempio di ogni scheda e fallisce se il documento non è
aggiornato. Un'operazione nuova o cambiata aggiorna la sua scheda, poi:

```sh
PLENORA_RIGENERA_DOC=1 cargo test -p plenora-io --test operazioni_doc
```

## Limiti dichiarati

### Precisione delle operazioni geografiche: 1 cm a terra

**Regola.** Ogni operazione geografica è garantita entro **1 cm a terra**,
precisione fissa, l'analogo del modello a precisione fissa di GEOS o di
`gridSize = 0.01` di PostGIS in metri. Nelle unità delle coordinate, con
una sola funzione, `plenora_core::crs::ResolvedCrs::precisione_coordinate`
(a cui delega `rust_backend::precision::Precision::from_crs` dei kernel):

- CRS proiettato: `0.01 / horizontal_unit_to_metre`;
- CRS geografico: 1 cm in gradi sul raggio di curvatura massimo
  dell'ellissoide del datum, `a / (1 - f)` (ai poli, lungo meridiano e
  parallelo): un grado non è mai più lungo di `a / (1 - f) * pi / 180`
  metri, quindi il passo vale al più 1 cm a terra ovunque e in entrambe le
  direzioni (per WGS 84 circa `8.953e-8` gradi). Un geografico senza
  ellissoide (risolto dal chiamante) non ha precisione: nessun raggio
  prudente copre con certezza ogni ellissoide (Clarke 1880 IGN arriva a
  6 400 057,7 m), e i kernel che la chiedono si rifiutano
  (`InvalidPrecision`).

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
  archi, senza controllo a posteriori contro la definizione;
- **linee a blocchi** (`rust_backend::buffer::blocchi`): il tratto di
  `i_overlay` mette in un solo overlay i contorni di tutti i segmenti e
  calcola ogni incrocio fra loro; su una linea i cui offset si
  sovrappongono su molti segmenti lontani (zig-zag stretto rispetto alla
  distanza: 1.000 vertici a 0,8 m, buffer di 200 m) il calcolo misurava 21
  GiB e 110 s per 16 KB d'ingresso. Per `LineString` e `MultiLineString`
  con estremità tonde o piatte, se le coppie di segmenti lontani nella linea
  (oltre 16 posizioni, o di linee diverse) a rettangoli allargati di `|d|`
  sovrapposti superano 4 per segmento (e 4.096), il buffer si calcola per
  blocchi di 8 segmenti sovrapposti di un segmento, uniti a coppie
  (`ceil(log2(blocchi))` unioni in catena, nel bilancio della griglia
  prima del calcolo). L'unione dei buffer esatti dei blocchi è il buffer
  esatto della linea (giunzioni tonde; l'estremità tonda di un blocco su un
  vertice interno è un disco contenuto nel buffer, quella piatta è il bordo
  del rettangolo del segmento condiviso), e i due limiti sopra valgono per
  l'unione: il risultato differisce dal tratto unico di al più `f + p` (gli
  archi dei blocchi cominciano da angoli diversi), senza errore. Sotto la
  soglia, e con estremità quadrate (sporgerebbero oltre i vertici interni),
  il tratto unico come prima. L'oracolo
  (`oracolo_a_blocchi_contro_tratto_unico_e_definizione`) confronta il
  percorso di produzione con la definizione (vertici entro `|d| + p / 2`,
  punti a `|d| - f - p` dentro) e con il tratto unico (bordi entro `f +
  p`). Sul profilo avversario del laboratorio (1.000 linee da 1.000
  vertici, 200 m) da 110 s e 20 GiB a 2,1 s e 112 MiB; 10.000 linee, prima
  oltre il tempo massimo di 300 s, 38 s e 794 MiB (release, 32 thread,
  mediana di 5).

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
- **Buffer delle linee a blocchi: un secondo algoritmo.** *Regola:* oltre
  la soglia di coppie di segmenti lontani («Buffer», voce «linee a
  blocchi») il buffer di una linea è l'unione dei buffer di blocchi di 8
  segmenti, non il tratto unico di `i_overlay`. *Ambito:* `buffer`,
  `buffer_with_cap` su `LineString` e `MultiLineString` con estremità tonde
  o piatte (non le collezioni, non i poligoni, non le estremità quadrate).
  *Hazard:* lo stesso ingresso poco sopra e poco sotto la soglia dà
  risultati diversi di al più `f + p` (vertici degli archi diversi, aree
  diverse di circa perimetro per `f`), entrambi nella fascia dichiarata; un
  poligono con anelli a zig-zag stretto rispetto alla distanza e le
  estremità quadrate restano al tratto unico, con il suo costo (nessun
  limite di memoria nel kernel: limite «Transitorio oltre la previsione non
  rilevato»). *Condizione di rientro:* un tratto di `i_overlay` che non
  calcoli gli incroci fra contorni già coperti, o i blocchi anche per gli
  anelli dei poligoni (buffer positivo: poligono unito al buffer degli
  anelli) con il loro oracolo.
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

### Misure geodetiche: l'ellissoide del datum

**Regola.** `geo.geodesic_distance`, `geo.geodesic_line_length`,
`geo.geodesic_area` e `geo.bearing` risolvono il problema geodetico
(Karney 2013, `geographiclib-rs`) sull'**ellissoide del datum del CRS
della colonna**, quello della tabella integrata
([«CRS integrati»](#crs-integrati)): Internazionale 1924 per ED50 e Monte
Mario, Clarke 1866 per NAD27, Airy per OSGB36, Bessel per DHDN, GRS 80,
WGS 84. `geo.haversine_distance` misura sulla sfera del raggio medio
IUGG `R1 = a (1 - f / 3)` dello stesso ellissoide. Il kernel riceve
l'ellissoide come argomento esplicito
(`plenora_kernels_geo::geodetica::EllissoideGeodetico::da_crs`), senza
valore predefinito; un CRS che non lo porta (risolto dal chiamante) si
rifiuta in validazione con `ELLIPSOID_REQUIRED`. I CRS proiettati si
rifiutano (`GEOGRAPHIC_CRS_REQUIRED`): le misure si chiedono sulle
coordinate geografiche, dopo `geo.reproject` se serve. L'oracolo
(`plenora-kernels-geo/tests/geodetica_oracolo.rs`) confronta distanze,
azimut, sfera, lunghezze e aree con GeographicLib 2.0 e PROJ su ogni CRS
geografico della tabella (`scripts/genera_riferimenti_geodetici.py`), entro
un micrometro e 1e-3 m²; con WGS 84 i kernel coincidono al bit con quelli
di `geo`.

**Area: topologia delle geodetiche.** La validazione OGC guarda il piano
lon/lat, ma i lati dell'area sono geodetiche: un lato lungo può passare
dall'altra parte di un buco o di un'altra parte, e l'area sottrarrebbe o
sommerebbe la regione sbagliata in silenzio.
`geodetica::verifica_topologia_geodetica` accetta solo i poligoni per cui
la topologia delle geodetiche è dimostrabilmente quella del piano:

- per ogni lato si maggiora lo scarto fra geodetica e corda nel piano
  lon/lat, solo con maggioranti certificati (nessun problema inverso,
  nessun azimut: `GeographicLib` arrotonda a zero le latitudini minime e
  su un lato quasi equatoriale di 179° perdeva un vertice a 11,8 m
  dall'equatore). La lunghezza della geodetica è al più `S = a / sqrt(1 -
  e^2) |dphi| + a |dlambda|` (la curva lineare in lon/lat non è più corta
  della geodetica); la latitudine lungo di essa al più
  `max(|phi1|, |phi2|) + S / (2 a (1 - e^2))`; da questa la curvatura della
  geodetica nel piano è al più `K`, la lunghezza nel piano al più `L`, e
  con `K L <= 1` la geodetica è un grafico sulla corda che se ne scosta al
  più di `l^2 / 8 * K / cos^3(K L)` (`l` la corda);
- **dominio accettato**: `S` al più 1000 km, latitudine maggiorata sotto
  90° e `K L <= 1`, su un ellissoide terrestre (`1 - e^2 >= 0.99`, da cui
  dipende il margine di arrotondamento dei maggioranti); ogni altro lato si
  rifiuta (`InvalidInput`). Poligoni
  catastali, comunali e regionali non ne sono toccati; lo sono lati di
  centinaia di chilometri (confini semplificati di stati) e lati a pochi
  chilometri da un polo;
- due lati senza estremi comuni devono avere le corde più lontane della
  somma dei loro scarti; di due lati con un estremo comune, l'altro estremo
  di ciascuno deve distare dalla corda dell'altro più del suo scarto (due
  geodetiche minime uscenti dallo stesso punto non si incontrano di nuovo
  prima di un estremo);
- allora gli anelli geodetici sono semplici, si toccano solo nei vertici
  comuni e i vertici non comuni restano fuori dai tubi degli altri anelli:
  contenimenti e disgiunzioni sono quelli del piano. Il segno dell'area di
  ogni anello conferma il verso.

**Ambito.** Le cinque misure sopra; l'azimut dove non è definito (punti
coincidenti, partenza su un polo, geodetica più breve non unica) si
rifiuta; l'area rifiuta un lato di almeno 180 gradi di longitudine e ogni
poligono che non passa la verifica di topologia (schede delle
operazioni).

**Hazard.** La verifica è prudente: rifiuta poligoni corretti con lati
lunghi vicini ad altri anelli (a 45° di latitudine, lati di 1 km con anelli
a meno di circa 3 cm; lati di 100 m sotto il millimetro), lati oltre
qualche centinaio di chilometri e lati vicino ai poli. Fino alla versione
di catalogo precedente (distanza e azimut 1, lunghezza e area 2) ogni
misura usava WGS 84 qualunque fosse il datum (circa 4 m ogni 100 km su
ED50) e l'area non verificava la topologia delle geodetiche, senza errore.

**Condizione di rientro.** Una verifica esatta della topologia delle
geodetiche (intersezioni di geodetiche, contenimento sul globo) al posto
del maggiorante, per accettare i poligoni che oggi si rifiutano per
prudenza.

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
  le coppie di rettangoli che si toccano sono già quadratiche. Nemmeno una
  griglia di circa una cella per segmento, misurata in alternanza nello
  stesso processo: 58 contro 91 µs sui lati frastagliati di una copertura da
  1.000 vertici (circa 40 confronti per segmento nella scansione), ma 432-470
  contro 269-324 µs su un cerchio da 10.000 vertici e più lenta sotto i 100
  (sulle booleane del laboratorio a 10.000 righe da 1.000 vertici, uscite
  frastagliate, dal 5 al 21% di tempo in meno); il noding di `polygonize`
  sulla stessa griglia non ha cambiato i tempi di `split`. Una soglia sulla
  stima dei confronti della scansione non ha separato i casi senza costo: non
  è stata tenuta. Serve una
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
`assert_foreign_key` e dell'indice di `pivot`). L'uguaglianza delle chiavi si decide
sempre sui valori o sui byte: l'hash sceglie i candidati, mai il risultato,
e nessun output dipende dall'ordine di visita di una mappa (le mappe si
interrogano per chiave; dove si visitano, il risultato si ordina o si
riduce con operazioni commutative, e `fuzzy_join` sceglie il blocco peggiore
con uno spareggio sulla chiave).

**Ambito.** `plenora-kernels-table`: raggruppamenti, join, finestre,
set operation, qualità, `fuzzy_join`.

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
`assert_unique` e `table_diff` non contabilizzano le proprie strutture di
chiavi (arena, indici, gruppi) su `max_governed_memory_bytes`: nel runner
le prevede il modello di costo del passo, che il budget controlla prima di
eseguirlo, senza contarle.

**Ambito.** I kernel elencati.

**Hazard.** Con molte chiavi distinte il picco reale supera la stima
dell'input: l'arena delle chiavi, due `usize` e una voce di mappa per chiave
distinta, fino a due `usize` per riga per l'assegnazione ai gruppi. Nel
runner il modello di costo ([«Budget di memoria»](#budget-di-memoria))
prevede queste strutture sul caso peggiore misurato (fixture `distinct`,
chiavi tutte distinte), senza contarle.

**Condizione di rientro.** Contabilità esplicita delle strutture di chiavi,
con errore `ResourceLimit` oltre il budget.

### Letterali JSON oltre `u64`

**Regola.** I numeri della config che decidono (estremi di
`table.assert_range`, bordi espliciti di `table.bin`, `tolerance` di
`table.asof_join`, valori di `table.filter` e `table.conditional`) si
leggono esatti: un intero JSON resta un intero `i64` o `u64`, un decimale
resta un decimale esatto (`NumeroConfig`, `NumericBound::parse`). Il
double serve solo dove il contratto è un double. `serde_json` è compilato
senza `arbitrary_precision`, quindi un intero oltre `i64`/`u64` o un
decimale oltre la precisione del double arriva già arrotondato:
`Pipeline::from_json` rifiuta (`InvalidPlan`) ogni numero del piano il cui
valore decimale esatto non è quello del double letto
(`plenora_core::json::ensure_numbers_exact`): `9007199254740993.0` e
`0.30000000000000001` si rifiutano, `0.1`, `2.5` e `1e3` passano. Il
valore letto si ricostruisce esatto dalla forma che `serde_json` riscrive,
esponente compreso (`NumericBound::parse`: `1e-7` è un decimale, non un
double): un intero oltre `u64` come `100000000000000000000` passa il
controllo perché il suo double lo rappresenta, e torna l'intero esatto.
Un numero finito che la forma esatta non tiene (più di 38 cifre
significative, o una scala oltre i 127 decimali: `1e-128`, `1e300`) non
ricade su un double: si rifiuta, nella config (`NumeroConfig`), nei valori
di `table.filter` e `table.validate_rules`, nei letterali di
`table.expression` e nelle celle di testo lette per decidere
(`scalar_as_numero`). Restano double solo `inf` e `NaN`, scritti così.

**Ambito.** I piani letti da `Pipeline::from_json` (runner e piani da
file).

**Hazard.** Chi chiama i kernel direttamente con una config già
deserializzata (`serde_json::Value`, struct) non passa dal controllo: il
testo del numero è già perso, e un numero arrotondato dal suo lettore JSON
arriva come double. Un letterale scritto come **stringa** passa da
`NumericBound::parse` e resta esatto dove l'operazione accetta stringhe
(`table.filter`, `table.conditional`).

**Condizione di rientro.** `arbitrary_precision` di `serde_json`, con il
testo del numero conservato fino al parse esatto anche per l'API diretta.

### Nomi delle colonne d'uscita

**Regola.** Due regole, dichiarate e uguali in analisi e nel kernel:

- le colonne che un passo **produce insieme** hanno nomi tutti distinti
  (`verifica_nomi_distinti`): chiavi e aggregazioni di `table.aggregate`
  (compresa la colonna `count` implicita), colonne indice e pivot di
  `table.pivot`, colonne di `table.transpose` (anche quando il nome viene
  dai dati di `id_column`), parti di `table.split_column`. Un nome ripetuto
  è `InvalidPlan`; prima una colonna sostituiva l'altra e spariva senza
  errore;
- un passo che **aggiunge** una colonna all'ingresso (`window_function`,
  `rolling_window`, `formula`, `conditional`, `concat_columns`,
  `split_column`, `bin`, `statistics`, `add_row_number`, …) sostituisce al
  suo posto una colonna dell'ingresso con lo stesso nome: è una
  sostituzione voluta, scritta nella scheda di ogni operazione.

`table.melt` risolve le collisioni dei suoi due nomi con un suffisso
(`resolve_melt_names`), come dice la sua scheda.

**Ambito.** Le operazioni tabellari elencate.

**Hazard.** Con la seconda regola un nome d'uscita scritto per sbaglio
uguale a una colonna d'ingresso la sostituisce: il contratto dichiarato lo
mostra (la colonna cambia tipo o identità), ma nessun errore lo segnala.

**Condizione di rientro.** Un parametro esplicito di sovrascrittura per
ogni operazione che aggiunge colonne, con errore in sua assenza.

### Somme intere esatte e tipi delle riduzioni

**Regola.** Una somma di interi non passa da `f64`. Sul dominio intero
(`int64`, `uint64`; `date32` in giorni, `date64` in millisecondi e
`timestamp` di ogni unità nel suo valore nativo per media, dispersione ed
estremi) `sum` di `table.aggregate`, `table.pivot`,
`table.rolling_window`, `table.statistics` e `cumsum` di
`table.window_function` sommano in `i128` ed escono `int64`; una somma
fuori dalla gamma di `int64` è `DataMapping`, mai un valore saturato o
arrotondato. Una somma di date o istanti (`date32`, `date64`, `timestamp`)
non ha un significato e si rifiuta in validazione (`InvalidPlan`); la loro media
resta, come istante in `float64` nell'unità della colonna.
La varianza (e la deviazione) sul dominio intero si calcola dagli scarti
esatti `(n x - S) / n` (`float64_source::varianza_intera`): valori uguali
oltre `2^53` hanno varianza zero. La somma di un gruppo senza valori
resta null (semantica SQL). Le riduzioni che scelgono una cella la rendono
nel tipo d'ingresso: `first`/`last` di `aggregate` e `pivot`, `lag`/`lead`
di `window_function`, e `min`/`max` sugli interi (date e istanti compresi)
e su `decimal128` (scelti sul valore esatto). Il tipo d'uscita di ogni riduzione ha un'autorità sola
per kernel e analisi (`float64_source::tipo_somma`, `tipo_estremo` e i
`tipo_uscita*` delle operazioni).

**Ambito.** `plenora-kernels-table`: le operazioni elencate.

**Hazard.** Restano `float64` per contratto, con l'arrotondamento
dichiarato:

- media, varianza, deviazione e `pct_change` sul dominio intero: partono
  dalla somma, dagli scarti o dalla differenza esatta e arrotondano al
  double alla fine; i valori interpolati dei quantili usano i valori
  arrotondati oltre `2^53`. Non coincidono con il vecchio calcolo in `f64`
  quando le somme parziali passano `2^53` (`[2^53, 1, -2^53]` ha media 1/3,
  il `f64` sequenziale dava 0): il valore nuovo è quello corretto;
- ogni riduzione numerica su `decimal128` e sul testo numerico (`sum`
  compresa): la cella si legge come il double più vicino;
- l'aritmetica di `table.formula` e `table.expression`, anche fra colonne
  `int64` (schede delle due operazioni).

**Condizione di rientro.** Somme `decimal128` esatte (`decimal128(38, s)`
in uscita) e aritmetica intera nelle formule, con errore di gamma.

### Colonne temporali e formati di data

**Regola.** Le operazioni su date (`table.type_cast` verso `date`,
`datetime`, `date32`, `timestamp_millis` e testo; `table.date_extract`,
`table.date_format`, `table.date_add`, `table.date_diff`,
`table.timezone_convert`) leggono una colonna temporale (`date32`;
`timestamp` in secondi, millisecondi, microsecondi o nanosecondi, con o
senza fuso) dal valore nativo, senza passare dal testo
(`crates/plenora-kernels-table/src/temporale.rs`): l'ora locale del fuso
della colonna (senza fuso, il valore com'è) e l'istante. Un formato di
lettura scritto per una colonna temporale si rifiuta. Un testo si legge
con il formato dichiarato o, senza, con i soli formati ISO 8601 (RFC 3339
con offset, data e ora con `T` o spazio e frazione, data sola): giorno e
mese non si indovinano mai, e `01/02/2024` si legge solo con un formato
esplicito. Un offset letto dà l'istante (`timestamp_millis`, `date_diff`,
`timezone_convert`); per `date`, `datetime` e le parti vale l'ora scritta.
Nessuna frazione di secondo si tronca in silenzio: `datetime` la scrive,
`timestamp_millis` rifiuta la riga se è sotto il millisecondo, e un testo
con cifre significative oltre il nanosecondo si rifiuta prima della
lettura (chrono le scarterebbe). Un secondo intercalare (`23:59:60`) si
rifiuta: un istante Arrow/POSIX non lo rappresenta, e chrono lo farebbe
cadere sul secondo dopo. `%s` è sempre un istante UTC. L'ora locale di un
istante si calcola con l'offset in aritmetica controllata: oltre
l'intervallo di chrono è un errore, non un panico.

Fuori da queste operazioni una colonna temporale si legge dal valore
nativo, in ogni unità e con o senza fuso
(`crates/plenora-kernels-table/src/interi_temporali.rs`); nessuna
conversione verso un'unità comune, che renderebbe uguali due microsecondi
dello stesso millisecondo:

- **chiavi d'identità** (raggruppamenti, `distinct`, `nunique`, join,
  indici di `pivot`, partizioni di `window_function`, `rolling_window`,
  `statistics`, `sample`, `add_row_number`, `assert_unique`,
  `assert_foreign_key`, `reconcile`, `table_diff`; `scalar_key_string`):
  un `timestamp` vale il suo istante, in nanosecondi dall'epoca
  (`frammento_chiave`, a larghezza fissa e in ordine cronologico), mai il
  suo testo; i gruppi di `aggregate` e `pivot` con una chiave `timestamp`
  escono quindi in ordine cronologico;
- **profilo testuale** (`text_convertible`, `scalar_as_string`: testo
  scritto, hash, confronti di uguaglianza con un testo della config): un
  `timestamp` si scrive in RFC 3339 nel fuso della colonna con tutte le
  cifre frazionarie che servono (nessuna, 3, 6 o 9), quindi due istanti
  distinti hanno testi distinti e lo stesso istante ha lo stesso testo in
  ogni unità. Un offset con i secondi (l'ora media locale di molti fusi
  prima dei fusi standard: `America/Anchorage` fino al 1900, `-09:59:36`)
  non ha forma RFC 3339: chrono lo arrotonderebbe al minuto lasciando
  l'ora locale esatta, e due istanti a 24 secondi avrebbero lo stesso
  testo; la cella si rifiuta (`Schema`). Lo stesso vale per un
  `output_format` di `timezone_convert` che scrive l'offset più grosso di
  quanto sia (`%z`, `%:z`, `%+` per un offset ai secondi; `%:::z` per un
  offset non a ore intere): la riga si rifiuta (`DataMapping` con
  diagnostica per riga, `conversion.offset_precision`), mentre `%::z` lo
  scrive esatto. L'offset è della cella, non della config: si controlla in
  esecuzione, mai in validazione su un istante di prova
  (`Australia/Adelaide` con `%:::z` scrive il 1896, a +09:00, e rifiuta il
  2000, a +10:30).
  Un `date64` si scrive `AAAA-MM-GG` solo se allineato al giorno,
  altrimenti la cella si rifiuta (`Schema`);
- **ordinamenti e comparatori** (`compare_cells_typed`: `sort`, `top_n`,
  ranghi, gruppi di `geo.collect`): l'istante, dal valore
  nativo; due unità diverse si confrontano esatte, in nanosecondi `i128`;
- **dominio numerico** (`dominio_numerico`, `scalar_compare`: filtri e
  regole ordinati, statistiche, `bin`, `table.expression`): il valore
  nativo nell'unità della colonna, secondi, milli, micro o nanosecondi
  dall'epoca (`date64` in millisecondi); un'espressione che legge come
  numero colonne temporali di unità diverse (anche `date32` con un
  `timestamp`) si rifiuta in validazione e nel kernel
  (`verifica_domini_temporali`): confrontarle o sottrarle darebbe un
  risultato sbagliato senza errore;
- **celle scelte** (`first`/`last`, `lag`/`lead`, `min`/`max`): la cella
  com'è, nel tipo d'ingresso con unità e fuso; `first`/`last` di
  `aggregate` non passano dal testo e non verificano il fuso
  (`validate_cella_prendibile`; `first`/`last` di `pivot` non hanno mai
  vincolato il tipo);
- **set operation**: il valore nativo (i due lati hanno lo stesso tipo);
- **`date_trunc`** di `table.expression`: ogni unità senza fuso, in
  millisecondi per difetto prima di troncare; il troncamento è almeno al
  secondo, quindi l'uscita `timestamp(ms)` è esatta, e un valore in
  secondi oltre la gamma dei millisecondi è un errore.

**Ambito.** Le operazioni su date elencate; per la lettura nativa fuori da
esse, ogni operazione tabellare e `geo.collect`.

**Hazard.**

- Nel dominio numerico lo stesso estremo vale istanti diversi su unità
  diverse: `{"operator": ">", "value": 1706696430123}` su un
  `timestamp(us)` è un microsecondo del 1970, non un millisecondo del
  2024. Il piano scrive l'estremo nell'unità della colonna; la conversione
  esplicita resta `table.type_cast` verso `timestamp_millis`, esatta o
  rifiutata riga per riga;
- un `timestamp(ns)` reale (circa `1.7e18`) supera sempre `2^53`: dove il
  contratto passa da `float64` (aritmetica di `table.expression` e
  `table.formula`, medie, quantili, bordi di `bin`, `pct_change`) il valore
  arrotonda a passi di qualche centinaio di nanosecondi (il limite
  generico degli interi oltre `2^53`, «Somme intere esatte e tipi delle
  riduzioni»); confronti, chiavi, estremi e celle scelte restano esatti;
- `table.type_cast` da `date64` verso un tipo numerico si valida e si
  rifiuta riga per riga (il testo `AAAA-MM-GG` non è un numero), mentre da
  `date32` si rifiuta in validazione;
- le chiavi di join e le colonne delle set operation hanno lo stesso tipo
  Arrow sui due lati, unità e fuso compresi: lo stesso istante in unità
  diverse non si abbina, e il piano si rifiuta in validazione;
- `date_trunc` su un `timestamp` con fuso resta rifiutato; le operazioni
  su date leggono un `date64` come il suo testo `AAAA-MM-GG` (allineato al
  giorno, o la riga si rifiuta), non come colonna temporale nativa;
- `date_add` sposta l'ora locale della colonna, non l'istante: con unità
  orarie su un `timestamp` con fuso può scrivere un'ora locale che nel
  cambio d'ora non esiste, senza errore;
- un `timestamp` senza fuso vale come istante UTC in `date_diff`, come ora
  locale di `source_timezone` in `timezone_convert` (la semantica
  dichiarata dell'operazione);
- un fuso Arrow a offset fisso (`+01:00`) non è un nome IANA: la colonna
  si rifiuta con `Schema` nelle operazioni su date e nel profilo testuale;
  comparatori, dominio numerico e celle scelte non leggono il fuso;
- `%Z` in un formato di lettura si legge ma non dà un offset: il nome del
  fuso non conta.

**Condizione di rientro.** Fusi a offset fisso nel profilo testuale,
`date64` letto nativamente dalle operazioni su date, `date_trunc` con fuso.

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
- la validazione interna dei kernel (ingressi di `split`, facce di
  `polygonize`, uscite di `make_valid`) usava `check_validation` e
  `validation_errors` di `geo`, quadratiche: ora la validazione rapida della
  voce precedente, stesso verdetto e stessi errori (oracolo di
  `validazione_ogc`, esteso a `errori_del_poligono`).

**Prestazioni, a risultato identico.** Misurate in release su Windows
(laboratorio, mediana di 5, `split` a un thread come nel runner): `split`
di 1.000 celle da 1.000 vertici con una lama da 74,8 a 9,3 s, con lame a
zig-zag da 235 a 57 s (e 1.000 celle da 100 vertici da 18,3 a 4,3 s);
`make_valid` di 1.000 stelle invalide da 1.000 vertici da 29,1 a 4,7 s; a
10.000 righe da 1.000 vertici, prima oltre i 300 s della campagna, 106 s,
829 s (macchina carica) e 46 s. Da dove:

- la validazione rapida al posto di quella di `geo` (sopra);
- il genitore di una faccia si cerca col punto interno di `geo` (che
  interseca tutti i lati e chiama `relate`) solo se un'altra faccia d'area
  maggiore ne contiene il rettangolo, condizione necessaria perché due
  anelli del grafo nodato non si attraversano; i punti interni
  dell'atomizzazione si calcolano solo quando servono (oracolo
  `scorciatoie_uguali_al_percorso_generico` contro il percorso generico);
- la copertura del bordo di `split` prova la distanza solo sui lati della
  sorgente il cui rettangolo allargato di `2 p + 16 ulp(M)` tocca il lato
  delle parti (un R-tree), condizione necessaria per la distanza entro `p`
  calcolata in `f64` (oracolo `copertura_con_indice_uguale_al_doppio_ciclo`);
- le parti codificate di `split` vanno subito nel buffer contiguo della
  colonna d'uscita (niente `Vec` per parte, niente seconda copia): sulle lame
  a zig-zag 1.000 celle da 100 vertici da 37 a 16 MiB, 10.000 da 1.000
  vertici 1,9 GB; oltre i 2 GiB di una colonna `Binary` un `ResourceLimit`
  esplicito invece del panico del builder.

Le righe di `split` restano una alla volta, in ordine: il calcolo in
parallelo provato in questo ciclo aggiungeva un transitorio proporzionale ai
thread e il limite `max_output_rows` arrivava dopo un blocco di righe.

Un panico di `interior_point` di `geo` su una faccia il cui punto interno
non serve non si verifica più: è l'unica differenza osservabile.

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
(146 operazioni, 75 geo) con lo stesso contratto pubblico di
`plenora-data-tools@190c493`: id, alias legacy, parametri, schema di output,
colonna `__class` (`polygon`, `cut_edge`, `dangle`, `invalid_ring`),
`__parent_index` di `split`, nomi delle varianti d'errore e attribuzione del
passo (`InvalidPlan`, `Internal` per ciò che è interno; dal ciclo dei
difetti geo i limiti di lavoro e d'uscita superati sono `ResourceLimit`,
[«Operazioni geo»](#operazioni-geo)). Nel descrittore
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

## Errori

Ogni errore è un `PlenoraError` con i quattro assi del contratto
`plenora-error-v1` (`plenora-contracts`, `specs/errors/ERRORS-1.0.md`):
categoria, fase, effetto sul supporto e ritentativo. Le enumerazioni sono
quelle dello schema (`concurrent_modification` e il ritentativo
`quarantine` ci sono anche se nessun errore del workspace li produce).
`PlenoraError::public_projection` dà il documento pubblico (`PublicError`,
serializzabile con serde), valido contro `schemas/error-v1.schema.json`
(test `crates/plenora-core/tests/errore_pubblico_schema.rs`, con gli schemi
copiati da `plenora-contracts@ade868c` e verificati per SHA-256):

- `message` è il testo dell'errore, già senza valori di righe o colonne,
  troncato a 2048 caratteri e mai vuoto. Per un errore di I/O il testo del
  sistema operativo (che può portare percorsi completi) resta nel solo
  `Display` locale: il messaggio pubblico è un testo fisso per
  `ErrorKind` con i soli contesti nostri davanti («io error: output `b`:
  entity not found», `PlenoraError::io_con_contesto`);
- `code`, dove l'errore ne ha uno stabile: `EXECUTION_DEADLINE_EXCEEDED`
  (`Timeout`), `EXECUTION_CANCELLED` (`Cancelled`), e il codice di
  `CrsError` (`CrsError::code`, `match` esaustivo) per un errore CRS nato
  da un `CrsError` (`PlenoraError::CrsCoded`, codice tipizzato, mai letto
  dal messaggio; `CodiceCrs` si ottiene solo da `CrsError::code`, quindi
  un codice fuori dal pattern dello schema non si costruisce). Un
  `PlenoraError::Crs` di solo testo non ha codice;
- `details.row_diagnostics`: il documento `plenora-row-diagnostics-v1`
  intero, se l'errore ha una diagnostica per riga;
- `details` oltre i limiti ERR-011/ERR-012 (byte, profondità, 128
  proprietà o elementi, stringhe da 4096 byte, 2048 nodi) non si tronca: la
  proiezione diventa un errore `internal` con codice
  `ERROR_DETAILS_NOT_PUBLISHABLE`, senza `details`. Con i limiti di esempi
  dei kernel (10) non scatta;
- effetto `unknown` con un ritentativo automatico diventa
  `requires_recovery` (ERR-006); oggi nessun errore ha effetto `unknown`.

`provider` ed `execution_id` non ci sono: nessun errore del workspace ne
ha uno.

Gli errori di I/O si classificano per `ErrorKind`: `TimedOut` →
`timeout`, `Interrupted`/`WouldBlock`/`ResourceBusy` → `transient`, tutti
ritentabili (`safe`); `NotFound` → `not_found`, `PermissionDenied` →
`authorization`, `AlreadyExists` → `conflict`, spazio, quota, file troppo
grande o memoria → `resource_limit`, `Unsupported` → `unsupported`, il resto
→ `io`, tutti `never`. Un `ErrorKind` non classificato è `io` e `never`, la
scelta prudente. Un errore di serde_json che nasce dall'I/O della lettura
resta `Io` con il suo `ErrorKind`; uno di sintassi, di dati o di fine
inattesa è `data_mapping` con il solo genere e la posizione («json error:
dati alla riga 1 colonna 15»), mai il testo che cita il valore letto.

### Effetto di un errore a metà della scrittura

`remote_effect` è `none` per costruzione (ogni file d'uscita è scritto in
modo atomico), tranne dove un confine dichiara di più con
`PlenoraError::with_remote_effect`: oggi solo `esegui_da_file`, che scrive
gli output uno alla volta e marca `partial` un errore dopo il primo output
scritto (i precedenti restano), e `committed` un'interruzione vista dopo
l'ultimo (`esegui_da_file_interrompibile`). Con un effetto già visibile un ritentativo
automatico (`safe`, `after`, `requires_idempotency_key`) diventa
`requires_recovery`; una causa che non si ritenta mai resta `never`. Il
primo effetto dichiarato vince anche sotto i wrapper di fase e di
diagnostica.

La scrittura atomica cancella il temporaneo di una scrittura fallita e ne
controlla l'esito: se la cancellazione fallisce l'errore ha effetto
`unknown` (e con una causa ritentabile `requires_recovery`), anche sopra un
effetto che la causa dichiarava già (`PlenoraError::override_remote_effect`,
l'unico punto in cui un effetto si sostituisce).

- **Temporaneo rimasto dopo una scrittura fallita.**
  *Regola*: un errore di `scrivi_atomico` non lascia nulla alla
  destinazione e cancella il temporaneo `.plenora-io-*.tmp` accanto a lei.
  *Ambito*: `plenora_io::atomico::scrivi_atomico` (ogni scrittura di
  `plenora-io`).
  *Hazard*: se la cancellazione fallisce (permessi cambiati, file bloccato
  da un altro processo), o se il processo muore prima di cancellarlo, il
  temporaneo con i dati scritti fin lì resta nella directory della
  destinazione. Nel primo caso l'errore dice `unknown`; nel secondo non c'è
  errore da leggere.
  *Rientro*: una pulizia dei `.plenora-io-*.tmp` orfani all'avvio, o
  temporanei fuori dalla directory della destinazione dove la rinomina
  atomica lo permette.

## Runner

`plenora-pipeline` concatena le operazioni **tabellari** e **geo** del
catalogo su tabelle intere in memoria: un `RecordBatch` per nome, niente
streaming ([«Operazioni geo»](#operazioni-geo) per quelle supportate).

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
  `max_string_bytes`, `max_regex_bytes`), poi `Limits::validate`. Gli altri
  limiti non sono dichiarabili, perché il runner non li applica: anche
  `max_temp_bytes` e `spill_partitions`, che esistevano finché il runner
  scriveva su disco, oggi sono campi sconosciuti e il piano si rifiuta
  (`InvalidPlan`).

### Validazione

**Regola**: ciò che schemi, config e limiti rendono prevedibile fallisce in
`validate`, mai dopo che qualche passo ha girato.

Prima di qualunque esecuzione, contro gli schemi degli input: nomi SSA;
limiti di complessità del piano (`PlanLimits::default()`: passi, input,
archi, fan-out, profondità, byte di config per passo, lunghezza dei nomi,
byte del testo JSON); operazione, arietà e dispatch (`table.concat` a più
di due input e le operazioni senza dispatch sono `Unsupported`); config
tipizzate una volta; contratti di output passo per passo con
`analyze_table_contract` e i limiti con cui i kernel eseguiranno, o con
`analyze_geo_contract` e il CRS di piano per le geo, un solo
`FieldAllocator`, base degli indici della diagnostica per riga di ogni
passo ([«Diagnostica per riga»](#diagnostica-per-riga)); colonne di ogni
input e di ogni contratto contro `max_columns`. Ogni contratto (input e
uscite dei passi) porta lo schema che il runner emette, con il blocco
canonico delle geometrie (`arrow_schema_from_contract`): il passo
seguente si analizza sullo schema delle tabelle che riceverà, e le
tabelle d'ingresso ricevono quello schema in `run` (stesse colonne, solo
metadati in più; per una tabella senza geometrie nulla cambia). Lo schema
di ogni output, quello che esce dal runner, è a parte: versione del
contratto e identità dei campi anche senza geometrie
([«Metadati Arrow»](#metadati-arrow)).
`table.transpose` e `table.pivot` senza `mapping` si rifiutano: il loro
schema d'uscita dipende dai dati. Con `mapping` (valore pivot come testo →
nome di colonna) lo schema lo fissa la config, e il kernel lo rispetta per
contratto: le colonne indice, poi una colonna per voce del mapping,
nell'ordine delle chiavi, anche per un valore che i dati non contengono
(colonna tutta null); i valori fuori dal mapping non danno colonne, ma le
loro righe contano per le chiavi indice, che restano tutte (con celle
null). Il tipo della colonna viene dall'aggregazione: `Float64` per
`sum`/`mean`/`min`/`max`, `Int64` per `count`, `Utf8` per `concat`, il
tipo del valore per `first` (il default) e `last`. Si rifiutano, in
analisi e nel kernel (`Pivot::verifica_mapping`): nomi di output vuoti,
ripetuti o uguali a una colonna indice; colonne indice ripetute; chiavi che
nessun valore potrebbe incontrare, perche' il valore si confronta con la
chiave come testo: con una `pivot_col` `Int64` o `UInt64` la chiave deve
essere la forma canonica dell'intero (`"1"`, non `"01"` ne' `"1.0"`), e un
mapping su una `pivot_col` che non sia testo o intero (float, date,
timestamp, decimali, booleani) si rifiuta, invece di dare in silenzio una
colonna tutta null. Senza `mapping`, un valore pivot che si chiama come una
colonna indice e' un errore del kernel.

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
`extract_all` di `string_extract` con gruppi con nome; `ignore_index` di
`concat` con ogni valore; `n` di `sample` con `fraction`, `random_state`
senza strati su un campione sempre vuoto; `null_literal` di `md5_hash` e
`sha256_hash` fuori da `null_policy=literal`; `invalid` e `ambiguous`
delle operazioni sulle date con ogni valore; `value` di `filter` e
`conditional` con `isnull`/`notnull`; `errors` di `type_cast` su `str`,
`binary_utf8`, `dictionary_utf8`; `separator` di `concat_columns` con una
colonna e di `table_diff` con una sola colonna in `compare_columns`;
`delimiter` di `split_column` con una colonna d'uscita e `max_splits` che
non riduce le parti; `width` 0 di `string_pad`; `n` 0 di `top_n` e
`offset` di `limit` con `n` 0; `min_rows` 0 di `assert_cardinality`;
`tolerance` 0 di `asof_join` con `allow_exact=false`; `columns` vuoto di
`drop_columns`, `renames` vuoto e `rename` di una colonna su se stessa,
`reorder_columns` che non sposta niente; `on_division_by_zero` di `formula`
ed `expression` senza divisioni; i nomi d'uscita che farebbero sparire una
colonna scritta prima: aggregazioni con lo stesso nome o con il nome di una
chiave di `aggregate`, statistiche e parti ripetute di `statistics` e
`date_extract`, due voci di `mask_data` sulla stessa colonna senza
`overwrite`; una voce vuota nell'`index_col` di `pivot`; un campo
sconosciuto dentro un nodo di `expression`; un `null` esplicito per un
parametro facoltativo, che si omette invece di scriverlo `null`). Si
rifiuta solo ciò che la config da sola rende senza effetto con ogni
ingresso: un parametro che non ha effetto soltanto su certe tabelle si
accetta, perché lo stesso piano deve girare su tabelle diverse (`default`
di `align_schema` su una colonna che esiste, `keep_extra` senza colonne non
dichiarate, `drop_columns` e `rename` di colonne assenti, `alphabetical`
con al più una colonna restante, `type_policy` su colonne omogenee,
`separator` di `table_diff` con le colonne ricavate dagli schemi). Un
parametro assente prende il suo default; uno scritto e senza effetto si
rifiuta, nell'analisi e nel kernel, con la stessa funzione
(`verifica_parametri`, `verifica_offset`, `verifica_ascending`,
`verifica_gruppi_con_nome`, `verifica_politiche`, `verifica_valore`,
`verifica_null_literal`, `verifica_separatore`, `verifica_colonne`,
`verifica_parti`, `verifica_risultati`, `nomi_uscita`; il `null` lo rifiuta
la deserializzazione, `mai_null`); il censimento di ogni campo di ogni
config tabellare è in `crates/plenora-pipeline/tests/censimento_parametri.rs`
e la parità analisi–kernel di ogni regola in
`crates/plenora-pipeline/tests/parametri_senza_effetto.rs`. Le
asserzioni vacue (`assert_not_null`, `assert_unique`, `assert_schema` senza
colonne, `assert_range` senza estremi, `assert_cardinality` senza vincoli,
`assert_metadata` senza chiavi, `conditional` senza condizioni, `sha256_hash`
e `stable_fingerprint` senza colonne); `melt` con variabile e valore
omonimi, `rename` con una sorgente ripetuta, `explode` con
`empty_policy=drop`; formati di data vuoti; `flatten_json` oltre
`max_columns`; `amount` di `date_add` che nessuna data sopporta, secondo
intercalare dell'ultimo giorno compreso (`dates::verifica_amount`); nomi
delle regole di `validate_rules` oltre 1024 byte; in `expression`, arietà
delle funzioni, pattern letterali di `regex_replace` (sintassi e
`max_regex_bytes`), testi letterali oltre `max_string_bytes` (anche nelle
liste di `in`), divisori
letterali zero e indici letterali negativi di `substring`, questi ultimi
solo dove la valutazione li guarderebbe (nessun argomento che li precede, o
la sostituzione, solo null).

**Testi e regex contro i limiti.** I testi e i pattern della config
(separatori, formati, valori sostitutivi, valori di `lookup`, `fill_na`,
`conditional`, `bin`, `align_schema`, pattern di `replace`, `assert_regex`,
`validate_rules`, `string_extract` ed `expression`) si confrontano con
`max_string_bytes` e `max_regex_bytes` in analisi. I testi che crescono con
i dati li controlla il kernel, con `ResourceLimit`, prima di pubblicarli:
`replace` con regex, `concat_columns`, `string_pad`, `text_normalize`,
`melt`, `transpose`, i testi calcolati da `expression` e `formula`, `concat`
di `aggregate` e `pivot`, `_diff_columns` e `_diff_old_values` di
`table_diff`, `extract_all` di `string_extract`, `mask_data`,
`flatten_json`, le etichette automatiche di `bin`; un pattern di
`regex_replace` calcolato dalle colonne si confronta con `max_regex_bytes`
riga per riga, e uno non valido rifiuta la riga
(`evaluation.invalid_regex`) senza il testo dell'errore del crate `regex`,
che riporterebbe il pattern, cioè una cella. Il testo scritto da un formato
di data non cresce con la cella: l'analisi ne limita la lunghezza con la
larghezza massima di ogni campo (i letterali per la loro lunghezza, l'anno 7
byte, il mese 2, il nome del mese 9…) e il kernel non la ricontrolla. I default dei due limiti sono uno solo per il
piano e per i kernel (`plenora_core::limits::DEFAULT_MAX_STRING_BYTES`, 16
MiB, e `DEFAULT_MAX_REGEX_BYTES`, 64 KiB; prima i kernel avevano 4096 byte
per le regex).

Il runner tiene un solo controllo proprio, perché non riguarda la config ma
l'ambiente del processo: la variabile `key_env` di `table.hmac_sha256`
deve esistere, non essere vuota ed essere UTF-8. La legge la stessa
funzione del kernel (`security::carica_chiave_hmac`), con una causa per
ciascuno dei tre rifiuti e senza il nome della variabile né il valore:
prima il runner accettava un valore non UTF-8 che il kernel poi trattava
come variabile assente.

### Esecuzione

Dopo ogni passo l'output del kernel deve avere nomi, tipi e metadati (di
campo e di schema) del contratto inferito (altrimenti `Internal`) e riceve lo schema del contratto; righe
per arco, colonne, nomi ripetuti e fattore di espansione si controllano
sui dati; il fattore, sulla base che il catalogo dichiara per l'operazione
(`expansion_constraint`), non per quelle che il catalogo ne esenta (righe
da tutto l'ingresso, come `polygonize`, o in numero fisso, come `dissolve`
e `reconcile`) né per `melt`, le cui righe sono le righe d'ingresso per le
colonne valore, fissate da config e schema: al posto del fattore il runner
verifica che l'uscita abbia esattamente quelle righe
([«Fattore di espansione»](#fattore-di-espansione)). Ogni tabella si libera appena ha girato il suo ultimo
consumatore; un'uscita che nessuno usa si libera subito, un input mai usato
prima del primo passo.

Il resoconto dà per passo operazione, righe in ingresso e in uscita,
picco previsto, margine passato al kernel, byte nuovi dell'output
(allocazioni che nessuna tabella residente raggiungeva prima del passo),
byte vivi con l'output e dopo i rilasci, tabelle liberate, e le righe in cui una divisione di `formula` o
`expression` ha trovato un divisore zero (`righe_divisione_per_zero`, un
conteggio senza valori: [«Divisione per zero»](#divisione-per-zero)).
`byte_vivi` (`plenora_core::memoria`) somma le allocazioni
Arrow delle tabelle residenti una volta ciascuna, per inizio
dell'allocazione e capacità, figli compresi: una slice, una rinomina o le
colonne di un batch letto da Arrow IPC non aggiungono nulla.

### Scadenza e annullamento

`PipelineValidata::run_interrompibile(tabelle, &Interruzione)` è `run`
con una scadenza (`Instant`, assoluta) e un segnale di annullamento
(`Arc<AtomicBool>`, alzato da un altro thread), entrambi facoltativi; `run`
è `run_interrompibile` senza nessuno dei due. Il runner li controlla prima
di ogni passo e prima di consegnare gli output:

- annullamento alzato: `Cancelled` (categoria `cancelled`, codice
  `EXECUTION_CANCELLED`);
- scadenza passata: `Timeout` (categoria `timeout`, codice
  `EXECUTION_DEADLINE_EXCEEDED`, come il vettore `data-run-timeout-error`
  del contratto);
- con entrambi vince l'annullamento.

Il testo dice dove («prima del passo `x` (table.sort)», «prima di
consegnare gli output»), la fase anche: `write` prima di un passo,
`finalize` prima della consegna. Nessun output è reso, quindi effetto `none` e
ritentativo `safe`: rieseguire lo stesso piano è deterministico (se
ritentare dopo un annullamento voluto lo decide il chiamante). Una scadenza
RFC 3339 (`plenora.execution.deadline` del binding di runtime) la converte
in `Instant` chi la riceve. Con una scadenza l'esito (output o `Timeout`) dipende
dal tempo, per natura; gli output resi sono sempre quelli di `run`. Il controllo è fra i passi, mai dentro un
kernel (limite in [«Limiti dichiarati del runner»](#limiti-dichiarati-del-runner)).

### Divisione per zero

**Semantica dichiarata** (decisione dell'utente): in `table.formula` e
`table.expression` una divisione con operandi non null e divisore zero
vale null di default; il piano chiede l'errore con
`"on_division_by_zero": "error"`. Non è un null silenzioso: il kernel conta
le righe in cui è successo e il runner le riporta nel resoconto del passo
(`ReportPasso::righe_divisione_per_zero`), con entrambe le politiche (con
`error` un passo riuscito ne ha zero, perché la prima lo fa fallire con la
diagnostica per riga `evaluation.division_by_zero`).

- In `expression` il null è quello del nodo della divisione e segue le
  regole dei null: `coalesce(a / b, 0)` dà 0, un ramo di `case` non scelto
  non si valuta e non conta; una riga con più divisioni per zero conta una
  volta. In `formula` ogni operatore propaga il null, quindi la riga intera
  diventa null.
- Un divisore letterale zero (`x / 0`, `x / -0.0`) resta un errore di piano
  con ogni politica, in validazione (per `expression` prima lo vedeva solo
  il kernel, in esecuzione).
- `on_division_by_zero` scritto in una formula o un'espressione senza
  divisioni si rifiuta, come ogni parametro senza effetto; un valore diverso
  da `"null"` ed `"error"` si rifiuta dalla config.
- Non ci sono modulo né divisione intera: `/` è l'unica divisione.
  `power(0, -1)` (infinito) e un quoziente che trabocca restano risultati non
  finiti (`evaluation.non_finite_result`), non divisioni per zero, con
  qualunque politica. In `formula` un risultato non finito da operandi
  finiti (overflow, anche intermedio come `a / (a * a)`) rifiuta la riga
  come in `expression`; un `NaN` o un infinito già nella colonna si propaga,
  come in `window_function` e `rolling_window`, che rifiutano anch'esse
  l'overflow da valori finiti; un `result` di `conditional` che si legge
  come numero non finito si rifiuta in validazione.

`table.formula` emette diagnostica per riga solo con `"error"` (la
divisione per zero è il suo unico rifiuto per riga), e il catalogo lo
dichiara (`emits_row_diagnostics`). Semantica 3 per `formula`, 4 per
`expression`.

### Fattore di espansione

`max_expansion_factor` (default 100) si controlla dopo il passo, sulle
righe dell'uscita già costruita: non difende la memoria, che difendono il
budget prima e dopo il passo, i preflight dei kernel con il margine e
`max_rows_per_edge` (10 milioni). È una guardia logica contro
un'espansione che nessun piano sensato chiede: un join molti-a-molti su una
chiave sbagliata, un prodotto cartesiano involontario, liste esplose più
lunghe del previsto.

Un left join reale di un arricchimento 1:N è stato rifiutato a 106 volte:
il vincolo dei join era `MaxRelative`, cioè l'uscita sul lato **minore**.
Quella base misura l'asimmetria dei lati, non l'espansione: un left join
di 10 600 righe su una dimensione di 100 righe con la chiave unica vale
106 volte la destra senza duplicare una riga, e ogni arricchimento su una
tabella con meno dell'1% delle righe dell'altra superava il default. Ora
`table.join`, `table.cross_join`, `table.fuzzy_join`, `geo.sjoin` e
`geo.overlay` misurano sulla **somma** dei lati (`SumRelative`), come le
altre operazioni a due ingressi senza una base propria (`clip` e le
booleane allineate sono `LeftRelative`):

- un abbinamento con la chiave unica su almeno un lato (1:1, 1:N, N:1) vale
  al più 1: l'uscita di un inner join è al più il lato maggiore, quella di
  un left o outer join al più la somma dei lati. Nessun arricchimento
  legittimo si avvicina a 100;
- un molti-a-molti vale `Σ l_k · r_k / (L + R)`: con 300 righe per lato
  sulla stessa chiave, 90 000 righe su 600, cioè 150, e si rifiuta; un
  `cross_join` di L per R righe vale `L·R/(L+R)`, circa il lato minore
  (10 000 per 5 scenari: 5, accettato; prima valeva 10 000).

Il default resta 100: dopo il cambio di base supera 100 solo un'uscita che
moltiplica i dati per più di cento volte la loro somma, e con gli ingressi
oltre 100 000 righe il tetto effettivo è già `max_rows_per_edge`.

**Portata della guardia.** Con la somma come base, un prodotto completo
di L per R righe vale `L·R/(L+R)`, cioè circa il lato minore: con il lato
minore di al più 100 righe il fattore 100 non ferma mai un prodotto
completo (100 per 50 000 righe danno 5 milioni di righe, fattore 99,8,
accettato). Il fattore ferma i molti-a-molti fra lati entrambi grandi; la
memoria e le uscite enormi le fermano il budget (prima del passo, con il
modello di costo, e dopo, con i byte veri), i preflight dei kernel
(`cross_join` e `fuzzy_join` contano le coppie prima di allocare) e i
limiti assoluti di righe (`max_rows_per_edge`, `max_output_rows`). Derivarlo
dal budget non avrebbe senso: quando il fattore si controlla la memoria è
già stata allocata e contata. Un piano che vuole un'espansione maggiore la
dichiara (`limits.max_expansion_factor`). Le operazioni con righe fissate
dalla config ne sono fuori (esenzioni del catalogo, e `melt` con la verifica
esatta sopra). Semantica 2 per i cinque join e per `melt` (un'uscita prima
rifiutata ora si produce).

### Diagnostica per riga

**Regola**: ogni ordine valido dei passi si accetta; un payload
`plenora-row-diagnostics-v1` ha solo indici di riga della sorgente
(`index_basis` `source_row_zero_based`, DIAG-002) e, dove la riga della
sorgente non si conosce, nessun indice (DIAG-003: mai un indice indovinato
o di un'altra base).

Un kernel riporta gli indici delle righe del suo primo ingresso (l'unico
per le unarie, il lato left per `assert_foreign_key`). Se quelle righe
siano della sorgente lo decide la validazione, dal catalogo
(`source_row_provenance`), e `PipelineValidata::base_indici(out)` lo dice
prima di eseguire:

- **`BaseIndici::Sorgente`**: l'ingresso discende da un input del piano
  solo attraverso passi che conservano numero e ordine delle righe
  (`rename`, `type_cast`, `formula`…). L'indice è la riga di quell'input,
  da zero; payload e testo sono quelli del kernel.
- **`BaseIndici::SenzaAttribuzione`**: a monte c'è un passo che filtra,
  riordina, espande, unisce o aggrega (`filter`, `sort`, `limit`, `sample`,
  `distinct`, `join`, `aggregate`, `explode`…). Il runner toglie gli esempi
  e tiene conteggi, cause e totale: la completezza `complete` diventa
  `partial` con il limite di conoscenza `read.row_attribution_unavailable`
  e `examples_truncated` vero (DIAG-007: esempi osservati omessi)
  (`RowDiagnostics::senza_attribuzione`). Il testo del kernel prende
  davanti «passo `<out>`: righe rifiutate nell'ingresso `<nome>` del passo,
  non riconducibili alla sorgente (diagnostica senza esempi)». Per trovare
  le righe si esegue a parte il piano spezzato: il prefisso fino a `<nome>`
  e poi il passo su quell'output come input, che ha gli esempi rispetto a
  `<nome>`. Dopo un'aggregazione o un pivot la riga rifiutata è un gruppo
  nuovo, e una sola riga d'origine può non esistere.

La «sorgente» è la tabella d'ingresso del piano: un piano spezzato in due
riporta gli indici del secondo rispetto ai suoi input, cioè alle uscite del
primo. Gli indici non si ricalcolano mai verso la sorgente attraverso un
passo che cambia le righe: nessuna mappa di righe si tiene in memoria, e
il budget non ha niente in più da contare (limite sotto, «Diagnostica senza
esempi dopo un passo che cambia le righe»).

Fase e scope di un rifiuto per riga: l'errore ha la fase derivata
`write` (`ErrorPhase`: l'esecuzione di un passo, il canone non ha una fase
«execute»), mai `read`, perché nessun kernel legge un supporto; lo `scope`
del payload è `read`, l'unico che il contratto v1 dà a un rifiuto di un
kernel: dice che la riga rifiutata è d'ingresso, non che si stava leggendo
un file.

### Operazioni geo

I passi `geo.*` passano dall'analisi dei kernel (`analyze_geo_contract`) e
dai kernel di `plenora-kernels-geo`, con lo stesso budget e gli stessi
controlli dopo il passo delle tabellari. Un passo geo
è una funzione `RecordBatch` → `RecordBatch` (`plenora_pipeline::geo`,
privato): calcola le colonne del contratto d'uscita, nel suo ordine, e le
monta sul suo schema. Niente fusione, niente streaming.

| forma | operazioni | uscita |
| --- | --- | --- |
| 1:1 in place | `centroid`, `convex_hull`, `envelope`, `boundary`, `point_on_surface`, `buffer`, `simplify`, `affine_transform`, `translate`, `scale`, `rotate`, `concave_hull`, `densify`, `snap_to_grid`, `line_substring`, `line_interpolate_point`, `snap`, `reproject` | la geometria della riga, attributi invariati |
| colonne in coda | `area`, `length`, `perimeter`, `geodesic_line_length`, `geodesic_area`, `vertex_count`, `to_wkt`, `bounds_extractor`, `geometry_accessors`, `line_locate_point`; contro la geometria `other_wkb` della config: `distance`, `hausdorff_distance`, `frechet_distance`, `haversine_distance`, `geodesic_distance`, `bearing`, i predicati `predicate_*` | la misura della riga, null per una geometria null |
| sostituzione | `geometry_diagnostics` | le dieci colonne diagnostiche al posto della geometria |
| produttori | `from_coords`, `from_wkt` | colonna geometria in coda, CRS da `crs` della config o di piano |
| espansioni 1:N | `explode`, `delaunay`, `subdivide`, `split` (lama `other_wkb` su ogni riga) | una riga per parte, attributi della riga madre, `__parent_index`; una geometria null non produce righe (`explode`, `delaunay`, `split`) o una riga null (`subdivide`), come a `190c493` |
| in place, su tutta la tabella | `make_valid`, `voronoi`, `clean_topology` | un risultato per riga non null, null le altre; in `clean_topology` una riga assorbita da una precedente diventa null (la geometria dell'uscita è nullable anche quando quella d'ingresso non lo è) |
| colonna in coda, su tutta la tabella | `cluster_dbscan` | etichetta del cluster, null per il rumore |
| aggregazioni a sole geometrie | `dissolve`, `line_builder`, `polygon_builder` (una riga), `line_merge` (una per linea fusa), `polygonize` (con `__class`), `collect` (una per gruppo, con le colonne chiave) | nessun attributo propagato |
| coperture | `coverage_validate`, `shared_paths` | schema nuovo, una riga per problema o tratto |
| griglia | `generate_grid` | le celle; l'ingresso fa solo da innesco |

Le operazioni su due tabelle (left, right; il catalogo chiede lo stesso
CRS proiettato sui due lati) hanno questa semantica delle righe, quella
di `190c493` (`execute_geo_binary` dell'executor per i join, `pair_arrow`
per ritaglio, overlay e booleane, che il DAG d'origine non eseguiva) e
quella che il contratto dell'analisi dichiara:

| operazioni | righe dell'uscita |
| --- | --- |
| `sjoin` (`predicate`), `nearest` (`max_distance`) | una per coppia trovata: le colonne di left di quella riga, `__right_index` (e `distance`), non nullable come la geometria di left nell'uscita; una riga di left senza coppie (o a geometria null) non compare |
| `within`, `count_points_in_polygons` | allineate a left: la colonna in coda (`within`: la geometria di left è dentro una di right; conteggio dei punti di right in ogni poligono di left), null per una geometria di left null |
| `clip` | allineata a left: ogni geometria ritagliata dall'unione di **tutte** le geometrie di right (la maschera), null dove il ritaglio è vuoto |
| `overlay` (`mode`) | una per pezzo: la geometria e le righe d'origine `__left_index`, `__right_index` (null dove il pezzo non viene da quel lato); nessun attributo |
| `intersection`, `union`, `difference`, `symmetric_difference` | allineate: riga `i` di left con riga `i` di right, **stesse righe richieste** (altrimenti `InvalidPlan`, in esecuzione: le righe non si conoscono a secco); null dove uno dei due è null o il risultato è vuoto |

Per `clip` e le quattro booleane un risultato vuoto è null: l'analisi
dichiara ora la geometria dell'uscita nullable anche quando quella di
left non lo è. Per `sjoin` e `within` il tetto delle coppie è il limite
di righe dell'arco, per `nearest` i confronti sono al più il quadrato del
maggiore fra `max_input_rows` e `max_rows_per_edge` (come nel progetto d'origine).

`collect` ordina i gruppi nell'ordine naturale dei valori delle chiavi,
con il comparatore di `table.sort` (`compare_cells_typed`: numeri per
valore, testo per byte, istanti per istante, null dopo i valori); a
`190c493` era l'ordine di una chiave testuale con la lunghezza in testa
(un valore di 10 caratteri prima di uno di 9). Resta un errore dei dati,
in esecuzione, una chiave di dizionario fuori dal dizionario.

**Config.** Si legge una volta, in validazione, con i tipi dell'analisi
(`plenora_kernels_geo::analyze::config`, pubblici per questo): nessuna
seconda copia di nomi e default. Le geometrie della config (`other_wkb`,
`point_wkb`, `reference_wkb`) si decodificano lì, già accettate
dall'analisi, che per `other_wkb` ora verifica anche la validità OGC come
per le altre due, e il tipo che il kernel chiede (`LineString` per
`frechet_distance`; `Point` per `haversine_distance`,
`geodesic_distance`, `bearing`): un kernel l'avrebbe rifiutata alla prima
riga non null, e su una tabella vuota o tutta null mai. Allo stesso modo
l'analisi di `collect` rifiuta le chiavi `group_by` senza un ordine
naturale (`is_sortable` dei kernel tabellari, come `table.sort`). La
validazione rifiuta esattamente ciò che l'analisi rifiuta (test
`la_validazione_rifiuta_esattamente_cio_che_l_analisi_rifiuta`); in più
solo `Unsupported` per le operazioni senza dispatch.

**Prima del kernel**, per ogni colonna geometria d'ingresso: ogni cella
si decodifica (contratto WKB strutturale) e ogni coordinata deve stare nel
dominio di validità del CRS della colonna (`Crs`, [«CRS
integrati»](#crs-integrati): i kernel non ricevono un CRS); se il
contratto dichiara i tipi geometrici con un elenco (`exact`, o `mixed`
con elenco), ogni cella deve essere di un tipo dichiarato (`Schema`). La
validazione OGC la fa il kernel, una volta per geometria; la decodifica
strutturale del controllo di dominio è quindi una seconda passata sui
byte, il prezzo di un controllo in un posto solo. Le geometrie prodotte
da `from_coords` e `from_wkt` stanno nel dominio del CRS dell'uscita.

**Precisione.** I kernel che la chiedono (`buffer`, `subdivide`, `split`,
`make_valid`, `dissolve`, `polygonize`, `voronoi`, `clean_topology`,
`coverage_validate`)
ricevono 1 cm a terra nelle unità del CRS della colonna
(`Precision::from_crs`, [«Precisione delle operazioni
geografiche»](#precisione-delle-operazioni-geografiche-1-cm-a-terra)),
calcolata in validazione.

**Dopo il kernel.** Una geometria null in una colonna che il contratto
dichiara non nullable è un errore del passo (`InvalidPlan`): per esempio
`point_on_surface` di una geometria vuota, o `from_coords` con una
coordinata null (il contratto dichiara la geometria prodotta non
nullable; a `190c493` diventava null). Ogni geometria prodotta deve avere
un tipo che il contratto d'uscita dichiara, altrimenti `Internal`
(analisi e kernel divergono).

**Limiti passati ai kernel**: il limite di righe dell'arco d'uscita
(`max_output_rows` per un output del piano, `max_rows_per_edge`
altrimenti, come nel progetto d'origine) come tetto delle righe prodotte da
espansioni, `line_merge` e `polygonize`; `MAX_CLEAN_VERTICES` e
`MAX_NODING_WORK` dei kernel; `100_000` punti per `voronoi` senza
`max_points`; `MAX_CELL_COORDINATES` per `concave_hull`, `densify` e
`delaunay`, `10^8` coppie di coordinate per riga per `hausdorff_distance`
e `frechet_distance` (l'ordine di `MAX_NODING_WORK`); una `Int64` di
`from_coords` oltre `2^53` in modulo si rifiuta (non è esatta in `f64`).

**Errori.** Categoria come nei kernel: `Internal` ciò che non ha concluso
o un'invariante violata, `Unsupported` la precisione insufficiente,
`ResourceLimit` un limite di lavoro, d'uscita o di coppie superato dai
dati (la definizione di `PlenoraError::ResourceLimit`: il piano è
corretto, sono i dati a non entrarci; a `190c493` e fino alle versioni di
catalogo precedenti a questo ciclo erano `InvalidPlan`), `InvalidPlan` il
resto (una config illeggibile anche per `geo.reproject`, che rendeva
`InvalidConfiguration`), con il nome dell'operazione e del passo; il testo
è quello dei kernel, senza valori.
Il primo errore è quello della prima riga, in ordine di riga.

**Diagnostica per riga.** Delle geo che il catalogo dichiara con
diagnostica per riga, nel runner la emette solo `from_wkt` (l'adapter dei
kernel), con la base degli indici delle tabellari
([«Diagnostica per riga»](#diagnostica-per-riga)); le altre rendono il
primo errore, senza indici di sorgente (limite sotto).

### Budget di memoria

`limits.max_governed_memory_bytes` del piano è il budget del runner. Tutto
sta in memoria: niente va su disco. Due regole lo tengono:

- **vivibilità**: ogni tabella si libera appena ha girato il suo ultimo
  consumatore ([«Esecuzione»](#esecuzione)), e `byte_vivi` conta esatti i
  byte delle tabelle residenti (`plenora_core::memoria`);
- **rifiuto prima di eseguire**: prima di ogni passo deve valere

```text
byte_vivi(residenti) + picco_previsto(passo) <= budget
```

altrimenti il passo si rifiuta con `ResourceLimit`, con il nome del passo e
dell'operazione e senza valori dei dati, prima che il kernel giri.

**Un passo che non sta nel budget si rifiuta; non c'è ripiego su disco.**
Fino al commit `47623ce` il runner, prima di rifiutare, sfrattava su file
Arrow IPC temporanei le tabelle residenti che il passo non usava e, per
`sort`, `distinct`, `aggregate` e le set operation, passava a una variante
dei kernel che scriveva su disco. È un cambiamento osservabile: un piano
che allora riusciva grazie allo sfratto o alla variante su disco oggi si
rifiuta con `ResourceLimit` prima del passo che non sta (le uscite dei
piani che riuscivano in memoria non cambiano). Il rimedio è un budget più
grande: i dati reali per cui il runner è pensato (il portafoglio più
grande, circa 3,3 milioni di righe in ingresso, ha un picco di circa
0,7 GiB) stanno in memoria.

Il picco previsto, per operazione, è

```text
picco_previsto = S * (a + max(r*R + c*B, r_s*R, c_l*B) + k*K + p*P)
```

dove `R` sono le righe di tutti gli ingressi (per `geo.generate_grid` le
celle d'uscita, note a secco), `B` i byte Arrow degli ingressi, `K = R *`
colonne del contratto d'uscita, `P` righe sinistra per righe destra, e
`S = 1.5`. I coefficienti vengono dalle misure Windows v4
(`PeakWorkingSet64` incrementale, profili wide, narrow, distinct avversario
fino a 5 milioni di righe per le tabellari; per le geo profili
default e avversari su feature per vertici, al livello del runner: colonne
GeoArrow-WKB, decodifica con validazione OGC, kernel, codifica) in
`data/misure/catalogo-memoria-v4.json`, estratte dal catalogo della
campagna con la sua provenienza (commit misurato, date, macchina, carico,
SHA-256 del catalogo d'origine; `python scripts/modello_costi.py --estrai
<catalog-v4.json>`). Li generano `python scripts/genera_costi_operazioni.py`
in `crates/plenora-pipeline/src/costi_operazioni.rs` e `python
scripts/genera_costi_geo.py` in `crates/plenora-pipeline/src/costi_geo.rs`,
con le regole di `scripts/modello_costi.py` (`--verifica` rigenera e
confronta; un test confronta l'impronta delle misure). Il catalogo tiene
anche i profili delle varianti dei kernel che scrivevano su disco
(`spilled_*`): restano nelle misure, ma i generatori non li usano. Per ogni
punto osservato `y = max(stima di budget, byte nuovi dell'output, 0)`:

- **piano** `a + r*R + c*B` (con `k*K` per `table.pivot`, con il solo
  `p*P` per `cross_join` e `fuzzy_join`): fra quelli che coprono **ogni**
  punto di **tutti** i profili in memoria dell'operazione, quello con la
  somma minima dei rapporti previsto/misurato (programma lineare risolto esattamente),
  con `a` non oltre il picco più piccolo misurato (la crescita la portano i
  termini per unità) e `c >= 1` dove l'uscita può essere una copia intera
  degli ingressi anche se le fixture ne tengono una parte (sottoinsiemi di
  righe, join senza espansione delle chiavi, chiavi indice di `pivot`);
- **rami di larghezza**: `r_s` inviluppo `max y/R` della classe di
  larghezza di riga (`B/R`) più stretta, `c_l` inviluppo `max y/B` della
  più larga (le classi sono i profili, e per le geo profilo per vertici per
  geometria), sui picchi interi, senza togliere `a`: il programma lineare
  può spostare in `a` parte di un costo che è per riga, e un inviluppo su
  `y - a` non coprirebbe più le righe strette (controesempio verificato a
  ogni generazione, `verifica_controesempio_rami`). Se il costo vero è
  `r1*R + c1*B` con `r1, c1 >= 0`, per un punto misurato di larghezza `w_i`
  vale `y_i/R_i = r1 + c1*w_i`: su righe più strette il costo è al più
  `(y_i/R_i)*R <= r_s*R`, su righe più larghe al più `(y_i/B_i)*B <=
  c_l*B`. Con una sola classe `max(r_s*R, c_l*B)` è per eccesso a ogni
  larghezza;
- coefficienti in millesimi di byte, per eccesso; un'operazione senza
  modello si rifiuta in validazione (`Unsupported`).

L'oracolo `crates/plenora-pipeline/tests/oracolo_costi.rs` verifica
l'invariante su ogni punto osservato: `a + max(...) + ...` senza `S` non è
sotto il picco misurato, quindi la previsione è almeno una volta e mezza la
misura. Fanno eccezione solo i profili avversari geo esclusi per nome
(limite «Modelli di costo geo»). Sugli 879 punti coperti di almeno 1 MiB il
rapporto previsto/misurato con `S` ha mediana 2,92, novantesimo percentile
15,3 e massimo 119 (`geo.predicate_contains`, profilo default); con i
modelli precedenti (v3 per le tabellari, provvisori per le geo, varianti
su disco comprese) era 6,4, 41,8 e 535, con 30 punti sotto la misura senza
`S`.

Da dove veniva il pessimismo dei modelli v3: `max(r*R, c*B)` con `r` e `c`
presi ciascuno dal profilo peggiore. Il `c` di `join` (5,8 byte per byte)
veniva dal profilo narrow, dove 26 byte per riga di input portano 80-100
byte per riga di tabelle hash e indici: applicato a righe reali da 500 byte
e oltre, trasformava un costo per riga in un costo per byte, circa cinque
volte l'input. Il `r` di `pivot` (745 byte per riga) veniva dal profilo
distinct, 64 colonne pivot e un indice tutto distinto: il costo lo fanno
le celle d'uscita, non le righe, e `k*K` le conta sulle colonne del
contratto d'uscita. Il piano additivo mette il costo per riga in `r`,
quello per byte in `c`, quello per cella in `k`.

Anche lo stato iniziale (gli input residenti) e quello finale (gli output
insieme) sono confini: oltre il budget sono un `ResourceLimit`, anche in un
piano senza passi.

Il kernel riceve come `max_governed_memory_bytes` il margine vero, budget
meno byte vivi, così i suoi preflight usano lo spazio che c'è. Lo stesso
margine va ai passi geo (`esegui_kernel` lo passa a `PassoGeo::esegui`),
che lo danno ai kernel i cui risultati crescono con i dati come
`plenora_kernels_geo::margine::MargineMemoria` (limite «Modelli di costo
geo»).

`B` del modello è il maggiore fra i byte vivi degli input e il costo
di una loro copia (`plenora_core::memoria::byte_dati`: colonne che sono lo
stesso array contano ciascuna, perché i kernel le copiano ciascuna). Dopo
il passo, byte vivi con l'output oltre il budget sono un `ResourceLimit`
esplicito.

### Limiti dichiarati del runner

- **Tabelle intere in memoria**: nessuno streaming, nessun batch parziale,
  niente su disco. Un passo che non sta nel budget si rifiuta prima di
  eseguirlo ([«Budget di memoria»](#budget-di-memoria)); fino al commit
  `47623ce` lo stesso piano poteva riuscire sfrattando tabelle su file
  temporanei o con le varianti su disco dei kernel.
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
  *Ambito*: `PipelineValidata::run`, modelli in
  `crates/plenora-pipeline/src/costi_operazioni.rs` e `costi_geo.rs`.
  *Hazard*:
  - non è un tetto duro sulla memoria del processo: il transitorio dentro
    il kernel è una previsione empirica (misure Windows di
    `PeakWorkingSet64`, fattore 1,5), non una misura. Il modello copre ogni
    punto misurato (oracolo `oracolo_costi.rs`), ma un input fuori dalle
    fixture misurate (più righe di 5 milioni, 1000 per lato per
    `cross_join` e `fuzzy_join`, distribuzioni di chiavi o config diverse
    da quelle misurate: una `type_cast` di tutte le colonne invece di una)
    può superarla senza errore;
  - fuori dalle larghezze di riga misurate per l'operazione (nella maggior
    parte delle fixture tabellari fra 8 e 150 byte per riga d'ingresso; le
    righe reali arrivano a qualche KiB) la previsione è per eccesso solo se
    il costo vero è una somma non negativa di un termine per riga e uno per
    byte, senza costante oltre `a` (rami `r_s` e `c_l`); un costo che cresce più che linearmente con la
    larghezza di una cella non è coperto;
  - la campagna v4 (`data/misure/catalogo-memoria-v4.json`, campo
    `caveat`) è stata fatta su una macchina carica (CPU media 68 % e fino a
    40 processi di build nella campagna tabellare): la memoria è
    affidabile, i tempi no. È misurata a `24698d6`, prima delle parti
    preparate della validazione OGC (`fd3325c`, `2697f72`: una geometria a
    più parti in validazione trattiene qualche centinaio di byte per
    vertice, per ogni cella in decodifica) e del runner geo di F4: quella
    memoria non è nelle misure, la copre solo il fattore 1,5;
  - i punti che il kernel ha rifiutato per i suoi limiti (`reconcile`,
    `concat` e `concat_by_name` wide) non entrano nel modello;
  - `cross_join` e `fuzzy_join` hanno solo il termine per coppia,
    calibrato sulle larghezze di riga misurate: con righe più larghe il
    picco previsto è basso, e la difesa è il preflight dell'output del
    kernel con il margine passato;
  - `pivot` conta le celle d'uscita come righe in ingresso per colonne
    d'uscita (per eccesso: le righe d'uscita sono le chiavi indice
    distinte, al più le righe in ingresso, che a secco non si conoscono).
    Le colonne sono quelle del contratto validato, che il runner accetta
    solo con `mapping` e che il kernel produce esattamente (una per voce,
    anche se i dati non la contengono; l'esecuzione verifica lo schema);
    le misure hanno da 3 a 65 colonne d'uscita, oltre è estrapolazione
    lineare nelle celle;
  - per `date_extract`, `lookup` e `string_length` (solo profilo wide) la
    crescita per riga fra gli ultimi due campioni supera 1,5: il modello
    resta lineare e copre il campione più grande;
  - esclusi: overhead dell'allocatore e strutture Rust; la memoria esterna
    (FFI) contata come la vede `byte_vivi` (limite sopra);
  - per le operazioni che dipendono dai dati (join, `cross_join`,
    `fuzzy_join`, `pivot`, `transpose`, `explode`, `unnest`, `melt`,
    `aggregate`, `dedup_advanced`, finestre, `flatten_json`) il modello
    copre il caso peggiore misurato; l'output lo limitano i preflight dei
    kernel con il margine passato e `max_rows`, e il controllo esatto dopo
    il passo, cioè dopo che è stato allocato. `c >= 1` copre un join senza
    espansione delle chiavi, non uno molti a molti;
  - un rilascio libera memoria solo se nessun altro tiene l'allocazione:
    un clone tenuto dal chiamante la tiene viva, e il resoconto non lo vede.

  *Rientro*: contabilità esplicita delle strutture di chiavi nei kernel
  (limite «Memoria delle chiavi dei kernel in memoria non governata»),
  misure di righe più larghe e dei casi oltre il dominio misurato, una campagna a macchina scarica sul
  `main` corrente, e un allocatore contato per il processo (un tetto vero,
  non una previsione).
- **Modelli di costo geo.**
  *Regola*: ogni operazione geo ha un modello generato dalle misure v4 al
  livello del runner, con le stesse regole delle tabellari; alcuni profili
  avversari ne sono esclusi per nome (`esclusi` in `costi_geo.rs`, motivi
  in `scripts/genera_costi_geo.py`): `buffer` su linee a zig-zag,
  `count_points_in_polygons`, `sjoin` e `within` con ogni coppia
  candidata, `overlay` e `coverage_validate` su sovrapposizioni, `nearest`
  con pareggi.
  *Ambito*: `crates/plenora-pipeline/src/costi_geo.rs`, tutte le `geo.*`.
  *Hazard*: la memoria dei profili esclusi cresce con una grandezza che il
  runner non conosce prima del passo (coppie candidate, pezzi
  d'intersezione, vicini equidistanti, forma della geometria): coprirli
  renderebbe il modello di ordini di grandezza più alto sui profili
  ordinari (per `sjoin` il default fino a 1500 volte la misura). Su quei
  profili la previsione arriva fino a circa 200 volte sotto la misura (mediana
  0,23 con `S`); il `buffer` a zig-zag da 1000 vertici per geometria ha
  misurato 20 GiB su 15 MiB d'ingresso. I punti andati oltre il tempo
  massimo della campagna (`buffer`, `make_valid`, `split`) o
  rifiutati (`line_merge`, `polygonize`, `voronoi`) non sono nel modello.

  **Modelli non rigenerati dopo il buffer a blocchi e la revisione di split.**
  Le misure di questo ciclo (laboratorio della campagna v4 sul codice
  corrente, Windows, 32 thread, mediana di 5 del `PeakWorkingSet64` oltre
  quello di un processo con gli stessi ingressi) non sono nel catalogo e
  i modelli restano quelli v4:
  - il `buffer` a zig-zag ora sta sotto il modello del profilo ordinario
    (1.000 linee da 1.000 vertici: 117 MB contro 398 MB senza `S`; 10.000:
    832 MB contro 3,9 GB), ma resta escluso finché una campagna non ne
    rigenera le misure;
  - `split`, a un thread e con le parti nel buffer contiguo, sta sotto il
    modello senza `S` su ogni punto rimisurato: lame a zig-zag 1.000 righe
    da 100 vertici 17 MB contro 39 MB, 1.000 da 1.000 vertici 115 MB
    contro 386 MB, 10.000 da 1.000 vertici (prima oltre il tempo massimo)
    1,9 GB contro 3,9 GB; la lama singola 10.000 da 1.000 vertici sotto il
    rumore della misura contro 1,9 GB;
  - `make_valid` sulle stelle invalide a 10.000 righe da 1.000 vertici,
    prima oltre il tempo massimo, 2,2 GB contro 2,4 GB senza `S`.

  La dipendenza del transitorio dai thread vale per ogni kernel per riga
  in parallelo: le misure v4 sono a 32 thread, e una macchina con più
  core ne trattiene di più (il buffer nel runner ne ha al più 64 in volo).
  **Guardia di memoria nei kernel: riduce il rischio, non è un tetto.** I
  kernel di questi profili ricevono il margine del passo
  (`plenora_kernels_geo::margine`, budget meno byte vivi) e contano le
  allocazioni **più grandi** del nostro codice, dove crescono con i dati: le
  geometrie decodificate degli ingressi (stimate dalle intestazioni del WKB
  prima di decodificarle), le coppie confermate di `sjoin`, `within` e
  `count_points_in_polygons` (con la riga più larga di left ripetuta
  nell'uscita, mai la media), i vicini equidistanti di `nearest`, le
  coppie candidate e i pezzi di `overlay`, le coppie candidate e le
  sovrapposizioni di `coverage_validate`, la copia di lavoro, i blocchi e
  l'uscita trattenuta del `buffer` (un solo conto per passo, righe a
  blocchi fissi di 64). Quando quelle non entrano nel margine il kernel si
  ferma con un `ResourceLimit` che lo nomina, invece di allocarle. Il
  controllo è deterministico.

  **Non è un tetto garantito.** Restano fuori: il transitorio dentro una
  chiamata di `geo` o di `i_overlay` (incroci, grafi, il risultato di un
  overlay fino al primo controllo); le strutture di `rstar` (gli R-tree,
  proporzionali agli ingressi); la crescita dei builder Arrow delle colonne
  d'uscita; alcuni vettori ausiliari (riferimenti alle righe, gruppi per
  riga, indici) e il transitorio delle codifiche WKB e della validazione
  OGC; l'overhead dell'allocatore. Il controllo dopo il passo con i byte
  esatti resta. *Rientro*: un limite di memoria del processo imposto dal
  sistema operativo (job object su Windows, cgroup su Linux), previsto con
  l'infrastruttura; una grandezza a secco per queste espansioni nel
  modello.
- **Transitorio oltre la previsione non rilevato.**
  *Regola*: prima del passo si controlla la previsione del modello, dopo il
  passo i byte vivi esatti delle tabelle residenti con l'uscita; la memoria
  che il kernel alloca e libera durante il passo non si misura.
  *Ambito*: ogni passo di `PipelineValidata::run` (il controllo dopo il
  passo in `esecuzione.rs` conta solo i buffer Arrow ancora vivi con
  l'uscita); in particolare il transitorio dentro `geo` e `i_overlay` dei
  sette profili geo esclusi dal modello, che il margine dei kernel non vede
  (limite «Modelli di costo geo»: i risultati trattenuti invece si
  contano), e le espansioni che dipendono dai dati senza preflight di
  memoria nel kernel (`join` molti a molti, `explode`, `unnest`, le geo
  fuori da quei profili).
  *Hazard*: un transitorio già liberato alla fine del passo può aver
  superato il budget senza alcun errore: nessun `ResourceLimit`, e nessun
  esaurimento di memoria se la macchina ne ha. Il budget dichiarato è stato
  superato in silenzio; il resoconto riporta solo i byte vivi dopo il
  passo. Se la macchina non ne ha, il processo si ferma per esaurimento di
  memoria invece che con un errore del runner.
  *Rientro*: un limite di memoria del processo imposto dal sistema
  operativo (job object, cgroup), previsto con l'infrastruttura: un tetto
  vero, che la guardia dei kernel geo (limite «Modelli di costo geo») non
  è.
- **Geo senza diagnostica per riga.**
  *Regola*: un passo geo rende il primo errore in ordine di riga, senza
  report `plenora-row-diagnostics-v1`; gli indici che alcuni messaggi dei
  kernel riportano sono righe dell'ingresso del passo, non della sorgente.
  *Ambito*: ogni `geo.*` tranne `from_wkt`.
  *Hazard*: un indice di riga dopo un passo che cambia righe o ordine
  (`table.filter`, `table.sort`) non punta alla riga del file d'origine.
  *Rientro*: la raccolta completa per riga del passo geo di `190c493`
  (`collect_cell_failures`), con la base degli indici del runner.
- **Diagnostica senza esempi dopo un passo che cambia le righe.**
  *Regola*: gli esempi della diagnostica per riga hanno l'indice della
  sorgente (`source_row_zero_based`) solo quando nessun passo a monte
  cambia numero o ordine delle righe; altrimenti il payload ha conteggi,
  cause e totale senza esempi, completezza `partial` e limite di
  conoscenza `read.row_attribution_unavailable`, e il testo nomina passo e
  ingresso ([«Diagnostica per riga»](#diagnostica-per-riga)). Garanzia
  indebolita: quante righe e perché, non quali.
  *Ambito*: ogni passo il cui primo ingresso discende da un'operazione con
  `source_row_provenance` `Unavailable` (catalogo).
  *Hazard*: nessun indice sbagliato (il contratto vieta la base
  dell'ingresso del passo che il runner pubblicava fino a `da5f779`), ma la
  riga rifiutata si trova solo eseguendo a parte il piano spezzato, e dopo
  un'aggregazione, un join o un'esplosione una sola riga d'origine può non
  esistere. `run` fallisce senza output parziali.
  *Rientro*: una mappa di righe verso la sorgente per i passi che
  selezionano o permutano senza duplicare (`filter`, `sort`, `limit`,
  `sample`, `distinct`), composta passo per passo e contata nei byte vivi,
  che riporterebbe gli esempi con l'indice della sorgente; i passi che
  duplicano o creano righe (`join`, `explode`, `aggregate`) restano senza
  esempi, perché il contratto v1 vuole un indice della sorgente unico per
  esempio.
- **Scadenza e annullamento solo fra i passi.**
  *Regola*: `run_interrompibile` controlla scadenza e annullamento prima
  di ogni passo e prima di consegnare gli output
  ([«Scadenza e annullamento»](#scadenza-e-annullamento)), mai dentro un
  kernel.
  *Ambito*: `PipelineValidata::run_interrompibile`; anche i controlli dopo
  il passo (budget, contratto) e la validazione degli input girano fino in
  fondo.
  *Hazard*: un passo lungo (un join enorme, un overlay) finisce anche
  oltre la scadenza o dopo l'annullamento, e l'errore arriva al controllo
  successivo: il ritardo è al più la durata del passo in corso. Il
  risultato non è mai sbagliato: un'esecuzione interrotta non rende
  output, una finita prima della scadenza li rende tutti.
  *Rientro*: controlli cooperativi dentro i kernel lunghi (per blocco di
  righe o di coppie), con lo stesso `Interruzione` passato ai kernel.
- **Run-end e union rifiutati al confine.**
  *Regola*: uno schema con una colonna `RunEndEncoded` o `Union`, a
  qualunque profondità (valori di una dictionary, figli di liste, struct e
  mappe), si rifiuta con `Unsupported` prima di ogni passo
  (`plenora_core::contract::arrow_schema::verifica_tipi_supportati`, in
  `contract_from_arrow_schema`), alla lettura Arrow IPC prima di
  decodificare i blocchi (Parquet non li produce) e in scrittura prima di
  creare il file. Nessun kernel li vede.
  *Ambito*: input del runner, `plenora-io`.
  *Hazard*: in Arrow 60.0.0 `concat` di run-end trabocca sulle fini
  `Int16` (la somma delle lunghezze in `concat_run_arrays` non è
  controllata: panico con `overflow-checks`) e `logical_nulls` sbaglia
  sulle union dense a un campo con id diverso da 0 (raccoglie i null con
  l'id fisso 0): una cella nulla diventerebbe un valore senza errore.
  `take` su run-end, che in 59.2.0 ignorava gli indici nulli, in 60.0.0 li
  tratta. Tutti e tre verificati sul sorgente e con una sonda fuori dal
  workspace, non con l'oracolo del rientro. Chi chiama i kernel
  direttamente, fuori dal runner, non ha il controllo.
  *Rientro*: quando Arrow corregge `take`/`concat` sulle run-end e
  `logical_nulls` delle union a un campo, verificato con un oracolo contro
  `logical_nulls` e contro la stessa tabella senza codifica.

- **Errori che dipendono dai valori delle celle, in esecuzione.**
  *Regola*: la validazione rifiuta ciò che config, schema e limiti rendono
  prevedibile; ciò che dipende dal valore di una cella fallisce, con un
  errore esplicito, quando il kernel la legge.
  *Ambito*: testo non numerico in una colonna Utf8 letta come numero
  (confronti ordinati, aggregazioni, statistiche, `assert_range`); valori
  che non si convertono nel tipo chiesto (`type_cast`, parse delle date);
  un `amount` di `date_add` che alcune date sopportano e quelle dei dati no;
  in `expression`, regex e indici di `substring` calcolati dalle colonne,
  divisori non letterali nulli con `on_division_by_zero=error`; testi
  prodotti oltre `max_string_bytes` e pattern calcolati oltre
  `max_regex_bytes`; le asserzioni violate dai dati.
  *Hazard*: i passi a monte hanno già girato quando l'errore arriva.
  *Rientro*: nessuno previsto, è la natura del dato. L'oracolo
  `crates/plenora-pipeline/tests/oracolo_config.rs` esegue ogni config che
  l'analisi accetta (varianti di ogni operazione del catalogo) e ammette in
  esecuzione solo queste classi, elencate con il motivo; per le config che
  l'analisi rifiuta con una regola «il kernel fallirebbe», chiama il kernel
  direttamente e verifica che fallisca davvero (nessun rifiuto falso).
  Oltre i dati delle fixture non prova: il confine di `verifica_amount` sul
  secondo intercalare ha un test a parte.
- **Parametri ignorati: censimento dei campi di primo livello.**
  *Regola*: nessun parametro scritto si ignora; si rifiuta in analisi e nel
  kernel.
  *Ambito*: `censimento_parametri.rs` legge da serde i campi di ogni config
  tabellare e fallisce per un campo senza voce; `parametri_senza_effetto.rs`
  prova ogni regola in validazione, nel kernel e sulla config gemella;
  l'oracolo `oracolo_config.rs` prova le regole contro i kernel sulle
  varianti delle fixture. I campi delle strutture annidate (aggregazioni,
  mascherature, regole, condizioni, colonne di `align_schema`, nodi di
  `expression`) non si enumerano da soli: li copre la voce del campo che li
  contiene.
  *Hazard*: restano accettati, e dichiarati, i parametri che hanno effetto
  ma non cambiano il risultato su certi dati (`distinct` con `min`/`max`);
  quelli senza effetto solo su certi schemi d'ingresso (elencati in
  «Validazione»: la regola è rifiutare ciò che la config da sola rende
  senza effetto); e questi senza effetto in casi limite: `fill_na` con `method=value` e senza
  `value` (riempie con null, non cambia niente); `unit` di `date_add` con
  `amount` 0 (riformatta soltanto); le politiche sui null
  (`allow_null`, `nulls_equal`, `null_policy`) su colonne che lo schema
  dichiara non nullable, perché la nullabilità dichiarata è spesso
  prudente e una dictionary non nullable può contenere null logici;
  `max_splits` di `split_column` che lascia sempre null le ultime colonne
  (ha effetto sull'ultima parte); `var_name` e `value_name` di `melt` che
  collidono con una colonna, rinominati con un suffisso come dichiara la
  scheda.
  *Rientro*: un parametro nuovo entra con la sua voce nel censimento, la sua
  regola in una `verifica_*` condivisa e un caso in
  `parametri_senza_effetto.rs`; le politiche sui null su colonne non
  nullable si rifiuteranno quando la nullabilità dei contratti sarà esatta.
- **Limiti dei testi: regole di config nell'analisi, testi prodotti nei
  kernel.**
  *Regola*: un testo o un pattern della config oltre `max_string_bytes` o
  `max_regex_bytes` si rifiuta in analisi; un testo prodotto dai dati oltre
  `max_string_bytes` si rifiuta nel kernel ([«Validazione»](#validazione)).
  *Ambito*: i kernel chiamati direttamente, fuori dal runner; le funzioni
  `expression`, `formula`, `aggregate`, `replace` e `mask_data`, che usano
  `Limits::default()` (le varianti `_con_effetti` e `_con_limiti` ricevono
  i limiti del chiamante, e il runner usa quelle); `geo.to_wkt`, che scrive
  WKT senza confrontarlo con `max_string_bytes`.
  *Hazard*: chi chiama un kernel senza l'analisi può passare testi e
  pattern di config oltre i limiti, e con le funzioni senza limiti ottiene
  i limiti di default, non i suoi; il WKT di una geometria grande supera
  `max_string_bytes` senza errore.
  *Rientro*: i limiti come parametro di ogni kernel tabellare che produce
  testo (oggi cambierebbe la firma di decine di chiamate); il controllo di
  `max_string_bytes` in `geo.to_wkt` con le geo.
- **Chiave HMAC controllata in validazione, dal runner**: è ambiente, non
  config, quindi non sta nell'analisi dei kernel; la legge la stessa
  funzione del kernel. La variabile d'ambiente può cambiare fra `validate`
  e `run`, e in quel caso l'errore arriva al passo.
- **Nome del passo negli errori**: aggiunto al messaggio conservando la
  categoria; gli errori con diagnostica per riga o già strutturati restano
  quelli del kernel, salvo la diagnostica sulla base dell'ingresso del
  passo, che nomina passo e ingresso.

## Metadati Arrow

Gli schemi che entrano ed escono dal componente seguono i contratti
pubblici *Arrow Interchange 1.0* e *Arrow Metadata Vocabulary 1.0* di
`plenora-contracts` (commit `ade868c`). I quattro vettori di conformità
del vocabolario sono copiati byte per byte, con provenienza e SHA-256, in
`crates/plenora-io/tests/fixtures/contratti/arrow-v1/`, e
`crates/plenora-io/tests/contratti_arrow.rs` li fa passare dal confine
pubblico: file Arrow IPC, `esegui_da_file` con un piano identità, file
d'uscita riletto. Il codec delle chiavi è
`plenora_core::contract::arrow_metadata`, la conversione fra schema e
contratto e la pubblicazione `plenora_core::contract::arrow_schema`.

### In uscita

Ogni output di `run` porta lo schema pubblicato
(`PipelineValidata::schema_uscita`, deciso in validazione), e con lui ogni
file di `esegui_da_file`:

- `plenora.contract.version = 1` nei metadati di schema, anche su una
  tabella senza geometrie (ARROW-001);
- `plenora.field_id` su ogni campo di primo livello (ARROW-003, ARROW-004,
  sotto);
- sulla colonna geometrica, `ARROW:extension:name = geoarrow.wkb` e il
  blocco canonico sempre completo (vocabolario, sezione 4). Ciò che il
  contratto non sa si dichiara con il valore che dice «non so», mai con
  una pretesa:

| chiave | valore |
| --- | --- |
| `encoding` | quello del contratto; non dichiarato: `wkb`, come la lettura lo completa dal nome d'estensione |
| `dimensions` | quella del contratto, anche `unknown` |
| `spatial_semantics` | `geometry`: i kernel sono planari, e `geography` si rifiuta in ingresso |
| `precision` | quella ereditata, che attraversa intatta le operazioni tabellari; dopo un'operazione geo, o se nessuno la dichiara, `float64` (ogni kernel geo ricodifica le coordinate in `f64`, e ogni coordinata WKB è un double) |
| `types_declaration`, `types` | la dichiarazione del contratto; non dichiarata: `unresolved` senza elenco |
| `crs_resolution` | `resolved`, `declared_unresolved` o `missing`, dallo stato del CRS |
| `crs_id`, `crs_definition`, `crs_definition_format`, `axis_order`, `srid` | come prima: con un CRS risolto `axis_order` è l'ordine GIS normalizzato (x = est/longitudine) e `srid` il codice d'autorità se sta in `i32`; con `missing` nessuna, salvo uno `srid` ereditato, che il vocabolario ammette |

Dentro il piano gli schemi restano quelli interni (versione e blocco
canonico solo sulle tabelle con geometrie, nessuna identità assegnata):
versione e identità sono del confine.

### Identità dei campi

L'identità segue la lineage dei metadati di campo: un'operazione che
propaga una colonna, la rinomina (anche i suffissi `_L`/`_R` di `join`) o
la riscrive al suo posto ne clona i metadati, e con loro
`plenora.field_id`; una colonna nuova nasce senza. È la stessa regola del
`FieldId` delle geometrie nel piano (`geo.buffer` riscrive la colonna e ne
tiene l'identità). All'uscita (`pubblica_schema`):

- un'identità portata da un solo campo dello schema, e dichiarata da un
  solo ingresso del piano, resta byte per byte;
- un'identità portata da più campi (una colonna duplicata, un self-join) o
  dichiarata da più di un ingresso si toglie a tutti i campi che la
  portano: i namespace degli ingressi sono indipendenti, e la stessa cifra
  direbbe due campi. L'identità si perde, non passa a un altro campo;
- ogni campo senza identità ne riceve una nuova, in ordine di colonna,
  dalla prima sopra la massima dichiarata da tutti gli ingressi del piano
  (non solo da quelli da cui l'uscita discende): una colonna nuova non
  prende mai il numero che un ingresso usava per un altro campo. Due uscite
  dello stesso piano possono dare lo stesso numero a campi diversi:
  l'unicità vale nello schema.

`concat`, `concat_by_name` e `union_distinct` tengono i metadati di campo
del primo ingresso, e con loro la sua identità: la colonna unita è quella
del primo ingresso, l'identità del secondo per la stessa colonna non
sopravvive.

Stesso piano e stessi schemi d'ingresso, stesse identità
(`crates/plenora-core/tests/identita_dei_campi.rs`,
`crates/plenora-pipeline/tests/identita_campi.rs`).

### In ingresso

`contract_from_arrow_schema` legge lo schema di ogni input del piano (e
`GeoParquet` in lettura e scrittura):

- uno schema senza alcuna chiave `plenora.` non è uno schema Plenora (una
  tabella di pandas, di GDAL, di GeoPandas) e si accetta: le geometrie si
  leggono dall'estensione `geoarrow.wkb` e dal metadato `geo`, come prima.
  ARROW-001 riguarda lo schema Plenora che attraversa il confine;
- con almeno una chiave `plenora.` (anche solo `plenora.field_id`, e
  anche su un figlio di struct, lista o mappa, a qualunque profondità:
  prima si guardavano solo i campi di primo livello)
  `plenora.contract.version` è obbligatoria e vale esattamente `1`: una
  versione decimale maggiore è `Unsupported` (ARROW-002), ogni altro testo
  (`0`, `01`, `1.0`) è `Schema`. Prima `0` e `01` passavano;
- `plenora.field_id`: intero decimale non negativo, unico nello schema
  (`Schema`); oltre `u32::MAX` o su un campo annidato `Unsupported`;
- `srid`: intero decimale con segno a 32 bit (`-1` ammesso; prima solo
  senza segno). Con `crs_resolution = missing` è ammesso (il vocabolario
  vieta con `missing` identificatore, definizione, formato e assi, non lo
  `srid`; prima si rifiutava) e lo stato resta `missing`: un indizio
  numerico non diventa un CRS (ARROW-007). `declared_unresolved` con il
  solo `srid` invece si rifiuta (`Crs`): il vocabolario vuole un
  identificatore o una definizione, e prima l'uscita dichiarava uno stato
  senza nessuno dei due;
- chiavi `plenora.geometry.*` su un campo senza `ARROW:extension:name =
  geoarrow.wkb` si rifiutano (`Schema`; prima bastavano da sole a
  dichiarare la colonna);
- storage `LargeBinary` di una colonna `geoarrow.wkb`: il runner lo
  converte in `Binary` all'ingresso, come `LargeUtf8` in `Utf8` (oltre i
  2 GiB di un `Binary`, `ResourceLimit`), e l'uscita è `Binary`. Lo
  ammette il vocabolario per entrambi i lati;
- `spatial_semantics = geography`, un `edges` non planare in
  `ARROW:extension:metadata` e un `crs` in `ARROW:extension:metadata` si
  rifiutano (`Unsupported`): i kernel sono planari, e un CRS che questo
  componente non legge passerebbe per assente (ARROW-007);
- metadati contraddittori falliscono con categoria `crs` se riguardano il
  CRS (stato, identificatore, definizione e formato, ordine degli assi,
  `srid`, divergenza dal `geo`), `schema` altrimenti (vocabolario,
  sezione 4). Fino a questo ciclo erano `InvalidPlan`: il cambio di
  categoria è una rottura dichiarata, voluta dal contratto;
- `types_declaration = unresolved` e la dichiarazione assente si leggono
  allo stesso modo (nessuna pretesa sui tipi): il contratto sopravvive al
  giro emissione-lettura.

Un `axis_order` con la prima coordinata nord (`lat_lon`,
`northing_easting`) si accetta: il vettore `resolved-point` lo dichiara
per `EPSG:4326`, e un piano di sole operazioni tabellari lo attraversa
intatto (ARROW-008). Le operazioni geo lo rifiutano (`Crs`), perché ogni
kernel geo legge x come est/longitudine: prima solo `geo.reproject` lo
controllava, e le misure geodetiche avrebbero scambiato latitudine e
longitudine senza errore.

`scrivi_tabella` resta un codec senza perdite: non aggiunge versione né
identità a una tabella qualsiasi, ma rifiuta uno schema con chiavi
`plenora.` senza versione `1` o con identità non valide, invece di
scrivere uno schema Plenora non conforme.

### Versioni del catalogo

Ogni operazione geo prende `contract_analysis_version` + 1: la sua analisi
passa da `analyze_geo_contract`, che ha una regola di validazione nuova
(rifiuto degli assi scambiati) e cambia lo schema d'uscita inferito (toglie
una `precision` ereditata diversa da `float64`), e la versione dell'analisi
si incrementa per l'una e per l'altra cosa. Semantica, config e kernel
restano. La tabella con le versioni prima del ciclo e l'incremento è
`contratti_arrow_portano_l_incremento_dell_analisi` in
`crates/plenora-core/src/catalog.rs`, e copre per costruzione ogni
operazione geo del catalogo.

Le tabellari non cambiano versione: né la loro analisi né i loro kernel
cambiano. Versione, identità dei campi e blocco geometrico completo li
aggiungono la pubblicazione dello schema e l'emissione canonica, che stanno
sotto il catalogo e valgono per tutte le operazioni allo stesso modo: è un
cambio del contratto di trasporto (Arrow Interchange 1.0), non di
un'operazione.

### Limiti dichiarati

- **Identità persa nelle collisioni.**
  *Regola*: ARROW-004 chiede di conservare l'identità di un campo
  invariato.
  *Ambito*: `pubblica_schema`, uscite di `run`.
  *Hazard*: un campo invariato perde l'identità quando lo stesso numero è
  dichiarato da due ingressi del piano o compare su due campi dell'uscita;
  riceve un'identità nuova, come una colonna derivata. Garanzia indebolita:
  il consumatore non ritrova il campo, ma non ne trova mai un altro. Non è
  un caso raro: ogni uscita numera da 0 i campi che non avevano identità,
  quindi due tabelle prodotte da questo runner condividono quasi sempre
  0, 1, 2…, e in un piano che le usa entrambe quasi tutte le identità si
  perdono.
  *Rientro*: un namespace d'identità condiviso fra gli ingressi (per
  esempio identità uniche per sorgente dichiarate dal contratto).
- **Identità per lineage.**
  *Regola*: ARROW-004 rimanda al contratto di ogni operazione la scelta di
  quali campi sono nuovi.
  *Ambito*: tutte le operazioni del catalogo.
  *Hazard*: qui un campo tiene l'identità quando l'operazione ne clona i
  metadati: una colonna riscritta al suo posto (`fill_na`, `replace`,
  `geo.buffer`) è lo stesso campo con valori nuovi, non un campo derivato.
  Le schede non lo ripetono operazione per operazione.
  *Rientro*: una dichiarazione per operazione nel catalogo, se il
  contratto la chiederà.
- **Precisione normalizzata dalle operazioni geo.**
  *Regola*: ARROW-010 vuole dichiarata ogni normalizzazione di metadati.
  *Ambito*: `analyze_geo_contract`, ogni operazione `geo.*`.
  *Hazard*: dopo un'operazione geo `precision` esce `float64` anche se
  l'ingresso dichiarava `float32` o `native`, e anche per le operazioni che
  restituiscono la geometria com'era (misure, predicati: dove la scheda
  dice che la colonna geometria passa invariata, vale salvo questa
  chiave): una `float32`
  ereditata direbbe il falso dopo un kernel che ricodifica in `f64`, e
  distinguere le operazioni che non ricodificano non vale la complessità.
  Si perde l'informazione sulla precisione d'origine; le operazioni
  tabellari la conservano.
  *Rientro*: la precisione nel contratto della colonna, con le operazioni
  che la conservano dichiarate.
- **`encoding` completato dal nome d'estensione.**
  *Regola*: l'encoding emesso è una dichiarazione sui byte.
  *Ambito*: colonne `geoarrow.wkb` senza `encoding` né `geo.encoding`.
  *Hazard*: l'uscita dichiara `wkb` anche per celle che nessun kernel ha
  letto (un piano di sole operazioni tabellari). È la lettura che lo
  standard GeoArrow dà del nome d'estensione e quella che il runner già
  faceva in ingresso, ma un produttore che mettesse EWKB sotto
  `geoarrow.wkb` senza dichiararlo vedrebbe la sua colonna etichettata
  `wkb`; i kernel geo rifiutano comunque lo SRID incorporato.
  *Rientro*: la dichiarazione obbligatoria in ingresso, con il rifiuto delle
  chiavi mancanti (sotto).
- **Metadato d'estensione dei file GeoParquet.**
  *Regola*: un CRS dichiarato non si perde (ARROW-007).
  *Ambito*: lettura GeoParquet.
  *Hazard*: il lettore toglie `ARROW:extension:metadata` dallo schema
  incorporato prima di leggerne il contratto, perché l'autorità è il
  metadato di file `geo`; un `crs` scritto lì e diverso da quello del `geo`
  non si confronta. Limite precedente a questo ciclo.
  *Rientro*: il confronto del `crs` d'estensione con quello del file.
- **Chiavi mancanti tollerate in ingresso.**
  *Regola*: il vocabolario (sezione 4) vuole su ogni campo `geoarrow.wkb`
  identità, encoding, dimensionalità, semantica, precisione, dichiarazione
  dei tipi e stato del CRS.
  *Ambito*: `contract_from_arrow_schema`.
  *Hazard*: uno schema versionato a cui ne manca qualcuna si legge con i
  completamenti di sempre (encoding dall'estensione, dimensionalità
  `unknown`, tipi non dichiarati, stato del CRS dalle rappresentazioni,
  semantica `geometry` come in GeoArrow), e l'uscita le porta tutte. Le
  contraddizioni restano errori.
  *Rientro*: un rifiuto delle chiavi mancanti quando i produttori le
  emetteranno tutte.
- **Storage `binary` in uscita.** Un ingresso `large_binary` esce `binary`
  (sopra): lo storage non è un passaggio senza perdite, i byte delle celle
  sì.

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
| Arrow IPC | `.arrow`, `.feather`, `.ipc` | file (Feather v2) o stream, riconosciuti dal contenuto; tutti i blocchi ricomposti in uno | file, a blocchi di circa 8 MiB, senza compressione |
| Arrow IPC stream | `.arrows` (`Formato::ArrowIpcStream`) | come sopra (il contenuto decide) | stream con il marcatore di fine, stessi blocchi, senza compressione |
| Parquet | `.parquet` | un batch grande quanto il file, anche con più row group | proprietà fisse, `ZSTD` livello 3 (o `SNAPPY`, o nessuna) |
| GeoParquet 1.1 | `.parquet` | quando c'è il metadato di file `geo` | quando lo schema ha una colonna geometrica |

Il formato viene dall'estensione (senza distinzione di maiuscole) o da
`Formato` esplicito; un'estensione diversa è `Unsupported`, mai un formato
indovinato. Feather v1 si rifiuta. Fino a questo ciclo `.arrows` si
scriveva in formato file: ora è lo stream (il nome che Arrow dà agli
stream), e un `.arrows` scritto prima si rilegge comunque, perché la
lettura riconosce il formato dal contenuto.

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
60.0.0`), pagine formato 1.0, row group di al più 1 048 576 righe,
statistiche di pagina, niente bloom filter, metadati JSON con chiavi in
ordine: la stessa tabella dà gli stessi byte, in Parquet e in Arrow IPC
(provato). I byte Parquet dipendono dalla versione di `parquet`: da 60.0.0
cambiano `created_by`, l'ordine dichiarato delle colonne float
(`IEEE_754_TOTAL_ORDER` al posto di `TYPE_DEFINED_ORDER`, PARQUET-2249) e
le statistiche float, che portano `nan_count` anche negli indici di
pagina; pagine e valori restano gli stessi byte. Un lettore che non
conosce quell'ordine ignora il minimo e il massimo delle colonne float
(pyarrow 25 li riporta assenti), non i valori. Dopo ogni scrittura si rilegge il footer: per Parquet lo schema
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
blocco canonico `plenora.geometry.*`, completo, e `plenora.contract.version`,
e rifiutano chiavi canoniche già presenti in conflitto (categoria `schema`,
o `crs` per le chiavi del CRS: [«Metadati Arrow»](#metadati-arrow)). Il
metadato `geo` di schema si toglie, `ARROW:extension:metadata` si
sostituisce. Le identità dei campi (`plenora.field_id`) le assegna il
runner all'uscita, non la lettura.

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
| dimensionalità `xy`/`xyz`, tipi non dichiarati (anche `unresolved`, che si legge come non dichiarati) | i tipi trovati nei dati (la rilettura li dichiara `exact`) |
| dimensionalità `unknown`, o dichiarazione `mixed` senza elenco | `[]` |

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
PROJ, uno con timestamp `INT96` e uno con i checksum di pagina
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
Un errore nella lettura di un input ha fase `read`.

`esegui_da_file_interrompibile` è la stessa esecuzione con
un'`Interruzione` (scadenza e annullamento, [«Scadenza e
annullamento»](#scadenza-e-annullamento)): oltre ai controlli del runner,
prima di leggere ogni input (fase `read`) e prima di scrivere ogni output
(fase `write`; dopo il primo output scritto l'effetto è `partial`); rende
anche nome, righe, colonne e formato di ogni output scritto (`EsitoFile`).
`valida_da_file` carica gli input allo stesso modo e valida il piano senza
eseguirlo.

### Memoria

Le tabelle caricate contano nel budget del piano
(`max_governed_memory_bytes`) come in `run`: ogni input si legge con il
budget residuo, e dopo la lettura i byte vivi esatti devono starci.

| passo | controllo prima | misura (Windows, allocazioni contate, 1 000–5 000 000 righe) |
| --- | --- | --- |
| lettura Arrow IPC | dimensione del file prima di leggerlo, il doppio prima di decodificare se i blocchi sono più di uno, e prima di ricomporli i byte vivi dei blocchi decodificati più la loro copia | picco 1,0 volte la tabella con un blocco, 2,0 con più blocchi |
| lettura Parquet | 5 volte la stima dal footer, più 1 MiB | picco fino a 2,8 volte la tabella da 20 000 righe in su (liste, interi e float con null; 4 volte a 1 000 righe, per i buffer fissi), sempre sotto il 75% della previsione |
| scrittura Arrow IPC | due volte il blocco più grande (circa 8 MiB, di più con righe molto più grandi della media), misurato sui blocchi veri con i dizionari interi, più 1 MiB | picco sotto 3 MiB |
| scrittura Parquet | 4 volte i byte del row group più grande, misurato sulle fette vere, più 8 MiB | picco fino a 45 MiB (liste, 5 milioni di righe) |

La **stima dal footer** Parquet è il maggiore fra i byte non compressi dei
column chunk e i valori per la larghezza fisica di ogni foglia, più i byte
decodificati dei `BYTE_ARRAY` quando il file li dichiara
(`unencoded_byte_array_data_bytes`). Le misure si rifanno con un
allocatore che conta, fuori dal workspace (niente `unsafe` qui).

### Confine di lettura

Il minimo perché un file malformato diventi un errore esplicito invece di
un panico o di un risultato sbagliato (`plenora_io::confine`). Non è una
difesa da file costruiti apposta (limiti dichiarati sotto).

- **Barriera anti-panico**: ogni chiamata ad `arrow-ipc`, `parquet` e
  `concat_batches` sui byte del file gira in
  `plenora_core::panic_policy::barriera_di_dipendenza`: un panico (per
  esempio di un `unwrap` sui campi opzionali del footer) diventa
  `DataMapping` con la sola forma del payload. Lo schema IPC si converte
  con `try_fb_to_schema`, che da `arrow-ipc` 60 restituisce un errore dove
  `fb_to_schema` andava in panico.
- **Budget prima di leggere, decodificare e ricomporre** (tabella in
  «Memoria»).
- **Arrow IPC**: il file si legge intero in un buffer allineato (già nel
  budget) e, prima di Arrow, se ne percorre la struttura: prefissi e
  lunghezze dei messaggi, metadati entro il tetto, corpi e blocchi del
  footer dentro il file. `FileDecoder` e `StreamDecoder` decodificano poi
  per viste dello stesso buffer. Alcuni controlli evitano un risultato
  sbagliato senza errore: il marcatore di fine dello stream (senza, uno
  stream tagliato fra due messaggi darebbe meno righe); solo messaggi di
  schema, dizionario e blocco (`StreamDecoder` salta un messaggio `NONE`,
  e un blocco col tipo cambiato sparirebbe); blocchi del footer né ripetuti
  né sovrapposti (moltiplicherebbero le righe) e coerenti col loro
  messaggio, cioè lunghezza dei metadati uguale a prefisso più messaggio,
  tipo atteso e stessa lunghezza del corpo (`FileDecoder` prende il corpo
  dall'offset del blocco: un blocco spostato leggerebbe metadati come
  valori); l'endianness (`StreamDecoder` non la guarda); le voci dei
  metadati di schema e di campo, a ogni profondità, con chiave e valore
  (anche vuoto) e chiavi non ripetute (`try_fb_to_schema` scarterebbe una
  voce senza chiave o senza valore e terrebbe l'ultima di due chiavi).
- **Parquet**: la lunghezza del footer, letta dalla coda, entro il tetto
  prima che `parquet` la usi; i row group entro il massimo e la loro somma
  di righe uguale a quella del footer dopo; i metadati chiave-valore del
  file con un valore (anche vuoto) e chiavi non ripetute, e lo schema
  incorporato con le stesse verifiche di un file IPC (`parquet` scarterebbe
  una voce senza valore). È una restrizione voluta: la specifica Parquet
  ammette una voce senza valore, ma qui sparirebbe in silenzio, quindi si
  rifiuta; pyarrow e arrow-rs scrivono sempre il valore, anche vuoto.

| limite (`LimitiLettura`) | predefinito | a che cosa si applica | errore |
| --- | --- | --- | --- |
| `max_byte_metadati` | 16 MiB, e mai oltre il budget residuo | ogni messaggio IPC, footer IPC, footer Parquet | `ResourceLimit` |
| `max_byte_metadati_custom` | 4 MiB | chiavi e valori dei metadati di schema e di tutti i campi, a ogni profondità; per Parquet più tutti i metadati chiave-valore del file, `ARROW:schema` compreso | `ResourceLimit` |
| `max_blocchi` | 100 000 | blocchi IPC (record batch), row group Parquet | `ResourceLimit` |

`leggi_tabella` usa i predefiniti, `leggi_tabella_con_limiti` li prende
espliciti. Gli errori dicono che cosa non va e quale limite, mai byte del
file o valori. Le prove sono in `crates/plenora-io/tests/confine.rs`: file
troncati a ogni lunghezza (Arrow IPC e Parquet: sempre errori), Arrow IPC
con ogni byte invertito (nessun panico; ogni posizione con la suite lunga,
testa, coda e una ogni 11 senza), lunghezze enormi di metadati, blocchi e
footer, stream senza fine o con byte dopo la fine, un blocco dello stream
cambiato in `NONE`, blocchi del footer ripetuti o spostati rispetto al loro
messaggio, una pagina Parquet con checksum e un valore corrotto, e ogni
limite della tabella.

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
- **Tipi rifiutati in scrittura**: in Parquet `Interval(MonthDayNano)`
  (errore di `parquet`, nessun file); in ogni formato `RunEndEncoded` e
  `Union`, a qualunque profondità, prima di creare il file (limite «Run-end
  e union rifiutati al confine» del runner, anche in lettura). Un test ne
  tiene l'elenco. Una tabella senza colonne non si scrive in Parquet (il numero di
  righe andrebbe perso): si usa Arrow IPC.
- **Codec Parquet**: solo `UNCOMPRESSED`, `SNAPPY`, `ZSTD` sono compilati;
  `GZIP`, `BROTLI`, `LZ4`, `LZ4_RAW`, `LZO` si rifiutano prima di decodificare
  (`Unsupported`). Arrow IPC compresso (LZ4/ZSTD) si rifiuta con l'errore di
  Arrow.
- **File costruiti apposta: aborto del processo.**
  *Regola*: il confine di lettura ferma i panici e verifica le lunghezze
  che costano poco (sopra); il contenuto dei footer, delle intestazioni di
  pagina e dei messaggi IPC resta a `parquet` e `arrow-ipc`.
  *Ambito*: `parquet_io::leggi` (footer Thrift, intestazioni e pagine),
  `ipc::leggi` (buffer e nodi dei messaggi, dizionari delta, compressione
  dichiarata).
  *Hazard*: un file Parquet malformato o costruito apposta (e i casi IPC non
  coperti) può far terminare il processo: `parquet` e `arrow-ipc` allocano
  dalle lunghezze dichiarate prima di verificarle (per esempio
  `Vec::with_capacity` sulla lunghezza di una lista Thrift del footer, la
  dimensione non compressa di una pagina, le righe dichiarate; in IPC le
  copie di buffer sovrapposti e non allineati), e un'allocazione impossibile
  è un aborto, che la barriera anti-panico non ferma. Anche uno schema
  Parquet annidato per migliaia di livelli esaurisce lo stack. I file
  scritti da scrittori conformi non lo fanno.
  *Rientro*: se si devono leggere file di fonti non fidate, aggiungere una
  pre-validazione (footer e intestazioni di pagina percorsi prima di
  `parquet`, contenuto dei messaggi IPC prima di Arrow).
- **Pagine Parquet senza checksum.**
  *Regola*: `parquet` verifica il CRC di ogni pagina che lo porta (feature
  `crc`); una pagina con CRC sbagliato è un errore (`DataMapping`).
  *Ambito*: `parquet_io::leggi`.
  *Hazard*: una pagina senza CRC (il default di pyarrow e di `parquet-rs`)
  corrotta ma ancora decodificabile, per esempio un byte di un valore
  numerico in una pagina non compressa, torna con altri valori senza errore.
  **I file scritti da `plenora-io` non portano CRC**: lo scrittore di
  `parquet` 60.0.0 (come 59.2.0) non sa scriverli (l'intestazione di
  pagina ha sempre `crc: None`, `column/page.rs`, «TODO: Add support for
  crc checksum», e `WriterProperties` non ha un'opzione), quindi la
  verifica protegge solo i file di altri scrittori che li hanno scritti.
  *Rientro*: scrivere i CRC (`write_page_checksum` in pyarrow) nei file da
  proteggere; per i nostri, una versione di `parquet` che li scriva; un
  controllo d'integrità del file intero a monte.
- **Parquet modificato durante la lettura.**
  *Regola*: la lunghezza del footer si verifica sul file aperto, poi
  `parquet` lo rilegge.
  *Ambito*: `parquet_io::leggi` (Arrow IPC no: si decodifica dal buffer
  verificato).
  *Hazard*: un altro processo che riscrive il file fra la verifica e la
  lettura può presentare a `parquet` un footer di lunghezza non verificata.
  *Rientro*: decodificare da un buffer letto una volta, come per Arrow IPC,
  al prezzo del file intero in memoria durante la decodifica.
- **Righe senza byte.**
  *Regola*: il budget conta i byte; una colonna `Null` o una tabella senza
  colonne dichiara righe che non occupano byte.
  *Ambito*: `ipc::leggi` (lunghezza del blocco e dei nodi `Null`),
  `parquet_io::leggi` (tabella senza colonne foglia).
  *Hazard*: un file di pochi byte può dichiarare miliardi di righe; la
  lettura riesce, e un'operazione che poi alloca per riga risponde al
  budget del runner, non al confine.
  *Rientro*: un tetto sulle righe dichiarate.
- **Strutture Arrow per blocco non contate.**
  *Regola*: il confine IPC conta i byte del file, non le strutture che
  Arrow crea per ogni colonna di ogni blocco (qualche centinaio di byte
  ciascuna), né le copie dei buffer non allineati.
  *Ambito*: `ipc::leggi` prima della verifica esatta dei byte vivi.
  *Hazard*: un file con molti blocchi di molte colonne vuote occupa durante
  la decodifica qualche volta i suoi byte di metadati, oltre la previsione;
  i blocchi restano entro `max_blocchi` e i metadati entro il file.
  *Rientro*: contare colonne per blocchi nella previsione.
- **Arrow IPC file V4.** `FileDecoder` di `arrow-rs` 60.0.0 (come 59.2.0)
  rifiuta un file (non uno stream) scritto con `metadata_version` V4, come
  lo scrive pyarrow con `metadata_version=V4` (`DataMapping`, «arrow error:
  ipc»); lo rifiuta anche `FileReader` da solo, prima di questo confine. Lo
  stream V4 si legge. *Rientro*: una versione di `arrow-ipc` che lo accetti.
- **Hook di panico.** La barriera trasforma il panico in errore, ma l'hook
  di `std` ne stampa il testo su stderr prima, e quel testo può contenere
  byte del file: chi usa `plenora-io` installa
  `plenora_core::panic_policy::install` (limite già dichiarato lì).
- **GeoParquet, ciò che il contratto non porta**: `orientation`, `bbox` e
  `covering` letti si validano e si perdono; `epoch` e `edges: spherical` si
  rifiutano; il contratto ammette una sola colonna geometrica, quindi
  un file con più colonne geometriche si rifiuta (`Schema`). Il PROJJSON
  letto si identifica per `id` e `type`: il resto del documento non si
  confronta con la tabella, e un documento che dichiara un `id` EPSG con
  parametri diversi da quelli del registro passa come quel codice.
- **Metadati geometrici normalizzati**: dopo GeoParquet il campo porta il
  `geo` di campo e il blocco canonico nella forma del contratto, non i byte
  di metadati che aveva prima della scrittura; il contratto si conserva
  (provato su ogni combinazione di dimensionalità, dati e dichiarazione dei
  tipi), i byte dei metadati no. Due cambi voluti: con dimensionalità nota
  e tipi non dichiarati (o `unresolved`) la rilettura dichiara `exact` i
  tipi dei dati; senza
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

## CLI `plenora-data`

`crates/plenora-cli` è la superficie pubblica del componente
`plenora-data-tools` secondo il profilo data-tools di `plenora-contracts`,
fissato al commit `ade868cf89c6652cffe20019e7194b383384ee78`: il binario
`plenora-data` (CLI 2.0) e le stesse quattro operazioni come funzioni Rust
(`plenora_cli::api`). Una sola tabella (`plenora_cli::operazioni::OPERAZIONI`)
dà comandi, aiuto, Capability Discovery 2.0 e la mappa degli export Rust; il
registro dei kernel di `data.catalog` deriva dal catalogo di `plenora-core`.

```sh
plenora-data --help [--format json]
plenora-data --version [--format json]
plenora-data capabilities [--format json]
plenora-data catalog [--format json]
plenora-data describe --input INPUT.arrow [CONTROLLI] [--format json]
plenora-data validate --plan PLAN.json [--input NAME=INPUT.arrow]... [CONTROLLI] [--format json]
plenora-data run --plan PLAN.json [--input NAME=INPUT.arrow]... --output [NAME=]OUTPUT.arrow...
                 [--overwrite] [CONTROLLI] [--format json]
# CONTROLLI: --deadline RFC3339 | --timeout-ms MS
```

| comando | operazione | contratto del risultato | controlli |
| --- | --- | --- | --- |
| `catalog` | `data.catalog` | `plenora-data-kernel-catalog-v1` | nessuno |
| `describe` | `data.describe` | `plenora-data-description-v1` | scadenza, annullamento |
| `validate` | `data.validate` | `plenora-data-plan-validation-result-v1` | scadenza, annullamento |
| `run` | `data.run` | `plenora-data-execution-result-v1` | scadenza, annullamento |

### Uscita e codici

Ogni invocazione scrive **un** documento JSON seguito da un a capo su
stdout, l'inviluppo `cli-envelope-v2` (`status`, `protocol_version` 2,
`component` `plenora-data-tools`, `component_version`, `contract`,
`command`, e `result` o `error`), e **niente su stderr**, tranne gli aborti
del processo che nessun hook intercetta (limite «Aborti senza inviluppo»
sotto): l'hook di panico è silenzioso (`plenora_core::panic_policy`, `Silent`) e un
panico diventa l'errore `internal` (exit 70) senza il testo del payload, con
effetto `unknown` per `run` (un output può essere già scritto). Senza
`--format` l'uscita è comunque JSON; solo `--help` senza `--format json`
stampa il testo d'aiuto (exit 0). Le chiavi escono in ordine: stesso
esito, stessi byte (provato su scoperta e `run`).

L'errore è la proiezione pubblica di `PlenoraError` ([«Errori»](#errori)),
mai un documento costruito a parte; il codice d'uscita è la proiezione
della categoria di CLI 2.0, sezione 8, con un `match` esaustivo:

| codice | categorie |
| --- | --- |
| 0 | successo |
| 2 | `invalid_plan`, `invalid_configuration` |
| 3 | `schema`, `data_mapping`, `crs`, `unsupported` |
| 4 | `resource_limit` |
| 5 | `io`, `not_found`, `conflict`, `concurrent_modification`, `protocol`, `authentication`, `authorization`, `timeout`, `transient` |
| 6 | `execution` |
| 70 | `internal` |
| 130 | `cancelled` |

Gli argomenti falliscono chiusi (`invalid_configuration`, exit 2): comando
o flag sconosciuto, flag di un altro comando, valore mancante (un valore
non comincia con `--`), flag ripetuto, argomento posizionale, `--format`
diverso da `json`, argomento non UTF-8 (tradurlo cambierebbe un percorso
in silenzio). I messaggi dicono la posizione, mai il testo ricevuto: un
argomento può essere un percorso. Gli alias deprecati del binding
(`inspect-dataset`, `transform`, …) non ci sono.

### Comandi

- **`catalog`**: `registry` è un documento `plenora-operation-registry-v1`
  con i kernel che un piano può eseguire (`{id, version, family}`);
  `kernels` un descrittore per ognuno dei 146 kernel del catalogo, con
  `status` (`available`, o `unavailable` con `reason`), `versions`
  (semantica, schema della config, analisi del contratto, kernel),
  `arity`, `result_shape`, `determinism`, `crs_requirement`,
  `required_backends` (sempre vuoto: niente GEOS né PROJ); `plan_format`
  `plenora-data-plan-v1`. `table.transpose` è `unavailable`
  (`plenora_pipeline::disponibilita`: il runner la rifiuta con ogni config,
  e `operazioni_doc.rs` prova l'elenco in entrambe le direzioni).
- **`describe`**: legge la tabella (Arrow IPC file o stream, o Parquet) con
  il budget di default (`DEFAULT_MAX_GOVERNED_MEMORY_BYTES`, 512 MiB), ne
  legge il contratto come lo leggerebbe un piano (stessa normalizzazione,
  stesse regole: metadati contraddittori sono un errore) e rende `rows`,
  `contract_version`, `active_geometry` e per colonna `name`, `type`
  (`descrivi_tipo`: senza metadati né fuso), `nullable`, `field_id` se
  c'è, e per una geometria il blocco canonico `plenora.geometry.*` senza
  `crs_definition`.
- **`validate`**: legge il piano (al più `max_plan_json_bytes`, UTF-8,
  `Pipeline::from_json`) e gli input, valida senza eseguire e rende
  `plan_format`, `plan_version`, `inputs`, `steps` (`out`, `op`) e per ogni
  output lo schema pubblicato (le colonne come in `describe`, con
  l'identità dei campi che `run` scriverà).
- **`run`**: `esegui_da_file_interrompibile`; i dati vanno nei file d'uscita
  (formato dall'estensione: `.arrow`/`.feather`/`.ipc` file, `.arrows`
  stream, `.parquet`), il documento dice per output `name`, `content_type`,
  `rows`, `columns` (mai il percorso) e per passo `out`, `op`, `rows_in`,
  `rows_out`, `division_by_zero_rows`. Una destinazione esistente è
  `conflict` senza `--overwrite`. `--output PATH` senza nome vale per un
  piano con un solo output; con più output ognuno si nomina
  (`--output NAME=PATH`, diviso al primo `=`).

### Scadenza e annullamento della CLI

`--deadline` (istante RFC 3339, come `plenora.execution.deadline` del
binding di runtime) e `--timeout-ms` (dall'avvio del comando), uno solo dei
due, diventano la scadenza dell'`Interruzione`; Ctrl-C e SIGTERM (crate
`ctrlc`, feature `termination`) alzano il segnale di annullamento. Si
controllano prima di leggere ogni input, prima di ogni passo, prima di
consegnare gli output, prima di scrivere ognuno e dopo l'ultimo: `timeout`
(exit 5, `EXECUTION_DEADLINE_EXCEEDED`) o `cancelled` (exit 130,
`EXECUTION_CANCELLED`), con la fase del punto di controllo. Dopo l'ultima
scrittura (fase `finalize`) gli output sono tutti alla destinazione e
l'effetto è `committed`, ritentativo `requires_recovery`: un annullamento
arrivato durante l'ultima scrittura non diventa un successo. Se il gestore
dei segnali non si installa, `describe`, `validate` e `run` non partono
(`internal`): dichiarano l'annullamento, e non girano senza.

### Capacità e attributi

`capabilities` descrive il binario che risponde: un'interfaccia (`cli`,
`plenora-cli-v2`, artefatto `plenora-data`) e le quattro operazioni del
catalogo pubblico con i suoi contratti, tipi di contenuto e controlli. Gli
`attributes` seguono il contratto `plenora-data-capability-attributes-v1`
(di questo componente; CAP-013):

| campo | significato |
| --- | --- |
| `contract` | `plenora-data-capability-attributes-v1` |
| `kernel_registry` | `plenora-data-kernel-catalog-v1`: l'operazione usa il registro di `catalog` |
| `extension_content_types.input`, `.output` | tipi in più rispetto al catalogo pubblico: `application/vnd.apache.parquet` |
| `bounded_materialization` | `true`: le tabelle si materializzano intere, entro il budget (ARROW-011) |

La superficie Rust è `plenora_cli::api::{catalogo, descrivi, valida,
esegui}`; la mappa operazione → export (`plenora_cli::capacita::mappa_rust`,
contratto `plenora-data-rust-surface-v1`) nasce dalla stessa tabella, e
`tests/superficie_rust.rs` la compila da consumatore.

### Verifica e adozione

- `cargo test -p plenora-cli`: il binario come sottoprocesso (stdout,
  stderr, codice, documento intero) contro gli schemi dei contratti copiati
  in `crates/plenora-cli/tests/fixtures/contratti/` con il loro SHA-256
  (`provenienza.json`): inviluppo, errore, capacità (più CAP-005 e
  CAP-007), registro dei kernel contro `data-kernels-v1`, operazioni contro
  `data-tools-v1`, comandi contro `bindings/cli-v1.json`, diagnostica per
  riga, canarini (dati e percorsi) mai nell'uscita;
- `python scripts/verifica_cli_contratti.py --binario <plenora-data>
  --contratti <checkout di plenora-contracts>`: le stesse verifiche di
  scoperta e d'errore con `jsonschema` e `tools/conformance_checks.py` dei
  contratti (il venv dei contratti ha i pacchetti);
- `python scripts/genera_manifesto_adozione.py`: il manifesto v4 dagli
  artefatti costruiti (versione e digest), con la sorgente
  `crates/plenora-cli/adozione.json` (pin, contratti, deviazioni). Non c'è
  un manifesto nel repository: senza un artefatto rilasciato il digest
  sarebbe di una build qualsiasi.

### Deviazioni dai contratti

Volute, da portare nell'aggiornamento dei contratti (e nella sorgente del
manifesto, `crates/plenora-cli/adozione.json`).

- **Formato del piano.**
  *Regola*: il profilo data-tools e Plan Budget 1.0 (PLAN-003) vogliono i
  piani `schema_version` 4, 5 e 6 con l'hash di piano, e l'equivalenza 4 =
  5.
  *Ambito*: `validate`, `run`, `plenora_cli::api`.
  *Hazard*: si accetta solo `plenora-data-plan-v1` (`"version": 1`, passi
  SSA, [«Il piano»](#il-piano)); un piano 4, 5 o 6 si rifiuta con
  `invalid_plan`, e non c'è un hash di piano. Nessun rischio silenzioso: il
  rifiuto è esplicito, e `catalog` dichiara `plan_format`.
  *Rientro*: il formato `plenora-data-plan-v1` nei contratti, al loro
  aggiornamento.
- **Versioni dei kernel.**
  *Regola*: Public Catalogs 1.0, sezione 6: identità e versione dei kernel
  uguali al registro comune `data-kernels-v1`, che dice 1 per tutti.
  *Ambito*: `catalog`.
  *Hazard*: `version` è la versione della semantica osservabile (104 kernel
  su 146 oltre 1), con le quattro versioni in `versions`; un consumatore che
  confronta con il registro comune vede la differenza, non la perde.
  *Rientro*: un registro `data-kernels-v2` con le versioni vere.
- **`table.transpose` non eseguibile.**
  *Regola*: il registro dichiara 146 kernel.
  *Ambito*: `catalog`, `validate`, `run`.
  *Hazard*: `transpose` è `unavailable` con il motivo e non sta nel
  `registry`; un piano che la usa fallisce in validazione (`unsupported`).
  *Rientro*: uno schema d'uscita di `transpose` fissato dalla config.
- **Effetto di `data.run`.**
  *Regola*: il catalogo pubblico dichiara `side_effect: none`.
  *Ambito*: `capabilities`, `run`.
  *Hazard*: la CLI scrive i file d'uscita, quindi dichiara `local`:
  dichiarare `none` per un comando che scrive file sarebbe falso.
  *Rientro*: il catalogo distingue l'effetto della superficie CLI (file) da
  quello del runtime (byte resi).
- **Più output.**
  *Regola*: il binding CLI ha un solo `--output OUTPUT.arrow`.
  *Ambito*: `run`.
  *Hazard*: nessuno per un piano con un output (la forma canonica vale);
  con più output ognuno si nomina, e `--output PATH` senza nome si rifiuta.
  *Rientro*: il binding con `--output NAME=PATH`.
- **Materializzazione limitata.**
  *Regola*: ARROW-011, lo stream si consuma senza materializzare tutto,
  salvo dichiarazione.
  *Ambito*: `describe`, `validate`, `run` (anche lo stream d'uscita si
  scrive da una tabella intera).
  *Hazard*: dichiarato (`bounded_materialization`); le tabelle stanno nel
  budget del piano ([«Budget di memoria»](#budget-di-memoria)).
  *Rientro*: nessuno previsto (decisione del maintainer: niente streaming).
- **Parquet.**
  *Regola*: il profilo scambia Arrow.
  *Ambito*: `describe`, `validate`, `run`.
  *Hazard*: Parquet in ingresso e in uscita è un'estensione dichiarata
  negli attributi, fuori da `content_types`.
  *Rientro*: un tipo Parquet nel catalogo pubblico, se servirà.
- **Frammento del budget.**
  *Regola*: Plan Budget 1.0 (PLAN-007..PLAN-021: `max_domain_memory_bytes`,
  formato 6, profilo isolato).
  *Ambito*: `limits` del piano.
  *Hazard*: c'è solo `max_governed_memory_bytes` (default pubblicato
  `DEFAULT_MAX_GOVERNED_MEMORY_BYTES`, 536 870 912 byte) con i limiti di
  righe e testi; il profilo isolato non esiste.
  *Rientro*: con l'isolamento, se arriverà.

### Limiti dichiarati della CLI

- **`validate` legge le tabelle intere.**
  *Regola*: la validazione guarda solo gli schemi.
  *Ambito*: `validate`, `plenora_io::valida_da_file`.
  *Hazard*: per avere lo schema si leggono gli input interi, entro il
  budget del piano: costa tempo e memoria quanto la lettura di `run`, e un
  input oltre il budget fallisce anche in validazione.
  *Rientro*: una lettura del solo schema (footer IPC, metadati Parquet)
  con le verifiche del confine di lettura.
- **Annullamento dal sistema operativo non provato da un test.**
  *Regola*: Ctrl-C e SIGTERM diventano `cancelled`, exit 130.
  *Ambito*: `main.rs` e il gestore di `ctrlc`.
  *Hazard*: i test alzano il segnale direttamente (lo stesso
  `Arc<AtomicBool>` del gestore); che il sistema operativo consegni Ctrl-C
  al gestore lo garantisce `ctrlc`, non un test di questo repository
  (mandare Ctrl-C a un sottoprocesso vorrebbe FFI). Un secondo Ctrl-C non
  interrompe di più: il processo finisce al controllo successivo.
  *Rientro*: un test d'integrazione Unix che manda SIGTERM al sottoprocesso.
- **Controlli fra le fasi, mai dentro.** Scadenza e annullamento si
  controllano fra letture, passi e scritture ([«Limiti dichiarati del
  runner»](#limiti-dichiarati-del-runner)): la lettura di un file grande o
  un passo lungo finiscono anche oltre la scadenza.
- **Stdout non scrivibile.**
  *Regola*: CLI 2.0, sezione 4: un documento JSON completo su stdout.
  *Ambito*: ogni comando (`plenora_cli::consegna`).
  *Hazard*: con una pipe chiusa o un disco pieno il documento manca o è
  troncato; il processo esce con 5 (la categoria `io`), mai 0, non scrive un
  secondo documento e niente su stderr. Il codice non dice più l'esito del
  comando: un `run` può aver scritto i suoi output, quindi chi riceve 5
  senza un documento completo tratta l'esito come ignoto.
  *Rientro*: nessuno dentro il processo (stdout è il solo canale del
  contratto); un chiamante che legge stdout fino in fondo non lo vede.
- **Panici fuori dal thread principale.**
  *Regola*: un panico diventa `internal` (CLI 2.0, sezione 6).
  *Ambito*: i thread che la CLI non intercetta con `catch_unwind`: il
  thread del gestore di Ctrl-C (`ctrlc` vi chiama `expect`).
  *Hazard*: l'hook di `main.rs` conta, senza payload, ogni panico fuori
  dalle barriere di dipendenza in qualunque thread
  (`panic_policy::panici_fuori_dalle_barriere`), e un conto cambiato
  durante l'invocazione trasforma un successo in `internal` (exit 70,
  effetto `unknown` per `run`). Il controllo è alla fine, non ai punti di
  controllo dell'annullamento: se il thread del gestore muore, il comando
  prosegue senza annullamento fino in fondo, e solo allora fallisce.
  *Rientro*: il conto letto anche ai punti di controllo dell'`Interruzione`.
- **Aborti senza inviluppo.**
  *Regola*: CLI 2.0, sezione 4: un documento su stdout e niente su stderr
  in ogni caso.
  *Ambito*: ogni comando.
  *Hazard*: un'allocazione impossibile (la libreria standard scrive
  «memory allocation of N bytes failed» su stderr e abortisce), uno stack
  esaurito e un panico dentro un `Drop` durante un altro panico terminano
  il processo senza inviluppo e senza passare dall'hook; il codice d'uscita
  è quello dell'aborto del sistema operativo, non uno della tabella sopra.
  Con file costruiti apposta l'allocazione impossibile è raggiungibile
  (limite «File costruiti apposta: aborto del processo» di
  [«File»](#file)). Garanzia indebolita: un chiamante che non trova un
  documento su stdout deve trattare l'esito come ignoto.
  *Rientro*: la CLI in un processo figlio sorvegliato da un processo padre
  che trasforma l'aborto in un errore `internal` con effetto `unknown`.
- **Messaggi delle config con i valori del piano.**
  *Regola*: i messaggi pubblici non portano valori di righe o colonne
  ([«Errori»](#errori)).
  *Ambito*: config dei passi rifiutate dalla deserializzazione
  (`config non valida: …`), con il testo di `serde`.
  *Hazard*: il testo può citare un valore scritto nella config del piano
  (un tipo sbagliato, una variante sconosciuta): è testo del piano, non dei
  dati, ma finisce nel messaggio pubblico. La lettura del piano intero
  (`Pipeline::from_json`) invece dice solo genere e posizione.
  *Rientro*: la stessa riduzione per le config, quando i messaggi dei
  campi sconosciuti (oggi utili a chi scrive il piano) avranno un codice.
- **Attributi senza schema JSON.** Il contratto
  `plenora-data-capability-attributes-v1` e i documenti dei risultati
  (`plenora-data-*-v1`) sono descritti qui, non da uno schema JSON
  pubblicato; i test ne verificano la forma campo per campo.

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
`plenora_kernels_geo::crs::validate_geometry_domain`, dopo la decodifica;
nel runner, su ogni colonna geometria d'ingresso di un passo geo e sulle
geometrie prodotte ([«Operazioni geo»](#operazioni-geo)).

**Precisione.** `ResolvedCrs::precisione_coordinate()` esprime 1 cm a terra
nelle unità del CRS: `0.01 / horizontal_unit_to_metre` per i proiettati,
`0.01` metri in gradi sul raggio di curvatura massimo `a / (1 - f)`
dell'ellissoide per i geografici; `None` per un geografico senza
ellissoide o quando il quoziente non
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
- **Nel runner, un passo per volta.** Il runner esegue `geo.reproject`
  con `reproject_batches` e analizza il passo seguente sul contratto con
  il CRS d'arrivo ([«Operazioni geo»](#operazioni-geo)). Un esecutore
  futuro che fonda le trasformazioni in place (`TransformInPlace`) deve
  rileggere il CRS dopo `geo.reproject`, che lo cambia a metà del gruppo.
- **Fuori ambito.** CRS fuori tabella, operazioni concatenate del registro,
  griglie non NTv2, percorsi di più di tre passi, CGCS2000 verso altri
  datum (nessuna trasformazione nel registro), coordinate Z/M.
