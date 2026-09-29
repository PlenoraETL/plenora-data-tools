### Che cosa fa

Aggiunge una colonna con l'hash MD5, in esadecimale minuscolo, dei valori di
alcune colonne di ogni riga. Serve a confrontare o raggruppare righe per
contenuto; con `normalize` (default) differenze di maiuscole e di spazi ai
lati non cambiano l'hash.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `columns` | lista di stringhe | obbligatorio | da 1 a `max_columns` colonne leggibili come testo, senza ripetizioni | colonne da cui si calcola l'hash |
| `output_column` | stringa | `md5_hash` | nome valido | colonna d'uscita |
| `normalize` | booleano | `true` | `true`, `false` | toglie gli spazi ai lati e porta in minuscolo ogni valore |
| `null_policy` | stringa | `empty` | `empty`, `literal`, `error` | come entra una cella nulla |
| `null_literal` | stringa | `"<null>"` | al più `max_string_bytes` byte | testo di una cella nulla con `literal` |

Leggibile come testo: `utf8`, `int64`, `uint64`, `float64`, `bool`,
`date32`, `timestamp(ms)`, `decimal128` con scala da 0 a 38, `binary`,
`dictionary<utf8>` con chiavi `int32`.

Il messaggio di una riga è il testo delle celle, con le colonne in ordine
di nome (l'ordine scritto in `columns` non conta), unite dal carattere
U+001F. Con `normalize` ogni testo passa da `trim` e `to_lowercase`
(Unicode), e così `null_literal`. Una cella nulla con `empty` vale il testo
vuoto, con `literal` vale `null_literal`, con `error` rifiuta la riga.

`null_literal` scritto con una `null_policy` diversa da `literal` non ha
effetto, senza errore.

### Schema

La colonna d'uscita è `utf8` non nullable, 32 cifre esadecimali: si
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

MD5 non è resistente alle collisioni: non usarlo come impronta di
sicurezza. Il messaggio non ha delimitazioni: un valore che contiene U+001F
può produrre lo stesso messaggio di valori diversi; con `empty` una cella
nulla e una vuota coincidono, con `literal` una cella nulla e una che
contiene `null_literal`. Nome e tipo delle colonne non entrano nell'hash.
Per un'impronta senza ambiguità: [`table.sha256_hash`](#tablesha256_hash) o
[`table.stable_fingerprint`](#tablestable_fingerprint).

### Complessità

Tempo O(byte delle colonne scelte), in parallelo per blocchi di righe;
memoria O(n) per la colonna d'uscita.

### Esempio

Con `normalize` le prime due righe hanno lo stesso hash; nella terza il
nome nullo vale il testo vuoto.

```json
{
  "config": {"columns": ["nome", "citta"]},
  "ingressi": [
    {"nome": "clienti", "colonne": [
      {"nome": "nome", "tipo": "utf8", "valori": ["Anna", " anna ", null]},
      {"nome": "citta", "tipo": "utf8", "valori": ["Roma", "ROMA", "Roma"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "nome", "tipo": "utf8", "valori": ["Anna", " anna ", null]},
    {"nome": "citta", "tipo": "utf8", "valori": ["Roma", "ROMA", "Roma"]},
    {"nome": "md5_hash", "tipo": "utf8", "valori": ["e31b753eebf823f115975d5448d1756f", "e31b753eebf823f115975d5448d1756f", "1a3394144c6d6416f9e84e1e86eaf230"]}
  ]}
}
```
