### Che cosa fa

Verifica che ogni valore della colonna `column` stia fra `min` e `max`.
Se l'asserzione regge, l'uscita è l'ingresso invariato; se un valore è fuori
intervallo, non è finito o è nullo senza `allow_null`, il passo fallisce con
una diagnostica per riga e non produce uscita.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa | obbligatorio | colonna `int64`, `uint64`, `float64`, `decimal128`, `date32`, `timestamp(ms)` o `utf8` | colonna da controllare |
| `min` | numero | assente | numero finito, non maggiore di `max` | estremo inferiore |
| `max` | numero | assente | numero finito | estremo superiore |
| `inclusive_min` | booleano | assente (incluso) | `true`, `false`; solo con `min`; `null` non ammesso | se `min` fa parte dell'intervallo |
| `inclusive_max` | booleano | assente (incluso) | `true`, `false`; solo con `max`; `null` non ammesso | se `max` fa parte dell'intervallo |
| `allow_null` | booleano | `false` | `true`, `false` | con `true` le celle nulle passano |

Almeno uno fra `min` e `max`. `inclusive_min` senza `min` (o
`inclusive_max` senza `max`) non avrebbe effetto e si rifiuta.

Come si confronta: nel dominio nativo della colonna, mai attraverso `f64`
lato cella: esatto sugli interi oltre `2^53` e sui decimali. Una `date32` vale
i giorni dall'epoca, un `timestamp(ms)` i millisecondi dall'epoca (l'istante,
qualunque sia il fuso). Un `utf8` si legge come numero, con gli spazi ai lati
ignorati e la virgola decimale ammessa. Un valore non finito (`inf`, `-inf`,
`NaN`, in `float64` o come testo) è sempre fuori intervallo.

### Schema

Identico all'ingresso: stesse colonne, tipi, nullabilità, metadati di campo
e di schema; il contratto conserva ordinamento dichiarato (`sorted_by`) e
numero di righe.

### Righe

1:1 se l'asserzione regge: tutte le righe, invariate. Altrimenti nessuna
uscita; nessuna riga viene scartata.

### Ordine

L'ordine d'ingresso.

### Errori

In validazione, `InvalidPlan`:

- né `min` né `max`; `min` maggiore di `max`;
- `inclusive_min` senza `min` o `inclusive_max` senza `max`;
- `inclusive_min` o `inclusive_max` `null` espliciti (un parametro
  facoltativo si omette);
- `column` assente o di un tipo fuori dall'elenco;
- config con campi sconosciuti.

In esecuzione:

- `DataMapping` con diagnostica per riga: righe fuori intervallo o non
  finite (causa `validation.value_out_of_range`) e, senza `allow_null`,
  righe nulle (causa `validation.required_value_missing`). La diagnostica dà
  il conteggio per causa e fino a 10 esempi in ordine di riga, con l'indice
  (da zero) della riga nella base del runner
  ([README, «Diagnostica per riga»](../README.md#diagnostica-per-riga))
  e il nome della colonna; mai i valori;
- `Schema`: una cella `utf8` non nulla che non è un numero. Il passo
  fallisce subito, senza diagnostica per riga.

### Limiti e deviazioni

`min` e `max` si leggono dal JSON come `f64`: un estremo intero oltre `2^53`
(`9007199254740993`) diventa il double più vicino prima di ogni confronto.
Il confronto con la cella resta esatto, ma contro l'estremo arrotondato.
Il testo non numerico in una colonna `utf8` fallisce solo in esecuzione
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner)).

### Complessità

Tempo O(n) sulle righe; memoria O(r) per le righe rifiutate, l'uscita
condivide le colonne dell'ingresso.

### Esempio

```json
{
  "config": {"column": "sconto", "min": 0, "max": 50, "inclusive_max": false, "allow_null": true},
  "ingressi": [
    {"nome": "ordini", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
      {"nome": "sconto", "tipo": "decimal128(5, 2)", "valori": ["0.00", null, "49.99"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
    {"nome": "sconto", "tipo": "decimal128(5, 2)", "valori": ["0.00", null, "49.99"]}
  ]}
}
```
