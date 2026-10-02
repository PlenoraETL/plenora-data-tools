### Che cosa fa

Sostituisce il punto di ogni riga con la sua cella di Voronoi, calcolata sui
punti di tutte le righe: la regione del piano più vicina a quel punto che a
ogni altro. Le celle di bordo, infinite, si chiudono ritagliandole sul
rettangolo d'ingombro dei punti allargato su ogni lato della metà del suo
lato maggiore. I punti duplicati ricevono la stessa cella.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `max_points` | intero | `100000` | intero non negativo, almeno 2 | numero massimo di punti (righe non nulle) |

### Schema

La colonna geometria si riscrive al suo posto, con lo stesso nome, lo
stesso CRS e dimensioni `xy`; i tipi dichiarati diventano esattamente
`Polygon` (le chiavi dei tipi ereditate si tolgono dai metadati del campo).
Le altre colonne, i metadati di schema e le proprietà del contratto
(`sorted_by`, `row_count`) restano.

### Righe

1:1: una cella per riga, ma ogni cella dipende da tutte le righe (il kernel
`advanced::voronoi_cells` riceve tutti i punti insieme, e il limite
`max_points` come argomento). A ogni punto va la prima cella, in ordine di
prima comparsa del sito, che lo interseca. Il kernel non conosce i
null: il runner gli passa solo le geometrie non nulle, nell'ordine delle
righe, e riporta ogni cella alla sua riga; una riga nulla resta nulla e
non è un sito. Gli altri attributi restano invariati.

### Ordine

Quello d'ingresso (parte del contratto, `semantic_version` 2). Stesso
ingresso, stessa uscita, anche sui punti cocircolari.

### Errori

In validazione (analisi del contratto; nell'ordine: forma dell'ingresso,
config, CRS):

- `InvalidPlan`: più o meno di un ingresso; config con campi sconosciuti o
  `max_points` non intero non negativo; `max_points` minore di 2;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come geometria WKB;
- `Unsupported`: colonna geometria con dimensioni diverse da `xy`;
- `Crs`: CRS della colonna assente o non risolto; CRS non proiettato o
  senza unità lineare.

In esecuzione, prima del kernel, su ogni cella non nulla ([Runner,
«Operazioni geo»](runner.md#operazioni-geo)): `InvalidPlan` per un WKB
malformato o con coordinate non finite, `Unsupported` per dimensioni Z/M o
SRID, `Crs` per una coordinata fuori dal dominio di validità del CRS della
colonna, `Schema` per una geometria di un tipo che il contratto
dell'ingresso dichiara con un elenco e che non vi compare. Poi la
decodifica completa con la validazione OGC: `InvalidPlan` per una
geometria non valida, `Internal` se la validazione non conclude.

Poi il kernel, con errore `AdvancedError` che il runner traduce così:
`ValidazioneNonConclusa` e `CalcoloNonConcluso` diventano `Internal`,
`PrecisionInsufficient` e `VerticeMalCondizionato` diventano
`Unsupported`, `PointLimitExceeded` diventa `ResourceLimit`, le altre
`InvalidPlan`; il messaggio è quello del kernel, e
un indice che vi compare conta le sole geometrie non nulle, non le righe.
Il kernel rifiuta l'intera colonna, nell'ordine, con `InsufficientPoints`
(meno di due righe non nulle), `PointLimitExceeded` (più righe non nulle
di `max_points`), `InvalidPoint` (una geometria non valida per l'OGC, con
il suo indice; `ValidazioneNonConclusa` se la validazione non conclude),
`ExpectedPoint`
(una geometria valida che non è un `Point`, con il suo indice), `Voronoi`
(coordinata non zero con modulo fuori da `[2^-142, 2^201]`, meno di due
punti distinti, punti tutti collineari), `PrecisionInsufficient` e
`VerticeMalCondizionato` (sotto, «Precisione»), `CalcoloNonConcluso`
(panico di `geo`, `spade` o `rstar`), `InvalidOutput` (cella non valida),
`UnmatchedPoint` (nessuna cella interseca un punto).

### Limiti e deviazioni

- Celle costruite sulla triangolazione caricata in blocco di `spade`, con
  il corpo di `voronoi_cells` di `geo` 0.33.1 ricopiato e un circocentro
  indipendente dalla rotazione della faccia: rispetto all'inserimento
  incrementale un vertice può differire di qualche ulp, e con quattro o più
  punti cocircolari i circocentri vengono da triangoli diversi; il caso
  peggiore resta quadratico ([Limiti dichiarati, «`geo.delaunay` e `geo.voronoi`:
  triangolazione caricata in blocco»](limiti.md#geodelaunay-e-geovoronoi-triangolazione-caricata-in-blocco)).
- Solo `Point`: un `MultiPoint` è `ExpectedPoint`. `ST_VoronoiPolygons` di
  PostGIS prende invece una geometria e rende una collezione di celle.
- Punti tutti collineari sono un errore, non celle a striscia.
- Ritaglio fisso sul rettangolo allargato della metà del lato maggiore:
  nessun parametro d'inviluppo.
- Nessuna diagnostica per riga: il passo rende il primo errore ([Runner,
  «Limiti dichiarati del runner»](runner.md#limiti-dichiarati-del-runner),
  voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).

### Precisione

La precisione `p` è 1 cm a terra nelle unità del CRS della colonna: il
runner la ricava con `Precision::from_crs` e il kernel la riceve come
argomento ([Limiti dichiarati, «Precisione delle operazioni geografiche: 1 cm a
terra»](limiti.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).
I vertici interni delle celle sono circocentri calcolati con un maggiorante
del loro errore d'arrotondamento: oltre `p / 4` l'operazione si rifiuta
(`VerticeMalCondizionato`; succede con triangoli molto sottili sul bordo
dell'inviluppo: misurato sotto `3,4e-4 p` fino a 30 km di lato, ma a
1.000 km con 200.000 punti un triangolo arriva a `1,17 p`). Si
rifiuta con `PrecisionInsufficient` se la spaziatura dei `f64` supera
`p / 64` al modulo dei punti più la distanza dei punti lontani dei raggi, o
al modulo dei vertici delle celle prima del ritaglio, e se la griglia di
`i_overlay` del ritaglio di una cella di bordo supererebbe `p / 2` (lo
stesso controllo a priori delle booleane). I vertici sul rettangolo di
ritaglio vengono dall'intersezione di `geo` e non sono confrontati con
l'esatto dopo il calcolo.

### Complessità

O(n log n) tipico sulla colonna, con `n` le righe: caricamento in blocco
della triangolazione, celle, ritaglio delle celle di bordo, associazione
punto-cella con un R-tree; quadratico nel caso peggiore. Memoria O(n).

### Esempio

Tre punti e un duplicato del primo, che riceve la stessa cella.

```json
{
  "config": {"max_points": 1000},
  "ingressi": [
    {"nome": "siti", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2, 3, 4]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POINT(0 0)", "POINT(4 0)", "POINT(0 4)", "POINT(0 0)"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2, 3, 4]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POLYGON((-2 2,-2 -2,2 -2,2 2,-2 2))", "POLYGON((2 2,2 -2,6 -2,6 6,2 2))", "POLYGON((-2 6,-2 2,2 2,6 6,-2 6))", "POLYGON((-2 2,-2 -2,2 -2,2 2,-2 2))"]}
  ]}
}
```
