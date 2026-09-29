### Che cosa fa

Porta la tabella da lunga a larga: una riga per ogni chiave delle colonne
`index_col`, una colonna per ogni valore distinto di `pivot_col`, e in ogni
cella l'aggregazione (`aggr_func`) dei valori di `value_col` delle righe
con quella chiave e quel valore. Le colonne d'uscita dipendono dai dati:
il runner rifiuta l'operazione in validazione, e l'esempio sotto è
eseguito chiamando il kernel.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `index_col` | stringa | obbligatorio | nomi di colonne separati da virgola | chiave delle righe; spazi ai lati tolti, voci vuote ignorate |
| `pivot_col` | stringa | obbligatorio | nome di una colonna dell'ingresso | i suoi valori diventano colonne |
| `value_col` | stringa | obbligatorio | nome di una colonna dell'ingresso | valori aggregati nelle celle |
| `aggr_func` | stringa | `"first"` | `first`, `last`, `min`, `max`, `sum`, `mean`, `count`, `concat` | aggregazione di una cella |
| `mapping` | oggetto | `{}` | testo → nome di colonna | se non vuoto, tiene solo i valori pivot che nomina e li rinomina |

Il valore pivot è il testo della cella (`1.0` è `"1"`, `-0.0` è `"-0"`,
ogni NaN `"NaN"`, le date `AAAA-MM-GG`); le righe con il valore pivot
nullo, o escluso da `mapping`, non riempiono celle ma la loro chiave dà
comunque una riga. Le celle, dalle righe della cella in ordine d'ingresso:

- `first`, `last`: la cella di `value_col` nella prima o nell'ultima riga,
  null compreso, con il tipo di `value_col`;
- `count`: `int64`, le celle non nulle;
- `concat`: `utf8`, i testi delle celle non nulle uniti da `,`; solo null
  dà il testo vuoto;
- `sum`, `mean`, `min`, `max`: `float64` sulla cella letta come `f64`
  (tipi numerici di [`table.aggregate`](#tableaggregate)); i null si
  saltano, solo null dà null; `min` e `max` ignorano i NaN salvo che siano
  tutti NaN, `sum` e `mean` no.

Una combinazione chiave-valore che non compare nei dati è null, anche con
`count`.

### Schema

Le colonne di `index_col` nel loro ordine, con tipo, nullabilità e
metadati di campo; poi una colonna per valore pivot, nell'ordine dei byte
del testo del valore (prima della rinomina di `mapping`), nullabile e
senza metadati di campo, con il tipo sopra. I metadati di schema non si
conservano.

### Righe

Aggregazione: una riga per chiave distinta di `index_col`, con
l'uguaglianza di [`table.distinct`](#tabledistinct); senza colonne indice
(`index_col` vuoto), una riga sola se l'ingresso ha righe.

### Ordine

Le righe nell'ordine delle chiavi di [`table.aggregate`](#tableaggregate):
null per primo, poi per la stringa `<lunghezza>:<testo>` byte per byte.

### Errori

In validazione, il runner rifiuta sempre `table.pivot`: `InvalidPlan` per
una config con campi sconosciuti o una colonna assente, altrimenti
`Unsupported` (lo schema d'uscita dipende dai dati).

Chiamando il kernel:

- `Schema`: una colonna assente; con `sum`, `mean`, `min`, `max`, una
  `value_col` di tipo non numerico o una sua cella `utf8` che non è un
  numero; una cella di chiave, di valore pivot o (con `concat`) di valore
  che non si converte in testo (tipo non leggibile come testo, `binary`
  non UTF-8 fra i valori pivot, date fuori intervallo);
- `InvalidPlan`: un nome di colonna d'uscita non valido (valore pivot
  vuoto o di soli spazi, o rinominato così da `mapping`);
- `ResourceLimit`: righe oltre `max_rows` o colonne oltre `max_columns`;
  più di `u32::MAX` righe.

### Limiti e deviazioni

Non si rifiutano nomi d'uscita ripetuti: due valori che `mapping` rinomina
allo stesso nome, o un valore pivot uguale a una colonna indice, danno due
colonne con lo stesso nome. `sum` e `mean` arrotondano un intero oltre
`2^53` o un `decimal128`, perché il risultato è `float64`. L'hash delle
chiavi non ha seme
([README, «Hash delle chiavi non keyed»](../README.md#hash-delle-chiavi-non-keyed)).

### Complessità

Tempo O(n) sulle righe più O(g log g + p log p) per ordinare g chiavi e p
valori pivot; memoria O(c) per le c celle presenti e O(g · p) per l'uscita.
Nessuna variante spilled.

### Esempio

```json
{
  "config": {"index_col": "negozio", "pivot_col": "mese", "value_col": "vendite", "aggr_func": "sum"},
  "ingressi": [
    {"nome": "vendite", "colonne": [
      {"nome": "negozio", "tipo": "utf8", "valori": ["A", "A", "B", "A"]},
      {"nome": "mese", "tipo": "utf8", "valori": ["gen", "feb", "gen", "gen"]},
      {"nome": "vendite", "tipo": "float64", "valori": [10.0, 5.0, 7.0, 1.0]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "negozio", "tipo": "utf8", "valori": ["A", "B"]},
    {"nome": "feb", "tipo": "float64", "valori": [5.0, null]},
    {"nome": "gen", "tipo": "float64", "valori": [11.0, 7.0]}
  ]}
}
```
