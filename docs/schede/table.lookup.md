### Che cosa fa

Traduce i valori di una colonna con una tabella di corrispondenza scritta
nella config: ogni cella il cui testo è una chiave di `mapping` diventa il
valore associato; le altre diventano `default`, oppure, se `default` è
`null`, restano come sono. Il risultato è testo, nella stessa colonna o in
una nuova.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa | obbligatorio | colonna leggibile come testo | colonna da tradurre |
| `mapping` | oggetto | obbligatorio | chiavi stringa, valori JSON qualsiasi il cui testo è al più `max_string_bytes` byte; al più `max_rows` voci | corrispondenze testo della cella → valore |
| `default` | JSON | `null` | qualsiasi, con testo al più `max_string_bytes` byte | valore delle celle non nulle senza voce; `null` le lascia invariate |
| `output_column` | stringa | `column` | nome valido (non vuoto, al più 1024 byte) | colonna d'uscita; assente, sovrascrive `column` |

Leggibile come testo: `utf8`, `int64`, `uint64`, `float64`, `bool`,
`date32`, `timestamp(ms)`, `decimal128` con scala da 0 a 38, `binary`,
`dictionary<utf8>` con chiavi `int32`. La chiave si confronta con il testo
della cella byte per byte: un `int64` `5` è `"5"`, un `float64` `2.0` è
`"2"`, una data è `AAAA-MM-GG`.

Il valore scritto è il testo del valore JSON: una stringa vale sé stessa, un
numero o un booleano il suo testo JSON (`1.50` diventa `1.5`), un `null` la
stringa vuota (non una cella nulla). Una chiave ripetuta in `mapping` vale
l'ultima occorrenza.

### Schema

La colonna d'uscita è `utf8` nullable: sostituisce `column` al suo posto
(tipo e metadati di campo originali si perdono) o si aggiunge in coda con il
nome `output_column`; se `output_column` è un'altra colonna esistente, la
sostituisce al suo posto. Le altre colonne e i metadati di schema restano.
`row_count` resta; `sorted_by` resta solo se nessuna colonna esistente è
sovrascritta.

### Righe

1:1. Una cella nulla resta nulla, anche con `default`.

### Ordine

Invariato.

### Errori

In validazione, `InvalidPlan`:

- `column` assente o non leggibile come testo;
- `mapping` con più di `max_rows` voci;
- il testo di un valore di `mapping` o di `default` oltre
  `max_string_bytes` byte;
- `output_column` vuoto o oltre 1024 byte;
- config con campi sconosciuti.

In esecuzione, `Schema`: una cella che non si converte in testo (`binary`
non UTF-8, data o istante fuori intervallo).

### Limiti e deviazioni

Nessuno oltre ai limiti comuni.

### Complessità

Tempo O(n + m) con m voci di `mapping` (una mappa hash costruita una volta);
memoria O(m) per la mappa più la colonna d'uscita. Su `utf8` la ricerca
procede in parallelo per blocchi di righe, con uscita nell'ordine delle
righe.

### Esempio

```json
{
  "config": {"column": "stato", "mapping": {"A": "attivo", "S": "sospeso"}, "default": "altro", "output_column": "stato_esteso"},
  "ingressi": [
    {"nome": "clienti", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2, 3, 4]},
      {"nome": "stato", "tipo": "utf8", "valori": ["A", "S", "X", null]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2, 3, 4]},
    {"nome": "stato", "tipo": "utf8", "valori": ["A", "S", "X", null]},
    {"nome": "stato_esteso", "tipo": "utf8", "valori": ["attivo", "sospeso", "altro", null]}
  ]}
}
```
