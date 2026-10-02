### Che cosa fa

Tiene le righe di sinistra la cui chiave non compare a destra, e scarta le
altre. Dalla destra non si prende nessuna colonna. È il complemento di
[`table.semi_join`](#tablesemi_join): ogni riga di sinistra finisce in
esattamente una delle due uscite.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `left_keys` | lista di stringhe | obbligatorio | colonne della sinistra, almeno una, senza ripetizioni | colonne chiave del lato sinistro |
| `right_keys` | lista di stringhe | obbligatorio | colonne della destra, tante quante `left_keys`, senza ripetizioni | colonne chiave del lato destro, nello stesso ordine |

Tipi di chiave come in `table.semi_join`: stesso tipo Arrow nella coppia,
fra `utf8`, `int64`, `uint64`, `float64`, `bool`, `date32`, `date64` (allineato al giorno), `timestamp` di ogni unità, `decimal128` con scala da 0 a 38, `binary` e
`dictionary<utf8>`.

### Schema

Identico alla sinistra: stesse colonne, tipi, nullabilità, metadati e
colonna geometrica. Delle proprietà del contratto resta l'ordinamento
dichiarato (`sorted_by`) della sinistra; il conteggio delle righe non è più
noto.

### Righe

Filtro della sinistra: ogni riga al più una volta. Le chiavi si
confrontano come in [`table.join`](#tablejoin). Una riga sinistra con una
colonna chiave nulla non ha mai corrispondenza e quindi **resta** (come
`NOT EXISTS` in SQL, non come `NOT IN`); le righe destre con una chiave
nulla non contano.

### Ordine

Le righe tenute restano nell'ordine della sinistra.

### Errori

In validazione, `InvalidPlan`:

- config con campi sconosciuti, `left_keys` o `right_keys` assenti;
- liste di chiavi vuote, di lunghezza diversa, con nomi ripetuti o oltre
  `max_columns`; colonna assente; tipi diversi nella coppia; tipo fuori
  dall'elenco sopra.

In esecuzione, `Schema`: una cella chiave `date32`, `date64` o `timestamp` fuori
dall'intervallo delle date rappresentabili, un `date64` non allineato al
  giorno, o un dizionario malformato.

### Limiti e deviazioni

Nessuna conversione fra tipi di chiave, come in `table.join`. L'insieme
delle chiavi di destra usa un hash deterministico senza seme
([Limiti dichiarati, «Hash delle chiavi non keyed»](limiti.md#hash-delle-chiavi-non-keyed)).

### Complessità

Tempo O(n + m) atteso (insieme delle chiavi di destra, sonda della
sinistra, in parallelo da 65.536 righe sinistre); memoria O(m) per
l'insieme, più la copia delle righe tenute.

### Esempio

```json
{
  "config": {"left_keys": ["cliente"], "right_keys": ["codice"]},
  "ingressi": [
    {"nome": "ordini", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2, 3, 4]},
      {"nome": "cliente", "tipo": "utf8", "valori": ["a", "b", null, "a"]}
    ]},
    {"nome": "attivi", "colonne": [
      {"nome": "codice", "tipo": "utf8", "valori": ["a", "a", null]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [2, 3]},
    {"nome": "cliente", "tipo": "utf8", "valori": ["b", null]}
  ]}
}
```
