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
`LineString` e rende nessun punto se è vuota. Il runner lo chiama su ogni
cella non nulla, in parallelo: una cella nulla resta nulla; una linea
vuota dà null, e se la colonna d'uscita (con la nullabilità di quella
d'ingresso) non ammette null il passo si rifiuta con `InvalidPlan`; una
riga di altro tipo (anche `MultiLineString`) è `InvalidPlan` («tipo
geometria non supportato»). L'analisi non controlla i tipi dichiarati
dell'ingresso ([Runner, «Operazioni geo»](runner.md#operazioni-geo)).

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

In esecuzione, prima del kernel, su tutta la colonna: `InvalidPlan` per
una cella che non è WKB strutturalmente valido, `Crs` per una coordinata
fuori dal dominio di validità del CRS della colonna, `Schema` per una
geometria di un tipo che il contratto d'ingresso non dichiara, quando li
dichiara con un elenco ([Runner, «Operazioni geo»](runner.md#operazioni-geo)).

Poi, per riga: `InvalidPlan` per una geometria che non è una
`LineString`. Il kernel rende `ExtendedAlgorithmError`, che il runner
porta in `Internal` per `Internal`, `ValidazioneNonConclusa` e
`CalcoloNonConcluso`, in `InvalidPlan` per le altre. Rifiuta la linea con `InvalidInput` (coordinate non finite o meno di due punti
distinti; `ValidazioneNonConclusa` se la validazione non conclude) e con
`CalcoloNonConcluso` (panico di `geo`).

Dopo il kernel, `InvalidPlan` per una linea vuota in una colonna che il
contratto dichiara non nullable (sopra, «Righe»). Il primo errore è quello della prima riga in ordine di riga, senza
diagnostica per riga.

### Limiti e deviazioni

Nel runner un errore non ha diagnostica per riga e il costo in memoria è
una previsione dalle misure
([Runner, «Limiti dichiarati del runner»](runner.md#limiti-dichiarati-del-runner),
voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).

- Solo `LineString` nel kernel; solo CRS proiettati.
- La frazione è della lunghezza euclidea planare, non geodetica.

### Precisione

Nessun controllo e nessun rifiuto di precisione. Con `ratio` 0 il punto è
il primo vertice, con i suoi bit; negli altri casi è calcolato in `f64`
(somma delle lunghezze dei lati, interpolazione sul lato), con un errore
relativo dell'ordine del numero di lati per 1e-16 della lunghezza: molto
sotto 1 cm su ogni linea realistica. Con `ratio` 1 il punto è l'ultimo
vertice a meno di qualche ulp ([Limiti dichiarati, «Precisione delle operazioni
geografiche: 1 cm a terra»](limiti.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

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
