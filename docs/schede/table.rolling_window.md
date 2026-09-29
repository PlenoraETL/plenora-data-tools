### Che cosa fa

Calcola per ogni riga un'aggregazione (somma, media, minimo, massimo,
deviazione standard) della colonna `column` sulle ultime `window` righe
della sua partizione, riga corrente compresa, e la aggiunge come colonna
`float64`. Con `order_column` le righe si riordinano prima su quella
colonna.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa | obbligatorio | colonna numerica (come in [`table.window_function`](#tablewindow_function)) | colonna aggregata |
| `function` | stringa | obbligatorio | `sum`, `mean`, `min`, `max`, `stddev` | aggregazione |
| `window` | intero | obbligatorio | da `1` a `max_rows` | righe della finestra, corrente compresa |
| `min_periods` | intero | `1` | da `1` a `window` | valori non nulli minimi per un risultato |
| `group_by` | stringa | nessuno | colonna leggibile come testo | partizione; senza, una partizione sola |
| `order_column` | stringa | nessuno | colonna di tipo ordinabile | ordinamento ascendente prima del calcolo |
| `ddof` | intero | `1` | `0` o più; solo con `stddev` | gradi di libertà sottratti al divisore |
| `output_column` | stringa | obbligatorio | nome di colonna valido | colonna d'uscita |

La finestra si misura in righe, non in valori: una cella nulla occupa il
suo posto e non conta fra i valori. Con meno di `min_periods` valori non
nulli nella finestra il risultato è null. `sum` somma in ordine di riga;
`mean` è la somma divisa per i valori; `min` e `max` ignorano i NaN salvo
che la finestra abbia solo NaN, `sum` e `mean` no; `stddev` divide per
`valori - ddof` e dà null con `valori <= ddof`.

### Schema

L'ingresso più la colonna `output_column`, `float64` nullabile, senza
metadati di campo, in coda; se il nome esiste già, la colonna è sostituita
al suo posto. Gli altri metadati si conservano. Il contratto dichiara
l'uscita ordinata in ascendente su `order_column` se c'è (altrimenti
conserva l'ordinamento dell'ingresso) e conserva il conteggio.

### Righe

1:1.

### Ordine

Come [`table.window_function`](#tablewindow_function): con `order_column`,
le righe escono ordinate su quella colonna in ascendente (stabile, null in
coda), senza raggrupparle per partizione; senza, nell'ordine d'ingresso.
Le partizioni si formano sul testo di `group_by` (il null è una partizione
a sé), e la finestra scorre le loro righe in quest'ordine.

### Errori

In validazione, `InvalidPlan`:

- `window` o `min_periods` uguali a 0, `min_periods` maggiore di `window`,
  `window` oltre `max_rows`;
- `ddof` con una funzione diversa da `stddev`;
- `column` assente o non numerica; `group_by` non leggibile come testo;
  `order_column` di tipo non ordinabile; `output_column` non valido;
- funzione fuori elenco, campi sconosciuti.

In esecuzione:

- `Schema`: una cella `utf8` di `column` che non è un numero; una cella di
  `group_by` che non si converte in testo; una chiave di dizionario di
  `order_column` fuori dal proprio dizionario;
- `ResourceLimit`: più di `u32::MAX` righe con `order_column`.

### Limiti e deviazioni

La cella si legge come `f64`: un intero oltre `2^53` o un `decimal128` si
arrotondano senza errore, perché il risultato è `float64`.

### Complessità

Tempo O(n · window): ogni riga riscorre la propria finestra (due volte per
`stddev`), più O(n log n) per l'ordinamento su `order_column`; memoria
O(n). Da 32.768 righe, con più di una partizione, le partizioni si
calcolano in parallelo, con lo stesso risultato. Nessuna variante spilled.

### Esempio

Somma mobile su due righe: il null occupa il suo posto ma non conta.

```json
{
  "config": {"column": "vendite", "function": "sum", "window": 2, "output_column": "somma_2"},
  "ingressi": [
    {"nome": "giorni", "colonne": [
      {"nome": "giorno", "tipo": "int64", "valori": [1, 2, 3, 4]},
      {"nome": "vendite", "tipo": "int64", "valori": [1, 2, null, 4]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "giorno", "tipo": "int64", "valori": [1, 2, 3, 4]},
    {"nome": "vendite", "tipo": "int64", "valori": [1, 2, null, 4]},
    {"nome": "somma_2", "tipo": "float64", "valori": [1.0, 3.0, 2.0, 4.0]}
  ]}
}
```
