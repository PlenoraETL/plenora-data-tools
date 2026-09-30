### Che cosa fa

Sostituisce ogni geometria con il suo confine OGC, nella stessa colonna:

- poligoni e multi-poligoni: una `MultiLineString` con tutti gli anelli,
  per ogni poligono l'esterno e poi i buchi;
- linea aperta: il `MultiPoint` dei due estremi; linea chiusa o vuota:
  `MULTIPOINT EMPTY`;
- `MultiLineString`: il `MultiPoint` degli estremi delle linee aperte che
  compaiono un numero dispari di volte (regola mod-2);
- punti e multi-punti: `GEOMETRYCOLLECTION EMPTY`;
- `GeometryCollection`: la collezione dei confini dei membri.

### Parametri

Nessuno: la config è `{}`.

### Schema

La colonna geometria resta al suo posto con lo stesso nome, CRS e
dimensioni (`xy`); le altre colonne, i metadati di schema e le proprietà
del contratto (`sorted_by`, `row_count`) passano invariati. I tipi
dichiarati della colonna diventano `exact` [`MultiPoint`,
`MultiLineString`, `GeometryCollection`]: le chiavi
`plenora.geometry.types` e `plenora.geometry.types_declaration` ereditate
si tolgono dal campo.

### Righe

1:1: il runner chiama il kernel (`operations::boundary`) su ogni cella non
nulla, in parallelo, e rimette la geometria al suo posto; una cella
nulla resta nulla ([README, «Operazioni geo»](../README.md#operazioni-geo)).

### Ordine

L'ordine d'ingresso: l'analisi conserva `sorted_by`. Gli anelli escono
nell'ordine d'ingresso; gli estremi di una `MultiLineString` in un ordine
deterministico per rappresentazione binaria delle coordinate (x, poi y),
non nell'ordine d'ingresso né in quello numerico per i valori negativi.

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
- `InvalidOutput`: il confine prodotto non supera la validazione OGC;
- `ValidazioneNonConclusa`: la validazione OGC va in panico dentro la
  barriera (il messaggio porta solo la forma del payload).

Una geometria prodotta oltre il limite di byte per cella (64 MiB di WKB)
è `ResourceLimit`. Il primo errore è quello della prima riga in ordine di riga, senza
diagnostica per riga.

### Limiti e deviazioni

Nel runner un errore non ha diagnostica per riga e il costo in memoria è
una previsione dalle misure
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).
Il confine di una `GeometryCollection`, che GEOS rifiuta, qui è la
collezione dei confini dei membri. Negli estremi di una `MultiLineString`
`-0.0` e `0.0` sono lo stesso punto.

### Precisione

Esatta: anelli ed estremi sono coordinate d'ingresso copiate (`-0.0`
diventa `0.0` negli estremi di una `MultiLineString`, stesso valore)
([README, «Precisione delle operazioni geografiche»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

### Complessità

Per geometria di n vertici: tempo O(n), O(n log n) per gli estremi di una
`MultiLineString` (mappa ordinata), memoria O(n); più la validazione OGC
dell'ingresso e dell'uscita (sub-quadratica nel caso tipico, O(n²) nel
peggiore:
[README, «Validazione OGC»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì)).

### Esempio

```json
{
  "config": {},
  "ingressi": [
    {"nome": "forme", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POLYGON((0 0,4 0,4 4,0 4,0 0))", "LINESTRING(0 0,2 0)", "POINT(1 1)"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["MULTILINESTRING((0 0,4 0,4 4,0 4,0 0))", "MULTIPOINT((0 0),(2 0))", "GEOMETRYCOLLECTION EMPTY"]}
  ]}
}
```
