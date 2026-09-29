### Che cosa fa

Tiene le righe in cui la colonna `column` soddisfa la condizione
`operator value` e scarta le altre. Le colonne e i loro tipi non cambiano.
Una cella nulla non soddisfa nessun operatore tranne `isnull`; `!=` scarta
quindi anche i null.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa | obbligatorio | nome di una colonna dell'ingresso | colonna su cui si valuta la condizione |
| `operator` | stringa | obbligatorio | `==`, `!=`, `>`, `>=`, `<`, `<=`, `contains`, `startswith`, `endswith`, `isnull`, `notnull`, `between` | confronto fra la cella e `value` |
| `value` | JSON | `null` | stringa, numero, booleano o `null`; per `between` il testo `"min,max"` | termine di confronto; un non-stringa vale il suo testo JSON, `null` vale `""` |

Come si confronta, per operatore:

- `==`, `!=` su `int64` e `float64`: confronto numerico esatto con `value`,
  che deve essere un numero (intero, decimale o esponenziale). Su
  `float64` `0.0 == -0.0` e `NaN == NaN`. Su ogni altro tipo si confronta il
  testo della cella con il testo di `value`, carattere per carattere
  (su `uint64` `5` e `"5"` sono uguali, `5.0` e `"05"` no; una `date32` vale
  `"2024-01-31"`, un `decimal128` di scala 2 vale `"12.30"`, un `bool`
  `"true"`/`"false"`);
- `>`, `>=`, `<`, `<=`, `between`: confronto nel dominio nativo del tipo,
  mai attraverso `f64`, su `int64`, `uint64`, `float64`, `decimal128`,
  `date32` (giorni dall'epoca), `timestamp(ms)` (millisecondi dall'epoca) e
  `utf8` il cui testo è un numero (spazi ai lati ignorati, virgola decimale
  ammessa). `value` deve essere numerico; `between` include entrambi gli
  estremi. Un `NaN`, nella cella o nell'estremo, rende falso il confronto;
- `contains` (senza distinzione fra maiuscole e minuscole), `startswith`,
  `endswith` (con distinzione): sul testo della cella;
- `isnull`, `notnull`: sulla nullità logica della cella (anche la voce
  nulla di un dizionario); `value` non conta.

### Schema

Identico all'ingresso: stesse colonne, tipi, nullabilità e metadati. Delle
proprietà del contratto resta solo l'ordinamento dichiarato (`sorted_by`):
il conteggio delle righe non è più noto.

### Righe

Filtro: da 0 a tutte le righe dell'ingresso, ciascuna al più una volta.

### Ordine

Le righe tenute restano nell'ordine d'ingresso.

### Errori

In validazione (analisi del contratto), `InvalidPlan`:

- `column` non è una colonna dell'ingresso;
- `==`/`!=` su `int64` o `float64` con `value` non numerico;
- `>`, `>=`, `<`, `<=`, `between` su un tipo fuori dall'elenco sopra, o con
  `value` non numerico; `between` senza la forma `min,max` o con un estremo
  non numerico;
- `contains`, `startswith`, `endswith` (e `==`/`!=` fuori da `int64` e
  `float64`) su una colonna che non si legge come testo scalare;
- config con campi sconosciuti o `operator` fuori elenco.

In esecuzione:

- `Schema`: una cella `utf8` che non è un numero sotto un operatore
  ordinato o `between` (dipende dai dati, quindi l'analisi non lo vede);
- `Schema`: una cella che sotto un operatore testuale non si legge come
  testo (un `binary` non UTF-8, una data fuori dall'intervallo di chrono);
- `ResourceLimit`: una riga tenuta con indice oltre `u32::MAX` (ingressi
  di più di 2^32 righe).

### Limiti e deviazioni

Il confronto fra numeri è esatto per costruzione, anche oltre `2^53` e fra
interi e decimali ([README, «Validazione»](../README.md#validazione)). Gli
operatori ordinati confrontano le date come numero di giorni dall'epoca e i
timestamp come millisecondi: `value` è un intero, non una data in testo;
`==` e `!=` invece confrontano il testo della data.

### Complessità

Tempo O(n) sulle righe dell'ingresso; memoria O(k) per le righe tenute
(copia delle colonne selezionate), più O(k) indici.

### Esempio

```json
{
  "config": {"column": "importo", "operator": ">", "value": 10},
  "ingressi": [
    {"nome": "ordini", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2, 3, 4]},
      {"nome": "importo", "tipo": "float64", "valori": [5.5, 12.0, null, 40.25]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [2, 4]},
    {"nome": "importo", "tipo": "float64", "valori": [12.0, 40.25]}
  ]}
}
```
