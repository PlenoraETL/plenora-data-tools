### Che cosa fa

Aggiunge vertici ai lati delle geometrie: ogni lato più lungo di
`max_segment_length` si divide in parti uguali, abbastanza da non superarla.
Un lato di lunghezza `L` diventa `ceil(L / max_segment_length)` lati; i
vertici d'ingresso restano, con i loro bit. Punti e multipunti non
cambiano; le collezioni si densificano membro per membro. La lunghezza è
quella euclidea nel piano del CRS.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `max_segment_length` | numero | obbligatorio | finito, maggiore di zero | lunghezza massima di un lato, nelle unità del CRS |

### Schema

Invariato: la colonna geometria si riscrive al suo posto, con lo stesso
nome, gli stessi metadati, lo stesso CRS, dimensioni `xy` e gli stessi tipi
dichiarati (la densificazione non cambia il tipo). Le altre colonne, i
metadati di schema e le proprietà del contratto (`sorted_by`, `row_count`)
restano.

### Righe

1:1: la geometria di ogni riga diventa la sua densificata. Il kernel
(`extended_algorithms::densify`) lavora su una geometria alla volta e
riceve il massimo di coordinate d'uscita come argomento. Il runner non
esegue ancora le operazioni geo ([README, «Che cosa non c'è ancora»](../README.md#che-cosa-non-cè-ancora)):
la resa di una cella nulla e il limite di coordinate per piano li fisserà
l'esecutore geo.

### Ordine

Quello d'ingresso. Dentro una geometria i vertici nuovi stanno fra i due
estremi del loro lato, nel verso del lato.

### Errori

In validazione (analisi del contratto; nell'ordine: forma dell'ingresso,
config, CRS):

- `InvalidPlan`: più o meno di un ingresso; config con campi sconosciuti,
  senza `max_segment_length` o con un tipo sbagliato; `max_segment_length`
  non finito o non maggiore di zero;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come geometria WKB;
- `Unsupported`: colonna geometria con dimensioni diverse da `xy`;
- `Crs`: CRS della colonna assente o non risolto; CRS non proiettato o
  senza unità lineare.

In esecuzione: il runner non esegue l'operazione, e agli errori del kernel
non è ancora assegnata una variante `PlenoraError`. Il kernel rifiuta la
geometria con `InvalidInput` (coordinate non finite o geometria non valida
per l'OGC; `ValidazioneNonConclusa` se la validazione non conclude),
`IndexOverflow` (conteggio delle coordinate oltre `u64`), `OutputLimit`
(coordinate d'uscita stimate, prima di allocare, o contate, dopo, oltre il
limite del chiamante), `CalcoloNonConcluso` (panico di `geo`),
`InvalidOutput` (uscita non valida per l'OGC). `UnsupportedGeometry`
(`Line`, `Rect`, `Triangle`) non si raggiunge da una colonna WKB.

### Limiti e deviazioni

- Il limite di coordinate d'uscita è un argomento del kernel, senza valore
  predefinito qui. In una `GeometryCollection` vale per il totale e, di
  nuovo, per ogni membro.
- Solo CRS proiettati: la densificazione è planare, non lungo le
  geodetiche.
- I lati di lunghezza zero (vertici consecutivi uguali) restano come sono.

### Precisione

Nessun controllo e nessun rifiuto di precisione. I vertici d'ingresso non
si spostano; quelli nuovi si calcolano come `inizio + (fine - inizio) * k / n`
in `f64` e stanno sul lato a meno di qualche ulp del modulo delle
coordinate (nanometri in UTM), molto sotto 1 cm in ogni dominio dei CRS
integrati ([README, «Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

### Complessità

O(n + m) per geometria, con `n` le coordinate d'ingresso e `m` quelle
d'uscita, più la validazione OGC dell'ingresso e dell'uscita (O(m²) nel
caso peggiore, [README, «Validazione OGC: la ricerca delle
auto-intersezioni non è quella di `geo`, il verdetto sì»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì)).
Memoria O(m).

### Esempio

Lati di 10 con `max_segment_length` 5: ogni lato si divide in due.

```json
{
  "config": {"max_segment_length": 5},
  "ingressi": [
    {"nome": "confini", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["LINESTRING(0 0,10 0)", "POLYGON((0 0,10 0,10 10,0 10,0 0))"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["LINESTRING(0 0,5 0,10 0)", "POLYGON((0 0,5 0,10 0,10 5,10 10,5 10,0 10,0 5,0 0))"]}
  ]}
}
```
