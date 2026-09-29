### Che cosa fa

Aggiunge alla sinistra una colonna booleana che dice se la sua geometria
sta dentro almeno una geometria della destra (kernel
`analysis::within_indexes`, predicato `within` del join spaziale). «Dentro»
è il `contains` di `geo` letto dalla destra: una geometria sul solo bordo,
come un punto sul lato di un poligono, non è dentro. Il runner non esegue
ancora le operazioni geo ([README, «Che cosa non c'è
ancora»](../README.md#che-cosa-non-cè-ancora)): lo schema qui descritto è
quello dell'analisi del contratto, i valori quelli del kernel.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `output_column` | stringa | `within` | nome non vuoto e libero nella sinistra | colonna aggiunta |

### Schema

Le colonne della sinistra, invariate, più `output_column` in coda, `bool`
nullable. Le colonne della destra non passano. La colonna geometria resta
quella della sinistra, con i suoi tipi dichiarati. I metadati di schema
sono la fusione dei due lati; le proprietà del contratto della sinistra
(`sorted_by`, `row_count`) restano.

### Righe

1:1 con la sinistra; la destra non aggiunge righe. Il kernel rende le
posizioni delle righe sinistre dentro almeno una destra, che nella colonna
sono `true`, le altre `false`. Le geometrie nulle o vuote, da un lato o
dall'altro, non sono mai dentro. Il passaggio dalle posizioni alla colonna,
e il valore per una geometria sinistra nulla (`false` o nullo), non sono
ancora codice di questo repository.

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

In esecuzione (kernel `analysis::within_indexes`, errore `AnalysisError`
che avvolge `SpatialJoinError`; nessun codice di questo repository lo
traduce ancora in `PlenoraError`):

- `InvalidPairLimit`: il limite delle coppie che il chiamante passa al
  kernel è zero;
- `PairLimitExceeded`: le coppie (sinistra, destra) confermate superano il
  limite; conta ogni destra che contiene una sinistra, anche se ne basta
  una;
- `NonFiniteCoordinate`, `InvalidGeometry`: una geometria ha coordinate
  NaN o infinite o non supera la validazione OGC (l'errore porta il lato e
  la posizione, mai i valori);
- `ValidazioneNonConclusa`, `CalcoloNonConcluso`: la validazione o il
  predicato di `geo` non ha concluso;
- `IndexOverflow`: un numero di righe non entra in `u64`.

### Limiti e deviazioni

Nessuno oltre ai limiti comuni.

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
