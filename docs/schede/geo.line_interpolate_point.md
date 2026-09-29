### Che cosa fa

Sostituisce ogni linea con il punto che sta alla frazione `ratio` della sua
lunghezza, misurata dall'inizio: la distanza `ratio * L` si percorre lato
per lato e il punto si interpola sul lato in cui cade. `ratio` 0 dà il
primo vertice, 1 l'ultimo. La lunghezza è quella euclidea nel piano del
CRS.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `ratio` | numero | obbligatorio | finito, in `[0, 1]` | frazione della lunghezza dall'inizio della linea |

### Schema

La colonna geometria si riscrive al suo posto, con lo stesso nome, lo
stesso CRS e dimensioni `xy`; i tipi dichiarati diventano esattamente
`Point` (le chiavi dei tipi ereditate si tolgono dai metadati del campo). Le altre colonne,
i metadati di schema e le proprietà del contratto (`sorted_by`,
`row_count`) restano.

### Righe

1:1. Il kernel (`extended_algorithms::line_interpolate_point`) riceve una
`LineString` e rende nessun punto se è vuota. Il runner non esegue ancora
le operazioni geo ([README, «Che cosa non c'è ancora»](../README.md#che-cosa-non-cè-ancora)):
che cosa rendano una cella nulla, una linea vuota e una riga di altro tipo
(anche `MultiLineString`) lo fisserà l'esecutore geo. L'analisi non
controlla i tipi dichiarati dell'ingresso.

### Ordine

Quello d'ingresso.

### Errori

In validazione (analisi del contratto; nell'ordine: forma dell'ingresso,
config, CRS):

- `InvalidPlan`: più o meno di un ingresso; config con campi sconosciuti,
  senza `ratio` o con un tipo sbagliato; `ratio` non finito o fuori da
  `[0, 1]`;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come geometria WKB;
- `Unsupported`: colonna geometria con dimensioni diverse da `xy`;
- `Crs`: CRS della colonna assente o non risolto; CRS non proiettato o
  senza unità lineare.

In esecuzione: il runner non esegue l'operazione, e agli errori del kernel
non è ancora assegnata una variante `PlenoraError`. Il kernel rifiuta la
linea con `InvalidInput` (coordinate non finite o meno di due punti
distinti; `ValidazioneNonConclusa` se la validazione non conclude) e con
`CalcoloNonConcluso` (panico di `geo`).

### Limiti e deviazioni

- Solo `LineString` nel kernel; solo CRS proiettati.
- La frazione è della lunghezza euclidea planare, non geodetica.

### Precisione

Nessun controllo e nessun rifiuto di precisione. Con `ratio` 0 il punto è
il primo vertice, con i suoi bit; negli altri casi è calcolato in `f64`
(somma delle lunghezze dei lati, interpolazione sul lato), con un errore
relativo dell'ordine del numero di lati per 1e-16 della lunghezza: molto
sotto 1 cm su ogni linea realistica. Con `ratio` 1 il punto è l'ultimo
vertice a meno di qualche ulp ([README, «Precisione delle operazioni
geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

### Complessità

O(n) per linea, con `n` i vertici, più la validazione OGC dell'ingresso.
Memoria O(1) oltre la linea.

### Esempio

```json
{
  "config": {"ratio": 0.25},
  "ingressi": [
    {"nome": "percorsi", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["LINESTRING(0 0,0 10,10 10)", "LINESTRING(0 0,4 0)"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POINT(0 5)", "POINT(1 0)"]}
  ]}
}
```
