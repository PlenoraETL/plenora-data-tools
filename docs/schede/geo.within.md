### Che cosa fa

Aggiunge alla sinistra una colonna booleana che dice se la sua geometria
sta dentro almeno una geometria della destra (kernel
`analysis::within_indexes_validated`, predicato `within` del join
spaziale; [README, «Operazioni geo»](../README.md#operazioni-geo)).
«Dentro» è il `contains` di `geo` letto dalla destra: una geometria sul
solo bordo, come un punto sul lato di un poligono, non è dentro.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `output_column` | stringa | `within` | nome non vuoto e libero nella sinistra | colonna aggiunta |

### Schema

Le colonne della sinistra, invariate, più `output_column` in coda, `bool`,
nullable solo se lo è la geometria della sinistra (null dove è null). Le colonne della destra non passano. La colonna geometria resta
quella della sinistra, con i suoi tipi dichiarati. I metadati di schema
sono la fusione dei due lati; le proprietà del contratto della sinistra
(`sorted_by`, `row_count`) restano.

### Righe

1:1 con la sinistra; la destra non aggiunge righe. Il kernel rende le
posizioni delle righe sinistre dentro almeno una destra, che nella colonna
sono `true`, le altre `false`. Una geometria sinistra nulla dà un valore
nullo; una vuota, o dentro solo geometrie destre nulle o vuote, `false`.
Le coppie (sinistra, destra) che il kernel conferma sono al più il limite
di righe dell'arco d'uscita (`max_output_rows` se il passo è un output
del piano, `max_rows_per_edge` altrimenti).

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

In esecuzione, prima del kernel, su ogni cella non nulla dei due lati
([README, «Operazioni geo»](../README.md#operazioni-geo)):

- `Schema`: il contratto di un lato dichiara i tipi geometrici con un
  elenco e la cella è di un altro tipo;
- `Crs`: una coordinata fuori dal dominio di validità del CRS della
  colonna;
- `InvalidPlan`: la cella viola il contratto WKB o non supera la
  validazione OGC (`Unsupported` per dimensioni Z o M, `Internal` se la
  validazione non conclude, `ResourceLimit` per una cella oltre il limite
  di byte).

Dal kernel (`analysis::within_indexes_validated`, errore `AnalysisError`
che avvolge `SpatialJoinError`, sulle geometrie già validate), nella
categoria del passo geo indicata fra parentesi:

- `PairLimitExceeded` (`ResourceLimit`): le coppie (sinistra, destra)
  confermate superano il limite di righe dell'arco; conta ogni destra che
  contiene una sinistra, anche se ne basta una;
- `MargineMemoria` (`ResourceLimit`): le stesse coppie non starebbero nel
  margine di memoria del passo (64 byte ciascuna, a maggiorante delle capacità; [README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner), voce «Modelli di costo geo»);
- `ValidazioneNonConclusa`, `CalcoloNonConcluso`, `Internal` (`Internal`):
  l'indice o il predicato di `geo` non ha concluso, o un'invariante
  interna violata;
- `IndexOverflow` (`InvalidPlan`): un numero di righe non entra in `u64`.

Il primo errore è quello della prima riga, in ordine di riga, senza
diagnostica per riga ([README, «Limiti dichiarati del
runner»](../README.md#limiti-dichiarati-del-runner), voce «Geo senza
diagnostica per riga»).

### Limiti e deviazioni

Il limite delle coppie conta tutte le destre che contengono una
sinistra, non solo la prima: una sinistra dentro molte destre sovrapposte
può superarlo anche se la colonna ha una riga per riga sinistra.

### Precisione

Nessun calcolo di geometrie e nessuna griglia: il predicato di `geo` si
valuta sulle coordinate `f64` d'ingresso, senza tolleranza, e la regola di
1 cm non sposta nulla. Una geometria a meno di 1 cm dal bordo è dentro o
fuori secondo le sue coordinate esatte ([README, «Precisione delle
operazioni geografiche: 1 cm a
terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra),
«Feature d'ingresso più vicine della precisione»).

### Complessità

Un R-tree dei rettangoli d'ingombro della destra, O(m log m) per `m`
righe; per ognuna delle `n` righe sinistre una ricerca nell'albero e il
predicato esatto sui soli candidati. Di norma O((n + m) log m) più il
costo dei predicati; nel caso peggiore (rettangoli tutti sovrapposti)
O(n · m) predicati. Più la validazione OGC di ogni geometria. Memoria O(m)
per l'albero più le coppie confermate.

### Esempio

Il secondo punto sta sul bordo del poligono, quindi non è dentro.

```json
{
  "config": {},
  "ingressi": [
    {"nome": "pozzi", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POINT(1 1)", "POINT(0 1)", "POINT(5 5)"]}
    ]},
    {"nome": "aree", "colonne": [
      {"nome": "nome", "tipo": "utf8", "valori": ["parco"]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POLYGON((0 0,2 0,2 2,0 2,0 0))"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POINT(1 1)", "POINT(0 1)", "POINT(5 5)"]},
    {"nome": "within", "tipo": "bool", "valori": [true, false, false]}
  ]}
}
```
