### Che cosa fa

Legge la colonna `column` come data o data e ora e ne estrae le parti
chieste (anno, mese, giorno, trimestre, giorno della settimana, settimana
ISO, ora, minuto, secondo), una colonna `int64` per parte. Se anche un
solo valore non si interpreta come data il passo fallisce e dice quali
righe.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa | obbligatorio | colonna temporale o leggibile come testo | date da leggere |
| `parts` | lista di stringhe | `["year"]` | non vuota, senza ripetizioni, fra `year`, `month`, `day`, `quarter`, `weekday`, `week`, `hour`, `minute`, `second` | parti da estrarre, nell'ordine delle colonne d'uscita |
| `prefix` | stringa | `""` | qualunque; `""` vale `<column>_` | prefisso dei nomi d'uscita (`<prefix><parte>`) |
| `date_format` | stringa o `null` | `null` | formato strftime di chrono, non vuoto, al più `max_string_bytes` byte; solo con una colonna di testo | formato delle date; `null` usa i formati ISO di default |
| `invalid` | stringa | assente | nessuno: scritto si rifiuta, anche `null` | un valore non interpretabile fa sempre fallire il passo, nessun valore avrebbe effetto |

Una colonna temporale (`date32`, `timestamp` di ogni unità, con o senza
fuso) si legge dal valore nativo, senza `date_format` (scritto, si
rifiuta): le parti sono dell'ora locale della colonna (del suo fuso; senza
fuso, il valore com'è). Ogni altra cella non nulla si legge come testo e
si interpreta:

- con `date_format`: prima come data e ora con quel formato, poi come sola
  data a mezzanotte;
- senza, solo ISO 8601: RFC 3339 con offset (`2024-01-31T10:00:00Z`,
  `2024-01-31 10:00:00.5+01:00`: le parti sono dell'ora scritta), data e
  ora con `T` o spazio e frazione facoltativa (`%Y-%m-%dT%H:%M:%S%.f`,
  `%Y-%m-%d %H:%M:%S%.f`), poi `%Y-%m-%d` a mezzanotte. Nessun formato con
  giorno e mese in un ordine da indovinare (`31/01/2024` serve un
  `date_format`) ([Limiti dichiarati, «Colonne temporali e formati di data»](limiti.md#colonne-temporali-e-formati-di-data)). Gli spazi non si tolgono.

Le parti: `year` l'anno del calendario gregoriano; `month` 1-12; `day`
1-31; `quarter` 1-4; `weekday` 0 per il lunedì fino a 6 per la domenica;
`week` il numero di settimana ISO 8601, 1-53, che a cavallo d'anno può
appartenere all'anno vicino (il 2021-01-01 è nella settimana 53);
`hour`, `minute`, `second`.

Un valore non interpretabile fa sempre fallire il passo: per questo
`invalid` scritto, con qualunque valore (anche `null`), si rifiuta.

### Schema

Una colonna `int64` nullable per parte, di nome `<prefix><parte>`: se
esiste già si sostituisce nella sua posizione (senza i metadati di campo di
prima), altrimenti si aggiunge in coda nell'ordine di `parts`. Le altre
colonne e i metadati di schema restano.

Contratto: il conteggio delle righe resta; l'ordinamento dichiarato resta
solo se nessuna colonna esistente è stata sostituita.

### Righe

1:1; la cella null dà null in ogni parte.

### Ordine

Righe nell'ordine d'ingresso.

### Errori

In validazione, `InvalidPlan`:

- `column` assente o di un tipo che non si legge come testo;
- `date_format` vuoto, oltre `max_string_bytes`, con un elemento strftime
  non riconosciuto, o scritto con una colonna temporale;
- un nome d'uscita `<prefix><parte>` vuoto, di soli spazi o oltre 1024
  byte;
- `parts` vuota o con una parte ripetuta;
- una parte fuori elenco, `invalid` scritto, config con campi sconosciuti.

In esecuzione:

- `DataMapping` con diagnostica per riga: almeno un valore non si
  interpreta come data. Causa `conversion.invalid_datetime`, conteggio e
  primi 10 esempi con indice di riga (da 0) e colonna, mai il valore;
- `Schema`: una cella che non si legge come testo.

### Limiti e deviazioni

Nessuna oltre quelle dette sopra: una colonna temporale si legge dal
valore nativo, un testo con `date_format` o con i formati ISO di default.

### Complessità

Tempo O(n) sulle righe (due passate: controllo e estrazione); memoria O(n)
per ogni parte estratta.

### Esempio

```json
{
  "config": {"column": "quando", "parts": ["year", "quarter", "weekday", "week"], "prefix": "q_"},
  "ingressi": [
    {"nome": "eventi", "colonne": [
      {"nome": "quando", "tipo": "utf8", "valori": ["2021-01-01", "2019-12-30T23:59:59+01:00", null]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "quando", "tipo": "utf8", "valori": ["2021-01-01", "2019-12-30T23:59:59+01:00", null]},
    {"nome": "q_year", "tipo": "int64", "valori": [2021, 2019, null]},
    {"nome": "q_quarter", "tipo": "int64", "valori": [1, 4, null]},
    {"nome": "q_weekday", "tipo": "int64", "valori": [4, 0, null]},
    {"nome": "q_week", "tipo": "int64", "valori": [53, 1, null]}
  ]}
}
```
