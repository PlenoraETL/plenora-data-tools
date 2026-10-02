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
colonna geometria tiene nome, CRS e dimensioni, ed è non nullable (una
riga a geometria null non produce parti). In coda si aggiunge
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
alla volta; il runner lo chiama su ogni cella non nulla e ripete sulle
parti le altre colonne della madre. Una cella nulla non produce righe.
`__parent_index` è la posizione della madre nella tabella d'ingresso del
passo, contata da 0. Il runner conta le righe prodotte su tutta la tabella: oltre il limite di righe dell'arco d'uscita (`max_output_rows` per un output del
piano, `max_rows_per_edge` altrimenti), `ResourceLimit`.

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

In esecuzione, prima del kernel, su ogni cella non nulla ([Runner,
«Operazioni geo»](runner.md#operazioni-geo)): `InvalidPlan` per un WKB
malformato o con coordinate non finite, `Unsupported` per dimensioni Z/M o
SRID, `Crs` per una coordinata fuori dal dominio di validità del CRS della
colonna, `Schema` per una geometria di un tipo che il contratto
dell'ingresso dichiara con un elenco e che non vi compare.

Poi il kernel, per geometria, con errore `OperationError` che il runner
traduce così: `ValidazioneNonConclusa` diventa `Internal`, le altre
`InvalidPlan`:

- `InvalidInput`: la geometria non supera la validazione OGC;
- `ValidazioneNonConclusa`: la validazione OGC va in panico dentro la
  barriera (il messaggio porta solo la forma del payload).

### Limiti e deviazioni

Un solo livello: `ST_Dump` di PostGIS scende invece fino alle geometrie
semplici. Nessuna diagnostica per riga: il passo rende il primo errore ([Runner,
«Limiti dichiarati del runner»](runner.md#limiti-dichiarati-del-runner),
voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).

### Precisione

Esatta: le parti sono copiate senza calcolo
([Limiti dichiarati, «Precisione delle operazioni geografiche»](limiti.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

### Complessità

Per geometria di n vertici: tempo e memoria O(n) per la copia delle parti,
più la validazione OGC dell'ingresso (sub-quadratica nel caso tipico,
O(n²) nel peggiore:
[Limiti dichiarati, «Validazione OGC»](limiti.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì)).

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
