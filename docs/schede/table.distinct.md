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
`bool`, `date32`, `date64`, `timestamp` di ogni unità (secondi, millisecondi,
microsecondi, nanosecondi; con timezone valida), `decimal128` con scala da
0 a 38, `binary`, `dictionary<utf8>` (chiavi `int32`). Un `timestamp` si
scrive in RFC 3339 con tutte le cifre frazionarie che servono: due istanti
distinti, anche di un solo nanosecondo, hanno testi distinti, e lo stesso
istante ha lo stesso testo in ogni unità; un istante il cui offset nel
fuso della colonna ha i secondi (ora media locale) non ha forma RFC 3339,
e dove serve il testo la cella si rifiuta. Un `date64` si scrive
`AAAA-MM-GG` se è allineato al giorno; altrimenti non è una data, e la
cella si rifiuta.

Uguaglianza delle chiavi, colonna per colonna: sul valore nella sua forma
in testo (un `timestamp` sul suo istante, non sul testo), quindi `-0.0` e
`0.0` sono diversi e ogni NaN è uguale a ogni
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

- `Schema`: una cella che non si converte in testo (`date32`, `date64` o
  `timestamp` fuori dall'intervallo delle date, `date64` non allineato al
  giorno);
- `ResourceLimit`: più di `u32::MAX` righe.

### Limiti e deviazioni

La mappa delle chiavi non è contabilizzata
([Limiti dichiarati, «Memoria delle chiavi dei kernel in memoria non governata»](limiti.md#memoria-delle-chiavi-dei-kernel-in-memoria-non-governata));
l'hash delle chiavi non ha seme
([Limiti dichiarati, «Hash delle chiavi non keyed»](limiti.md#hash-delle-chiavi-non-keyed)).

### Complessità

Tempo O(n) sulle righe (una passata con una mappa delle chiavi), più
l'ordinamento degli indici tenuti; memoria O(k) per le k chiavi distinte e
O(k) indici.

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
