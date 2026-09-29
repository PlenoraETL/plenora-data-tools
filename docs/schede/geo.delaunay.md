### Che cosa fa

Triangola i vertici di ogni geometria (triangolazione di Delaunay non
vincolata) e ne fa una riga per triangolo. I vertici sono tutte le
coordinate della geometria, anche quelle di linee e anelli, e i duplicati
contano una volta; i lati d'ingresso non vincolano la triangolazione. Ogni
triangolo è un poligono chiuso antiorario `[a, b, c, a]`.

### Parametri

Nessuno: la config è `{}`.

### Schema

La colonna geometria resta al suo posto, con lo stesso nome, lo stesso CRS
e dimensioni `xy`; i tipi dichiarati diventano esattamente `Polygon` (le
chiavi dei tipi ereditate si tolgono dai metadati del campo). Si aggiunge in
coda `__parent_index`, `uint64` non nullable, con l'indice della riga
d'origine. Le altre colonne e i metadati di schema restano; delle proprietà
del contratto resta `sorted_by`, cade `row_count`.

### Righe

Espansione 1:N: ogni riga dà i suoi triangoli, con le altre colonne copiate
e `__parent_index` alla riga d'origine; meno di tre punti distinti, o punti
tutti collineari, danno zero righe. Il kernel
(`extended_algorithms::delaunay`) triangola una geometria alla volta e
riceve come argomenti il massimo di coordinate d'ingresso e di triangoli.
Il runner non esegue ancora le operazioni geo ([README, «Che cosa non c'è
ancora»](../README.md#che-cosa-non-cè-ancora)): la resa di una cella nulla e
i limiti per piano li fisserà l'esecutore geo.

### Ordine

Le righe d'origine nel loro ordine, i triangoli di ognuna consecutivi e in
ordine canonico, parte del contratto (`semantic_version` 2): ogni triangolo
parte dal suo vertice comparso per primo nell'ingresso, e i triangoli sono
in ordine lessicografico della prima comparsa dei loro tre vertici. Stesso
ingresso, stessa uscita, anche dove la triangolazione non è unica.

### Errori

In validazione (analisi del contratto; nell'ordine: forma dell'ingresso,
config, CRS, nomi):

- `InvalidPlan`: più o meno di un ingresso; config non vuota;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come geometria WKB; `__parent_index` già
  presente;
- `Unsupported`: colonna geometria con dimensioni diverse da `xy`;
- `Crs`: CRS della colonna assente o non risolto; CRS non proiettato o
  senza unità lineare.

In esecuzione: il runner non esegue l'operazione, e agli errori del kernel
non è ancora assegnata una variante `PlenoraError`. Il kernel rifiuta la
geometria con `InvalidInput` (coordinate non finite o geometria non valida
per l'OGC; `ValidazioneNonConclusa` se la validazione non conclude),
`CoordinateLimit` (coordinate d'ingresso, duplicati compresi, oltre il
limite del chiamante), `Triangulation` (coordinata non zero con modulo
fuori da `[2^-142, 2^201]`, il dominio dei predicati esatti di `spade`, per
il primo punto fuori in ordine d'ingresso; oppure vertici persi o fusi dal
caricamento in blocco), `CalcoloNonConcluso` (panico nella
triangolazione), `OutputLimit` (triangoli oltre il limite del chiamante),
`IndexOverflow`, `InvalidOutput` (triangolo non valido).

### Limiti e deviazioni

- Triangolazione caricata in blocco con `spade`, non l'inserimento
  incrementale di `geo`: sugli ingressi con quattro o più punti
  cocircolari (griglie, reticoli) è un'altra triangolazione di Delaunay
  valida, con le stesse facce altrove; il caso peggiore resta quadratico
  ([README, «`geo.delaunay` e `geo.voronoi`: triangolazione caricata in
  blocco»](../README.md#geodelaunay-e-geovoronoi-triangolazione-caricata-in-blocco)).
- Non vincolata e senza tolleranza: `ST_DelaunayTriangles` di PostGIS ha un
  parametro di tolleranza per fondere i vertici vicini, qui assente (si
  fondono solo i punti uguali, con `-0.0` uguale a `0.0`).
- I limiti di coordinate e di triangoli sono argomenti del kernel, senza
  valore predefinito qui.

### Precisione

Esatta: nessuna coordinata si calcola. I vertici dei triangoli sono i punti
d'ingresso con i loro bit e i predicati d'orientazione e del cerchio sono
esatti; nessun controllo e nessun rifiuto di precisione servono ([README,
«Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).
Due punti distinti a meno di 1 cm restano due vertici.

### Complessità

O(n log n) tipico per geometria, con `n` le coordinate, quadratico nel caso
peggiore (punti quasi tutti allineati); più la validazione OGC
dell'ingresso e dei triangoli. Memoria O(n).

### Esempio

Quattro punti non cocircolari danno due triangoli; una linea di due punti
non ne dà.

```json
{
  "config": {},
  "ingressi": [
    {"nome": "punti", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["MULTIPOINT((0 0),(10 0),(0 10),(12 12))", "LINESTRING(0 0,1 0)"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 1]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POLYGON((0 0,10 0,0 10,0 0))", "POLYGON((10 0,12 12,0 10,10 0))"]},
    {"nome": "__parent_index", "tipo": "uint64", "valori": [0, 0]}
  ]}
}
```
