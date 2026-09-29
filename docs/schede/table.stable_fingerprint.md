### Che cosa fa

Aggiunge a ogni riga un'impronta stabile, in esadecimale minuscolo, dei
valori di alcune colonne (di default tutte): SHA-256 o MD5 di una codifica
canonica in cui ogni valore è delimitato dalla propria lunghezza e
accompagnato da nome e tipo della colonna. I valori entrano come sono, senza
normalizzazione, e una cella nulla è distinta da una vuota: due righe hanno
la stessa impronta solo se hanno gli stessi valori.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `columns` | lista di stringhe | `[]`, cioè tutte | colonne leggibili come testo, senza ripetizioni, al più `max_columns` | colonne dell'impronta, nell'ordine scritto |
| `output_column` | stringa | `fingerprint` | nome valido | colonna d'uscita |
| `algorithm` | stringa | `sha256` | `sha256`, `md5` | funzione di hash |

Leggibile come testo: `utf8`, `int64`, `uint64`, `float64`, `bool`,
`date32`, `timestamp(ms)`, `decimal128` con scala da 0 a 38, `binary`,
`dictionary<utf8>` con chiavi `int32`. Con `columns` vuoto entrano tutte le
colonne nell'ordine dello schema, e tutte devono esserlo.

La codifica di una riga, con `f(x)` = lunghezza di x in 8 byte big-endian
seguita da x:

```text
"plenora-fingerprint-v1\0"
per ogni colonna, nell'ordine di columns (o dello schema):
  f(nome) f(tipo Arrow, es. "Int64")
  poi 0x00 se la cella è nulla, altrimenti 0x01 f(testo della cella)
```

Il testo della cella è la sua resa testuale: decimali per gli interi, la
resa più corta di `f64` (`2.0` è `2`), `true`/`false`, `AAAA-MM-GG` per le
date, RFC 3339 per gli istanti.

### Schema

La colonna d'uscita è `utf8` non nullable, 64 cifre esadecimali (32 con
`md5`): si aggiunge in coda o sostituisce al suo posto una colonna con lo
stesso nome (perdendone tipo e metadati di campo). Metadati di schema
conservati; `row_count` resta; `sorted_by` resta solo se nessuna colonna
esistente è sovrascritta.

### Righe

1:1.

### Ordine

Invariato.

### Errori

In validazione, `InvalidPlan`:

- `columns` con ripetizioni o con più di `max_columns` colonne;
- una colonna assente o non leggibile come testo (con `columns` vuoto,
  una qualunque colonna dello schema);
- `columns` vuoto su un ingresso senza colonne;
- `output_column` non valido;
- config con campi sconosciuti o `algorithm` fuori elenco.

In esecuzione, `Schema`: una cella che non si converte in testo (`binary`
non UTF-8, data o istante fuori intervallo).

### Limiti e deviazioni

L'impronta dipende da nome e tipo Arrow delle colonne: rinominare o cambiare
tipo cambia l'impronta. Con `columns` vuoto entra anche una colonna
esistente con il nome di `output_column`, prima di essere sostituita. `md5`
non è resistente alle collisioni costruite apposta. Non è un'impronta con
chiave ([`table.hmac_sha256`](#tablehmac_sha256)).

### Complessità

Tempo O(byte delle colonne scelte), in parallelo per blocchi di righe;
memoria O(n) per la colonna d'uscita.

### Esempio

Una cella vuota e una nulla danno impronte diverse.

```json
{
  "config": {"columns": ["id", "nome"], "algorithm": "md5"},
  "ingressi": [
    {"nome": "clienti", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 1]},
      {"nome": "nome", "tipo": "utf8", "valori": ["", null]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 1]},
    {"nome": "nome", "tipo": "utf8", "valori": ["", null]},
    {"nome": "fingerprint", "tipo": "utf8", "valori": ["52e58079ed6208a36c5131ac15569f8c", "9faaee9b07aec554bd05233d0ee0fa81"]}
  ]}
}
```
