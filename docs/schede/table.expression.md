### Che cosa fa

Calcola una colonna nuova da un'espressione scritta come albero JSON:
colonne, letterali, operatori aritmetici, di confronto e logici, funzioni
e `case`. Il tipo del risultato (numero, booleano, testo, data o istante) si
decide dallo schema prima di leggere i dati. I confronti fra numeri sono
esatti sul valore d'origine; un numero non finito, in ingresso o nel
risultato, rifiuta la riga.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `output_column` | stringa | obbligatorio | nome valido (non vuoto, al più 1024 byte) | colonna d'uscita |
| `expression` | oggetto | obbligatorio | nodo della grammatica sotto; profondità al più 64, al più 4096 nodi | espressione da calcolare |
| `output_type` | stringa | `auto` | `auto`, `number`, `boolean`, `text`, `date32`, `timestamp_ms` | tipo della colonna d'uscita; `auto` lo deduce |
| `on_division_by_zero` | stringa | `"null"` | `"null"`, `"error"`; solo in un'espressione con almeno una divisione | che cosa dà una divisione con divisore zero (sotto) |

Nodi (campo `kind`):

```text
{"kind": "column",   "name": "<colonna>"}
{"kind": "literal",  "value": null | booleano | numero finito | stringa}
{"kind": "unary",    "op": "not" | "negate" | "is_null" | "is_not_null", "value": nodo}
{"kind": "binary",   "op": <operatore>, "left": nodo, "right": nodo}
{"kind": "function", "name": <funzione>, "args": [nodo, …]}          (al più 64 argomenti)
{"kind": "case",     "branches": [{"when": nodo, "then": nodo}, …],  (da 1 a 64 rami)
                     "else_value": nodo}
```

Un campo non previsto dentro un nodo si rifiuta. Un letterale di testo non
supera `max_string_bytes` byte.

Tipi delle colonne: `bool` è booleano; `int64`, `uint64`, `float64`,
`decimal128`, `date32` (giorni dall'epoca) e `timestamp(ms)` (millisecondi
dall'epoca) sono numeri; `utf8`, `binary` e `dictionary<utf8>` sono testo.
Ogni altro tipo si rifiuta.

Operatori (`binary`), con null che propaga salvo dove detto:

- `add`, `subtract`, `multiply`, `divide`: numero, numero → numero, in
  `f64`; per la divisione per zero vedi `on_division_by_zero`;
- `equal`, `not_equal`, `greater`, `greater_equal`, `less`, `less_equal`:
  due operandi dello stesso tipo → booleano; i numeri si confrontano sul
  valore esatto (`int64` oltre `2^53`, `decimal128`, il letterale `0.1` come
  decimale), i testi per byte, `false < true`;
- `and`, `or`: booleani, logica a tre valori (`false and null` è `false`,
  `true or null` è `true`).

Unari: `not` (booleano), `negate` (numero, esatto), `is_null` e
`is_not_null` (qualsiasi, mai null).

Funzioni (argomenti → risultato):

| funzione | argomenti | risultato | note |
| --- | --- | --- | --- |
| `coalesce` | 1..N qualsiasi | tipo degli argomenti | primo non nullo |
| `null_if` | 2 dello stesso tipo | tipo del primo | null se uguali |
| `lower`, `upper`, `trim` | 1 testo | testo | Unicode |
| `length` | 1 testo | numero | caratteri Unicode |
| `year` | 1 testo | numero | anno dei primi 10 byte letti come `AAAA-MM-GG` |
| `concat` | 1..N testo | testo | null se un argomento è null |
| `contains`, `starts_with`, `ends_with` | 2 testo | booleano | con distinzione di maiuscole |
| `abs`, `round`, `floor`, `ceil` | 1 numero | numero | `round`: metà lontano da zero; `abs` esatto |
| `power` | 2 numero | numero | base elevata all'esponente |
| `substring` | testo, numero, numero? | testo | inizio da 0 e lunghezza in caratteri, troncati verso zero; senza lunghezza fino alla fine |
| `regex_replace` | 3 testo | testo | sintassi della crate `regex`, tutte le occorrenze, `$1`/`$nome` nella sostituzione |
| `between` | 3 dello stesso tipo | booleano | estremi compresi; null se un argomento è null |
| `in` | valore, lista letterale | booleano | la lista è `{"kind": "literal", "value": [ … ]}` di scalari; lista vuota → `false` |
| `greatest`, `least` | 1..N dello stesso tipo | tipo degli argomenti | null se un argomento è null |
| `date_trunc` | unità letterale, colonna | `date32` o `timestamp(ms)` | unità `year`, `month`, `day` (e `hour`, `minute`, `second` solo per `timestamp(ms)`); il secondo argomento è una colonna `date32` o `timestamp(ms)` senza fuso, un altro `date_trunc` o `null` |

`case` valuta i rami in ordine e rende il `then` del primo `when` vero; un
`when` null vale falso; nessun ramo vero rende `else_value`. I rami non
scelti non si valutano.

Divisione con operandi non nulli e divisore zero:

- `on_division_by_zero = "null"` (default): il nodo della divisione vale
  null, e il null segue poi le regole di sopra (`coalesce(a / b, 0)` dà 0;
  in un ramo di `case` non scelto la divisione non si valuta). Ogni riga in
  cui è successo si conta una volta nel campo `righe_divisione_per_zero`
  del resoconto del passo nel runner: un conteggio, mai valori;
- `on_division_by_zero = "error"`: la riga si rifiuta con diagnostica
  `evaluation.division_by_zero` e il passo fallisce.

Un divisore letterale zero (`x / 0`) si rifiuta in validazione con
qualunque politica.

Un pattern letterale di `regex_replace` non supera `max_regex_bytes` byte.
Ogni testo prodotto da una funzione (`concat`, `lower`, `upper`,
`regex_replace`, `substring`, …) non supera `max_string_bytes` byte.

Con `auto` il tipo d'uscita è l'unico tipo che l'espressione può produrre
(`case` e `coalesce` uniscono i tipi dei rami); solo null dà `text`. Con un
tipo dichiarato, l'espressione deve poterlo produrre: non si converte, e una
riga che produce un altro tipo fallisce.

### Schema

La colonna d'uscita è `float64` (numero), `bool`, `utf8`, `date32` o
`timestamp(ms)` senza fuso, nullable: si aggiunge in coda o sostituisce al
suo posto una colonna con lo stesso nome (perdendone tipo e metadati di
campo). Metadati di schema conservati; `row_count` resta; `sorted_by` resta
solo se nessuna colonna esistente è sovrascritta.

### Righe

1:1.

### Ordine

Invariato.

### Errori

In validazione, `InvalidPlan`:

- nodo non riconosciuto o con un campo sconosciuto, profondità oltre 64,
  più di 4096 nodi, più di 64 argomenti o rami, `case` senza rami, nome di
  colonna vuoto;
- una colonna assente o di tipo non ammesso (anche `timestamp` in unità
  diverse dai millisecondi);
- letterale non scalare o non finito; letterale di testo oltre
  `max_string_bytes` byte; `in` senza lista letterale di scalari;
- numero di argomenti o tipo di un operando non ammessi; confronto fra tipi
  diversi (anche solo possibili, come `coalesce` di testo e numero);
- unità di `date_trunc` non letterale o fuori elenco, unità oraria su
  `date32`, `date_trunc` su testo o su `timestamp` con fuso;
- divisione per il numero zero scritto nell'espressione, con qualunque
  `on_division_by_zero`;
- `on_division_by_zero` scritto in un'espressione senza divisioni, o con
  un valore fuori elenco;
- pattern letterale di `regex_replace` non valido o oltre
  `max_regex_bytes` byte, indice letterale negativo di `substring` (solo
  dove la valutazione lo guarderebbe);
- con `auto`, più tipi possibili; con un tipo dichiarato, un tipo che
  l'espressione non produce mai;
- `output_column` non valido; config con campi sconosciuti.

In esecuzione:

- `DataMapping` con diagnostica per riga: divisione per zero, solo con
  `on_division_by_zero = "error"` (`evaluation.division_by_zero`), `NaN` o
  infinito letto da una colonna
  (`evaluation.non_finite_input`), risultato non finito di un'operazione o
  di `power` (`evaluation.non_finite_result`); il passo non produce uscita;
- `InvalidPlan`: pattern di `regex_replace` o indice di `substring`
  calcolati dalle colonne e non validi;
- `Schema`: `year` su un testo che non inizia con una data; una riga che
  produce un tipo diverso da `output_type`; `negate` o `abs` di un
  `decimal128` fuori dominio; una cella che non si converte in testo;
- `ResourceLimit`: `length` di un testo oltre `u32::MAX` caratteri; un
  pattern di `regex_replace` calcolato dalle colonne oltre
  `max_regex_bytes` byte; un testo prodotto da una funzione oltre
  `max_string_bytes` byte.

### Limiti e deviazioni

L'aritmetica e l'uscita numerica sono in `f64`: gli interi oltre `2^53` e i
decimali si arrotondano nel calcolo, non nei confronti. `date_trunc` tronca
in UTC e non accetta istanti con fuso. Gli errori che dipendono dai valori
(regex e indici calcolati) arrivano in esecuzione
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner)).
Un divisore calcolato uguale a zero dà null per default, contato in
`righe_divisione_per_zero`; con `on_division_by_zero = "error"` rifiuta la
riga in esecuzione. Il divisore letterale zero si vede già in validazione.

### Complessità

Tempo O(n · t) per t nodi dell'espressione (più la compilazione di una
regex calcolata per riga); memoria O(n) per la colonna d'uscita.

### Esempio

Un `when` nullo vale falso: la terza riga prende `else_value`.

```json
{
  "config": {"output_column": "fascia", "expression": {"kind": "case", "branches": [{"when": {"kind": "binary", "op": "greater_equal", "left": {"kind": "column", "name": "importo"}, "right": {"kind": "literal", "value": 100}}, "then": {"kind": "literal", "value": "alta"}}], "else_value": {"kind": "literal", "value": "bassa"}}},
  "ingressi": [
    {"nome": "ordini", "colonne": [
      {"nome": "importo", "tipo": "float64", "valori": [150.0, 20.0, null]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "importo", "tipo": "float64", "valori": [150.0, 20.0, null]},
    {"nome": "fascia", "tipo": "utf8", "valori": ["alta", "bassa", "bassa"]}
  ]}
}
```
