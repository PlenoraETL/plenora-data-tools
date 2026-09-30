### Che cosa fa

Fonde le linee in percorsi più lunghi possibile: due linee si uniscono in
un estremo solo se quell'estremo è condiviso da esattamente due linee. Un
nodo in cui si incontrano tre o più linee, o l'estremo libero di una linea,
è sempre un confine fra percorsi. Una linea percorsa al contrario si
inverte per proseguire il percorso. Gli estremi si confrontano per bit, con
`-0.0` uguale a `0.0`, senza tolleranza.

### Parametri

Nessuno: la config è `{}`.

### Schema

Solo la colonna geometria, non nullable (una riga per linea fusa), con lo stesso nome, lo stesso CRS,
dimensioni `xy` e i metadati del campo d'ingresso, tranne le chiavi dei
tipi ereditate; i tipi dichiarati diventano esattamente `LineString`. Le
colonne attributo cadono. I metadati di schema restano; le proprietà del
contratto (`sorted_by`, `row_count`) cadono.

### Righe

Aggregazione: per il contratto l'uscita ha solo la colonna geometria, una
riga per percorso fuso. Il kernel (`extended_algorithms::line_merge`) fonde
le linee di una geometria (`LineString`, `MultiLineString` o una
`GeometryCollection` di sole linee, anche annidata) e riceve come
argomenti il massimo di coordinate d'ingresso e di linee d'uscita. Il
runner riunisce le geometrie non nulle di tutte le righe, nell'ordine delle
righe, in una `GeometryCollection` e la fonde con un solo calcolo: una
cella nulla si salta, e una tabella vuota o tutta nulla non dà righe. I
limiti che passa sono `MAX_CLEAN_VERTICES` (100.000.000) coordinate e il
limite di righe dell'arco d'uscita (`max_output_rows` per un output del
piano, `max_rows_per_edge` altrimenti) come linee. Le linee vuote si
ignorano; una linea chiusa esce sempre da sola, com'è.

### Ordine

Deterministico e indipendente dall'hash. Prima, nell'ordine delle linee
d'ingresso, le linee chiuse e i percorsi che toccano un nodo di grado
diverso da due, ciascuno dal primo estremo della sua prima linea se quello
non ha grado due, altrimenti dall'ultimo; poi gli anelli fatti solo di nodi
di grado due, ciascuno dalla prima linea non ancora usata e dal suo estremo
minore (bit di `x`, poi di `y`). Nelle giunzioni un vertice uguale al
precedente non si ripete.

### Errori

In validazione (analisi del contratto; nell'ordine: forma dell'ingresso,
config, CRS):

- `InvalidPlan`: più o meno di un ingresso; config non vuota;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come geometria WKB;
- `Unsupported`: colonna geometria con dimensioni diverse da `xy`;
- `Crs`: CRS della colonna assente o non risolto; CRS non proiettato o
  senza unità lineare.

In esecuzione, prima del kernel, su ogni cella non nulla ([README,
«Operazioni geo»](../README.md#operazioni-geo)): `InvalidPlan` per un WKB
malformato o con coordinate non finite, `Unsupported` per dimensioni Z/M o
SRID, `Crs` per una coordinata fuori dal dominio di validità del CRS della
colonna, `Schema` per una geometria di un tipo che il contratto
dell'ingresso dichiara con un elenco e che non vi compare. Poi la
decodifica completa con la validazione OGC: `InvalidPlan` per una
geometria non valida, `Internal` se la validazione non conclude.

Poi il kernel, con errore `ExtendedAlgorithmError` che il runner traduce
così: `Internal`, `ValidazioneNonConclusa` e `CalcoloNonConcluso`
diventano `Internal`, i limiti (`CoordinateLimit`, `OutputLimit`)
`ResourceLimit`, le altre `InvalidPlan`. Il kernel rifiuta la
geometria riunita con `InvalidInput` (coordinate non finite o geometria non valida
per l'OGC; `ValidazioneNonConclusa` se la validazione non conclude),
`CoordinateLimit` (coordinate d'ingresso oltre il limite passato dal
runner),
`UnsupportedGeometry` (punti o poligoni, anche dentro una collezione),
`IndexOverflow`, `OutputLimit` (linee d'uscita oltre il limite di righe
dell'arco), `InvalidOutput` (linea fusa non valida), `Internal`
(invariante interna violata).

### Limiti e deviazioni

- Nessuna tolleranza: estremi distinti anche di un ulp non si uniscono.
- Come il line merge di GEOS (`ST_LineMerge` di PostGIS) un nodo di grado
  diverso da due separa i percorsi; l'ordine e il verso dei percorsi
  d'uscita sono quelli descritti sopra, non quelli di GEOS.
- I limiti di coordinate e di linee sono quelli del runner, sopra; non si
  scelgono dal piano.
- Nessuna diagnostica per riga: il passo rende il primo errore ([README,
  «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
  voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).

### Precisione

Esatta: nessuna coordinata si calcola, i vertici d'uscita sono quelli
d'ingresso con i loro bit. Due estremi distinti a meno di 1 cm non si
uniscono: sono feature più vicine della precisione, fuori dalla garanzia
([README, «Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

### Complessità

O(n) per geometria, con `n` le coordinate (una mappa degli estremi, ogni
linea percorsa una volta), più la validazione OGC dell'ingresso e delle
linee d'uscita. Memoria O(n).

### Esempio

Due linee con un estremo in comune (la seconda al contrario) e una linea
staccata, in una sola riga.

```json
{
  "config": {},
  "ingressi": [
    {"nome": "tratte", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["MULTILINESTRING((0 0,1 0),(2 0,1 0),(5 5,6 5))"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["LINESTRING(0 0,1 0,2 0)", "LINESTRING(5 5,6 5)"]}
  ]}
}
```
