### Che cosa fa

Calcola una colonna nuova da una formula aritmetica scritta come testo, per
esempio `prezzo * quantita + 2`: quattro operazioni, parentesi, numeri,
testi fra apici e nomi di colonna. Con un operando di testo `+` concatena.
Il tipo del risultato (numero o testo) si decide dallo schema, prima di
leggere i dati. Per condizioni, confronti e funzioni c'è
[`table.expression`](#tableexpression).

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `new_column` | stringa | obbligatorio | nome valido (non vuoto, al più 1024 byte) | colonna d'uscita |
| `formula` | stringa | obbligatorio | formula della grammatica sotto, non vuota, al più `max_string_bytes` byte | espressione da calcolare |
| `on_division_by_zero` | stringa | `"null"` | `"null"`, `"error"`; solo in una formula con almeno un `/`; `null` non ammesso | che cosa dà una divisione con divisore zero (sotto) |

Grammatica (spazi ASCII ignorati fra i simboli):

```text
formula  := termine (("+" | "-") termine)*
termine  := fattore (("*" | "/") fattore)*
fattore  := "-" fattore | "(" formula ")" | numero | testo | colonna
numero   := cifre e punti, con esponente facoltativo (1, 2.5, .5, 3e-2)
testo    := '…' oppure "…", senza sequenze di escape
colonna  := [A-Za-z_][A-Za-z0-9_]*   (nome esatto della colonna)
```

Non ci sono funzioni, confronti né `+` unario. Una colonna con un nome fuori
da questa forma (spazi, accenti, trattini) non si può nominare.

Tipi:

- `int64` e `float64` sono numeri; ogni altro tipo, anche `int32`,
  `uint64` e `decimal128`, è testo, letto con la sua resa testuale e deve
  essere leggibile come testo (`utf8`, `int64`, `uint64`, `float64`,
  `bool`, `date32`, `timestamp(ms)`, `decimal128` con scala da 0 a 38,
  `binary`, `dictionary<utf8>` con chiavi `int32`);
- numero op numero dà un numero (`f64`); `+` con almeno un operando di testo
  concatena i due testi (un numero con la resa più corta di `f64`: `2.0`
  diventa `2`); `-`, `*`, `/` e il `-` unario su un testo si rifiutano;
- un operando nullo rende nullo il risultato, anche nella concatenazione;
- un testo concatenato non supera `max_string_bytes` byte.

Divisione con operandi non nulli e divisore calcolato zero: con
`on_division_by_zero = "null"` (default) la divisione vale null e, poiché
ogni operatore propaga il null, tutta la riga dà null; ogni riga in cui è
successo si conta una volta nel campo `righe_divisione_per_zero` del
resoconto del passo nel runner (un conteggio, mai valori). Con `"error"` la
riga si rifiuta. Un divisore letterale zero si rifiuta in validazione con
qualunque politica.

Un'operazione con entrambi gli operandi finiti il cui risultato non è
finito (overflow di `f64`, anche intermedio: `a / (a * a)` con `a = 1e308`)
rifiuta la riga, con qualunque `on_division_by_zero`: la politica riguarda
solo il divisore zero.

### Schema

La colonna d'uscita è `float64` se la formula è numerica, `utf8` se
contiene una concatenazione, nullable: si aggiunge in coda o sostituisce al
suo posto una colonna con lo stesso nome (perdendone tipo e metadati di
campo). Metadati di schema conservati; `row_count` resta; `sorted_by` resta
solo se nessuna colonna esistente è sovrascritta.

### Righe

1:1.

### Ordine

Invariato.

### Errori

In validazione, `InvalidPlan`:

- `formula` vuota, oltre `max_string_bytes` o non conforme alla grammatica
  (parentesi non bilanciate, testo non chiuso o con `\`, numero o esponente
  non validi, carattere non ammesso, simboli in coda);
- un letterale numerico non finito (`1e999`, `-1e400`);
- una divisione per il numero zero scritto nella formula (`x / 0`,
  `x / -0.0`), con qualunque `on_division_by_zero`;
- `on_division_by_zero` scritto in una formula senza `/`, con un valore
  fuori elenco o `null` esplicito (il parametro si omette);
- una colonna assente o non leggibile come testo;
- `-`, `*`, `/` o il `-` unario applicati a un testo;
- `new_column` non valido; config con campi sconosciuti.

In esecuzione:

- `DataMapping` con diagnostica per riga, solo con
  `on_division_by_zero = "error"`: una divisione per un divisore calcolato
  che vale zero (`evaluation.division_by_zero`); il passo non produce
  uscita;
- `DataMapping` con diagnostica per riga, con qualunque
  `on_division_by_zero`: un'operazione su operandi finiti con risultato non
  finito (`evaluation.non_finite_result`); il passo non produce uscita;
- `ResourceLimit`: un testo concatenato oltre `max_string_bytes` byte;
- `Schema`: una cella che non si converte in testo.

### Limiti e deviazioni

Il calcolo è in `f64`: un `int64` oltre `2^53` si arrotonda. Un `NaN` o
un infinito già presenti in una colonna si propagano senza errore (a
differenza di [`table.expression`](#tableexpression), che rifiuta la
riga); solo un risultato non finito da operandi finiti si rifiuta.

### Complessità

Tempo O(n · t) per t simboli della formula; memoria O(n) per la colonna
d'uscita e O(t) per la pila di valutazione.

### Esempio

```json
{
  "config": {"new_column": "totale", "formula": "prezzo * quantita + 2"},
  "ingressi": [
    {"nome": "righe", "colonne": [
      {"nome": "prezzo", "tipo": "float64", "valori": [10.5, 3.0, null]},
      {"nome": "quantita", "tipo": "int64", "valori": [2, 4, 1]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "prezzo", "tipo": "float64", "valori": [10.5, 3.0, null]},
    {"nome": "quantita", "tipo": "int64", "valori": [2, 4, 1]},
    {"nome": "totale", "tipo": "float64", "valori": [23.0, 14.0, null]}
  ]}
}
```
