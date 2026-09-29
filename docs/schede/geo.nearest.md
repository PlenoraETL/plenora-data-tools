### Che cosa fa

Abbina ogni riga della sinistra alla riga della destra più vicina, con la
distanza planare fra le due geometrie (kernel `analysis::nearest_matches`):
in caso di pari tutte le destre alla distanza minima, come `sjoin_nearest`
di GeoPandas. Il runner non esegue ancora le operazioni geo ([README, «Che
cosa non c'è ancora»](../README.md#che-cosa-non-cè-ancora)): lo schema qui
descritto è quello dell'analisi del contratto, gli abbinamenti quelli del
kernel.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `max_distance` | numero | assente: nessun limite | finito, `>= 0`, nelle unità del CRS | una riga sinistra il cui vicino è più lontano non ha abbinamenti; un vicino esattamente a `max_distance` vale |

### Schema

Le colonne della sinistra, invariate, più in coda `__right_index`
(`uint64`, la posizione della riga destra) e `distance` (`float64`, nelle
unità del CRS), entrambe nullable. Le colonne della destra non passano: si
ricollegano con `__right_index`. La colonna geometria resta quella della
sinistra. I metadati di schema sono la fusione dei due lati; le proprietà
del contratto (`sorted_by`, `row_count`) si perdono.

### Righe

Il kernel rende, per ogni riga sinistra con geometria non nulla e non
vuota, le righe destre (non nulle, non vuote) alla distanza minima: una di
solito, più di una in caso di pari, quindi l'uscita può superare la
sinistra. Una riga sinistra nulla o vuota, senza destre utilizzabili o con
il vicino oltre `max_distance` non ha abbinamenti. Il kernel non la
emette; le colonne nullable del contratto lasciano spazio a una riga con
`__right_index` e `distance` nulli. Quale delle due valga, e il passo dagli
abbinamenti alle righe, non sono ancora codice di questo repository.

### Ordine

Per riga sinistra, poi per `__right_index` crescente.

### Errori

In validazione (analisi del contratto):

- `InvalidPlan`: un numero di ingressi diverso da due; config con campi
  sconosciuti; `max_distance` negativa o non finita; un metadato di
  schema presente sui due lati con valori diversi;
- `Schema`: `__right_index` o `distance` esiste già nella sinistra; un
  lato senza esattamente una colonna geometria, o con una colonna non
  riconoscibile come geometria WKB (né estensione `geoarrow.wkb` né chiavi
  canoniche `plenora.geometry.*`);
- `Unsupported`: una colonna geometria non XY;
- `Crs`: un lato senza CRS risolto, un CRS non proiettato (o senza unità
  lineare), CRS dei due lati non equivalenti.

In esecuzione (kernel `analysis::nearest_matches`, errore
`AnalysisError`; nessun codice di questo repository lo traduce ancora in
`PlenoraError`):

- `InvalidWorkLimit`: il limite dei confronti o degli abbinamenti che il
  chiamante passa al kernel è zero;
- `WorkLimitExceeded`: i confronti della forza bruta, righe sinistre non
  nulle per righe destre non nulle e non vuote, superano il limite (anche
  se l'indice ne fa meno);
- `ResultLimitExceeded`: gli abbinamenti superano il limite (ogni altro
  errore, il primo in ordine di riga, ha la precedenza);
- `InvalidGeometry`: una geometria ha coordinate NaN o infinite o non
  supera la validazione OGC (l'errore porta il lato e la posizione, mai i
  valori);
- `ValidazioneNonConclusa`: la validazione OGC non ha concluso;
- `CalcoloNonConcluso`: la distanza di `geo` è andata in panico su un
  candidato;
- `IndexOverflow`: un indice non entra in `u64`.

### Limiti e deviazioni

Un R-tree sceglie i candidati e scarta una destra solo quando la sua
distanza non può essere il minimo, con un margine appoggiato alla stima
d'errore della distanza di `geo` letta dai sorgenti, non dimostrata; le
geometrie fuori da quella stima non si scartano mai. L'oracolo confronta
il risultato con la forza bruta sui bit ([README, «`geo.nearest`: lo
scarto dell'R-tree si appoggia alla stima d'errore di
`geo`»](../README.md#geonearest-lo-scarto-dellr-tree-si-appoggia-alla-stima-derrore-di-geo)).
Il catalogo dichiara il vincolo `uscita / sinistra`, ma i pari possono dare
più righe della sinistra.

### Precisione

Nessun calcolo di geometrie e nessuna griglia: la distanza è
`Euclidean.distance` di `geo` in `f64` sulle coordinate d'ingresso, con un
errore di arrotondamento stimato sotto `64 · eps` volte il modulo massimo
delle coordinate su geometrie regolari (circa 0,14 µm con coordinate
fino a 10.000 km), molto sotto 1 cm. I pari sono
uguaglianze esatte dei valori calcolati: due destre la cui distanza vera è
uguale possono non risultare pari se i loro `f64` differiscono nell'ultima
cifra ([README, «Precisione delle operazioni geografiche: 1 cm a
terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

### Complessità

Un R-tree dei rettangoli d'ingombro della destra, O(m log m) per `m`
righe; per ognuna delle `n` righe sinistre, in parallelo, un primo vicino
nell'albero, una finestra di candidati e la distanza sui soli candidati.
Di norma O((n + m) log m) più le distanze; nel caso peggiore (destre
equidistanti da molte sinistre, rettangoli grandi sovrapposti) O(n · m).
Memoria O(m) per l'albero più gli abbinamenti.

### Esempio

Il primo pozzo ha due fontane alla stessa distanza, ed esce due volte.

```json
{
  "config": {},
  "ingressi": [
    {"nome": "pozzi", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POINT(0 0)", "POINT(10 10)"]}
    ]},
    {"nome": "fontane", "colonne": [
      {"nome": "codice", "tipo": "utf8", "valori": ["a", "b", "c"]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POINT(-1 0)", "POINT(1 0)", "POINT(10 13)"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 1, 2]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POINT(0 0)", "POINT(0 0)", "POINT(10 10)"]},
    {"nome": "__right_index", "tipo": "uint64", "valori": [0, 1, 2]},
    {"nome": "distance", "tipo": "float64", "valori": [1.0, 1.0, 3.0]}
  ]}
}
```
