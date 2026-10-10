# Limiti dichiarati

Dove le garanzie si fermano. Un limite dice la regola, dove vale
(ambito), che cosa può andare storto senza errore (hazard) e a quale
condizione il limite cade (rientro). Questo documento raccoglie i limiti
che attraversano più parti del codice; quelli di un solo sottosistema
stanno nella sua guida:

| sottosistema | limiti |
| --- | --- |
| runner | [`runner.md`, «Limiti dichiarati del runner»](runner.md#limiti-dichiarati-del-runner) |
| metadati Arrow | [`metadati-arrow.md`, «Limiti dichiarati»](metadati-arrow.md#limiti-dichiarati) |
| file | [`file.md`, «Limiti dichiarati»](file.md#limiti-dichiarati) |
| CLI | [`cli.md`, «Limiti dichiarati della CLI»](cli.md#limiti-dichiarati-della-cli) |
| CRS integrati | [`crs.md`, «Limiti dei CRS integrati»](crs.md#limiti-dei-crs-integrati) |
| riproiezione | [`riproiezione.md`, «Limiti dichiarati della riproiezione»](riproiezione.md#limiti-dichiarati-della-riproiezione) |
| operazioni topologiche | [`topologia.md`, «Differenze da GEOS»](topologia.md#differenze-da-geos) |

## Precisione delle operazioni geografiche: 1 cm a terra

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
(vedi [«Differenze da GEOS»](topologia.md#differenze-da-geos)).

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

## Misure geodetiche: l'ellissoide del datum

**Regola.** `geo.geodesic_distance`, `geo.geodesic_line_length`,
`geo.geodesic_area` e `geo.bearing` risolvono il problema geodetico
(Karney 2013, `geographiclib-rs`) sull'**ellissoide del datum del CRS
della colonna**, quello della tabella integrata
([«CRS integrati»](crs.md#crs-integrati)): Internazionale 1924 per ED50 e Monte
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

## `geo.reproject`: il cambio di datum vale quanto l'accuratezza accettata

**Regola.** La matematica della riproiezione resta entro la precisione
(proiezioni e trasformazioni entro 1e-8 m da PROJ, lati densificati entro
mezzo centimetro); il **cambio di datum** vale quanto l'accuratezza EPSG del
percorso fra i datum. Oltre 1 cm il cambio si rifiuta
(`REPROJECTION_ACCURACY_NOT_ACCEPTED`) salvo che la config dichiari
`accuratezza_accettata_m` almeno pari: è una garanzia indebolita per scelta
esplicita di chi scrive il piano, mai implicita.

**Ambito.** `geo.reproject` fra datum diversi non equivalenti per il
registro ([«La regola dell'accuratezza»](riproiezione.md#la-regola-dellaccuratezza)).

**Hazard.** Il risultato può scostarsi dal vero fino all'accuratezza
accettata (metri per Monte Mario, ED50, OSGB36, NAD27 senza griglie), senza
errore; due geometrie vicine possono usare percorsi diversi e scostarsi fra
loro fino alla somma delle due accuratezze. WGS 84 e la famiglia ETRS89
sono equivalenti per convenzione (EPSG:1149 conta 0,
[«WGS 84 = ETRS89 per convenzione»](riproiezione.md#wgs-84--etrs89-per-convenzione)):
oggi 50–80 cm reali in Europa, senza errore, salvo
`convenzione_wgs84_etrs89: false`. Gli altri limiti sono in
[«Limiti dichiarati della riproiezione»](riproiezione.md#limiti-dichiarati-della-riproiezione).

**Condizione di rientro.** Nessuna: è l'accuratezza del registro. Con le
griglie NTv2 ufficiali (per esempio IGM per l'Italia) l'accuratezza scende a
quella della griglia.

## Validazione OGC: la ricerca delle auto-intersezioni non è quella di `geo`, il verdetto sì

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

## `geo.nearest`: lo scarto dell'R-tree si appoggia alla stima d'errore di `geo`

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

## Hash delle chiavi non keyed

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

## Memoria delle chiavi dei kernel in memoria non governata

**Regola.** `aggregate`, `distinct`/`dedup_advanced`, le set operation,
`assert_unique` e `table_diff` non contabilizzano le proprie strutture di
chiavi (arena, indici, gruppi) su `max_governed_memory_bytes`: nel runner
le prevede il modello di costo del passo, che il budget controlla prima di
eseguirlo, senza contarle.

**Ambito.** I kernel elencati.

**Hazard.** Con molte chiavi distinte il picco reale supera la stima
dell'input: l'arena delle chiavi, due `usize` e una voce di mappa per chiave
distinta, fino a due `usize` per riga per l'assegnazione ai gruppi. Nel
runner il modello di costo ([«Budget di memoria»](runner.md#budget-di-memoria))
prevede queste strutture sul caso peggiore misurato (fixture `distinct`,
chiavi tutte distinte), senza contarle.

**Condizione di rientro.** Contabilità esplicita delle strutture di chiavi,
con errore `ResourceLimit` oltre il budget.

## Letterali JSON oltre `u64`

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

## Nomi delle colonne d'uscita

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

## Somme intere esatte e tipi delle riduzioni

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

## Colonne temporali e formati di data

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

## `geo.make_valid`, `geo.polygonize`, `geo.split`: equivalenza a GEOS verificata, non dimostrata

**Regola.** Le tre operazioni girano sui kernel Rust del laboratorio
(`plenora_kernels_geo::rust_backend`), non su GEOS. L'equivalenza con GEOS
è semantica (stesse facce, stessi residui, stessa area) e poggia su una
campagna, non su una prova: 28.672 confronti differenziali e 861 casi curati
nel laboratorio, eseguiti su `geo` 0.33.1 **non patchato**. Qui `geo` ha
`orient2d` esatto: le prove indipendenti da GEOS sono rieseguite, quelle
differenziali no. Le differenze note sono in
[«Differenze da GEOS»](topologia.md#differenze-da-geos).

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

## `geo.delaunay` e `geo.voronoi`: triangolazione caricata in blocco

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

## Profilo di compilazione di chi usa i crate come dipendenza

**Regola.** Un overflow aritmetico intero è un panico dichiarato anche in
release, mai un valore avvolto in silenzio: `[profile.release]` del
`Cargo.toml` radice ha `overflow-checks = true`, e così `fuzz/`.

**Ambito.** Chi compila i crate di data-tools come dipendenza (per percorso
o git) con un proprio workspace. Cargo applica i profili, come le
`[patch]`, solo dal workspace radice: il profilo del consumatore decide
anche per il codice di data-tools e delle sue dipendenze. Le copie
vendorizzate di `geo`, `wkt` e `parquet` invece arrivano al consumatore
(pacchetti con nome proprio, `tests/consumatore_esterno.rs` di
`plenora-cli`).

**Hazard.** In una build di release con il profilo di default di Cargo
(`overflow-checks = false`) l'aritmetica intera non controllata avvolge.
Dove questo workspace conta sul panico, per esempio la somma non
controllata di `concat_run_arrays` di Arrow 60.0.0 su fini `Int16`
([«Runner»](runner.md#runner)), il consumatore avrebbe un valore sbagliato
invece di un errore. Nessuna prova lo rileva dal consumatore.

**Condizione di rientro.** Il consumatore dichiara `overflow-checks = true`
nel proprio `[profile.release]`, oppure una guardia a runtime che rifiuta
di eseguire un piano in una build senza controlli di overflow.
