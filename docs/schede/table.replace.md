### Che cosa fa

Sostituisce testo nella colonna `utf8` `column`. Senza `regex` il
confronto è sulla cella intera: una cella uguale a `old_value` diventa
`new_value`, le altre restano (non si sostituiscono sottostringhe). Con
`regex` ogni match di `old_value` nella cella, da sinistra e senza
sovrapposizioni, si sostituisce con `new_value`.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa | obbligatorio | colonna `utf8` dell'ingresso | colonna da modificare |
| `old_value` | stringa | obbligatorio | con `regex`, regex valida del crate `regex` di al più `max_regex_bytes` byte; senza, al più `max_string_bytes` byte | cella da sostituire, o pattern |
| `new_value` | stringa | obbligatorio | al più `max_string_bytes` byte | testo sostitutivo |
| `regex` | booleano | `false` | `true`, `false` | interpreta `old_value` come espressione regolare |

Con `regex` il `new_value` riconosce i riferimenti ai gruppi: `$1`,
`${1}`, `$nome`, `${nome}`; `$$` scrive un `$`. Un riferimento seguito da
lettere o cifre va scritto tra graffe: `$1a` è il gruppo di nome `1a` (che
non esiste, e vale testo vuoto), `${1}a` è il gruppo 1 seguito da `a`.
Senza `regex` il `$` non ha significato speciale.

Con `regex` ogni cella sostituita non supera `max_string_bytes` byte; una
regex che combacia con il testo vuoto inserisce `new_value` in ogni
posizione, e può superarlo.

### Schema

Stesse colonne; `column` resta nella sua posizione, `utf8`, con i suoi
metadati di campo, e diventa nullable. Contratto: il conteggio delle righe
resta; l'ordinamento dichiarato cade.

### Righe

1:1; il null resta null.

### Ordine

Righe nell'ordine d'ingresso.

### Errori

In validazione, `InvalidPlan`:

- `column` assente o non `utf8`;
- con `regex`, `old_value` non valido o oltre `max_regex_bytes`; senza,
  `old_value` oltre `max_string_bytes`;
- `new_value` oltre `max_string_bytes`;
- config con campi sconosciuti.

Il limite di `old_value` lo applica anche il kernel.

In esecuzione, `ResourceLimit`: con `regex`, una cella sostituita oltre
`max_string_bytes` byte.

### Limiti e deviazioni

- Per sostituire una sottostringa letterale serve `regex: true` con i
  metacaratteri protetti da `\`.

### Complessità

Tempo lineare nei byte della colonna (confronto, o ricerca regex in tempo
lineare); memoria pari ai byte della colonna d'uscita.

### Esempio

```json
{
  "config": {"column": "telefono", "old_value": "^\\+39\\s*(\\d+)$", "new_value": "0039-${1}", "regex": true},
  "ingressi": [
    {"nome": "contatti", "colonne": [
      {"nome": "telefono", "tipo": "utf8", "valori": ["+39 0612345", "0612345", null]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "telefono", "tipo": "utf8", "valori": ["0039-0612345", "0612345", null]}
  ]}
}
```
