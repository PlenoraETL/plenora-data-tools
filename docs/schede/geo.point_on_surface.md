### Che cosa fa

Sostituisce ogni geometria con un `Point` che le appartiene, nella stessa
colonna. Per i poligoni è il punto medio del tratto interno più lungo di
una linea orizzontale a metà altezza dell'ingombro, spostata se passa per
un vertice (se nessun tratto risulta interno, il primo vertice); in un
`MultiPolygon` vince il tratto più lungo fra le parti; per le linee il vertice non estremo più vicino al
centroide, o il primo se la linea ha due vertici; per i punti quello più
vicino al centroide. A differenza del centroide il punto sta sempre sulla
geometria. Per una geometria vuota il kernel non dà un punto.

### Parametri

Nessuno: la config è `{}`.

### Schema

La colonna geometria resta al suo posto con lo stesso nome, CRS e
dimensioni (`xy`); le altre colonne, i metadati di schema e le proprietà
del contratto (`sorted_by`, `row_count`) passano invariati. I tipi
dichiarati della colonna diventano `exact` [`Point`]: le chiavi
`plenora.geometry.types` e `plenora.geometry.types_declaration` ereditate
si tolgono dal campo.

### Righe

1:1 per contratto. Il kernel (`operations::point_on_surface`) lavora su
una geometria alla volta e nessun adapter lo chiama ancora sulle righe: il
trattamento di una cella nulla, e la resa del caso senza punto (geometria
vuota), non sono definiti da codice eseguito.

### Ordine

L'ordine d'ingresso: l'analisi conserva `sorted_by`.

### Errori

In validazione (analisi del contratto):

- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non si riconosce come WKB (né estensione `geoarrow.wkb` né chiavi
  `plenora.geometry.*`);
- `Unsupported`: dimensioni della geometria diverse da `xy`;
- `InvalidPlan`: config diversa da `{}`;
- `Crs`: CRS della colonna assente o non risolto, geografico, o proiettato
  senza unità lineare.

In esecuzione, dal kernel, per geometria (`OperationError`, che nessun
codice traduce ancora in `PlenoraError`):

- `InvalidInput`: la geometria non supera la validazione OGC;
- `ValidazioneNonConclusa`, `CalcoloNonConcluso`: la validazione OGC o
  `interior_point` di `geo` (che passa da `relate`, capace di andare in
  panico anche su geometrie valide) vanno in panico dentro la barriera (il
  messaggio porta solo la forma del payload).

### Limiti e deviazioni

Il runner non esegue ancora le operazioni geo
([README, «Che cosa non c'è ancora»](../README.md#che-cosa-non-cè-ancora)).
Il punto è quello di `interior_point` di `geo`, che non coincide
necessariamente con quello di `ST_PointOnSurface` di PostGIS. Le coordinate
`-0.0` si portano a `0.0` su una copia prima del calcolo: con `-0.0` e
`0.0` insieme la scansione di `geo` 0.33.1 rende in release un punto
sbagliato senza errore.

### Precisione

Nessuna griglia e nessun rifiuto `PrecisionInsufficient`. Per linee e
punti il risultato è un vertice d'ingresso, esatto; per i poligoni le
coordinate sono calcolate in `f64` da `geo` (punto medio del tratto) e
`relate` conferma che il punto sta dentro: la garanzia è topologica, senza
un bilancio metrico rispetto alla regola di 1 cm
([README, «Precisione delle operazioni geografiche»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

### Complessità

Per un poligono di n vertici: la scansione di `geo` incrocia la linea con
tutti i lati (sweep line, O(n log n) più le intersezioni) e verifica i
tratti, dal più lungo, con `relate` finché uno è interno; per linee e punti
O(n). Più la validazione OGC
dell'ingresso (sub-quadratica nel caso tipico, O(n²) nel peggiore:
[README, «Validazione OGC»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì)).

### Esempio

```json
{
  "config": {},
  "ingressi": [
    {"nome": "lotti", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POLYGON((0 0,4 0,4 2,0 2,0 0))", "LINESTRING(0 0,2 0,4 0)"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POINT(2 1)", "POINT(2 0)"]}
  ]}
}
```
