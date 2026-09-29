### Che cosa fa

Spezza le geometrie multi-parte in una riga per parte: un `MultiPoint`, una
`MultiLineString` o un `MultiPolygon` danno una riga per ogni punto, linea
o poligono; una `GeometryCollection` una riga per ogni membro, di un solo
livello (una collezione annidata resta una riga). Una geometria semplice
resta una riga, invariata. Le altre colonne si ripetono sulle righe della
stessa madre, e `__parent_index` dice da quale riga vengono.

### Parametri

Nessuno: la config è `{}`.

### Schema

Le colonne d'ingresso restano, nell'ordine, con gli stessi tipi; la
colonna geometria tiene nome, CRS e dimensioni. In coda si aggiunge
`__parent_index`, `uint64` non nullable: l'indice della riga madre. I
metadati di schema restano; delle proprietà del contratto resta
`sorted_by`, il conteggio delle righe non è più noto. I tipi dichiarati
della colonna si mappano sulle parti: `Point` e `MultiPoint` danno `Point`,
`LineString` e `MultiLineString` danno `LineString`, `Polygon` e
`MultiPolygon` danno `Polygon`, `GeometryCollection` uno qualunque dei
sette tipi; una dichiarazione `exact` resta `exact`, `mixed` resta `mixed`,
assente o `unresolved` resta tale.

### Righe

Espansione 1:N: da 0 righe per madre (un multi o una collezione vuoti) a
una per parte. Il kernel (`operations::explode`) lavora su una geometria
alla volta e nessun adapter lo chiama ancora sulle righe: il trattamento di
una cella nulla, e i valori di `__parent_index` (nell'esempio contati da 0,
come nell'adapter di `geo.split`), non sono definiti da codice
eseguito per questa operazione.

### Ordine

Le righe d'uscita seguono l'ordine delle madri, e dentro una madre l'ordine
delle parti; l'analisi conserva `sorted_by`.

### Errori

In validazione (analisi del contratto):

- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non si riconosce come WKB (né estensione `geoarrow.wkb` né chiavi
  `plenora.geometry.*`); `__parent_index` esiste già;
- `Unsupported`: dimensioni della geometria diverse da `xy`;
- `InvalidPlan`: config diversa da `{}`;
- `Crs`: CRS della colonna assente o non risolto (ogni CRS risolto è
  ammesso).

In esecuzione, dal kernel, per geometria (`OperationError`, che nessun
codice traduce ancora in `PlenoraError`):

- `InvalidInput`: la geometria non supera la validazione OGC;
- `ValidazioneNonConclusa`: la validazione OGC va in panico dentro la
  barriera (il messaggio porta solo la forma del payload).

### Limiti e deviazioni

Il runner non esegue ancora le operazioni geo
([README, «Che cosa non c'è ancora»](../README.md#che-cosa-non-cè-ancora)).
Un solo livello: `ST_Dump` di PostGIS scende invece fino alle geometrie
semplici.

### Precisione

Esatta: le parti sono copiate senza calcolo
([README, «Precisione delle operazioni geografiche»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

### Complessità

Per geometria di n vertici: tempo e memoria O(n) per la copia delle parti,
più la validazione OGC dell'ingresso (sub-quadratica nel caso tipico,
O(n²) nel peggiore:
[README, «Validazione OGC»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì)).

### Esempio

```json
{
  "config": {},
  "ingressi": [
    {"nome": "siti", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2]},
      {"nome": "geometry", "tipo": "geometry", "valori": ["MULTIPOINT((0 0),(1 1))", "POLYGON((0 0,1 0,1 1,0 0))"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 1, 2]},
    {"nome": "geometry", "tipo": "geometry", "valori": ["POINT(0 0)", "POINT(1 1)", "POLYGON((0 0,1 0,1 1,0 0))"]},
    {"nome": "__parent_index", "tipo": "uint64", "valori": [0, 0, 1]}
  ]}
}
```
