### Che cosa fa

Calcola statistiche descrittive (conteggio, minimo, massimo, somma, media,
mediana, deviazione standard, varianza, quartili) dei valori di una colonna
numerica, sull'intera tabella o per gruppi, e le scrive in colonne nuove
ripetute su ogni riga del gruppo. Le righe non si aggregano.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa | obbligatorio | colonna numerica | colonna dei valori |
| `group_by` | stringa | assente | colonna leggibile come testo | colonna dei gruppi; assente, un gruppo solo |
| `stats` | lista di stringhe | `["count", "min", "max", "mean", "median", "std"]` | `count`, `min`, `max`, `sum`, `mean`, `median`, `std`, `var`, `q25`, `q75` | statistiche, una colonna ciascuna nell'ordine scritto |
| `output_prefix` | stringa | `""`, cioè `<column>_` | qualsiasi | prefisso dei nomi: la colonna di `mean` è `<output_prefix>mean` |

Colonna numerica: `float64`, `int64`, `uint64`, `date32` (giorni
dall'epoca), `timestamp(ms)` (millisecondi dall'epoca), `decimal128`,
`utf8` il cui testo è un numero (spazi ai lati ignorati, virgola decimale
ammessa). Leggibile come testo: `utf8`, `int64`, `uint64`, `float64`,
`bool`, `date32`, `timestamp(ms)`, `decimal128` con scala da 0 a 38,
`binary`, `dictionary<utf8>` con chiavi `int32`.

Le statistiche, sui valori non nulli del gruppo:

- `count`: quanti sono;
- `min`, `max`, `median`, `q25`, `q75`: sui valori ordinati; i quantili
  interpolano linearmente fra i due valori vicini (posizione `q · (c - 1)`);
- `sum`, `mean`: somma in `f64` nell'ordine delle righe, e somma diviso
  `count`;
- `var`, `std`: varianza campionaria (divisore `count - 1`) e sua radice;
  null con meno di due valori.

I gruppi si formano sul testo della cella di `group_by`; le celle nulle
formano un gruppo. Una statistica ripetuta in `stats` scrive due volte la
stessa colonna.

### Schema

Una colonna `float64` nullable per statistica, nell'ordine di `stats`, in
coda; una colonna con lo stesso nome di una esistente la sostituisce al suo
posto (perdendone tipo e metadati di campo). Le colonne d'ingresso restano.
Metadati di schema conservati; `row_count` resta; `sorted_by` resta solo se
nessuna colonna esistente è sovrascritta.

### Righe

1:1: ogni riga riceve le statistiche del proprio gruppo. Un gruppo senza
valori non nulli ha tutte le statistiche nulle, `count` compreso.

### Ordine

Invariato.

### Errori

In validazione, `InvalidPlan`:

- `column` assente o non numerica;
- `group_by` assente o non leggibile come testo;
- una voce di `stats` fuori elenco, o config con campi sconosciuti.

In esecuzione, `Schema`: una cella `utf8` di `column` che non è un numero;
una cella di `group_by` che non si converte in testo.

### Limiti e deviazioni

Il risultato è `float64` per contratto: `int64` e `uint64` oltre `2^53` e i
`decimal128` si convertono arrotondando, e la somma accumula gli errori di
arrotondamento di `f64` nell'ordine delle righe. Un `NaN` nei valori entra
nei calcoli: somma, media e varianza diventano `NaN`, e nell'ordinamento di
minimo, massimo e quantili sta dopo ogni numero.

### Complessità

Tempo O(n) per somme e varianza più O(n log n) per l'ordinamento quando
servono minimo, massimo o quantili; memoria O(n) per i valori raggruppati e
le colonne d'uscita.

### Esempio

```json
{
  "config": {"column": "importo", "group_by": "regione", "stats": ["count", "mean", "max"]},
  "ingressi": [
    {"nome": "vendite", "colonne": [
      {"nome": "regione", "tipo": "utf8", "valori": ["nord", "nord", "sud", "sud"]},
      {"nome": "importo", "tipo": "float64", "valori": [10.0, 20.0, 5.0, null]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "regione", "tipo": "utf8", "valori": ["nord", "nord", "sud", "sud"]},
    {"nome": "importo", "tipo": "float64", "valori": [10.0, 20.0, 5.0, null]},
    {"nome": "importo_count", "tipo": "float64", "valori": [2.0, 2.0, 1.0, 1.0]},
    {"nome": "importo_mean", "tipo": "float64", "valori": [15.0, 15.0, 5.0, 5.0]},
    {"nome": "importo_max", "tipo": "float64", "valori": [20.0, 20.0, 5.0, 5.0]}
  ]}
}
```
