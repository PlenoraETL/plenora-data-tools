### Che cosa fa

Legge date e ore scritte come testo, le interpreta come ora locale di un
fuso orario di partenza e le riscrive come ora locale di un fuso di arrivo,
con un formato che può mostrare l'offset o il nome del fuso. I fusi sono
nomi IANA (`Europe/Rome`, `UTC`, `America/New_York`).

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa | obbligatorio | colonna leggibile come testo | colonna da leggere |
| `input_format` | stringa | obbligatorio | formato `chrono` non vuoto, al più `max_string_bytes` byte | formato di lettura |
| `output_format` | stringa | `"%Y-%m-%d %H:%M:%S"` | formato `chrono` non vuoto, al più `max_string_bytes` byte | formato di scrittura; ammette `%z`, `%:z`, `%Z`, `%+` |
| `source_timezone` | stringa | obbligatorio | nome IANA noto a `chrono-tz` | fuso dei valori letti |
| `target_timezone` | stringa | obbligatorio | nome IANA noto a `chrono-tz` | fuso dei valori scritti |
| `output_column` | stringa | obbligatorio | nome valido | colonna d'uscita |
| `invalid` | stringa | `null` | `null`, `error` | accettato per compatibilità, senza effetto |
| `ambiguous` | stringa | `error` | `error`, `null`, `earliest`, `latest` | accettato per compatibilità, senza effetto |

Leggibile come testo: `utf8`, `int64`, `uint64`, `float64`, `bool`,
`date32`, `timestamp(ms)`, `decimal128` con scala da 0 a 38, `binary`,
`dictionary<utf8>` con chiavi `int32`. Una colonna non testuale si legge
con la sua resa testuale: `date32` come `AAAA-MM-GG`, `timestamp(ms)` come
RFC 3339.

La lettura deve consumare tutto il testo della cella; un formato senza
campi orari legge una data e la pone a mezzanotte. Un eventuale offset nel
testo letto non conta: il valore è sempre ora locale di
`source_timezone`. Un'ora locale che nel fuso di partenza si ripete (il
ritorno all'ora solare) o non esiste (il passaggio all'ora legale) rifiuta
sempre la riga: `ambiguous` e `invalid` non scelgono un'alternativa.

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
- `source_timezone` o `target_timezone` non riconosciuti;
- un formato vuoto, oltre `max_string_bytes`, con un campo non
  riconosciuto, o che non si sa scrivere per un valore con fuso;
- `output_column` non valido;
- config con campi sconosciuti, `invalid` o `ambiguous` fuori elenco.

In esecuzione:

- `DataMapping` con diagnostica per riga: una cella non nulla che non si
  legge (`conversion.invalid_datetime`), un'ora locale ambigua
  (`conversion.ambiguous_local_time`) o inesistente
  (`conversion.nonexistent_local_time`) nel fuso di partenza; il passo non
  produce uscita;
- `DataMapping`, senza diagnostica per riga: un valore che `output_format`
  non sa scrivere (anno fuori da 0..=9999 con `%C`);
- `Schema`: una cella che non si converte in testo.

### Limiti e deviazioni

Le regole dei fusi sono quelle della banca dati IANA inclusa in
`chrono-tz` 0.10.4: un cambio di regole successivo non si vede finché la
dipendenza non si aggiorna. `ambiguous` e `invalid` non hanno effetto e non
si rifiutano
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner)).

### Complessità

Tempo O(n) (due letture per riga: il controllo, poi la conversione);
memoria O(n) per la colonna d'uscita.

### Esempio

```json
{
  "config": {"column": "ora", "input_format": "%Y-%m-%d %H:%M", "output_format": "%Y-%m-%d %H:%M %z", "source_timezone": "Europe/Rome", "target_timezone": "UTC", "output_column": "ora_utc"},
  "ingressi": [
    {"nome": "eventi", "colonne": [
      {"nome": "ora", "tipo": "utf8", "valori": ["2024-01-15 10:00", "2024-07-15 10:00", null]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "ora", "tipo": "utf8", "valori": ["2024-01-15 10:00", "2024-07-15 10:00", null]},
    {"nome": "ora_utc", "tipo": "utf8", "valori": ["2024-01-15 09:00 +0000", "2024-07-15 08:00 +0000", null]}
  ]}
}
```
