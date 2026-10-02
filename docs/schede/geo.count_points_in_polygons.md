### Che cosa fa

Aggiunge alla sinistra, di norma poligoni, una colonna con il numero di
geometrie della destra, di norma punti, che ogni sua geometria contiene
(kernel `analysis::count_points_in_polygons_validated`; [Runner,
«Operazioni geo»](runner.md#operazioni-geo)). «Contiene» è il
`contains` di `geo`: i punti sul bordo non contano, come il
`predicate="within"` di Manipola, e un punto dentro più poligoni conta in
ognuno.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `output_column` | stringa | `count` | nome non vuoto e libero nella sinistra | colonna aggiunta |

### Schema

Le colonne della sinistra, invariate, più `output_column` in coda,
`uint64`, nullable solo se lo è la geometria della sinistra (null dove è
null). Le colonne della destra non passano. La colonna
geometria resta quella della sinistra, con i suoi tipi dichiarati. I
metadati di schema sono la fusione dei due lati; le proprietà del
contratto della sinistra (`sorted_by`, `row_count`) restano.

### Righe

1:1 con la sinistra; la destra non aggiunge righe. Il kernel rende un
conteggio per ogni geometria sinistra, nella stessa posizione, 0 se non ne
contiene nessuna (anche per una geometria vuota); una geometria sinistra
nulla dà un valore nullo. Una geometria destra nulla o vuota non conta
mai. Il tipo delle geometrie non si controlla: conta ogni geometria destra
contenuta, anche una linea. Le coppie punto-poligono che il kernel
conferma sono al più il limite di righe dell'arco d'uscita
(`max_output_rows` se il passo è un output del piano, `max_rows_per_edge`
altrimenti).

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
([Runner, «Operazioni geo»](runner.md#operazioni-geo)):

- `Schema`: il contratto di un lato dichiara i tipi geometrici con un
  elenco e la cella è di un altro tipo;
- `Crs`: una coordinata fuori dal dominio di validità del CRS della
  colonna;
- `InvalidPlan`: la cella viola il contratto WKB o non supera la
  validazione OGC (`Unsupported` per dimensioni Z o M, `Internal` se la
  validazione non conclude, `ResourceLimit` per una cella oltre il limite
  di byte).

Dal kernel (`analysis::count_points_in_polygons_validated`, errore
`AnalysisError` che avvolge `SpatialJoinError`, sulle geometrie già
validate), nella categoria del passo geo indicata fra parentesi:

- `PairLimitExceeded` (`ResourceLimit`): le coppie punto-poligono
  confermate superano il limite di righe dell'arco;
- `MargineMemoria` (`ResourceLimit`): le stesse coppie non starebbero nel
  margine di memoria del passo (64 byte ciascuna; guardia che riduce il rischio, non un tetto garantito: [Runner, «Limiti dichiarati del runner»](runner.md#limiti-dichiarati-del-runner), voce «Modelli di costo geo»);
- `ValidazioneNonConclusa`, `CalcoloNonConcluso`, `Internal` (`Internal`):
  l'indice o il predicato di `geo` non ha concluso, o un'invariante
  interna violata;
- `IndexOverflow` (`InvalidPlan`): un numero di righe non entra in `u64`.

Il runner verifica che il kernel renda un conteggio per ogni riga
sinistra, altrimenti `Internal`.

Il primo errore è quello della prima riga, in ordine di riga, senza
diagnostica per riga ([Runner, «Limiti dichiarati del
runner»](runner.md#limiti-dichiarati-del-runner), voce «Geo senza
diagnostica per riga»).

### Limiti e deviazioni

Il limite delle coppie conta ogni coppia punto-poligono confermata: un
punto dentro molti poligoni sovrapposti conta una coppia per poligono.

### Precisione

Nessun calcolo di geometrie e nessuna griglia: il predicato di `geo` si
valuta sulle coordinate `f64` d'ingresso, senza tolleranza, e la regola di
1 cm non sposta nulla. Un punto a meno di 1 cm dal bordo conta o no
secondo le sue coordinate esatte ([Limiti dichiarati, «Precisione delle operazioni
geografiche: 1 cm a
terra»](limiti.md#precisione-delle-operazioni-geografiche-1-cm-a-terra),
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
