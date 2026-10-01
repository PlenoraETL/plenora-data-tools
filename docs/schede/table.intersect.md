### Che cosa fa

Intersezione insiemistica di due tabelle con lo stesso schema (`INTERSECT`
di SQL): le righe distinte di sinistra che compaiono anche a destra,
ognuna una volta. Due righe sono uguali se lo sono tutte le loro colonne.

### Parametri

Nessuno: la config è `{}`.

Ogni colonna ha un tipo fra `utf8`, `int64`, `uint64`, `float64`, `bool`,
`date32`, `date64`, `timestamp` di ogni unità, `decimal128`, `binary` e
`dictionary<utf8>`
(chiave int32).

### Schema

Identico alla sinistra: stesse colonne, tipi, nullabilità, metadati e
colonna geometrica. Nessuna proprietà del contratto sopravvive, nemmeno
l'ordinamento dichiarato, benché l'ordine sia quello della sinistra.

### Righe

Una riga per ogni riga distinta di sinistra che compare a destra, presa
dalla sua prima comparsa a sinistra. L'uguaglianza è quella di
[`table.union_distinct`](#tableunion_distinct): per valore, colonna per
colonna, null uguale a null, NaN uguali fra loro, `-0.0` diverso da `0.0`.

### Ordine

Quello della sinistra.

### Errori

In validazione, `InvalidPlan`:

- numero di colonne diverso, o nome o tipo diversi in una posizione (la
  nullabilità non conta);
- una colonna di tipo fuori dall'elenco sopra;
- config non vuota.

In esecuzione, `ResourceLimit`: più di `u32::MAX` righe tenute.

### Limiti e deviazioni

Le chiavi non si contano su `max_governed_memory_bytes`
([README, «Memoria delle chiavi dei kernel in memoria non governata»](../README.md#memoria-delle-chiavi-dei-kernel-in-memoria-non-governata)),
e l'insieme usa un hash deterministico senza seme
([README, «Hash delle chiavi non keyed»](../README.md#hash-delle-chiavi-non-keyed)).

### Complessità

Tempo O(n + m) atteso; memoria O(byte delle chiavi distinte di destra) più
la copia delle righe tenute.

### Esempio

```json
{
  "config": {},
  "ingressi": [
    {"nome": "a", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [3, 1, 2, 1, null]}
    ]},
    {"nome": "b", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, null, 4]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, null]}
  ]}
}
```
