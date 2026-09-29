### Che cosa fa

Aggiunge una colonna con l'hash SHA-256, in esadecimale minuscolo, dei
valori di alcune colonne di ogni riga. A differenza di
[`table.md5_hash`](#tablemd5_hash) ogni valore entra delimitato dalla
propria lunghezza, insieme al nome e al tipo della colonna: nessuna
concatenazione di valori diversi dà lo stesso messaggio. Con `normalize` (default) differenze di
maiuscole e di spazi ai lati non cambiano l'hash.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `columns` | lista di stringhe | obbligatorio | da 1 a `max_columns` colonne leggibili come testo, senza ripetizioni | colonne da cui si calcola l'hash |
| `output_column` | stringa | `sha256_hash` | nome valido | colonna d'uscita |
| `normalize` | booleano | `true` | `true`, `false` | toglie gli spazi ai lati e porta in minuscolo ogni valore |
| `null_policy` | stringa | `empty` | `empty`, `literal`, `error` | come entra una cella nulla |
| `null_literal` | stringa | `"<null>"` | al più `max_string_bytes` byte | testo di una cella nulla con `literal` |

Leggibile come testo: `utf8`, `int64`, `uint64`, `float64`, `bool`,
`date32`, `timestamp(ms)`, `decimal128` con scala da 0 a 38, `binary`,
`dictionary<utf8>` con chiavi `int32`.

Il messaggio di una riga, con `f(x)` = lunghezza di x in 8 byte big-endian
seguita da x:

```text
"plenora-sha256-v1\0"
per ogni colonna, in ordine di nome:
  f(nome) f(tipo Arrow, es. "Utf8") 0x01 f(testo della cella)
```

Con `normalize` il testo passa da `trim` e `to_lowercase` (Unicode), e così
`null_literal`. Una cella nulla con `empty` vale il testo vuoto, con
`literal` vale `null_literal`, con `error` rifiuta la riga.
`null_literal` scritto con una `null_policy` diversa da `literal` non ha
effetto, senza errore.

### Schema

La colonna d'uscita è `utf8` non nullable, 64 cifre esadecimali: si
aggiunge in coda o sostituisce al suo posto una colonna con lo stesso nome
(perdendone tipo e metadati di campo). Metadati di schema conservati;
`row_count` resta; `sorted_by` resta solo se nessuna colonna esistente è
sovrascritta.

### Righe

1:1. Con `null_policy` `error` una sola cella nulla nelle colonne scelte fa
fallire il passo.

### Ordine

Invariato.

### Errori

In validazione, `InvalidPlan`:

- `columns` vuoto, con ripetizioni o con più di `max_columns` colonne;
- una colonna assente o non leggibile come testo;
- `output_column` non valido; `null_literal` oltre `max_string_bytes`;
- config con campi sconosciuti o `null_policy` fuori elenco.

In esecuzione:

- `DataMapping` con diagnostica per riga, solo con `null_policy` `error`:
  ogni riga con una cella nulla (`validation.required_value_missing`, sulla
  prima colonna nulla in ordine di nome); il passo non produce uscita;
- `Schema`: una cella che non si converte in testo (`binary` non UTF-8,
  data o istante fuori intervallo).

### Limiti e deviazioni

Con `empty` una cella nulla e una vuota danno lo stesso hash, con `literal`
una cella nulla e una che contiene `null_literal`; per distinguerle
[`table.stable_fingerprint`](#tablestable_fingerprint). Nome e tipo Arrow
entrano nell'hash: rinominare una colonna o cambiarne il tipo (anche da
`utf8` a `large_utf8`) cambia l'hash. Non è un'impronta con chiave: chi
conosce i valori possibili può ricalcolarla
([`table.hmac_sha256`](#tablehmac_sha256)).

### Complessità

Tempo O(byte delle colonne scelte), in parallelo per blocchi di righe;
memoria O(n) per la colonna d'uscita.

### Esempio

Con `normalize` le prime due righe hanno lo stesso hash; la terza è
l'hash di `<null>`.

```json
{
  "config": {"columns": ["codice"], "null_policy": "literal"},
  "ingressi": [
    {"nome": "articoli", "colonne": [
      {"nome": "codice", "tipo": "utf8", "valori": ["A1", "a1", null]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "codice", "tipo": "utf8", "valori": ["A1", "a1", null]},
    {"nome": "sha256_hash", "tipo": "utf8", "valori": ["58425b65410b46c41250fc521d6f76745f2d4ebb307139af96090359e5f393cb", "58425b65410b46c41250fc521d6f76745f2d4ebb307139af96090359e5f393cb", "5636dc6e756406d6553fdb4b6d3750aa830edc19a1ef4e2c9ddd01bdf3679534"]}
  ]}
}
```
