### Che cosa fa

Raggruppa le righe per i valori delle colonne `group_by` e produce una riga
per gruppo: le colonne di gruppo, poi una colonna per ogni aggregazione di
`aggregations` (conteggi, somme, medie, estremi, varianze, quantili,
testi). Senza aggregazioni conta le righe di ogni gruppo.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `group_by` | lista di stringhe | obbligatorio | nomi di colonne leggibili come testo, almeno uno, senza ripetizioni | chiave di gruppo |
| `aggregations` | lista di oggetti | `[]` | al più `max_columns` voci, campi sotto | aggregazioni, nell'ordine delle colonne d'uscita |

Campi di ogni aggregazione:

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa | obbligatorio | nome di una colonna dell'ingresso | colonna aggregata |
| `function` | stringa | `"count"` | `count`, `sum`, `avg`, `mean`, `min`, `max`, `first`, `last`, `concat`, `nunique`, `variance`, `stddev`, `quantile` | funzione (`avg` è `mean`) |
| `alias` | stringa | `""` | nome di colonna valido, o vuoto | nome della colonna d'uscita |
| `separator` | stringa | `", "` | entro `max_string_bytes`; solo con `concat` | separatore di `concat` |
| `distinct` | booleano | `false` | non con `count`, `nunique`, `first`, `last` | aggrega solo i valori distinti |
| `skip_null` | booleano | `true` | non con `count`, `first`, `last` | ignora i null (vedi sotto) |
| `quantile` | numero | nessuno | da `0` a `1`; obbligatorio con `quantile`, solo con `quantile` | quantile chiesto |
| `ddof` | intero | `1` | `0` o più; solo con `variance` e `stddev` | gradi di libertà sottratti al divisore |

Un parametro scritto per una funzione che non lo usa si rifiuta.

Nome della colonna d'uscita: `alias` se non è vuoto; altrimenti
`<column>_<funzione>` (`avg` scrive `mean`) se la stessa `column` compare
in più aggregazioni; altrimenti `column`. Un nome uguale a una colonna già
prodotta (di gruppo o di un'aggregazione precedente) la sostituisce al suo
posto: vale l'ultima.

Funzioni:

- `count`: `int64` non nullabile, le celle non nulle del gruppo; ammette
  una colonna di qualsiasi tipo;
- `nunique`: `int64` non nullabile, i testi distinti del gruppo, più uno
  se c'è un null e `skip_null` è `false`;
- `first`, `last`: `utf8`, il testo della cella nella prima o nell'ultima
  riga del gruppo in ordine d'ingresso, null compreso;
- `concat`: `utf8`, i testi delle celle in ordine d'ingresso uniti da
  `separator`; con `distinct` solo la prima occorrenza di ogni testo; un
  null si salta, o vale il testo vuoto con `skip_null: false`;
- `sum`, `mean`, `min`, `max`, `variance`, `stddev`, `quantile`:
  `float64`. La cella si legge come `f64` (vedi i limiti). Con
  `skip_null: false` un null nel gruppo dà null; un gruppo senza valori dà
  null. `sum` somma in ordine d'ingresso; `min` e `max` ignorano i NaN
  salvo che il gruppo abbia solo NaN, la somma no; `variance` e `stddev`
  dividono per `valori - ddof` e danno null con `valori <= ddof`;
  `quantile` interpola linearmente fra i valori, ordinati come i
  `float64` di [`table.sort`](#tablesort), alla posizione
  `quantile * (valori - 1)`. Con `distinct` i valori si
  deduplicano sul valore esatto (su `float64` per bit: `-0.0` e `0.0`
  distinti) e si riducono in ordine crescente.

`nunique`, `concat`, `first`, `last` vogliono una colonna leggibile come
testo (i tipi di [`table.distinct`](#tabledistinct)); le funzioni numeriche
una colonna `int64`, `uint64`, `float64`, `decimal128`, `date32` (giorni),
`timestamp(ms)` (millisecondi) o `utf8` il cui testo è un numero (spazi ai
lati ignorati, virgola decimale ammessa).

### Schema

Le colonne di `group_by` nel loro ordine, con tipo, nullabilità e metadati
di campo dell'ingresso; poi una colonna per aggregazione, nell'ordine di
`aggregations`, senza metadati di campo (tipi sopra); senza aggregazioni,
una colonna `count` `int64` non nullabile con le righe del gruppo, null
compresi. I metadati di schema si conservano. Una colonna geometrica resta
geometrica solo se è una chiave di gruppo. Il contratto non dichiara né
ordinamento né conteggio.

### Righe

Aggregazione: una riga per chiave di gruppo distinta. L'uguaglianza delle
chiavi è quella di [`table.distinct`](#tabledistinct): un null è un gruppo
a sé, `-0.0` e `0.0` sono gruppi diversi, tutti i NaN un gruppo solo.

### Ordine

I gruppi escono nell'ordine della loro chiave in testo, colonna per
colonna: il gruppo del null per primo, poi i valori nell'ordine dei byte
della stringa `<n>:<testo>`, con `n` la lunghezza in byte del testo scritta
in decimale. Quindi non è l'ordine dei valori: `"sud"` (`3:sud`) precede
`"nord"` (`4:nord`), `9` precede `-1`, `-1` precede `-2` e `10`, e un
testo di 10 byte precede uno di 1 byte (`10:` precede `1:`). L'ordine è
deterministico e non dipende dall'hash.

### Errori

In validazione, `InvalidPlan`:

- `group_by` vuoto, con un nome ripetuto o non valido, o oltre il limite
  di colonne; `aggregations` oltre il limite di colonne;
- una colonna assente; una chiave di gruppo non leggibile come testo;
- una funzione su un tipo che non accetta (sopra);
- `quantile` assente con `function: "quantile"`, o fuori da `0..1`;
- un parametro scritto per una funzione che non lo usa; `separator` oltre
  `max_string_bytes`; un nome d'uscita non valido (per esempio un `alias`
  di soli spazi);
- funzione fuori elenco, campi sconosciuti.

In esecuzione:

- `Schema`: una cella `utf8` che non è un numero sotto una funzione
  numerica; una cella che non si converte in testo (date fuori
  intervallo, `binary` non UTF-8 sotto `first`, `last`, `concat`,
  `nunique`);
- `ResourceLimit`: più di `u32::MAX` righe; nella variante spilled, file
  temporanei oltre `max_temp_bytes`, o una partizione i cui batch superano
  `max_governed_memory_bytes`;
- `Io`: nella variante spilled, un errore sui file temporanei.

### Limiti e deviazioni

Le funzioni numeriche calcolano in `f64` perché il risultato è `float64`:
un intero oltre `2^53`, un `decimal128` o un testo con più cifre di quante
un `f64` ne tenga si arrotondano senza errore (con `distinct` i distinti si
decidono comunque sul valore esatto). Due aggregazioni con lo stesso nome
d'uscita, o un nome uguale a una chiave di gruppo, non si rifiutano: vale
l'ultima, e la colonna sostituita sparisce. Le strutture di chiavi e gruppi
non sono contabilizzate
([README, «Memoria delle chiavi dei kernel in memoria non governata»](../README.md#memoria-delle-chiavi-dei-kernel-in-memoria-non-governata));
l'hash delle chiavi non ha seme
([README, «Hash delle chiavi non keyed»](../README.md#hash-delle-chiavi-non-keyed)).

### Complessità

Tempo O(n) sulle righe per il raggruppamento, più O(g log g) per ordinare
i g gruppi e, per `quantile` e `distinct`, O(m log m) sugli m valori di
ogni gruppo; memoria O(n) per l'assegnazione delle righe ai gruppi più
l'uscita. Da 32.768 righe, con gruppi di almeno 8 righe in media, il
calcolo per gruppo va in parallelo, con lo stesso risultato.

La variante spilled la sceglie il runner quando quella in memoria non sta
nel budget ([README, «Budget di memoria»](../README.md#budget-di-memoria)):
le righe si dividono per hash della chiave di gruppo in `spill_partitions`
file Arrow IPC temporanei, così ogni gruppo sta in una partizione sola con
le sue righe in ordine d'ingresso; ogni partizione si rilegge (entro
`max_governed_memory_bytes`) e si aggrega in memoria, poi le righe si
riordinano sulla chiave. L'uscita è identica a quella in memoria.

### Esempio

`importo` compare due volte: la somma si chiama `importo_sum`, il conteggio
ha un alias. `"sud"` precede `"nord"` perché il testo è più corto.

```json
{
  "config": {"group_by": ["regione"], "aggregations": [
    {"column": "importo", "function": "sum"},
    {"column": "importo", "function": "count", "alias": "n"}
  ]},
  "ingressi": [
    {"nome": "vendite", "colonne": [
      {"nome": "regione", "tipo": "utf8", "valori": ["nord", "sud", "nord", null]},
      {"nome": "importo", "tipo": "float64", "valori": [10.0, 5.0, null, 2.5]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "regione", "tipo": "utf8", "valori": [null, "sud", "nord"]},
    {"nome": "importo_sum", "tipo": "float64", "valori": [2.5, 5.0, 10.0]},
    {"nome": "n", "tipo": "int64", "valori": [1, 1, 1]}
  ]}
}
```
