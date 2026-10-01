### Che cosa fa

Differenza insiemistica di due tabelle con lo stesso schema (`EXCEPT` di
SQL): le righe distinte di sinistra che non compaiono a destra, ognuna una
volta. Due righe sono uguali se lo sono tutte le loro colonne.

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

Una riga per ogni riga distinta di sinistra che non compare a destra, presa
dalla sua prima comparsa a sinistra: anche i duplicati interni alla
sinistra si riducono a una riga. L'uguaglianza è quella di
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

In esecuzione, solo nella variante spilled: `ResourceLimit` per file
temporanei oltre `max_temp_bytes`, o chiavi distinte di destra, o righe
tenute distinte, di una partizione oltre `max_governed_memory_bytes`
(lunghezza della chiave più 64 byte per chiave, ciascuna delle due
contabilità); `Io` sui file temporanei.

### Limiti e deviazioni

Il runner passa alla variante spilled quando il passo in memoria non sta
nel budget ([README, «Budget di memoria»](../README.md#budget-di-memoria)),
con la stessa uscita del percorso in memoria. In memoria le chiavi non si
contano su `max_governed_memory_bytes`
([README, «Memoria delle chiavi dei kernel in memoria non governata»](../README.md#memoria-delle-chiavi-dei-kernel-in-memoria-non-governata)),
e gli insiemi usano un hash deterministico senza seme
([README, «Hash delle chiavi non keyed»](../README.md#hash-delle-chiavi-non-keyed)).

### Complessità

Tempo O(n + m) atteso; memoria O(byte delle chiavi distinte dei due lati)
più la copia delle righe tenute. Spilled: memoria di lavoro O(chiavi
distinte di una partizione).

### Esempio

```json
{
  "config": {},
  "ingressi": [
    {"nome": "a", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [3, 1, 2, 3, null]}
    ]},
    {"nome": "b", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, null, 4]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [3, 2]}
  ]}
}
```
