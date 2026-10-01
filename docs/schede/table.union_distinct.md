### Che cosa fa

Unione insiemistica di due tabelle con lo stesso schema (`UNION` di SQL):
le righe distinte che compaiono in almeno uno dei due ingressi, ognuna una
volta. Due righe sono uguali se lo sono tutte le loro colonne.

### Parametri

Nessuno: la config è `{}`.

Ogni colonna ha un tipo fra `utf8`, `int64`, `uint64`, `float64`, `bool`,
`date32`, `date64`, `timestamp` di ogni unità, `decimal128`, `binary` e
`dictionary<utf8>`
(chiave int32).

### Schema

Le colonne della sinistra, con nome, tipo e metadati di campo della
sinistra; una colonna è nullable se lo è in almeno un ingresso. I metadati
di schema dei due ingressi si fondono: una chiave presente in un ingresso
solo, o con lo stesso valore, resta. La colonna geometrica è quella della
sinistra. Nessuna proprietà del contratto sopravvive.

### Righe

Una riga per ogni riga distinta dei due ingressi, presa dalla sua prima
comparsa (prima a sinistra, poi a destra). L'uguaglianza è per valore,
colonna per colonna: null è uguale a null, tutti i NaN sono uguali fra
loro, `-0.0` è diverso da `0.0`, un `dictionary` vale il testo della sua
voce (una voce nulla è null), un `timestamp` o un `date64` il suo valore
nella sua unità (i due lati hanno lo stesso tipo: la stessa unità).

### Ordine

Le righe di sinistra tenute, nell'ordine di sinistra, poi quelle di destra
che non comparivano prima, nell'ordine di destra.

### Errori

In validazione, `InvalidPlan`:

- numero di colonne diverso, o nome o tipo diversi in una posizione (la
  nullabilità non conta);
- una colonna di tipo fuori dall'elenco sopra;
- metadati di schema con la stessa chiave e valori diversi;
- config non vuota.

In esecuzione, `ResourceLimit`: righe dei due ingressi insieme oltre
`max_rows` (nel runner `max_input_rows`), anche se le righe distinte sono
meno. Nella variante spilled anche: file temporanei oltre `max_temp_bytes`
(una sola chiave di riga oltre la quota compresa), chiavi distinte di una
partizione oltre `max_governed_memory_bytes` (lunghezza della chiave più 64
byte per chiave); `Io` sui file temporanei.

### Limiti e deviazioni

Il runner passa alla variante spilled quando il passo in memoria non sta
nel budget ([README, «Budget di memoria»](../README.md#budget-di-memoria)):
le chiavi di riga vanno su file temporanei in `spill_partitions`
partizioni e le righe tenute si riselezionano dagli ingressi; l'uscita è
la stessa del percorso in memoria, e resta intera in memoria. In memoria le
chiavi non si contano su `max_governed_memory_bytes`
([README, «Memoria delle chiavi dei kernel in memoria non governata»](../README.md#memoria-delle-chiavi-dei-kernel-in-memoria-non-governata)),
e l'insieme usa un hash deterministico senza seme
([README, «Hash delle chiavi non keyed»](../README.md#hash-delle-chiavi-non-keyed)).

### Complessità

Tempo O(n + m) atteso; memoria O(byte delle chiavi distinte) più la copia
delle righe tenute. Spilled: tempo O(n + m) più la scrittura e la lettura
delle chiavi, memoria di lavoro O(chiavi distinte di una partizione).

### Esempio

```json
{
  "config": {},
  "ingressi": [
    {"nome": "a", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2, 1, null]},
      {"nome": "tag", "tipo": "utf8", "valori": ["x", "y", "x", "z"]}
    ]},
    {"nome": "b", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [2, 3, null]},
      {"nome": "tag", "tipo": "utf8", "valori": ["y", "x", "z"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2, null, 3]},
    {"nome": "tag", "tipo": "utf8", "valori": ["x", "y", "z", "x"]}
  ]}
}
```
