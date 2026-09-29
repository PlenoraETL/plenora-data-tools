### Che cosa fa

Toglie le righe ripetute: due righe sono uguali se hanno gli stessi valori
nelle colonne di `subset` (tutte le colonne se `subset` è vuoto). Di ogni
gruppo di righe uguali tiene la prima, l'ultima, oppure nessuna.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `subset` | lista di stringhe | `[]` | nomi di colonne leggibili come testo, senza ripetizioni | colonne della chiave; vuoto vale tutte le colonne |
| `keep` | stringa | `"first"` | `"first"`, `"last"`, `"false"` | tiene la prima occorrenza, l'ultima, o solo le righe la cui chiave compare una volta |

Colonne leggibili come testo: `utf8`, `int64`, `uint64`, `float64`,
`bool`, `date32`, `timestamp(ms)` (con timezone valida), `decimal128` con
scala da 0 a 38, `binary`, `dictionary<utf8>` (chiavi `int32`).

Uguaglianza delle chiavi, colonna per colonna: sul valore nella sua forma
in testo, quindi `-0.0` e `0.0` sono diversi e ogni NaN è uguale a ogni
altro NaN; `binary` sui byte; un null è uguale a un null e diverso da ogni
valore, anche dal testo vuoto; la voce nulla di un dizionario è un null.

### Schema

Identico all'ingresso: colonne, tipi, nullabilità e metadati. Delle
proprietà del contratto resta solo l'ordinamento dichiarato: il conteggio
delle righe non è più noto.

### Righe

Filtro: una riga per chiave distinta con `"first"` e `"last"`; con
`"false"` solo le righe la cui chiave compare una volta sola.

### Ordine

Le righe tenute restano nell'ordine d'ingresso, per ogni valore di `keep`.

### Errori

In validazione, `InvalidPlan`:

- `subset` con un nome ripetuto o non valido, o oltre il limite di colonne;
- una colonna di `subset` assente o non leggibile come testo; con `subset`
  vuoto, una colonna qualsiasi dell'ingresso non leggibile come testo;
- `keep` fuori elenco, campi sconosciuti.

In esecuzione:

- `Schema`: una cella che non si converte in testo (`date32` o
  `timestamp` fuori dall'intervallo delle date);
- `ResourceLimit`: più di `u32::MAX` righe; nella variante spilled, file
  temporanei oltre `max_temp_bytes` o mappa delle chiavi oltre
  `max_governed_memory_bytes`;
- `InvalidPlan`: nella variante spilled, un ingresso con una colonna di
  nome `__plenora_spill_ordinal`, riservata allo spill;
- `Io`: nella variante spilled, un errore sui file temporanei.

### Limiti e deviazioni

La mappa delle chiavi non è contabilizzata nel percorso in memoria
([README, «Memoria delle chiavi dei kernel in memoria non governata»](../README.md#memoria-delle-chiavi-dei-kernel-in-memoria-non-governata));
l'hash delle chiavi non ha seme
([README, «Hash delle chiavi non keyed»](../README.md#hash-delle-chiavi-non-keyed)).
La variante spilled rifiuta un ingresso con una colonna
`__plenora_spill_ordinal`, che quella in memoria accetta.

### Complessità

Tempo O(n) sulle righe (una passata con una mappa delle chiavi), più
l'ordinamento degli indici tenuti; memoria O(k) per le k chiavi distinte e
O(k) indici.

La variante spilled la sceglie il runner quando quella in memoria non sta
nel budget ([README, «Budget di memoria»](../README.md#budget-di-memoria)):
le righe, con il loro indice originale, si dividono per hash della chiave
in `spill_partitions` file Arrow IPC temporanei; una lettura in streaming
accumula per ogni chiave prima e ultima occorrenza e conteggio, in una
mappa globale contata su `max_governed_memory_bytes` (lunghezza della
chiave più 64 byte per chiave). L'uscita è identica a quella in memoria.

### Esempio

```json
{
  "config": {"subset": ["cliente"], "keep": "last"},
  "ingressi": [
    {"nome": "ordini", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2, 3, 4]},
      {"nome": "cliente", "tipo": "utf8", "valori": ["a", "b", "a", null]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [2, 3, 4]},
    {"nome": "cliente", "tipo": "utf8", "valori": ["b", "a", null]}
  ]}
}
```
