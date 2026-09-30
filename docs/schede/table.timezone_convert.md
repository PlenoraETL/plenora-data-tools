### Che cosa fa

Legge date e ore scritte come testo, le interpreta come ora locale di un
fuso orario di partenza e le riscrive come ora locale di un fuso di arrivo,
con un formato che può mostrare l'offset o il nome del fuso. I fusi sono
nomi IANA (`Europe/Rome`, `UTC`, `America/New_York`).

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa | obbligatorio | colonna temporale o leggibile come testo | colonna da leggere |
| `input_format` | stringa | assente | formato `chrono` non vuoto, al più `max_string_bytes` byte; obbligatorio per un testo, rifiutato per una colonna temporale; `null` non ammesso | formato di lettura di un testo |
| `output_format` | stringa | `"%Y-%m-%d %H:%M:%S"` | formato `chrono` non vuoto, al più `max_string_bytes` byte, che scrive al più `max_string_bytes` byte per valore | formato di scrittura; ammette `%z`, `%:z`, `%Z`, `%+` |
| `source_timezone` | stringa | obbligatorio | nome IANA noto a `chrono-tz` | fuso dei valori letti |
| `target_timezone` | stringa | obbligatorio | nome IANA noto a `chrono-tz` | fuso dei valori scritti |
| `output_column` | stringa | obbligatorio | nome valido | colonna d'uscita |
| `invalid` | stringa | assente | nessuno: scritto si rifiuta, anche `null` | un valore non leggibile rifiuta sempre la riga, nessun valore avrebbe effetto |
| `ambiguous` | stringa | assente | nessuno: scritto si rifiuta, anche `null` | un'ora locale ambigua o inesistente rifiuta sempre la riga, nessun valore avrebbe effetto |

Una colonna temporale (`date32`, `timestamp` in secondi, millisecondi,
microsecondi o nanosecondi, con o senza fuso) si legge dal valore nativo,
senza `input_format` (scritto, si rifiuta): vale l'ora locale della
colonna (del suo fuso; senza fuso, il valore com'è), e una data è la sua
mezzanotte ([README, «Colonne temporali e formati di data»](../README.md#colonne-temporali-e-formati-di-data)). Ogni altra colonna si legge come testo, con
`input_format` obbligatorio; leggibili come testo: `utf8`, `int64`,
`uint64`, `float64`, `bool`, `decimal128` con scala da 0 a 38, `binary`,
`dictionary<utf8>` con chiavi `int32`.

La lettura deve consumare tutto il testo della cella; un formato senza
campi orari legge una data e la pone a mezzanotte. Un valore con un
istante si converte dall'istante: una colonna `timestamp` con fuso (che
dev'essere `source_timezone`, altrimenti il piano si rifiuta) o un testo
letto con un offset (`%z`/`%:z`: l'offset letto prevale su
`source_timezone`). Ogni altro valore (testo senza offset, `date32` a
mezzanotte, `timestamp` senza fuso) è ora locale di `source_timezone`. Un'ora locale che nel fuso di partenza si ripete (il
ritorno all'ora solare) o non esiste (il passaggio all'ora legale) rifiuta
sempre la riga, come un valore non leggibile: per questo `ambiguous` e
`invalid` non si accettano.

Il testo scritto da `output_format` non può superare `max_string_bytes`
byte per valore. Il limite si controlla in validazione, esatto per campo:
il testo letterale conta per la sua lunghezza, ogni campo `strftime` per
la sua larghezza massima (anno 7 byte col segno; secolo `%C` 2, perché si
scrive solo per gli anni 0..=9999; mese, giorno, ora 2; nome del mese o del
giorno 9; offset `%z` 5, `+0530`, `%:z` 6, `+05:30`, `%::z` 9, `%:::z` 3;
frazioni `%3f`, `%6f`, `%9f` 3, 6, 9 e `%.3f`, `%.6f`, `%.9f` 4, 7, 10;
nome del fuso 32); `%Y%m` scrive al più 9 byte.

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

- `column` assente o non leggibile come testo; `input_format` assente con
  una colonna di testo, o scritto con una colonna temporale; una colonna
  `timestamp` con un fuso diverso da `source_timezone`;
- `source_timezone` o `target_timezone` non riconosciuti;
- un formato vuoto, oltre `max_string_bytes`, con un campo non
  riconosciuto, o che non si sa scrivere per un valore con fuso;
- `output_format` che può scrivere più di `max_string_bytes` byte per
  valore;
- `output_column` non valido;
- `invalid` o `ambiguous` scritti, con qualunque valore, anche `null`;
- config con campi sconosciuti.

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
dipendenza non si aggiorna.

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
