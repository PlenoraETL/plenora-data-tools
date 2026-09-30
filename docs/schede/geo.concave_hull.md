### Che cosa fa

Sostituisce ogni geometria con un poligono concavo che ne racchiude tutte
le coordinate (vertici degli anelli interni e punti ripetuti compresi).
È l'algoritmo di `geo`, porting di `concaveman`: parte dall'inviluppo
convesso e scava verso i punti interni ogni lato più lungo di
`length_threshold`, se il punto candidato sta entro la lunghezza del lato
divisa per `concavity`. Più `concavity` è piccola, più il poligono è
concavo; molto grande, è l'inviluppo convesso.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `concavity` | numero | obbligatorio | finito, maggiore di zero | concavità relativa: più piccola, più concavo |
| `length_threshold` | numero | `0` | finito, non negativo | lati più corti di così non si scavano; `0` scava ogni lato |

Il kernel (`extended::concave_hull`) riceve `length_threshold` sempre
esplicito, insieme al limite di coordinate `max_coordinates`: il runner
passa `0` quando manca (ogni lato si può scavare) e `MAX_CELL_COORDINATES`
(4 194 304) come limite.

### Schema

Stesse colonne, tipi, nullabilità e metadati di schema. La colonna
geometria resta al suo posto con lo stesso CRS e le stesse dimensioni
(XY), ma dichiara ora il solo tipo `Polygon` (dichiarazione esatta): le
chiavi `plenora.geometry.types` e `plenora.geometry.types_declaration`
ereditate si tolgono dal campo. Le proprietà del contratto (`sorted_by`,
`row_count`) restano.

### Righe

1:1: il runner chiama il kernel (`extended::concave_hull`) su ogni cella non
nulla, in parallelo, e rimette la geometria al suo posto; una cella
nulla resta nulla ([README, «Operazioni geo»](../README.md#operazioni-geo)).
Una geometria senza coordinate dà un `POLYGON EMPTY`.

### Ordine

Per contratto quello d'ingresso (forma 1:1).

### Errori

In validazione (analisi del contratto):

- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non si dichiara geometria WKB;
- `Unsupported`: dimensioni della colonna diverse da XY (anche non
  dichiarate);
- `InvalidPlan`: config con campi sconosciuti, `concavity` assente, non
  finita o non positiva, `length_threshold` scritto e non finito o
  negativo;
- `Crs`: CRS della colonna mancante o non risolto, non proiettato
  (`PROJECTED_CRS_REQUIRED`) o senza unità lineare
  (`LINEAR_UNIT_REQUIRED`).

In esecuzione, prima del kernel, su tutta la colonna: `InvalidPlan` per
una cella che non è WKB strutturalmente valido, `Crs` per una coordinata
fuori dal dominio di validità del CRS della colonna, `Schema` per una
geometria di un tipo che il contratto d'ingresso non dichiara, quando li
dichiara con un elenco ([README, «Operazioni geo»](../README.md#operazioni-geo)).

Poi dal kernel, per geometria (`ExtendedError`, che il runner porta in `PlenoraError`: `Internal` per
`ValidazioneNonConclusa` e `CalcoloNonConcluso`, `InvalidPlan` per le
altre):

- `InvalidInput`: coordinate NaN o infinite o geometria non valida OGC;
- `CoordinateLimit`: più coordinate di `max_coordinates` (il runner passa
  `MAX_CELL_COORDINATES`, 4 194 304);
- `InvalidOutput`: il poligono prodotto non è valido OGC, come per un
  punto solo, due punti distinti o punti tutti allineati;
- `CalcoloNonConcluso`: `geo` va in panico, per esempio con coordinate
  vicine al massimo di `f64`; `ValidazioneNonConclusa`: la validazione
  OGC non conclude.

Una geometria prodotta oltre il limite di byte per cella (64 MiB di WKB)
è `ResourceLimit`. Il primo errore è quello della prima riga in ordine di riga, senza
diagnostica per riga.

### Limiti e deviazioni

Nel runner un errore non ha diagnostica per riga e il costo in memoria è
una previsione provvisoria
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voci «Geo senza diagnostica per riga» e «Modelli di costo geo provvisori»).
Il lavoro è limitato da `max_coordinates`, un argomento del kernel e non
della config: nel runner `MAX_CELL_COORDINATES`. Il poligono non è quello
di `ST_ConcaveHull` di PostGIS, che
ha altri parametri (frazione dell'area convessa, buchi ammessi).

### Precisione

Nessuna coordinata calcolata: i vertici del poligono sono coordinate
dell'ingresso, con i loro bit. Quali punti entrano nel bordo lo decidono
distanze e confronti in `f64` di `geo`, non predicati esatti: per punti
più vicini di 1 cm al bordo la scelta segue l'arrotondamento, il caso
fuori ambito della regola
([README, «Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).
Nessun rifiuto `PrecisionInsufficient`.

### Complessità

Tempo almeno O(n log n) sulle n coordinate (inviluppo convesso e R-tree
dei punti di `geo`; il costo dello scavo nel caso peggiore non è
dichiarato), più la validazione OGC di ingresso e uscita; memoria O(n).

### Esempio

Con `concavity` 1 il bordo scava fino al punto `(1.5, 1)`; con 3 il
risultato sarebbe il quadrato convesso.

```json
{
  "config": {"concavity": 1.0, "length_threshold": 0.0},
  "ingressi": [
    {"nome": "rilievi", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["MULTIPOINT((0 0),(2 0),(1.5 1),(2 2),(0 2))"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POLYGON((2 0,1.5 1,2 2,0 2,0 0,2 0))"]}
  ]}
}
```
