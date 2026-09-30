### Che cosa fa

Converte la colonna `column` nel tipo `target_type`, nella stessa
posizione. Ogni cella non nulla si legge come testo e il testo si
interpreta nel tipo chiesto. Se anche una sola cella non si converte il
passo fallisce e dice quali righe (con `coerce` e `raise`, i due
equivalenti): nessuna cella diventa null in silenzio.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa | obbligatorio | colonna dell'ingresso leggibile come testo (sotto) | colonna da convertire |
| `target_type` | stringa | `"str"` | `str`, `int`, `float`, `bool`, `date`, `datetime`, `date32`, `timestamp_millis`, `decimal128`, `binary_utf8`, `uint64`, `dictionary_utf8` | tipo d'arrivo |
| `date_format` | stringa | `""` | formato strftime di chrono, solo con `date`, `datetime`, `date32`, `timestamp_millis`, al più `max_string_bytes` byte | formato delle date; `""` usa i formati di default |
| `errors` | stringa | `"coerce"` | `coerce`, `raise`, `ignore`; non con `str`, `binary_utf8`, `dictionary_utf8`; `null` non ammesso | che cosa succede a una cella che non si converte |
| `precision` | intero | assente | da 1 a 38, obbligatorio con `decimal128` e solo lì; `null` non ammesso | cifre totali del decimale |
| `scale` | intero | assente | da 0 a `precision`, obbligatorio con `decimal128` e solo lì; `null` non ammesso | cifre dopo la virgola |
| `timezone` | stringa | assente | nome IANA (`Europe/Rome`), solo con `timestamp_millis`; `null` non ammesso | fuso dei testi senza fuso, e fuso della colonna d'uscita |

Tipi d'arrivo: `str`, `date`, `datetime` → `utf8`; `int` → `int64`;
`uint64` → `uint64`; `float` → `float64`; `bool` → `bool`; `date32` →
`date32`; `timestamp_millis` → `timestamp(ms)`, con `timezone` se data;
`decimal128` → `decimal128(precision, scale)`; `binary_utf8` → `binary`;
`dictionary_utf8` → `dictionary<utf8>` (chiavi `int32`).

La colonna d'ingresso può essere `utf8`, `int64`, `uint64`, `float64`,
`bool`, `date32`, `timestamp(ms)` (con o senza fuso), `decimal128` con
scala da 0 a 38, `binary`, `dictionary<utf8>`. Il suo testo è: l'intero in
decimale; il `float64` nella forma più corta che lo rilegge (`1.0` dà
`"1"`, niente esponente, `NaN`, `inf`); `true`/`false`; la data
`AAAA-MM-GG`; il timestamp in RFC 3339 nel suo fuso
(`"2024-01-31T10:00:00.123+00:00"`); il decimale con tutte le cifre della
scala (`"12.30"`); i byte di un `binary` letti come UTF-8.

Come si interpreta il testo, per tipo d'arrivo:

- `str`, `binary_utf8`, `dictionary_utf8`: il testo com'è; non falliscono;
- `int`, `uint64`: senza spazi ai lati, un intero decimale con segno
  facoltativo nel dominio del tipo; `"1.0"`, `"1e3"`, `"12.30"` falliscono;
- `float`: senza spazi ai lati, ogni virgola diventa un punto (`"1,5"` vale
  1,5), poi il parse `f64` di Rust (esponente, `NaN`, `inf` ammessi), al
  `f64` più vicino;
- `bool`: senza spazi ai lati e in minuscolo, `true`, `1`, `yes`, `si`,
  `sì`, `vero`, `t`, `y`, `s` sono vero; `false`, `0`, `no`, `falso`, `f`,
  `n` sono falso;
- `date`, `date32`: con `date_format`, quel formato; senza, nell'ordine
  `%Y-%m-%d`, `%d/%m/%Y`, `%d-%m-%Y`, `%Y/%m/%d`. Gli spazi non si tolgono.
  `date` scrive `AAAA-MM-GG`;
- `datetime`: con `date_format`, quel formato, che deve avere anche l'ora;
  senza, `%Y-%m-%dT%H:%M:%S`, `%Y-%m-%d %H:%M:%S`, `%d/%m/%Y %H:%M:%S`, poi
  i formati di sola data a mezzanotte. Scrive `AAAA-MM-GGTHH:MM:SS`, al
  secondo;
- `timestamp_millis`: senza `date_format` prima RFC 3339 con fuso (che dà
  l'istante), poi i formati di `datetime`; con `date_format`, solo quello,
  che deve avere anche l'ora.
  Un testo senza fuso è l'ora locale di `timezone` (un'ora ambigua o
  inesistente nel cambio d'ora fallisce), o UTC senza `timezone`;
- `decimal128`: senza spazi ai lati, `-` e poi `+` facoltativi, cifre, al
  più un punto; almeno una cifra prima del punto (`".5"` fallisce, `"5."`
  no), al più `scale` cifre dopo (nessun arrotondamento: con scala 1
  `"1.50"` fallisce), al più `precision` cifre significative contando le
  `scale` cifre decimali.

Con `errors`:

- `coerce`, `raise`: prima di convertire si controllano tutte le celle; se
  una non si converte il passo fallisce con la diagnostica per riga;
- `ignore`: accettato in validazione; se una cella non si converte il passo
  fallisce alla prima, senza diagnostica per riga.

Con `str`, `binary_utf8` e `dictionary_utf8` nessuna cella può fallire:
`errors` scritto, con qualunque valore, si rifiuta.

### Schema

`column` resta nella sua posizione con il tipo d'arrivo, nullable, senza i
metadati di campo di prima (una colonna geometrica convertita non è più
geometrica: il contratto diventa tabellare). Le altre colonne e i metadati
di schema restano.

Contratto: il conteggio delle righe resta; l'ordinamento dichiarato cade.

### Righe

1:1; il null resta null.

### Ordine

Righe nell'ordine d'ingresso.

### Errori

In validazione, `InvalidPlan`:

- `column` assente o di un tipo che non si legge come testo (`int32`,
  `list`, `struct`, `timestamp` non in millisecondi…), o `timestamp` con un
  fuso Arrow non valido;
- `date_format` con un target che non lo usa, oltre `max_string_bytes`, o
  con un elemento strftime non riconosciuto;
- `decimal128` senza `precision` o `scale`, o fuori da
  `1 <= precision <= 38`, `0 <= scale <= precision`; `precision` o `scale`
  con un altro target;
- `timezone` con un target diverso da `timestamp_millis`, o non un nome
  IANA;
- `errors` scritto con `str`, `binary_utf8` o `dictionary_utf8`;
- `errors`, `precision`, `scale` o `timezone` `null` espliciti: un
  parametro facoltativo si omette;
- valori fuori elenco, config con campi sconosciuti.

Le regole su `date_format`, `precision`, `scale`, `timezone` ed `errors`
le applica anche il kernel, con la stessa funzione della validazione.

In esecuzione:

- `DataMapping` con diagnostica per riga (`coerce`, `raise`): almeno una
  cella non si converte. Per ogni causa il conteggio
  (`conversion.invalid_integer`, `invalid_unsigned_integer`,
  `invalid_float`, `invalid_boolean`, `invalid_date`, `invalid_datetime`,
  `invalid_timestamp`, `invalid_decimal`) e i primi 10 esempi, con l'indice
  di riga (da 0) e la colonna, mai il valore;
- `InvalidPlan` (`ignore`): una cella non si converte;
- `Schema`: una cella che non si legge come testo (un `binary` non UTF-8,
  una data fuori dall'intervallo di chrono).

### Limiti e deviazioni

- **`coerce` non trasforma in null**: è un nome storico; qui una cella
  non convertibile è sempre un errore.
- **Arrotondamento di `float`**: un intero oltre 2^53 o un decimale
  diventa il `f64` più vicino, con perdita delle cifre basse.
- **Timestamp verso le date**: il testo di una colonna `timestamp` porta il
  fuso, e i formati di default di `date`, `datetime`, `date32` lo
  rifiutano: ogni cella fallisce. Si converte in `str` o in
  `timestamp_millis`, o si dà un `date_format` con `%:z`.
- **Segni nel `decimal128`**: `"-+5"` si legge come -5.
- Il testo di un `float64` con parte decimale non diventa mai `int`: si
  arrotonda prima con un'altra operazione.

### Complessità

Tempo O(n) sulle righe (due passate con `coerce` e `raise`: controllo e
conversione); memoria pari alla colonna d'uscita, più testi temporanei
per cella.

### Esempio

```json
{
  "config": {"column": "importo", "target_type": "decimal128", "precision": 10, "scale": 2},
  "ingressi": [
    {"nome": "righe", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
      {"nome": "importo", "tipo": "utf8", "valori": [" 12.5", "-0.05", null]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
    {"nome": "importo", "tipo": "decimal128(10, 2)", "valori": ["12.50", "-0.05", null]}
  ]}
}
```
