### Che cosa fa

Divide i valori numerici di una colonna in classi e scrive, per ogni riga,
l'etichetta della sua classe in una colonna di testo. Le classi sono
intervalli chiusi a destra, `(a, b]`, dati come bordi espliciti oppure come
numero di classi di uguale ampiezza fra il minimo e il massimo dei dati.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa | obbligatorio | colonna numerica | colonna da classificare |
| `bins` | intero o lista di numeri | `5` | intero da 2 a 100, oppure da 3 a 101 bordi strettamente crescenti | numero di classi di uguale ampiezza, o bordi delle classi |
| `labels` | lista di stringhe | assente | tante quante le classi, ciascuna al più `max_string_bytes` byte | etichette delle classi, nell'ordine; assenti, `(a, b]` |
| `output_column` | stringa | `<column>_bin` | nome valido | colonna d'uscita |

Colonna numerica: `float64`, `int64`, `uint64`, `date32` (giorni
dall'epoca), `timestamp(ms)` (millisecondi dall'epoca), `decimal128`,
`utf8` il cui testo è un numero (spazi ai lati ignorati, virgola decimale
ammessa).

Con bordi espliciti `[e0, e1, …, ek]` la classe i è `(e_i, e_i+1]`, e la
prima comprende anche `e0`; un valore sotto `e0` o sopra `ek` non ha
classe. Con un numero k di classi i bordi si calcolano in `f64`: minimo e
massimo dei valori finiti, ampiezza `(max - min) / k`, ultimo bordo uguale
al massimo, primo bordo abbassato di `(max - min) · 0,001` (come
`pandas.cut`); con tutti i valori uguali i bordi vanno da `v - d` a `v + d`,
con `d = |v| · 0,001` (`0,001` se `v` è zero). In questo modo un valore
sotto il primo bordo o sopra l'ultimo (anche `±inf`) cade nella classe
esterna.

L'etichetta di default scrive i bordi con la resa decimale più corta di
`f64` (`(0, 18]`, `(2.5, 5]`).

### Schema

La colonna d'uscita è `utf8` nullable: si aggiunge in coda, o sostituisce
al suo posto una colonna con lo stesso nome (perdendone tipo e metadati di
campo). Metadati di schema conservati; `row_count` resta; `sorted_by` resta
solo se nessuna colonna esistente è sovrascritta.

### Righe

1:1. Una cella nulla, un `NaN` o un valore fuori dai bordi espliciti danno
null.

### Ordine

Invariato.

### Errori

In validazione, `InvalidPlan`:

- `column` assente o non numerica;
- numero di classi fuori da 2..=100, bordi fuori da 3..=101 o non
  strettamente crescenti;
- numero di `labels` diverso dal numero di classi; un'etichetta oltre
  `max_string_bytes` byte;
- config con campi sconosciuti.

In esecuzione, `Schema`:

- con un numero di classi, nessun valore finito nella colonna (anche una
  tabella vuota o tutta nulla);
- una cella `utf8` che non è un numero.

### Limiti e deviazioni

I bordi di uguale ampiezza sono calcolati in `f64`; la classe invece la
decide il confronto esatto del valore d'origine con il bordo, quindi un
`int64` oltre `2^53` o un `decimal128` non cadono nella classe accanto per
arrotondamento ([README, «Validazione»](../README.md#validazione)).

### Complessità

Tempo O(n log k) per n righe e k classi (ricerca binaria sui bordi,
scansione lineare se i bordi calcolati coincidono per arrotondamento);
memoria O(n) per i valori letti e la colonna d'uscita.

### Esempio

```json
{
  "config": {"column": "eta", "bins": [0, 18, 65, 120], "labels": ["minore", "adulto", "anziano"]},
  "ingressi": [
    {"nome": "persone", "colonne": [
      {"nome": "eta", "tipo": "int64", "valori": [0, 18, 70, 130]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "eta", "tipo": "int64", "valori": [0, 18, 70, 130]},
    {"nome": "eta_bin", "tipo": "utf8", "valori": ["minore", "minore", "anziano", null]}
  ]}
}
```
