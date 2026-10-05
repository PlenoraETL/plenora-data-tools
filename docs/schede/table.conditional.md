### Che cosa fa

Aggiunge una colonna calcolata con regole «se… allora»: per ogni riga
valuta le `conditions` sulla colonna `column`, nell'ordine, e scrive il
`result` della prima vera; se nessuna è vera scrive `default_value`. Le
condizioni si valutano come in [`table.filter`](#tablefilter). La colonna
d'uscita è numerica (`float64`) se tutti i risultati possibili sono numeri,
testuale (`utf8`) altrimenti.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa | obbligatorio | colonna dell'ingresso | colonna su cui si valutano le condizioni |
| `conditions` | lista di oggetti | obbligatorio | da 1 a 4096 condizioni | regole, in ordine di precedenza |
| `conditions[].operator` | stringa | `"=="` | gli operatori di `table.filter` | confronto fra la cella e `value` |
| `conditions[].value` | JSON | obbligatorio, tranne con `isnull`, `notnull` | come `value` di `table.filter`, mai `null`; con `isnull`, `notnull` non si scrive | termine di confronto |
| `conditions[].result` | JSON | `null` | stringa, numero, booleano o `null` (la cella nulla); testo al più `max_string_bytes` byte | valore scritto se la condizione è la prima vera |
| `default_value` | JSON | `null` | stringa, numero, booleano o `null` (la cella nulla); testo al più `max_string_bytes` byte | valore scritto se nessuna condizione è vera |
| `output_column` | stringa | `"result"` | nome non vuoto, al più 1024 byte | colonna d'uscita |

Il tipo d'uscita dipende solo dalla config. Un `result` o il
`default_value` `null` dà la cella nulla, come `THEN NULL` di SQL (e il
`default_value` assente, `null`, come un `CASE` senza `ELSE`); gli altri si
leggono come testo (una stringa com'è, un numero o un booleano come testo
JSON):

- se ogni testo, sostituite le virgole con punti, è un numero per il parse
  `f64` di Rust (esponente ammesso), l'uscita è `float64` nullable: il
  `null` dà null, gli altri il numero (`"1,5"` dà 1,5, `"1e3"` dà 1000). Il
  testo vuoto `""` non è un numero. Un risultato scritto come intero che il
  `float64` non rappresenta esattamente (`9007199254740993`) si rifiuta
  invece di diventare un altro intero;
- altrimenti l'uscita è `utf8`, nullable solo se un `result` o il
  `default_value` è `null`, e ogni cella è il testo del valore scelto
  (`""` dà `""`, `true` dà `"true"`, `2` dà `"2"`) o null per un `null`.

Fino alla 1.1.0 `null` valeva il testo vuoto: nell'uscita `utf8` dava `""`,
e un `""` accanto a numeri dava un `float64` nullo.

Un `result` o un `default_value` che si legge come numero non finito
(`"NaN"`, `"inf"`, `"1e999"`) si rifiuta, in validazione e nel kernel.

Una cella nulla non soddisfa nessun operatore tranne `isnull`. Con
`isnull` e `notnull` il `value` non avrebbe effetto: scritto, anche
`null`, si rifiuta; con gli altri operatori è obbligatorio e non `null`,
come in `table.filter`.

### Schema

La colonna `output_column`: se esiste già si sostituisce nella sua
posizione (con il tipo nuovo, senza i metadati di campo di prima),
altrimenti si aggiunge in coda. Le altre colonne e i metadati di schema
restano.

Contratto: il conteggio delle righe resta; l'ordinamento dichiarato resta
solo se `output_column` è una colonna nuova.

### Righe

1:1: ogni riga riceve esattamente un valore.

### Ordine

Righe nell'ordine d'ingresso; per ogni riga le condizioni nell'ordine di
`conditions`, e vince la prima vera.

### Errori

In validazione, `InvalidPlan`:

- `column` assente;
- `conditions` assente, vuota o con più di 4096 condizioni;
- per ogni condizione, gli stessi rifiuti di operatore, tipo di colonna e
  `value` di `table.filter`, compreso `value` scritto (anche `null`) con
  `isnull` o `notnull` e `value` `null` o assente con gli altri operatori;
- il testo di un `result` o del `default_value` oltre `max_string_bytes`
  byte, o che si legge come numero non finito;
- uscita `float64` con un risultato intero non esatto in `float64`;
- `output_column` vuoto, di soli spazi o oltre 1024 byte;
- config con campi sconosciuti.

In esecuzione, `Schema`: una cella `utf8` che non è un numero sotto un
operatore ordinato o `between`, o una cella che non si legge come testo
sotto un operatore testuale.

### Limiti e deviazioni

- **Tipo deciso dai letterali**: basta un risultato non numerico (anche
  `""`) perché anche i risultati numerici diventino testo (`2` diventa
  `"2"`).
- **Virgola decimale**: in un risultato ogni virgola vale un punto, quindi
  `"1,5"` è 1,5 ma `"1.000,5"` non è un numero e rende testuale l'uscita.

### Complessità

Tempo O(n·k) per n righe e k condizioni; memoria O(n) per la colonna
d'uscita (un testo per riga prima della conversione numerica).

### Esempio

```json
{
  "config": {
    "column": "classe",
    "conditions": [
      {"operator": "==", "value": "A", "result": "1,5"},
      {"operator": "==", "value": "B", "result": 1},
      {"operator": "isnull", "result": null}
    ],
    "default_value": "0",
    "output_column": "coefficiente"
  },
  "ingressi": [
    {"nome": "t", "colonne": [
      {"nome": "classe", "tipo": "utf8", "valori": ["A", "B", null, "C"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "classe", "tipo": "utf8", "valori": ["A", "B", null, "C"]},
    {"nome": "coefficiente", "tipo": "float64", "valori": [1.5, 1.0, null, 0.0]}
  ]}
}
```
