### Che cosa fa

Semplifica ogni geometria togliendo vertici, nella stessa colonna, con uno
di due algoritmi:

- `douglas_peucker` (default): Ramer-Douglas-Peucker su ogni linea e ogni
  anello; toglie i vertici che distano meno di `tolerance` (unità del CRS)
  dalla linea semplificata. Un anello resta di almeno quattro coordinate;
  la topologia non è garantita e un risultato non valido è un errore;
- `preserve_topology`: Visvalingam-Whyatt con conservazione della topologia
  di `geo` (`simplify_vw_preserve`): toglie i vertici il cui triangolo con i
  due vicini ha **area** minore di `tolerance` (unità del CRS al quadrato),
  senza creare intersezioni.

Punti e multi-punti passano invariati; una collezione si semplifica membro
per membro. Con `tolerance` 0 la geometria non cambia.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `tolerance` | numero | obbligatorio | finito, maggiore o uguale a 0 | soglia: distanza con `douglas_peucker`, area con `preserve_topology` |
| `policy` | stringa | `douglas_peucker` | `douglas_peucker`, `preserve_topology` | algoritmo |

### Schema

Identico all'ingresso: la colonna geometria resta al suo posto con lo
stesso nome, CRS, dimensioni e dichiarazione dei tipi; le altre colonne, i
metadati di schema e le proprietà del contratto (`sorted_by`, `row_count`)
passano invariati.

### Righe

1:1 per contratto. Il kernel (`operations::simplify_with_policy`) lavora
su una geometria alla volta e nessun adapter lo chiama ancora sulle righe:
il trattamento di una cella nulla non è definito da codice eseguito.

### Ordine

L'ordine d'ingresso: l'analisi conserva `sorted_by`. I vertici tenuti
restano nel loro ordine.

### Errori

In validazione (analisi del contratto):

- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non si riconosce come WKB (né estensione `geoarrow.wkb` né chiavi
  `plenora.geometry.*`);
- `Unsupported`: dimensioni della geometria diverse da `xy`;
- `InvalidPlan`: `tolerance` assente, non finita o negativa, `policy` fuori
  elenco, campi sconosciuti;
- `Crs`: CRS della colonna assente o non risolto, geografico, o proiettato
  senza unità lineare.

In esecuzione, dal kernel, per geometria (`OperationError`, che nessun
codice traduce ancora in `PlenoraError`):

- `InvalidInput`: la geometria non supera la validazione OGC;
- `InvalidOutput`: la geometria semplificata non supera la validazione OGC
  (per esempio anelli che si incrociano dopo Douglas-Peucker);
- `Internal`: con `douglas_peucker`, una distanza intermedia non è un
  numero finito (coordinate vicine ai limiti di `f64`); nessun risultato
  parziale;
- `ValidazioneNonConclusa`, `CalcoloNonConcluso`: la validazione OGC o il
  calcolo di `geo` vanno in panico dentro la barriera (il messaggio porta
  solo la forma del payload);
- `InvalidParameter`: `tolerance` non finita o negativa (l'analisi la
  rifiuta prima).

### Limiti e deviazioni

Il runner non esegue ancora le operazioni geo
([README, «Che cosa non c'è ancora»](../README.md#che-cosa-non-cè-ancora)).
`preserve_topology` non è `TopologyPreservingSimplifier` di GEOS
(`ST_SimplifyPreserveTopology`), che usa una distanza: qui la soglia è
un'area, e lo stesso numero semplifica in modo molto diverso. Le coordinate
di modulo oltre `1e150`, o non nulle e sotto `1e-150`, si semplificano in
uno spazio scalato uniformemente e si riportano indietro.

### Precisione

Esatta: i vertici d'uscita sono vertici d'ingresso, senza calcolo, fuori
dal percorso scalato (coordinate oltre `1e150` o sotto `1e-150` in modulo,
fuori da ogni dominio di un CRS reale), dove passano per una divisione e
una moltiplicazione. Lo scarto dalla forma originale è quello chiesto con
`tolerance`, non un errore di precisione
([README, «Precisione delle operazioni geografiche»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

### Complessità

Per geometria di n vertici: `douglas_peucker` O(n log n) nel caso tipico e
O(n²) nel peggiore, con una seconda traversata dello stesso costo che
verifica le distanze; `preserve_topology` O(n log n) nel caso tipico (coda di priorità e
indice spaziale di `geo`). Più la validazione OGC dell'ingresso e
dell'uscita (sub-quadratica nel caso tipico, O(n²) nel peggiore:
[README, «Validazione OGC»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì)).

### Esempio

```json
{
  "config": {"tolerance": 0.5},
  "ingressi": [
    {"nome": "tratte", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["LINESTRING(0 0,5 0.2,10 0)", "POLYGON((0 0,10 0,10 10,5 10.1,0 10,0 0))"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["LINESTRING(0 0,10 0)", "POLYGON((0 0,10 0,10 10,0 10,0 0))"]}
  ]}
}
```
