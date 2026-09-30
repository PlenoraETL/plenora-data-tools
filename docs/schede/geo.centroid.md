### Che cosa fa

Sostituisce ogni geometria con il suo centroide, un `Point`, nella stessa
colonna. Per le geometrie areali è il baricentro pesato per l'area, per le
lineari quello pesato per la lunghezza, per i punti la media delle
coordinate. In una collezione contano solo le parti della dimensione più
alta: un punto accanto a un poligono non sposta il centroide del poligono.

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

1:1: il runner chiama il kernel (`transform_geometry` con
`Operation::Centroid`) su ogni cella non nulla, in parallelo, e rimette la
geometria al suo posto; una cella nulla resta nulla ([README, «Operazioni geo»](../README.md#operazioni-geo)).

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

Poi dal kernel, per geometria:

- `InvalidPlan`: la geometria d'ingresso non supera la validazione OGC, o
  è vuota (nessun centroide);
- `Internal`: la validazione OGC o il calcolo di `geo` non concludono
  (panico dentro la barriera; il messaggio porta solo la forma del
  payload).

Una geometria prodotta oltre il limite di byte per cella (64 MiB di WKB)
è `ResourceLimit`. Il primo errore è quello della prima riga in ordine
di riga, senza diagnostica per riga.

### Limiti e deviazioni

Nel runner un errore non ha diagnostica per riga e il costo in memoria è
una previsione provvisoria
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voci «Geo senza diagnostica per riga» e «Modelli di costo geo provvisori»).
Una geometria vuota è un errore, dove GEOS e PostGIS rendono `POINT EMPTY`.

### Precisione

Nessuna griglia e nessun rifiuto `PrecisionInsufficient`: il centroide è il
calcolo in `f64` di `geo` (ogni anello traslato sul suo primo vertice prima
delle somme), senza un bilancio d'errore dichiarato rispetto alla regola di
1 cm ([README, «Precisione delle operazioni geografiche»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

### Complessità

Per geometria di n vertici: tempo O(n) per il calcolo, più la validazione
OGC dell'ingresso (sub-quadratica nel caso tipico, O(n²) nel peggiore:
[README, «Validazione OGC»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì));
memoria O(n) per la validazione.

### Esempio

```json
{
  "config": {},
  "ingressi": [
    {"nome": "lotti", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POLYGON((0 0,4 0,4 2,0 2,0 0))", "LINESTRING(0 0,10 0)"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POINT(2 1)", "POINT(5 0)"]}
  ]}
}
```
