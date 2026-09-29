### Che cosa fa

Legge date e ore scritte come testo con un formato e le riscrive con un
altro formato in una colonna nuova (per esempio da `31/01/2024` a
`2024-01-31`). I formati sono quelli di `strftime` della libreria `chrono`.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa | obbligatorio | colonna leggibile come testo | colonna da leggere |
| `input_format` | stringa | obbligatorio | formato `chrono` non vuoto, al più `max_string_bytes` byte | formato di lettura |
| `output_format` | stringa | `"%Y-%m-%d %H:%M:%S"` | formato `chrono` non vuoto, al più `max_string_bytes` byte, senza fuso | formato di scrittura |
| `output_column` | stringa | obbligatorio | nome valido | colonna d'uscita |
| `invalid` | stringa | `null` | `null`, `error` | accettato per compatibilità, senza effetto |

Leggibile come testo: `utf8`, `int64`, `uint64`, `float64`, `bool`,
`date32`, `timestamp(ms)`, `decimal128` con scala da 0 a 38, `binary`,
`dictionary<utf8>` con chiavi `int32`. Una colonna non testuale si legge
con la sua resa testuale: `date32` come `AAAA-MM-GG`, `timestamp(ms)` come
RFC 3339 (`2024-01-31T10:00:00+00:00`).

La lettura deve consumare tutto il testo della cella. Un formato senza
campi orari legge una data e la pone a mezzanotte. `output_format` non può
contenere campi di fuso o di offset (`%z`, `%:z`, `%Z`, `%+`): il valore
letto non ha fuso. Un valore non leggibile rifiuta sempre la riga, qualunque
sia `invalid`.

### Schema

La colonna d'uscita è `utf8` nullable: si aggiunge in coda o sostituisce al
suo posto una colonna con lo stesso nome (perdendone tipo e metadati di
campo). Metadati di schema conservati; `row_count` resta; `sorted_by` resta
solo se nessuna colonna esistente è sovrascritta.

### Righe

1:1. Una cella nulla dà null.

### Ordine

Invariato.

### Errori

In validazione, `InvalidPlan`:

- `column` assente o non leggibile come testo;
- un formato vuoto, oltre `max_string_bytes`, con un campo non riconosciuto
  (`%Q`, `%` finale), o `output_format` con campi di fuso;
- `output_column` non valido;
- config con campi sconosciuti o `invalid` fuori elenco.

In esecuzione:

- `DataMapping` con diagnostica per riga: ogni cella non nulla che non si
  legge con `input_format` (`conversion.invalid_datetime`); il passo non
  produce uscita;
- `DataMapping`, senza diagnostica per riga: un valore che `output_format`
  non sa scrivere (anno fuori da 0..=9999 con `%C`);
- `Schema`: una cella che non si converte in testo.

### Limiti e deviazioni

`invalid` non ha effetto: non esiste un modo di trasformare un valore non
leggibile in null. È un parametro scritto senza effetto che non si rifiuta
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner)).

### Complessità

Tempo O(n) (due letture per riga: il controllo, poi la conversione con il
formato compilato una volta); memoria O(n) per la colonna d'uscita.

### Esempio

```json
{
  "config": {"column": "data", "input_format": "%d/%m/%Y", "output_format": "%Y-%m-%d", "output_column": "data_iso"},
  "ingressi": [
    {"nome": "fatture", "colonne": [
      {"nome": "data", "tipo": "utf8", "valori": ["31/01/2024", "29/02/2024", null]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "data", "tipo": "utf8", "valori": ["31/01/2024", "29/02/2024", null]},
    {"nome": "data_iso", "tipo": "utf8", "valori": ["2024-01-31", "2024-02-29", null]}
  ]}
}
```
