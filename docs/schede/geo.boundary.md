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

1:1 per contratto. Il kernel (`operations::boundary`) lavora su una
geometria alla volta e nessun adapter lo chiama ancora sulle righe: il
trattamento di una cella nulla non è definito da codice eseguito.

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

In esecuzione, dal kernel, per geometria (`OperationError`, che nessun
codice traduce ancora in `PlenoraError`):

- `InvalidInput`: la geometria non supera la validazione OGC;
- `InvalidOutput`: il confine prodotto non supera la validazione OGC;
- `ValidazioneNonConclusa`: la validazione OGC va in panico dentro la
  barriera (il messaggio porta solo la forma del payload).

### Limiti e deviazioni

Il runner non esegue ancora le operazioni geo
([README, «Che cosa non c'è ancora»](../README.md#che-cosa-non-cè-ancora)).
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
