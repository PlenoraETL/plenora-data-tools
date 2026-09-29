### Che cosa fa

Aggiunge alla sinistra, di norma poligoni, una colonna con il numero di
geometrie della destra, di norma punti, che ogni sua geometria contiene
(kernel `analysis::count_points_in_polygons`). «Contiene» è il `contains`
di `geo`: i punti sul bordo non contano, come il `predicate="within"` di
Manipola, e un punto dentro più poligoni conta in ognuno. Il runner non
esegue ancora le operazioni geo ([README, «Che cosa non c'è
ancora»](../README.md#che-cosa-non-cè-ancora)): lo schema qui descritto è
quello dell'analisi del contratto, i valori quelli del kernel.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `output_column` | stringa | `count` | nome non vuoto e libero nella sinistra | colonna aggiunta |

### Schema

Le colonne della sinistra, invariate, più `output_column` in coda,
`uint64` nullable. Le colonne della destra non passano. La colonna
geometria resta quella della sinistra, con i suoi tipi dichiarati. I
metadati di schema sono la fusione dei due lati; le proprietà del
contratto della sinistra (`sorted_by`, `row_count`) restano.

### Righe

1:1 con la sinistra; la destra non aggiunge righe. Il kernel rende un
conteggio per ogni geometria sinistra, nella stessa posizione, 0 se non ne
contiene nessuna (anche per una cella nulla o vuota). Una geometria destra
nulla o vuota non conta mai. Il tipo delle geometrie non si controlla:
conta ogni geometria destra contenuta, anche una linea. Il valore per una
geometria sinistra nulla (0 o nullo) non è ancora fissato da codice di
questo repository.

### Ordine

Quello della sinistra.

### Errori

In validazione (analisi del contratto):

- `InvalidPlan`: un numero di ingressi diverso da due; config con campi
  sconosciuti; `output_column` vuota; un metadato di schema presente sui
  due lati con valori diversi;
- `Schema`: `output_column` esiste già nella sinistra; un lato senza
  esattamente una colonna geometria, o con una colonna non riconoscibile
  come geometria WKB (né estensione `geoarrow.wkb` né chiavi canoniche
  `plenora.geometry.*`);
- `Unsupported`: una colonna geometria non XY;
- `Crs`: un lato senza CRS risolto, un CRS non proiettato (o senza unità
  lineare), CRS dei due lati non equivalenti.

In esecuzione (kernel `analysis::count_points_in_polygons`, errore
`AnalysisError` che avvolge `SpatialJoinError`; nessun codice di questo
repository lo traduce ancora in `PlenoraError`):

- `InvalidPairLimit`: il limite delle coppie che il chiamante passa al
  kernel è zero;
- `PairLimitExceeded`: le coppie punto-poligono confermate superano il
  limite;
- `NonFiniteCoordinate`, `InvalidGeometry`: una geometria ha coordinate
  NaN o infinite o non supera la validazione OGC (l'errore porta il lato e
  la posizione, mai i valori; nel join i punti sono il lato `left`);
- `ValidazioneNonConclusa`, `CalcoloNonConcluso`: la validazione o il
  predicato di `geo` non ha concluso;
- `IndexOverflow`: un numero di righe non entra in `u64`.

### Limiti e deviazioni

Nessuno oltre ai limiti comuni.

### Precisione

Nessun calcolo di geometrie e nessuna griglia: il predicato di `geo` si
valuta sulle coordinate `f64` d'ingresso, senza tolleranza, e la regola di
1 cm non sposta nulla. Un punto a meno di 1 cm dal bordo conta o no
secondo le sue coordinate esatte ([README, «Precisione delle operazioni
geografiche: 1 cm a
terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra),
«Feature d'ingresso più vicine della precisione»).

### Complessità

Un R-tree dei rettangoli d'ingombro della sinistra, O(n log n) per `n`
poligoni; per ognuna delle `m` geometrie destre una ricerca nell'albero e
il predicato esatto sui soli candidati. Di norma O((n + m) log n) più il
costo dei predicati; nel caso peggiore O(n · m) predicati. Più la
validazione OGC di ogni geometria. Memoria O(n) per l'albero e i
conteggi, più le coppie confermate.

### Esempio

Il terzo punto sta sul bordo del primo poligono, quindi non conta.

```json
{
  "config": {},
  "ingressi": [
    {"nome": "aree", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POLYGON((0 0,2 0,2 2,0 2,0 0))", "POLYGON((10 10,12 10,12 12,10 12,10 10))"]}
    ]},
    {"nome": "pozzi", "colonne": [
      {"nome": "codice", "tipo": "utf8", "valori": ["a", "b", "c", "d"]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POINT(1 1)", "POINT(0.5 0.5)", "POINT(0 1)", "POINT(20 20)"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POLYGON((0 0,2 0,2 2,0 2,0 0))", "POLYGON((10 10,12 10,12 12,10 12,10 10))"]},
    {"nome": "count", "tipo": "uint64", "valori": [2, 0]}
  ]}
}
```
