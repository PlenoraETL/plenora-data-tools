### Che cosa fa

Semplifica ogni geometria togliendo vertici, nella stessa colonna, con uno
di due algoritmi:

- `douglas_peucker` (default): Ramer-Douglas-Peucker su ogni linea e ogni
  anello; toglie i vertici che distano meno di `tolerance` (una
  **distanza**, unità del CRS) dalla linea semplificata. Un anello resta di
  almeno quattro coordinate; la topologia non è garantita e un risultato
  non valido è un errore;
- `preserve_topology`: Visvalingam-Whyatt con conservazione della topologia
  di `geo` (`simplify_vw_preserve`): toglie i vertici il cui triangolo con i
  due vicini ha **area** non maggiore di `min_area` (unità del CRS al
  quadrato), senza creare intersezioni.

La soglia ha un nome per algoritmo perché non è la stessa grandezza:
`tolerance` con `douglas_peucker`, `min_area` con `preserve_topology`, e
l'altra si rifiuta. Punti e multi-punti passano invariati; una collezione
si semplifica membro per membro. Con soglia 0 la geometria non cambia.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `tolerance` | numero | obbligatorio con `douglas_peucker`, vietato con `preserve_topology` | finito, maggiore o uguale a 0 | distanza massima di un vertice tolto (unità del CRS) |
| `min_area` | numero | obbligatorio con `preserve_topology`, vietato con `douglas_peucker` | finito, maggiore o uguale a 0 | area del triangolo sotto la quale, uguaglianza compresa, un vertice si toglie (unità del CRS al quadrato) |
| `policy` | stringa | `douglas_peucker` | `douglas_peucker`, `preserve_topology` | algoritmo |

### Schema

Identico all'ingresso: la colonna geometria resta al suo posto con lo
stesso nome, CRS, dimensioni e dichiarazione dei tipi; le altre colonne, i
metadati di schema e le proprietà del contratto (`sorted_by`, `row_count`)
passano invariati.

### Righe

1:1: il runner chiama il kernel (`operations::simplify_with_policy`) su ogni cella non
nulla, in parallelo, e rimette la geometria al suo posto; una cella
nulla resta nulla ([README, «Operazioni geo»](../README.md#operazioni-geo)).

### Ordine

L'ordine d'ingresso: l'analisi conserva `sorted_by`. I vertici tenuti
restano nel loro ordine.

### Errori

In validazione (analisi del contratto):

- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non si riconosce come WKB (né estensione `geoarrow.wkb` né chiavi
  `plenora.geometry.*`);
- `Unsupported`: dimensioni della geometria diverse da `xy`;
- `InvalidPlan`: la soglia dell'algoritmo assente (`tolerance` con
  `douglas_peucker`, `min_area` con `preserve_topology`), non finita o
  negativa; la soglia dell'altro algoritmo presente (`tolerance` con
  `preserve_topology`: «la soglia di Visvalingam-Whyatt è un'area,
  `min_area`»; `min_area` con `douglas_peucker`); `policy` fuori elenco;
  campi sconosciuti;
- `Crs`: CRS della colonna assente o non risolto, geografico, o proiettato
  senza unità lineare.

In esecuzione, prima del kernel, su tutta la colonna: `InvalidPlan` per
una cella che non è WKB strutturalmente valido, `Crs` per una coordinata
fuori dal dominio di validità del CRS della colonna, `Schema` per una
geometria di un tipo che il contratto d'ingresso non dichiara, quando li
dichiara con un elenco ([README, «Operazioni geo»](../README.md#operazioni-geo)).

Poi dal kernel, per geometria (`OperationError`, che il runner porta in `PlenoraError`:
`Internal` per `Internal`, `ValidazioneNonConclusa` e
`CalcoloNonConcluso`, `Unsupported` per `PrecisionInsufficient`,
`InvalidPlan` per le altre):

- `InvalidInput`: la geometria non supera la validazione OGC;
- `InvalidOutput`: la geometria semplificata non supera la validazione OGC
  (per esempio anelli che si incrociano dopo Douglas-Peucker);
- `Internal`: con `douglas_peucker`, una distanza intermedia non è un
  numero finito (coordinate vicine ai limiti di `f64`); nessun risultato
  parziale;
- `ValidazioneNonConclusa`, `CalcoloNonConcluso`: la validazione OGC o il
  calcolo di `geo` vanno in panico dentro la barriera (il messaggio porta
  solo la forma del payload);
- `InvalidParameter`: soglia non finita o negativa (l'analisi la
  rifiuta prima); sul percorso scalato (sotto), un'area positiva che nella
  scala delle coordinate diventa zero.

Una geometria prodotta oltre il limite di byte per cella (64 MiB di WKB)
è `ResourceLimit`. Il primo errore è quello della prima riga in ordine di riga, senza
diagnostica per riga.

### Limiti e deviazioni

Nel runner un errore non ha diagnostica per riga e il costo in memoria è
una previsione dalle misure
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).
`preserve_topology` non è `TopologyPreservingSimplifier` di GEOS
(`ST_SimplifyPreserveTopology`), che usa una distanza: qui la soglia è
un'area, e per questo si chiama `min_area`. Fino alla versione 2 del
catalogo si scriveva `tolerance` anche qui, e un piano scritto pensando a
GEOS semplificava in modo molto diverso senza errore: ora quel piano si
rifiuta in validazione. Le coordinate di modulo oltre `1e150`, o non
nulle e sotto `1e-150`, si semplificano in uno spazio scalato
uniformemente e si riportano indietro, con la soglia scalata come la sua
grandezza (una distanza per il fattore, un'area per il suo quadrato; fino
alla versione 2 l'area si scalava come una distanza).

### Precisione

Esatta: i vertici d'uscita sono vertici d'ingresso, senza calcolo, fuori
dal percorso scalato (coordinate oltre `1e150` o sotto `1e-150` in modulo,
fuori da ogni dominio di un CRS reale), dove passano per una divisione e
una moltiplicazione. Lo scarto dalla forma originale è quello chiesto con
la soglia, non un errore di precisione
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
