### Che cosa fa

Legge la colonna `column` come data o data e ora e ne estrae le parti
chieste (anno, mese, giorno, trimestre, giorno della settimana, settimana
ISO, ora, minuto, secondo), una colonna `int64` per parte. Se anche un
solo valore non si interpreta come data il passo fallisce e dice quali
righe.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa | obbligatorio | colonna dell'ingresso leggibile come testo | date da leggere |
| `parts` | lista di stringhe | `["year"]` | `year`, `month`, `day`, `quarter`, `weekday`, `week`, `hour`, `minute`, `second` | parti da estrarre, nell'ordine delle colonne d'uscita |
| `prefix` | stringa | `""` | qualunque; `""` vale `<column>_` | prefisso dei nomi d'uscita (`<prefix><parte>`) |
| `date_format` | stringa o `null` | `null` | formato strftime di chrono, non vuoto, al più `max_string_bytes` byte | formato delle date; `null` usa i formati di default |
| `invalid` | stringa | `"null"` | `null`, `error` | nessun effetto (sotto) |

Ogni cella non nulla si legge come testo (una `date32` come `AAAA-MM-GG`,
un `timestamp` in RFC 3339 con il fuso) e si interpreta:

- con `date_format`: prima come data e ora con quel formato, poi come sola
  data a mezzanotte;
- senza: `%Y-%m-%dT%H:%M:%S`, `%Y-%m-%d %H:%M:%S`, `%d/%m/%Y %H:%M:%S`,
  poi `%Y-%m-%d`, `%d/%m/%Y`, `%d-%m-%Y`, `%Y/%m/%d` a mezzanotte. Gli
  spazi non si tolgono.

Le parti: `year` l'anno del calendario gregoriano; `month` 1-12; `day`
1-31; `quarter` 1-4; `weekday` 0 per il lunedì fino a 6 per la domenica;
`week` il numero di settimana ISO 8601, 1-53, che a cavallo d'anno può
appartenere all'anno vicino (il 2021-01-01 è nella settimana 53);
`hour`, `minute`, `second`.

`invalid` si accetta per compatibilità ma non cambia niente: un valore non
interpretabile fa sempre fallire il passo, anche con `"null"`.

### Schema

Una colonna `int64` nullable per parte, di nome `<prefix><parte>`: se
esiste già si sostituisce nella sua posizione (senza i metadati di campo di
prima), altrimenti si aggiunge in coda nell'ordine di `parts`. Una parte
ripetuta scrive due volte la stessa colonna; `parts` vuota non aggiunge
niente. Le altre colonne e i metadati di schema restano.

Contratto: il conteggio delle righe resta; l'ordinamento dichiarato resta
solo se nessuna colonna esistente è stata sostituita.

### Righe

1:1; la cella null dà null in ogni parte.

### Ordine

Righe nell'ordine d'ingresso.

### Errori

In validazione, `InvalidPlan`:

- `column` assente o di un tipo che non si legge come testo;
- `date_format` vuoto, oltre `max_string_bytes` o con un elemento
  strftime non riconosciuto;
- un nome d'uscita `<prefix><parte>` vuoto, di soli spazi o oltre 1024
  byte;
- una parte o un `invalid` fuori elenco, config con campi sconosciuti.

In esecuzione:

- `DataMapping` con diagnostica per riga: almeno un valore non si
  interpreta come data. Causa `conversion.invalid_datetime`, conteggio e
  primi 10 esempi con indice di riga (da 0) e colonna, mai il valore;
- `Schema`: una cella che non si legge come testo.

### Limiti e deviazioni

- **`invalid` senza effetto accettato**: è un parametro scritto che non
  cambia il risultato, e non si rifiuta.
- **Colonne `timestamp`**: il loro testo porta il fuso (`+00:00`), che i
  formati di default non riconoscono: ogni cella fallisce. Serve un
  `date_format` con il fuso, per esempio `%Y-%m-%dT%H:%M:%S%.f%:z`; le parti
  sono quelle dell'ora locale nel fuso della colonna.

### Complessità

Tempo O(n) sulle righe (due passate: controllo e estrazione); memoria O(n)
per ogni parte estratta.

### Esempio

```json
{
  "config": {"column": "quando", "parts": ["year", "quarter", "weekday", "week"], "prefix": "q_"},
  "ingressi": [
    {"nome": "eventi", "colonne": [
      {"nome": "quando", "tipo": "utf8", "valori": ["2021-01-01", "30/12/2019 23:59:59", null]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "quando", "tipo": "utf8", "valori": ["2021-01-01", "30/12/2019 23:59:59", null]},
    {"nome": "q_year", "tipo": "int64", "valori": [2021, 2019, null]},
    {"nome": "q_quarter", "tipo": "int64", "valori": [1, 4, null]},
    {"nome": "q_weekday", "tipo": "int64", "valori": [4, 0, null]},
    {"nome": "q_week", "tipo": "int64", "valori": [53, 1, null]}
  ]}
}
```
