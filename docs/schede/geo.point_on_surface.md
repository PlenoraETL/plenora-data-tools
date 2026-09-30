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

1:1: il runner chiama il kernel (`operations::point_on_surface`) su ogni cella non
nulla, in parallelo, e rimette la geometria al suo posto; una cella
nulla resta nulla ([README, «Operazioni geo»](../README.md#operazioni-geo)).
Una geometria senza punto interno (vuota) dà null: la colonna d'uscita ha
la nullabilità di quella d'ingresso, e se non ammette null il passo si
rifiuta con `InvalidPlan`.

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

In esecuzione, prima del kernel, su tutta la colonna: `InvalidPlan` per
una cella che non è WKB strutturalmente valido, `Crs` per una coordinata
fuori dal dominio di validità del CRS della colonna, `Schema` per una
geometria di un tipo che il contratto d'ingresso non dichiara, quando li
dichiara con un elenco ([README, «Operazioni geo»](../README.md#operazioni-geo)).

Poi dal kernel, per geometria (`OperationError`, che il runner porta in `PlenoraError`:
`Internal` per `Internal`, `ValidazioneNonConclusa` e
`CalcoloNonConcluso`, `Unsupported` per `PrecisionInsufficient`,
`InvalidPlan` per le altre):

- `InvalidInput`: la geometria non supera la validazione OGC;
- `ValidazioneNonConclusa`, `CalcoloNonConcluso`: la validazione OGC o
  `interior_point` di `geo` (che passa da `relate`, capace di andare in
  panico anche su geometrie valide) vanno in panico dentro la barriera (il
  messaggio porta solo la forma del payload).

Dopo il kernel, `InvalidPlan` per una geometria vuota in una colonna che il
contratto dichiara non nullable (sopra, «Righe»). Il primo errore è quello della prima riga in ordine di riga, senza
diagnostica per riga.

### Limiti e deviazioni

Nel runner un errore non ha diagnostica per riga e il costo in memoria è
una previsione dalle misure
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).
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
