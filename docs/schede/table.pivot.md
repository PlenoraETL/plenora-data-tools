### Che cosa fa

Porta la tabella da lunga a larga: una riga per ogni chiave delle colonne
`index_col`, una colonna per ogni valore distinto di `pivot_col`, e in ogni
cella l'aggregazione (`aggr_func`) dei valori di `value_col` delle righe
con quella chiave e quel valore. Con `mapping` le colonne d'uscita le
fissa la config e il runner esegue l'operazione; senza, dipendono dai dati:
il runner la rifiuta in validazione, e l'esempio sotto (senza `mapping`) è
eseguito chiamando il kernel.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `index_col` | stringa | obbligatorio | nomi di colonne separati da virgola, almeno uno, senza ripetizioni | chiave delle righe; spazi ai lati tolti, voci vuote ignorate |
| `pivot_col` | stringa | obbligatorio | nome di una colonna dell'ingresso | i suoi valori diventano colonne |
| `value_col` | stringa | obbligatorio | nome di una colonna dell'ingresso | valori aggregati nelle celle |
| `aggr_func` | stringa | `"first"` | `first`, `last`, `min`, `max`, `sum`, `mean`, `count`, `concat` | aggregazione di una cella |
| `mapping` | oggetto | `{}` | testo → nome di colonna | se non vuoto, fissa le colonne pivot: una per voce, con il nome della voce |

Il valore pivot è il testo della cella (`1.0` è `"1"`, `-0.0` è `"-0"`,
ogni NaN `"NaN"`, le date `AAAA-MM-GG`); le righe con il valore pivot
nullo, o escluso da `mapping`, non riempiono celle ma la loro chiave dà
comunque una riga. Con `mapping` il valore si confronta con le chiavi come
testo: la `pivot_col` deve essere `utf8`, dizionario di `utf8`, `int64` o
`uint64`, e con un intero ogni chiave deve essere la forma canonica di un
intero (`"1"`, non `"01"` né `"1.0"`), perché una chiave che nessun valore
può incontrare darebbe una colonna tutta null senza errore. Le celle, dalle
righe della cella in ordine d'ingresso:

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
metadati di campo; poi le colonne pivot, nullabili e senza metadati di
campo, con il tipo sopra:

- senza `mapping`, una per valore pivot distinto dei dati, nell'ordine dei
  byte del testo del valore;
- con `mapping`, una per voce, nell'ordine delle chiavi (byte per byte) e
  con il nome della voce, anche per un valore che i dati non contengono
  (colonna tutta null); i valori fuori dal mapping non danno colonne.

I metadati di schema si conservano.

### Righe

Aggregazione: una riga per chiave distinta di `index_col`, con
l'uguaglianza di [`table.distinct`](#tabledistinct).

### Ordine

Le righe nell'ordine delle chiavi di [`table.aggregate`](#tableaggregate):
null per primo, poi per la stringa `<lunghezza>:<testo>` byte per byte.

### Errori

In validazione, in quest'ordine:

1. `InvalidPlan`: campi sconosciuti; una colonna assente fra quelle di
   `index_col`, la `pivot_col` e la `value_col`. Questi due controlli
   valgono anche senza `mapping`;
2. `Unsupported`: `mapping` assente o vuoto (lo schema d'uscita dipende
   dai dati). Senza `mapping` la validazione si ferma qui: `index_col`
   senza colonne o con una colonna ripetuta, e ogni controllo di tipo
   sotto, danno `Unsupported`, non `InvalidPlan` (il kernel, chiamato
   direttamente, li rifiuta con `InvalidPlan` o `Schema`);
3. solo con `mapping` non vuoto:
   - `InvalidPlan`: `index_col` senza colonne, con una colonna ripetuta o
     oltre `max_columns`; una colonna indice o la `pivot_col` che non si
     legge come testo; con `sum`, `mean`, `min`, `max` una `value_col`
     non numerica, con `concat` una che non si legge come testo; un nome
     del mapping non valido, ripetuto o uguale a una colonna indice; una
     chiave che non è la forma canonica di un intero su una `pivot_col`
     intera; `mapping` su una `pivot_col` che non è testo né intero
     (float, date, istanti, decimali, booleani);
   - `ResourceLimit`: colonne indice più voci del mapping oltre
     `max_columns`.

In esecuzione (dal runner con `mapping`, o chiamando il kernel):

- `Schema`: una colonna assente; con `sum`, `mean`, `min`, `max`, una
  `value_col` di tipo non numerico o una sua cella `utf8` che non è un
  numero; una cella di chiave, di valore pivot o (con `concat`) di valore
  che non si converte in testo (tipo non leggibile come testo, `binary`
  non UTF-8 fra i valori pivot, date fuori intervallo);
- `InvalidPlan`: le regole di `Pivot::verifica_mapping`, le stesse della
  validazione (`index_col`, nomi e chiavi del mapping); senza `mapping`, un
  valore pivot vuoto o di soli spazi, o uguale a una colonna indice;
- `ResourceLimit`: righe oltre `max_rows` o colonne oltre `max_columns`;
  più di `u32::MAX` righe.

### Limiti e deviazioni

Senza `mapping` il runner non esegue l'operazione (lo schema dipende dai
dati); `table.transpose` ha lo stesso limite. `sum` e `mean` arrotondano
un intero oltre `2^53` o un `decimal128`, perché il risultato è `float64`.
L'hash delle chiavi non ha seme
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
