### Che cosa fa

Maschera il contenuto di una o più colonne sostituendo con un carattere di
maschera la parte centrale del testo: codici fiscali, indirizzi email,
numeri di telefono e IBAN hanno una forma fissa, `custom` lascia in chiaro
un numero scelto di caratteri all'inizio e alla fine. Il risultato va in
una colonna nuova `<colonna>_masked` o sovrascrive la colonna.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `maskings` | lista di oggetti | obbligatorio | da 1 a `max_columns` voci | mascherature, applicate in sequenza |
| `maskings[].column` | stringa | obbligatorio | colonna dell'ingresso leggibile come testo | colonna da mascherare |
| `maskings[].mask_type` | stringa | `custom` | `cf`, `email`, `phone`, `iban`, `custom` | forma della maschera |
| `maskings[].chars_start` | intero | `3` | intero non negativo; solo con `custom` | caratteri iniziali in chiaro |
| `maskings[].chars_end` | intero | `3` | intero non negativo; solo con `custom` | caratteri finali in chiaro |
| `maskings[].mask_char` | stringa | `"*"` | un solo carattere; solo con `custom` | carattere di maschera |
| `overwrite` | booleano | `false` | `true`, `false` | `true` sovrascrive la colonna, `false` scrive `<colonna>_masked` |

Leggibile come testo: `utf8`, `int64`, `uint64`, `float64`, `bool`,
`date32`, `timestamp(ms)`, `decimal128` con scala da 0 a 38, `binary`,
`dictionary<utf8>` con chiavi `int32`.

Le forme, contando i caratteri Unicode (non i byte):

- `custom`: restano `chars_start` caratteri iniziali e `chars_end` finali,
  ognuno degli altri diventa `mask_char`; un testo non più lungo di
  `chars_start + chars_end` resta com'è;
- `cf`: come `custom` con 3 e 3 e `*`;
- `iban`: come `custom` con 4 e 4 e `*` (gli spazi contano come caratteri);
- `email`: la parte prima dell'ultima `@` diventa il suo primo carattere
  seguito da un `*` per ciascuno degli altri (un solo `*` se ha al più un
  carattere); il dominio resta; senza `@` il testo resta com'è;
- `phone`: si tengono solo cifre e `+`; se ne restano meno di 6 il testo
  originale resta com'è, altrimenti il testo compattato con 3 caratteri
  iniziali e 4 finali in chiaro (`+39 333 1234567` → `+39******4567`).

Con `overwrite` una seconda voce sulla stessa colonna maschera il risultato
della prima; senza, la seconda riscrive `<colonna>_masked` partendo dalla
colonna originale. Una voce non può nominare una colonna `_masked` creata da
una voce precedente: l'analisi cerca le colonne nell'ingresso e la rifiuta.

### Schema

Ogni colonna d'uscita è `utf8` nullable. Con `overwrite` sostituisce la
colonna al suo posto (perdendone tipo e metadati di campo); senza,
`<colonna>_masked` si aggiunge in coda nell'ordine delle voci (o sostituisce
al suo posto una colonna esistente con quel nome). Metadati di schema
conservati; `row_count` resta; `sorted_by` resta solo se nessuna colonna
esistente è sovrascritta.

### Righe

1:1. Una cella nulla resta nulla.

### Ordine

Invariato.

### Errori

In validazione, `InvalidPlan`:

- `maskings` vuoto o con più di `max_columns` voci;
- una `column` assente dall'ingresso o non leggibile come testo;
- `chars_start`, `chars_end` o `mask_char` con un `mask_type` diverso da
  `custom`;
- `mask_char` che non è un solo carattere;
- nome d'uscita non valido (vuoto o oltre 1024 byte);
- config con campi sconosciuti o `mask_type` fuori elenco.

In esecuzione, `Schema`: una cella che non si converte in testo (`binary`
non UTF-8, data o istante fuori intervallo).

### Limiti e deviazioni

La maschera conta i caratteri Unicode, non i grafemi: un carattere composto
da più code point (emoji con modificatori, lettere con diacritici
combinanti) conta per più di uno. Non riconosce il formato del dato: un
`cf` o un `iban` non validi si mascherano con la stessa regola.

### Complessità

Tempo O(byte delle colonne mascherate); memoria O(n) per ogni colonna
d'uscita.

### Esempio

```json
{
  "config": {"maskings": [{"column": "email", "mask_type": "email"}, {"column": "codice", "chars_start": 2, "chars_end": 1}]},
  "ingressi": [
    {"nome": "clienti", "colonne": [
      {"nome": "email", "tipo": "utf8", "valori": ["mario.rossi@example.com", null]},
      {"nome": "codice", "tipo": "utf8", "valori": ["ABC12345", "XY"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "email", "tipo": "utf8", "valori": ["mario.rossi@example.com", null]},
    {"nome": "codice", "tipo": "utf8", "valori": ["ABC12345", "XY"]},
    {"nome": "email_masked", "tipo": "utf8", "valori": ["m**********@example.com", null]},
    {"nome": "codice_masked", "tipo": "utf8", "valori": ["AB*****5", "XY"]}
  ]}
}
```
