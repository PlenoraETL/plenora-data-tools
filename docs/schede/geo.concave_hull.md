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
| `length_threshold` | numero | non deciso | finito, non negativo | lati più corti di così non si scavano; `0` scava ogni lato |

`length_threshold` è facoltativo per l'analisi, ma il kernel
(`extended::concave_hull`) lo riceve sempre esplicito, insieme al limite
di coordinate `max_coordinates`, e nessun esecutore lo chiama ancora: il
valore usato quando manca non è deciso.

### Schema

Stesse colonne, tipi, nullabilità e metadati di schema. La colonna
geometria resta al suo posto con lo stesso CRS e le stesse dimensioni
(XY), ma dichiara ora il solo tipo `Polygon` (dichiarazione esatta): le
chiavi `plenora.geometry.types` e `plenora.geometry.types_declaration`
ereditate si tolgono dal campo. Le proprietà del contratto (`sorted_by`,
`row_count`) restano.

### Righe

1:1 per contratto. Il runner non esegue ancora le operazioni geo e nessun
esecutore chiama il kernel su una tabella: l'analisi conserva la
nullabilità della colonna, il kernel lavora su una geometria alla volta.
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

In esecuzione il kernel rende `ExtendedError`, che nessun esecutore
traduce ancora in `PlenoraError`:

- `InvalidInput`: coordinate NaN o infinite o geometria non valida OGC;
- `CoordinateLimit`: più coordinate di `max_coordinates`;
- `InvalidOutput`: il poligono prodotto non è valido OGC, come per un
  punto solo, due punti distinti o punti tutti allineati;
- `CalcoloNonConcluso`: `geo` va in panico, per esempio con coordinate
  vicine al massimo di `f64`; `ValidazioneNonConclusa`: la validazione
  OGC non conclude.

### Limiti e deviazioni

Il lavoro è limitato da `max_coordinates`, un argomento del kernel e non
della config. Il poligono non è quello di `ST_ConcaveHull` di PostGIS, che
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
