### Che cosa fa

Unisce ogni riga di sinistra alle righe di destra che hanno la stessa
chiave, fatta di una o più colonne (`left_keys[i]` con `right_keys[i]`).
`how` sceglie quali righe senza corrispondenza restano: nessuna (`inner`),
le sinistre (`left`), le destre (`right`), tutte (`outer`). Le colonne
chiave di destra non compaiono nell'uscita; le altre colonne prendono un
suffisso.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `left_keys` | lista di stringhe | obbligatorio | colonne della sinistra, almeno una, senza ripetizioni | colonne chiave del lato sinistro |
| `right_keys` | lista di stringhe | obbligatorio | colonne della destra, tante quante `left_keys`, senza ripetizioni | colonne chiave del lato destro, nello stesso ordine |
| `how` | stringa | `inner` | `inner`, `left`, `right`, `outer` | righe senza corrispondenza da tenere |

Le due colonne di ogni coppia hanno lo stesso tipo Arrow (timezone,
precisione e scala comprese), scelto fra `utf8`, `int64`, `uint64`,
`float64`, `bool`, `date32`, `timestamp(ms)` (timezone assente o valida),
`decimal128` con scala da 0 a 38, `binary` e `dictionary<utf8>`. Con
`right` e `outer` la chiave d'uscita fonde i due lati, e i tipi ammessi
sono solo `utf8`, `int64`, `uint64`, `float64`, `bool` e `date32`.

### Schema

Prima tutte le colonne di sinistra, nel loro ordine, poi quelle di destra
che non sono chiave. Le colonne chiave di sinistra tengono il nome, le
altre di sinistra prendono il suffisso `_L`, tutte quelle di destra `_R`,
anche quando il nome non collide. Tipi e metadati di campo restano quelli
d'origine; ogni colonna d'uscita è nullable. I metadati di schema dei due
lati si fondono: una chiave presente da un lato solo, o con lo stesso
valore, resta. La colonna geometrica di un lato si conserva con il suo
nome d'uscita; l'uscita ne ammette una sola. Nessuna proprietà del
contratto (`sorted_by`, `row_count`) sopravvive.

### Righe

Per ogni riga di sinistra, una riga per ogni riga di destra con la stessa
chiave: una chiave presente `m` volte a sinistra e `n` a destra dà `m·n`
righe. Due chiavi sono uguali se lo sono tutte le loro colonne, confrontate
per valore nel tipo comune: testi e `binary` byte per byte, `float64` con
tutti i NaN uguali fra loro e `-0.0` diverso da `0.0`, `dictionary` sul
testo della voce. Una chiave con almeno una colonna nulla non si abbina
mai, nemmeno a un'altra chiave nulla.

Le righe senza corrispondenza:

- `inner`: si scartano;
- `left` e `outer`: ogni riga sinistra senza corrispondenza resta una
  volta, con le colonne di destra nulle;
- `right` e `outer`: ogni riga destra senza corrispondenza si aggiunge una
  volta, con le colonne di sinistra nulle.

Con `inner` e `left` una colonna chiave d'uscita vale la chiave sinistra.
Con `right` e `outer` vale la chiave sinistra e, dove questa è nulla,
quella destra: le righe destre senza corrispondenza portano così la
propria chiave.

### Ordine

Comanda la sinistra: per ogni riga di sinistra, nell'ordine d'ingresso, le
sue corrispondenze nell'ordine delle righe di destra (o la riga senza
corrispondenza, al suo posto). Con `right` e `outer` le righe destre senza
corrispondenza vengono in coda, nell'ordine di destra. Anche con `right`
l'ordine segue la sinistra: non è `left` con i lati scambiati. L'ordine non
dipende dal parallelismo della sonda.

### Errori

In validazione (analisi del contratto), `InvalidPlan`:

- config con campi sconosciuti, `how` fuori elenco, `left_keys` o
  `right_keys` assenti;
- liste di chiavi vuote, di lunghezza diversa, con nomi ripetuti o oltre
  `max_columns`; colonna assente; tipi diversi nella coppia; tipo fuori
  dall'elenco sopra;
- `how` `right` o `outer` con una chiave di tipo non fondibile;
- nomi d'uscita che collidono dopo i suffissi (a sinistra una chiave
  `a_L` e una colonna non chiave `a`), nome oltre 1024 byte, più colonne
  di `max_columns`;
- due colonne geometriche nell'uscita (una per lato);
- metadati di schema con la stessa chiave e valori diversi sui due lati.

In esecuzione:

- `ResourceLimit`: righe d'uscita oltre `max_rows` (nel runner
  `max_input_rows`), contate prima di costruirle;
- `Schema`: una cella chiave `date32` o `timestamp(ms)` fuori
  dall'intervallo delle date rappresentabili, o un dizionario malformato.

Chiamato senza l'analisi, il kernel ripete i controlli su chiavi, tipi e
nomi con `Schema` (le colonne oltre `max_columns` con `ResourceLimit`).

### Limiti e deviazioni

Nessuna conversione fra tipi: `int64` contro `float64`, o contro un intero
di altra larghezza, si rifiuta, e gli interi diversi da `int64`/`uint64`
non sono chiavi. Le mappe delle chiavi usano un hash deterministico senza
seme ([README, «Hash delle chiavi non keyed»](../README.md#hash-delle-chiavi-non-keyed)).
Il kernel non confronta con `max_governed_memory_bytes` né la mappa delle
chiavi né l'uscita: il suo limite è `max_rows`; nel runner il picco lo
prevede il modello di costo ([README, «Budget di memoria»](../README.md#budget-di-memoria)).

### Complessità

Tempo O(n + m + k) atteso, con `n` e `m` le righe dei due lati e `k` quelle
d'uscita: mappa sulle chiavi di destra, sonda delle chiavi di sinistra
(in parallelo da 65.536 righe sinistre). Memoria O(m) per la mappa, O(k)
per gli indici e le colonne d'uscita. Le chiavi `int64`, `uint64`,
`float64`, `bool` e `utf8` si confrontano sui valori nativi; gli altri tipi
passano da una chiave in byte costruita per riga.

### Esempio

```json
{
  "config": {"left_keys": ["cliente"], "right_keys": ["cliente"], "how": "outer"},
  "ingressi": [
    {"nome": "ordini", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
      {"nome": "cliente", "tipo": "utf8", "valori": ["a", "b", null]}
    ]},
    {"nome": "clienti", "colonne": [
      {"nome": "cliente", "tipo": "utf8", "valori": ["a", "a", "c"]},
      {"nome": "nome", "tipo": "utf8", "valori": ["Anna", "Alba", "Carla"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id_L", "tipo": "int64", "valori": [1, 1, 2, 3, null]},
    {"nome": "cliente", "tipo": "utf8", "valori": ["a", "a", "b", null, "c"]},
    {"nome": "nome_R", "tipo": "utf8", "valori": ["Anna", "Alba", null, null, "Carla"]}
  ]}
}
```
