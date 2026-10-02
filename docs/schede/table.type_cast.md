### Che cosa fa

Converte la colonna `column` nel tipo `target_type`, nella stessa
posizione. Ogni cella non nulla si legge come testo e il testo si
interpreta nel tipo chiesto. Se anche una sola cella non si converte il
passo fallisce e dice quali righe (con `coerce` e `raise`, i due
equivalenti): nessuna cella diventa null in silenzio.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa | obbligatorio | colonna temporale o leggibile come testo (sotto) | colonna da convertire |
| `target_type` | stringa | `"str"` | `str`, `int`, `float`, `bool`, `date`, `datetime`, `date32`, `timestamp_millis`, `decimal128`, `binary_utf8`, `uint64`, `dictionary_utf8` | tipo d'arrivo |
| `date_format` | stringa | `""` | formato strftime di chrono, solo con `date`, `datetime`, `date32`, `timestamp_millis` e una colonna di testo, al più `max_string_bytes` byte | formato delle date; `""` usa i formati ISO di default |
| `errors` | stringa | `"coerce"` | `coerce`, `raise`, `ignore`; non con `str`, `binary_utf8`, `dictionary_utf8`; `null` non ammesso | che cosa succede a una cella che non si converte |
| `precision` | intero | assente | da 1 a 38, obbligatorio con `decimal128` e solo lì; `null` non ammesso | cifre totali del decimale |
| `scale` | intero | assente | da 0 a `precision`, obbligatorio con `decimal128` e solo lì; `null` non ammesso | cifre dopo la virgola |
| `timezone` | stringa | assente | nome IANA (`Europe/Rome`), solo con `timestamp_millis`; `null` non ammesso | fuso dei testi senza fuso, e fuso della colonna d'uscita |

Tipi d'arrivo: `str`, `date`, `datetime` → `utf8` (**testo**: `date`
scrive `AAAA-MM-GG`, `datetime` `AAAA-MM-GGTHH:MM:SS`; i tipi temporali
Arrow sono `date32` e `timestamp_millis`); `int` → `int64`;
`uint64` → `uint64`; `float` → `float64`; `bool` → `bool`; `date32` →
`date32`; `timestamp_millis` → `timestamp(ms)`, con `timezone` se data;
`decimal128` → `decimal128(precision, scale)`; `binary_utf8` → `binary`;
`dictionary_utf8` → `dictionary<utf8>` (chiavi `int32`).

Una colonna **temporale** (`date32`; `timestamp` in secondi,
millisecondi, microsecondi o nanosecondi, con o senza fuso) si converte
dal valore nativo, senza passare dal testo e senza `date_format`
(scritto, si rifiuta) ([Limiti dichiarati, «Colonne temporali e formati di data»](limiti.md#colonne-temporali-e-formati-di-data)):

- `str`, `binary_utf8`, `dictionary_utf8`: la data `AAAA-MM-GG`, l'istante
  in RFC 3339 nel fuso della colonna (senza fuso `+00:00`), con le cifre
  frazionarie che servono (`"2024-01-31T10:00:00.123+00:00"`);
- `date`, `datetime`, `date32`: la data e l'ora **locali** della colonna
  (del suo fuso; senza fuso, il valore com'è); `datetime` scrive la
  frazione di secondo quando c'è;
- `timestamp_millis`: lo stesso istante (una data è la sua mezzanotte nel
  fuso `timezone`, o in UTC); un istante con una parte sotto il
  millisecondo si rifiuta (`conversion.timestamp_precision`) invece di
  troncarla;
- `int`, `uint64`, `float`, `bool`, `decimal128`: rifiutati in validazione
  (un numero da un istante non ha un significato scritto).

Ogni altra colonna si legge come testo: `utf8`, `int64`, `uint64`,
`float64`, `bool`, `decimal128` con scala da 0 a 38, `binary`,
`dictionary<utf8>`. Il suo testo è: l'intero in decimale; il `float64`
nella forma più corta che lo rilegge (`1.0` dà `"1"`, niente esponente,
`NaN`, `inf`); `true`/`false`; il decimale con tutte le cifre della scala
(`"12.30"`); i byte di un `binary` letti come UTF-8.

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
- `date`, `date32`: con `date_format`, quel formato; senza, i soli
  formati ISO 8601: RFC 3339 con offset, data e ora con `T` o spazio e
  frazione facoltativa, `%Y-%m-%d`. Nessun formato con giorno e mese in un
  ordine da indovinare (`31/01/2024`, `2024/01/31` servono un
  `date_format`). Gli spazi non si tolgono. Con un offset vale la data
  scritta. `date` scrive `AAAA-MM-GG`;
- `datetime`: con `date_format`, quel formato, che deve avere anche l'ora;
  senza, i formati ISO di sopra (la data sola a mezzanotte). Scrive
  `AAAA-MM-GGTHH:MM:SS` più la frazione di secondo quando c'è (tre, sei o
  nove cifre: non si tronca); con un offset vale l'ora scritta;
- `timestamp_millis`: con un offset (RFC 3339 senza `date_format`, o `%z`
  nel formato) l'istante; con `date_format`, solo quello, che deve avere
  anche l'ora. Un testo senza fuso è l'ora locale di `timezone` (un'ora
  ambigua o inesistente nel cambio d'ora fallisce), o UTC senza
  `timezone`. Una frazione sotto il millisecondo fallisce invece di
  troncarsi;
- `decimal128`: senza spazi ai lati, un segno facoltativo (`-` o `+`, uno
  solo: `"-+5"` fallisce come `"--5"`), cifre, al
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

- `column` assente o di un tipo che non si legge come testo né è
  temporale (`int32`, `list`, `struct`…), o `timestamp` con un fuso Arrow
  non valido;
- una colonna temporale con un target numerico o booleano, o con
  `date_format`;
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
  `invalid_timestamp`, `timestamp_precision`, `invalid_decimal`) e i primi
  10 esempi, con l'indice
  di riga (da 0) e la colonna, mai il valore;
- `InvalidPlan` (`ignore`): una cella non si converte;
- `Schema`: una cella che non si legge come testo (un `binary` non UTF-8,
  una data fuori dall'intervallo di chrono).

### Limiti e deviazioni

- **`coerce` non trasforma in null**: è un nome storico; qui una cella
  non convertibile è sempre un errore.
- **Arrotondamento di `float`**: un intero oltre 2^53 o un decimale
  diventa il `f64` più vicino, con perdita delle cifre basse.
- **Nomi dei target**: `date` e `datetime` producono testo, `date32` e
  `timestamp_millis` tipi temporali Arrow; i nomi restano per
  compatibilità, e un nome fuori elenco (`timestamp`, `date64`) si
  rifiuta.
- **Offset nei testi verso `date`/`datetime`**: si legge e vale l'ora
  scritta; l'istante si perde, come nel testo d'uscita che non ha fuso. Per
  tenerlo serve `timestamp_millis`.
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
