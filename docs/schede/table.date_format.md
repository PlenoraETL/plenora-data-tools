### Che cosa fa

Legge date e ore scritte come testo con un formato e le riscrive con un
altro formato in una colonna nuova (per esempio da `31/01/2024` a
`2024-01-31`). I formati sono quelli di `strftime` della libreria `chrono`.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa | obbligatorio | colonna temporale o leggibile come testo | colonna da leggere |
| `input_format` | stringa | assente | formato `chrono` non vuoto, al più `max_string_bytes` byte; obbligatorio per un testo, rifiutato per una colonna temporale; `null` non ammesso | formato di lettura di un testo |
| `output_format` | stringa | `"%Y-%m-%d %H:%M:%S"` | formato `chrono` non vuoto, al più `max_string_bytes` byte, senza fuso, che scrive al più `max_string_bytes` byte per valore | formato di scrittura |
| `output_column` | stringa | obbligatorio | nome valido | colonna d'uscita |
| `invalid` | stringa | assente | nessuno: scritto si rifiuta, anche `null` | un valore non leggibile rifiuta sempre la riga, nessun valore avrebbe effetto |

Una colonna temporale (`date32`, `timestamp` in secondi, millisecondi,
microsecondi o nanosecondi, con o senza fuso) si legge dal valore nativo,
senza `input_format` (scritto, si rifiuta): vale l'ora locale della
colonna (del suo fuso; senza fuso, il valore com'è), e una data è la sua
mezzanotte ([Limiti dichiarati, «Colonne temporali e formati di data»](limiti.md#colonne-temporali-e-formati-di-data)). Ogni altra colonna si legge come testo, con
`input_format` obbligatorio; leggibili come testo: `utf8`, `int64`,
`uint64`, `float64`, `bool`, `decimal128` con scala da 0 a 38, `binary`,
`dictionary<utf8>` con chiavi `int32`.

La lettura deve consumare tutto il testo della cella. Un formato senza
campi orari legge una data e la pone a mezzanotte. `output_format` non può
contenere campi di fuso o di offset (`%z`, `%:z`, `%Z`, `%+`): il valore
letto non ha fuso. Un valore non leggibile rifiuta sempre la riga.

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

- `column` assente o non leggibile come testo;
- `input_format` assente con una colonna di testo, o scritto con una
  colonna temporale;
- un formato vuoto, oltre `max_string_bytes`, con un campo non riconosciuto
  (`%Q`, `%` finale), o `output_format` con campi di fuso;
- `output_format` che può scrivere più di `max_string_bytes` byte per
  valore;
- `output_column` non valido;
- `invalid` scritto, con qualunque valore, anche `null`;
- config con campi sconosciuti.

In esecuzione:

- `DataMapping` con diagnostica per riga: ogni cella non nulla che non si
  legge con `input_format` (`conversion.invalid_datetime`); il passo non
  produce uscita;
- `DataMapping`, senza diagnostica per riga: un valore che `output_format`
  non sa scrivere (anno fuori da 0..=9999 con `%C`);
- `Schema`: una cella che non si converte in testo.

### Limiti e deviazioni

Non esiste un modo di trasformare un valore non leggibile in null: per
questo `invalid` si rifiuta.

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
