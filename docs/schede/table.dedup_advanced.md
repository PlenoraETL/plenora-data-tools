### Che cosa fa

Come [`table.distinct`](#tabledistinct), ma prima ordina le righe per
`order_column`: «prima» e «ultima» occorrenza si riferiscono a
quell'ordine. Serve, per esempio, a tenere per ogni cliente la riga più
recente.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `subset` | lista di stringhe | obbligatorio | nomi di colonne leggibili come testo, almeno uno, senza ripetizioni | colonne della chiave |
| `keep` | stringa | `"first"` | `"first"`, `"last"` | tiene la prima o l'ultima occorrenza di ogni chiave |
| `order_column` | stringa | nessuno | nome di una colonna di tipo ordinabile | ordinamento stabile prima della deduplica |
| `ascending` | booleano | `true` | `true`, `false`; solo con `order_column`; `null` non ammesso | verso dell'ordinamento |

Colonne leggibili come testo e uguaglianza delle chiavi come in
[`table.distinct`](#tabledistinct); tipi ordinabili e confronto come in
[`table.sort`](#tablesort). `keep: "false"` si rifiuta, e `ascending`
scritto senza `order_column` si rifiuta invece di essere ignorato.

### Schema

Identico all'ingresso: colonne, tipi, nullabilità e metadati. Con
`order_column` il contratto dichiara l'uscita ordinata su quella colonna
nel verso chiesto; senza, conserva l'ordinamento dichiarato dell'ingresso.
Il conteggio delle righe non è più noto.

### Righe

Filtro: una riga per chiave distinta.

### Ordine

Con `order_column`, le righe tenute sono nell'ordine di
[`table.sort`](#tablesort) su quella colonna: stabile, null in coda in
ascendente e in testa in discendente. Senza, nell'ordine d'ingresso.

### Errori

In validazione, `InvalidPlan`:

- `keep: "false"`;
- `subset` vuoto, con un nome ripetuto o non valido, o oltre il limite di
  colonne; una sua colonna assente o non leggibile come testo;
- `ascending` senza `order_column`; `ascending` o `order_column` `null`
  espliciti (il parametro si omette);
- `order_column` assente o di tipo non ordinabile;
- campi sconosciuti.

In esecuzione:

- `Schema`: una cella della chiave che non si converte in testo (date
  fuori intervallo), una chiave di dizionario fuori dal proprio dizionario;
- `ResourceLimit`: più di `u32::MAX` righe.

### Limiti e deviazioni

La mappa delle chiavi non è contabilizzata
([README, «Memoria delle chiavi dei kernel in memoria non governata»](../README.md#memoria-delle-chiavi-dei-kernel-in-memoria-non-governata));
l'hash delle chiavi non ha seme
([README, «Hash delle chiavi non keyed»](../README.md#hash-delle-chiavi-non-keyed)).

### Complessità

Con `order_column`, tempo O(n log n) per l'ordinamento più O(n) per la
deduplica; senza, O(n). Memoria O(n) per la copia ordinata e O(k) per le k
chiavi distinte. Nessuna variante spilled.

### Esempio

La riga più recente di ogni cliente.

```json
{
  "config": {"subset": ["cliente"], "order_column": "data", "ascending": false},
  "ingressi": [
    {"nome": "ordini", "colonne": [
      {"nome": "cliente", "tipo": "utf8", "valori": ["a", "b", "a"]},
      {"nome": "data", "tipo": "date32", "valori": ["2024-01-10", "2024-02-01", "2024-03-05"]},
      {"nome": "importo", "tipo": "float64", "valori": [10.0, 20.0, 30.0]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "cliente", "tipo": "utf8", "valori": ["a", "b"]},
    {"nome": "data", "tipo": "date32", "valori": ["2024-03-05", "2024-02-01"]},
    {"nome": "importo", "tipo": "float64", "valori": [30.0, 20.0]}
  ]}
}
```
