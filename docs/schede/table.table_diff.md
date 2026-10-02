### Che cosa fa

Confronta due versioni di una tabella, allineate su una chiave: la
sinistra è la versione vecchia, la destra la nuova. Ogni chiave riceve uno
stato: `ADDED` (solo a destra), `DELETED` (solo a sinistra), `MODIFIED`
(una colonna confrontata cambia) o `UNCHANGED`; per le righe modificate
elenca le colonne cambiate e i loro valori precedenti.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `left_keys` | lista di stringhe | obbligatorio | colonne della sinistra leggibili come testo, almeno una, senza ripetizioni | chiave a sinistra |
| `right_keys` | lista di stringhe | obbligatorio | colonne della destra, tante quante `left_keys`, dello stesso tipo coppia per coppia | chiave a destra |
| `compare_columns` | lista di stringhe | `[]` | colonne presenti in entrambe, dello stesso tipo, leggibili come testo | colonne confrontate; vuota vale le colonne non chiave della sinistra presenti anche a destra |
| `include_unchanged` | stringa | `"no"` | `"yes"`, `"no"` | emette anche le righe `UNCHANGED` |
| `separator` | stringa | `"#"` | al più `max_string_bytes` byte; `null` non ammesso; non con una sola colonna in `compare_columns` | separatore di `_diff_columns` e `_diff_old_values` |

Colonne leggibili come testo: i tipi di [`table.distinct`](#tabledistinct).
Due chiavi si abbinano con l'uguaglianza di `table.distinct`: un null è
uguale a un null. Due celle confrontate sono uguali se hanno lo stesso
valore (su `float64` per bit, con `-0.0` diverso da `0.0` e ogni NaN
uguale a ogni NaN; sugli altri tipi fuori da `int64`, `uint64`, `utf8`,
`bool`, per testo); un null è uguale solo a un null.

### Schema

Le colonne di `left_keys` (nomi della sinistra), poi quelle confrontate,
tutte nullabili e con il tipo d'ingresso; poi `_diff_status` (`utf8` non
nullabile), `_diff_columns` e `_diff_old_values` (`utf8` nullabili).
Nessun metadato di campo; i metadati di schema dei due lati si fondono, e
una chiave con valori diversi è un errore. Il contratto non dichiara né
ordinamento né conteggio.

I valori di chiavi e colonne confrontate vengono dalla destra quando la
riga c'è (`ADDED`, `MODIFIED`, `UNCHANGED`), dalla sinistra per `DELETED`.
`_diff_columns` e `_diff_old_values` sono non nulli solo per `MODIFIED`: i
nomi delle colonne cambiate nell'ordine di confronto e il testo dei loro
valori a sinistra, uniti da `separator`; un valore precedente nullo si
scrive come testo vuoto.

### Righe

Allineamento binario sulla chiave: una riga per chiave di uno dei due
lati, tranne le `UNCHANGED` con `include_unchanged: "no"`. Una chiave
ripetuta su un lato è un errore.

### Ordine

Prima le chiavi della sinistra, nel suo ordine; poi quelle solo a destra
(`ADDED`), nell'ordine della destra.

### Errori

In validazione, `InvalidPlan`:

- `left_keys` vuota o di lunghezza diversa da `right_keys`; una lista con
  un nome ripetuto o non valido, o oltre il limite di colonne;
- una colonna assente; una coppia di chiavi di tipi diversi; una chiave
  non leggibile come testo;
- una colonna confrontata assente da un lato, non leggibile come testo, o
  di tipi diversi fra i lati;
- `separator` oltre `max_string_bytes`, `null` esplicito (il parametro si
  omette), o scritto quando `compare_columns` elenca esattamente una
  colonna (non avrebbe effetto con nessun ingresso); con `compare_columns`
  vuota le colonne vengono dagli schemi e `separator` si accetta anche se
  se ne confronta una sola: dipende dall'ingresso;
- metadati di schema in conflitto; `include_unchanged` fuori da
  `"yes"`/`"no"`; campi sconosciuti.

In esecuzione:

- `InvalidPlan`: una chiave ripetuta nella sinistra o nella destra;
- `Schema`: una cella che non si converte in testo (date fuori
  intervallo, `binary` non UTF-8 fra i valori confrontati per testo);
- `ResourceLimit`: righe d'uscita oltre `max_rows`; colonne d'uscita oltre
  `max_columns`; più di `u32::MAX` righe; `_diff_columns` o
  `_diff_old_values` di una riga oltre `max_string_bytes` byte.

### Limiti e deviazioni

In `_diff_old_values` un valore precedente nullo e il testo vuoto si
scrivono allo stesso modo, e un `separator` che compare nei valori rende
il testo ambiguo. La mappa delle chiavi non è contabilizzata
([Limiti dichiarati, «Memoria delle chiavi dei kernel in memoria non governata»](limiti.md#memoria-delle-chiavi-dei-kernel-in-memoria-non-governata));
l'hash delle chiavi non ha seme
([Limiti dichiarati, «Hash delle chiavi non keyed»](limiti.md#hash-delle-chiavi-non-keyed)).

### Complessità

Tempo O(n + m) sulle righe dei due lati (una mappa delle chiavi) per il
numero di colonne confrontate; memoria O(n + m).

### Esempio

La chiave `1` è invariata e non esce; `30.0` si scrive `30` fra i valori
precedenti.

```json
{
  "config": {"left_keys": ["id"], "right_keys": ["id"]},
  "ingressi": [
    {"nome": "prima", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
      {"nome": "prezzo", "tipo": "float64", "valori": [10.0, 20.0, 30.0]}
    ]},
    {"nome": "dopo", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 3, 4]},
      {"nome": "prezzo", "tipo": "float64", "valori": [10.0, 35.0, 40.0]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [2, 3, 4]},
    {"nome": "prezzo", "tipo": "float64", "valori": [20.0, 35.0, 40.0]},
    {"nome": "_diff_status", "tipo": "utf8", "valori": ["DELETED", "MODIFIED", "ADDED"]},
    {"nome": "_diff_columns", "tipo": "utf8", "valori": [null, "prezzo", null]},
    {"nome": "_diff_old_values", "tipo": "utf8", "valori": [null, "30", null]}
  ]}
}
```
