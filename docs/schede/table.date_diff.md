### Che cosa fa

Legge due colonne di date e ore scritte come testo, con lo stesso formato,
e scrive la differenza `fine - inizio` in giorni, ore, minuti o secondi,
come numero con parte frazionaria (un giorno e mezzo è `1.5`) e con segno.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `start_column` | stringa | obbligatorio | colonna leggibile come testo | istante iniziale |
| `end_column` | stringa | obbligatorio | colonna leggibile come testo | istante finale |
| `input_format` | stringa | obbligatorio | formato `chrono` non vuoto, al più `max_string_bytes` byte | formato di lettura di entrambe le colonne |
| `unit` | stringa | obbligatorio | `days`, `hours`, `minutes`, `seconds` | unità della differenza |
| `output_column` | stringa | obbligatorio | nome valido | colonna d'uscita |
| `invalid` | stringa | `null` | `null`, `error` | accettato per compatibilità, senza effetto |

Leggibile come testo: `utf8`, `int64`, `uint64`, `float64`, `bool`,
`date32`, `timestamp(ms)`, `decimal128` con scala da 0 a 38, `binary`,
`dictionary<utf8>` con chiavi `int32`. Una colonna non testuale si legge
con la sua resa testuale: `date32` come `AAAA-MM-GG`, `timestamp(ms)` come
RFC 3339.

La lettura deve consumare tutto il testo della cella; un formato senza
campi orari legge una data e la pone a mezzanotte. I valori non hanno fuso:
un giorno è sempre 86 400 secondi. La differenza è il numero di nanosecondi
diviso per `10^9` e poi per 86 400, 3 600, 60 o 1.

### Schema

La colonna d'uscita è `float64` nullable: si aggiunge in coda o sostituisce
al suo posto una colonna con lo stesso nome (perdendone tipo e metadati di
campo). Metadati di schema conservati; `row_count` resta; `sorted_by` resta
solo se nessuna colonna esistente è sovrascritta.

### Righe

1:1. Se una delle due celle è nulla il risultato è null.

### Ordine

Invariato.

### Errori

In validazione, `InvalidPlan`:

- `start_column` o `end_column` assenti o non leggibili come testo;
- `input_format` vuoto, oltre `max_string_bytes` o con un campo non
  riconosciuto;
- `output_column` non valido;
- config con campi sconosciuti, `unit` o `invalid` fuori elenco.

In esecuzione:

- `DataMapping` con diagnostica per riga: una cella non nulla che non si
  legge (`conversion.invalid_datetime`, sulla colonna iniziale se non si
  legge quella, altrimenti sulla finale), o una differenza oltre `i64`
  nanosecondi, circa 292 anni (`conversion.datetime_range`); il passo non
  produce uscita;
- `Schema`: una cella che non si converte in testo.

### Limiti e deviazioni

Il risultato è `float64` per contratto: oltre `2^53` nanosecondi (circa 104
giorni) il numero esatto di nanosecondi si arrotonda al double più vicino
prima della divisione. `invalid` non ha effetto e non si rifiuta
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner)).
Mesi e anni non sono unità ammesse, perché non hanno durata fissa.

### Complessità

Tempo O(n) (due letture per riga: il controllo, poi il calcolo); memoria
O(n) per la colonna d'uscita.

### Esempio

```json
{
  "config": {"start_column": "inizio", "end_column": "fine", "input_format": "%Y-%m-%d %H:%M", "unit": "days", "output_column": "giorni"},
  "ingressi": [
    {"nome": "attivita", "colonne": [
      {"nome": "inizio", "tipo": "utf8", "valori": ["2024-01-01 00:00", "2024-03-10 12:00", "2024-01-01 00:00"]},
      {"nome": "fine", "tipo": "utf8", "valori": ["2024-01-02 12:00", "2024-03-01 12:00", null]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "inizio", "tipo": "utf8", "valori": ["2024-01-01 00:00", "2024-03-10 12:00", "2024-01-01 00:00"]},
    {"nome": "fine", "tipo": "utf8", "valori": ["2024-01-02 12:00", "2024-03-01 12:00", null]},
    {"nome": "giorni", "tipo": "float64", "valori": [1.5, -9.0, null]}
  ]}
}
```
