### Che cosa fa

Verifica che ogni valore della colonna di testo `column` corrisponda
all'espressione regolare `pattern`. Se l'asserzione regge, l'uscita è
l'ingresso invariato; se un valore non corrisponde, o è nullo senza
`allow_null`, il passo fallisce con una diagnostica per riga e non produce
uscita. La corrispondenza è una ricerca nel testo: senza `^` e `$` basta che
una parte del valore corrisponda.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa | obbligatorio | colonna `utf8` dell'ingresso | colonna da controllare |
| `pattern` | stringa | obbligatorio | regex non vuota, sintassi del crate `regex`, al più `max_regex_bytes` byte | espressione che ogni valore deve contenere |
| `allow_null` | booleano | `false` | `true`, `false` | con `true` le celle nulle passano |

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

- `pattern` vuoto, oltre `max_regex_bytes` o non compilabile;
- `column` assente o non `utf8` (un dizionario di testi non basta);
- config con campi sconosciuti.

In esecuzione, `DataMapping` con diagnostica per riga: righe il cui valore
non corrisponde (causa `validation.regex_mismatch`) e, senza `allow_null`,
righe nulle (causa `validation.required_value_missing`). La diagnostica dà il
conteggio per causa e fino a 10 esempi in ordine di riga, con l'indice (da
zero) della riga nella base del runner
([README, «Diagnostica per riga»](../README.md#diagnostica-per-riga))
e il nome della colonna; mai i valori.

### Limiti e deviazioni

Sintassi e limiti del crate `regex`: niente riferimenti all'indietro né
lookaround, tempo lineare nella lunghezza del testo. `max_regex_bytes` è un
limite del piano (default 65536 byte, 64 KiB), lo stesso per il piano e per
i kernel.

### Complessità

Tempo O(n·m) su righe e lunghezza dei testi; memoria O(r) per le righe
rifiutate, più l'automa della regex.

### Esempio

```json
{
  "config": {"column": "provincia", "pattern": "^[A-Z]{2}$", "allow_null": true},
  "ingressi": [
    {"nome": "sedi", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
      {"nome": "provincia", "tipo": "utf8", "valori": ["MI", null, "TO"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
    {"nome": "provincia", "tipo": "utf8", "valori": ["MI", null, "TO"]}
  ]}
}
```
