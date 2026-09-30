### Che cosa fa

Legge date e ore scritte come testo, vi aggiunge (o toglie, con `amount`
negativo) una quantità fissa di anni, mesi, settimane, giorni, ore, minuti
o secondi e scrive il risultato come testo in una colonna nuova. Anni e mesi
seguono il calendario: il giorno oltre la fine del mese diventa l'ultimo
giorno del mese (31 gennaio più un mese è 29 febbraio 2024).

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa | obbligatorio | colonna leggibile come testo | colonna da leggere |
| `input_format` | stringa | obbligatorio | formato `chrono` non vuoto, al più `max_string_bytes` byte | formato di lettura |
| `output_format` | stringa | `"%Y-%m-%d %H:%M:%S"` | formato `chrono` non vuoto, al più `max_string_bytes` byte, senza fuso, che scrive al più `max_string_bytes` byte per valore | formato di scrittura |
| `amount` | intero | obbligatorio | intero a 64 bit che almeno una data sopporta | quantità da aggiungere, con segno |
| `unit` | stringa | obbligatorio | `years`, `months`, `weeks`, `days`, `hours`, `minutes`, `seconds` | unità di `amount` |
| `output_column` | stringa | obbligatorio | nome valido | colonna d'uscita |
| `invalid` | stringa | assente | nessuno: scritto si rifiuta, anche `null` | un valore non leggibile rifiuta sempre la riga, nessun valore avrebbe effetto |

Leggibile come testo: `utf8`, `int64`, `uint64`, `float64`, `bool`,
`date32`, `timestamp(ms)`, `decimal128` con scala da 0 a 38, `binary`,
`dictionary<utf8>` con chiavi `int32`. Una colonna non testuale si legge
con la sua resa testuale: `date32` come `AAAA-MM-GG`, `timestamp(ms)` come
RFC 3339.

La lettura deve consumare tutto il testo della cella; un formato senza
campi orari legge una data e la pone a mezzanotte. Il valore non ha fuso:
settimane, giorni, ore, minuti e secondi sono durate fisse (un giorno è
sempre 24 ore, senza ora legale). `years` vale 12 mesi. Un valore non
leggibile rifiuta sempre la riga.

Il testo scritto da `output_format` non può superare `max_string_bytes`
byte per valore. Il limite si controlla in validazione, esatto per campo:
il testo letterale conta per la sua lunghezza, ogni campo `strftime` per
la sua larghezza massima (anno 7 byte col segno, mese, giorno, ora 2, nome
del mese o del giorno 9, offset 9, nome del fuso 32); `%Y%m` scrive al più
9 byte.

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
- un formato vuoto, oltre `max_string_bytes`, con un campo non
  riconosciuto, o `output_format` con campi di fuso (`%z`, `%:z`, `%Z`,
  `%+`);
- `output_format` che può scrivere più di `max_string_bytes` byte per
  valore;
- `amount` che nessuna data rappresentabile sopporta nell'unità data;
- `output_column` non valido;
- `invalid` scritto, con qualunque valore, anche `null`;
- config con campi sconosciuti o `unit` fuori elenco.

In esecuzione:

- `DataMapping` con diagnostica per riga: una cella non nulla che non si
  legge con `input_format` (`conversion.invalid_datetime`) o il cui
  risultato esce dalle date rappresentabili (`conversion.datetime_range`);
  il passo non produce uscita;
- `DataMapping`, senza diagnostica per riga: un valore che `output_format`
  non sa scrivere (anno fuori da 0..=9999 con `%C`);
- `Schema`: una cella che non si converte in testo.

### Limiti e deviazioni

Le date rappresentabili sono quelle di `chrono` (anni da -262143 a
262142). Un `amount` che alcune date sopportano e quelle dei dati no
fallisce in esecuzione
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner)).

### Complessità

Tempo O(n) (due letture per riga: il controllo, poi la conversione);
memoria O(n) per la colonna d'uscita.

### Esempio

```json
{
  "config": {"column": "scadenza", "input_format": "%Y-%m-%d", "output_format": "%Y-%m-%d", "amount": 1, "unit": "months", "output_column": "rinnovo"},
  "ingressi": [
    {"nome": "contratti", "colonne": [
      {"nome": "scadenza", "tipo": "utf8", "valori": ["2024-01-31", "2024-03-15", null]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "scadenza", "tipo": "utf8", "valori": ["2024-01-31", "2024-03-15", null]},
    {"nome": "rinnovo", "tipo": "utf8", "valori": ["2024-02-29", "2024-04-15", null]}
  ]}
}
```
