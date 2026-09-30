### Che cosa fa

Sostituisce ogni linea con la sua porzione fra le frazioni `start_ratio` e
`end_ratio` della lunghezza, misurate dall'inizio. Il primo e l'ultimo
punto sono quelli di [`geo.line_interpolate_point`](#geoline_interpolate_point)
alle due frazioni; in mezzo restano, con i loro bit, i vertici d'ingresso
che cadono strettamente fra le due distanze. Con due frazioni uguali la
porzione è un punto. La lunghezza è quella euclidea nel piano del CRS.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `start_ratio` | numero | obbligatorio | finito, in `[0, 1]`, non maggiore di `end_ratio` | frazione della lunghezza a cui comincia la porzione |
| `end_ratio` | numero | obbligatorio | finito, in `[0, 1]` | frazione della lunghezza a cui finisce la porzione |

### Schema

La colonna geometria si riscrive al suo posto, con lo stesso nome, lo
stesso CRS e dimensioni `xy`; i tipi dichiarati diventano esattamente
`Point` e `LineString` (le chiavi dei tipi ereditate si tolgono dai
metadati del campo). Le altre colonne, i metadati di schema e le proprietà
del contratto (`sorted_by`, `row_count`) restano.

### Righe

1:1. Il kernel (`extended_algorithms::line_substring`) riceve una
`LineString`, rende una `LineString`, un `Point` se `start_ratio` e
`end_ratio` sono uguali (confronto numerico `==`: `-0.0` e `0.0` sono
uguali), e nessuna geometria se la linea
è vuota. Il runner lo chiama su ogni cella non nulla, in parallelo: una
cella nulla resta nulla; una linea vuota dà null, e se la colonna d'uscita
(con la nullabilità di quella d'ingresso) non ammette null il passo si
rifiuta con `InvalidPlan`; una riga di altro tipo (anche
`MultiLineString`) è `InvalidPlan` («tipo geometria non supportato»).
L'analisi non controlla i tipi dichiarati dell'ingresso
([README, «Operazioni geo»](../README.md#operazioni-geo)).

### Ordine

Quello d'ingresso; dentro la porzione i vertici seguono il verso della
linea.

### Errori

In validazione (analisi del contratto; nell'ordine: forma dell'ingresso,
config, CRS):

- `InvalidPlan`: più o meno di un ingresso; config con campi sconosciuti,
  senza una delle due frazioni o con un tipo sbagliato; frazione non
  finita o fuori da `[0, 1]`; `start_ratio` maggiore di `end_ratio`;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come geometria WKB;
- `Unsupported`: colonna geometria con dimensioni diverse da `xy`;
- `Crs`: CRS della colonna assente o non risolto; CRS non proiettato o
  senza unità lineare.

In esecuzione, prima del kernel, su tutta la colonna: `InvalidPlan` per
una cella che non è WKB strutturalmente valido, `Crs` per una coordinata
fuori dal dominio di validità del CRS della colonna, `Schema` per una
geometria di un tipo che il contratto d'ingresso non dichiara, quando li
dichiara con un elenco ([README, «Operazioni geo»](../README.md#operazioni-geo)).

Poi, per riga: `InvalidPlan` per una geometria che non è una
`LineString`. Il kernel rende `ExtendedAlgorithmError`, che il runner
porta in `Internal` per `Internal`, `ValidazioneNonConclusa` e
`CalcoloNonConcluso`, in `InvalidPlan` per le altre. Rifiuta la linea con `InvalidInput` (coordinate non finite o meno di due punti
distinti; `ValidazioneNonConclusa` se la validazione non conclude),
`CalcoloNonConcluso` (panico di `geo`) e `InvalidOutput` quando la porzione
non è una geometria valida: due frazioni diverse i cui punti coincidono in
`f64` danno una linea di un solo punto distinto (per esempio le frazioni
0,5 e 0,5000000000000001 su un lato di 1 m a un milione di metri
dall'origine).

Dopo il kernel, `InvalidPlan` per una linea vuota in una colonna che il
contratto dichiara non nullable (sopra, «Righe»). Il primo errore è quello della prima riga in ordine di riga, senza
diagnostica per riga.

### Limiti e deviazioni

Nel runner un errore non ha diagnostica per riga e il costo in memoria è
una previsione dalle misure
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).

- Solo `LineString` nel kernel; solo CRS proiettati.
- Due frazioni diverse ma troppo vicine per dare due punti distinti sono
  un errore, non un punto: il punto esce solo con frazioni uguali.

### Precisione

Nessun controllo e nessun rifiuto di precisione. I vertici interni sono
quelli d'ingresso, con i loro bit. I due estremi sono calcolati in `f64`
come in [`geo.line_interpolate_point`](#geoline_interpolate_point), con un
errore relativo dell'ordine del numero di lati per 1e-16 della lunghezza:
molto sotto 1 cm su ogni linea realistica ([README, «Precisione delle
operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).
Un vertice a distanza cumulata quasi uguale a un estremo può entrare o
restare fuori secondo l'arrotondamento delle due somme.

### Complessità

O(n) per linea, con `n` i vertici, più la validazione OGC dell'ingresso e
dell'uscita. Memoria O(n).

### Esempio

```json
{
  "config": {"start_ratio": 0.25, "end_ratio": 0.75},
  "ingressi": [
    {"nome": "percorsi", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["LINESTRING(0 0,0 10,10 10)", "LINESTRING(0 0,8 0)"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["LINESTRING(0 5,0 10,5 10)", "LINESTRING(2 0,6 0)"]}
  ]}
}
```
