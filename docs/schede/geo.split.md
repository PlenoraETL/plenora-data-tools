### Che cosa fa

Divide la geometria di ogni riga con una lama lineare e scrive una riga per
parte, con gli attributi della riga d'origine e il suo indice in
`__parent_index`. Una sorgente `Polygon` o `MultiPolygon` si taglia in
Rust puro: bordo e lama si nodano insieme, se ne estraggono le facce e si
tengono quelle interne alla sorgente, poi area e copertura del bordo si
verificano contro la sorgente. Una sorgente `LineString` si spezza nei
punti in cui la lama la tocca, entro `tolerance`.

La lama arriva dalla config (`other_wkb`), nello stesso CRS della colonna.
L'esecuzione Arrow (`rust_backend::arrow::split_batches`) riceve una lama
per riga, allineata alle sorgenti; il piano ne dichiara una sola. Il runner
non la chiama ancora.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `other_wkb` | stringa | obbligatorio | WKB esadecimale, coordinate nel dominio del CRS della colonna | la lama: linee (`LineString`, `MultiLineString`, collezioni di linee) per le sorgenti poligonali; per le sorgenti lineari anche punti e contorni di poligoni |
| `tolerance` | numero | `0` | finito, `>= 0` | distanza entro cui un punto della lama taglia una sorgente `LineString` |

`tolerance` vale solo per le sorgenti `LineString`: sulle poligonali si
accetta e non ha effetto.

### Schema

Le colonne dell'ingresso, nelle stesse posizioni e con gli stessi tipi, più
`__parent_index` (`uint64`, non nullable) in coda. Il campo geometria
conserva i metadati d'ingresso tranne la dichiarazione dei tipi, e la sua
nullabilità. Il contratto dichiara i tipi `exact`: `Polygon` per sorgenti
`Polygon`/`MultiPolygon`, `LineString` per sorgenti `LineString`, entrambi
se l'ingresso non ha una dichiarazione `exact` che li restringa. Resta
`sorted_by`; `row_count` cade.

### Righe

Espansione 1:N: ogni sorgente dà le sue parti (una sola se la lama non la
taglia). Su una sorgente lineare tagli consecutivi distanti al più
`tolerance` lungo la linea si fondono in uno; un tratto collineare della lama taglia ai suoi
due estremi. Una riga con la sorgente o la lama
nulla non produce righe, ma una cella non nulla dell'altro lato si
decodifica e si valida lo stesso. Le facce del taglio che cadono fuori
dalla sorgente (una lama chiusa che sporge) si scartano.

### Ordine

Quello delle sorgenti; dentro una sorgente poligonale, l'ordine delle facce
del polygonize interno, non quello di GEOS; dentro una sorgente lineare,
dall'inizio alla fine della linea.

### Errori

In validazione (analisi del contratto):

- `InvalidPlan`: config con campi sconosciuti; `other_wkb` mancante, non
  esadecimale o strutturalmente malformato; `tolerance` negativa o non
  finita;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come WKB; una colonna `__parent_index` esiste
  già;
- `Unsupported`: dimensioni della geometria diverse da `xy`; `other_wkb`
  con dimensioni Z/M o SRID;
- `Crs`: colonna senza CRS risolto o CRS non proiettato; coordinate della
  lama fuori dal dominio del CRS.

In esecuzione (esecuzione Arrow):

- `InvalidPlan`: righe di sorgenti e lame non allineate; sorgente di tipo
  diverso da `LineString`, `Polygon`, `MultiPolygon`; lama di tipo non
  ammesso; WKB malformato o OGC-invalido; limiti superati (coordinate per
  cella, per ciascun ingresso e per la loro somma; 100.000.000 coppie di
  noding o test d'intersezione; righe d'uscita oltre `max_output_rows`,
  cumulate su tutte le righe); area non conservata (`AreaMismatch`) o bordo
  non ricoperto (`CoverageMismatch`);
- `ResourceLimit`: cella oltre il limite di byte; prenotazione di memoria
  fallita;
- `Unsupported`: noding non convergente; segno d'area non decidibile;
  `PrecisionInsufficient` (sotto, «Precisione»);
- `Internal`: panico di `geo` dentro il kernel, invariante violata.

### Limiti e deviazioni

- Lo split poligonale gira sul kernel del laboratorio, qualificato contro
  GEOS per equivalenza semantica
  ([README, «`geo.make_valid`, `geo.polygonize`, `geo.split`: equivalenza a
  GEOS verificata, non dimostrata»](../README.md#geomake_valid-geopolygonize-geosplit-equivalenza-a-geos-verificata-non-dimostrata));
  lo split lineare è il codice precedente al porting.
- La scelta delle facce usa un campione interno e un test pari-dispari con
  il lato del punto deciso in modo esatto, non `point_on_surface` e `covers`
  di GEOS; area e copertura del bordo sono verificate dopo: un'incoerenza è
  un errore, mai parti in più o in meno.
- Il budget di parti e coordinate conta tutto l'output del polygonize
  interno, anche facce fuori dalla sorgente e residui scartati, non solo le
  parti tenute; il limite di coordinate vale per ciascun ingresso e per la
  somma.
- Elenco completo: [README, «Differenze da GEOS»](../README.md#differenze-da-geos).

### Precisione

Sorgenti poligonali
([README, «Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)):

- guardia di spaziatura delle coordinate e noding del polygonize interno:
  ogni incrocio arrotondato entro `p / 5` dai segmenti che divide, al più
  cinque giri; oltre, `PrecisionInsufficient`;
- verifiche locali dopo il calcolo: l'area delle parti può differire da
  quella della sorgente al più di `p` per la lunghezza dei lati di bordo con
  un estremo calcolato dal noding (più l'arrotondamento delle aree), e la
  lunghezza del bordo sorgente non coperta dalle parti, sommata per anello,
  al più `p`. Oltre, `AreaMismatch` o `CoverageMismatch`: una parte mancante
  più larga di 1 cm è sempre un errore.

Sorgenti lineari: prima del taglio la stessa guardia di spaziatura, che
tiene il margine numerico di `split_line` sotto `p / 2`: un punto a più di
1 cm dalla linea, con `tolerance` nulla, non taglia.

Nessuna griglia di `i_overlay`. `p` è la precisione del CRS della colonna.

### Complessità

Per riga, sorgente poligonale con `n` segmenti fra bordo e lama: noding
O(n²) coppie nel caso peggiore (tetto 100.000.000), facce O(e log e), poi la
verifica della copertura, O(b · s) con `b` i lati di bordo delle parti e
`s` i lati della sorgente. Sorgente lineare: O(s · l) test fra segmenti
della sorgente e primitive della lama, entro 100.000.000. Memoria O(n) per
la riga più le parti prodotte; l'uscita si costruisce in un batch solo.

### Esempio

Un quadrato tagliato da una retta verticale (`other_wkb` è
`LINESTRING(5 -1,5 11)`).

```json
{
  "config": {"other_wkb": "0102000000020000000000000000001440000000000000f0bf00000000000014400000000000002640"},
  "ingressi": [
    {"nome": "lotti", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [7]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POLYGON((0 0,10 0,10 10,0 10,0 0))"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [7, 7]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POLYGON((0 10,0 0,5 0,5 10,0 10))", "POLYGON((5 10,5 0,10 0,10 10,5 10))"]},
    {"nome": "__parent_index", "tipo": "uint64", "valori": [0, 0]}
  ]}
}
```
