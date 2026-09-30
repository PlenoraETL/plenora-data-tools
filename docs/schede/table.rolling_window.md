### Che cosa fa

Calcola per ogni riga un'aggregazione (somma, media, minimo, massimo,
deviazione standard) della colonna `column` sulle ultime `window` righe
della sua partizione, riga corrente compresa, e la aggiunge come colonna
(`float64`, o del tipo detto sotto). Con `order_column` le righe si
riordinano prima su quella colonna.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa | obbligatorio | colonna numerica (come in [`table.window_function`](#tablewindow_function)) | colonna aggregata |
| `function` | stringa | obbligatorio | `sum`, `mean`, `min`, `max`, `stddev` | aggregazione |
| `window` | intero | obbligatorio | da `1` a `max_rows` | righe della finestra, corrente compresa |
| `min_periods` | intero | `1` | da `1` a `window` | valori non nulli minimi per un risultato |
| `group_by` | stringa | nessuno | colonna leggibile come testo | partizione; senza, una partizione sola |
| `order_column` | stringa | nessuno | colonna di tipo ordinabile | ordinamento ascendente prima del calcolo |
| `ddof` | intero | `1` | `0` o più; solo con `stddev`; `null` non ammesso | gradi di libertà sottratti al divisore |
| `output_column` | stringa | obbligatorio | nome di colonna valido | colonna d'uscita |

La finestra si misura in righe, non in valori: una cella nulla occupa il
suo posto e non conta fra i valori. Con meno di `min_periods` valori non
nulli nella finestra il risultato è null. Sulle colonne intere (`int64`,
`uint64`) `sum` è esatta ed esce `int64` (una somma oltre `int64` è un
errore); `sum` su `date32` o `timestamp` si rifiuta in validazione. Su
interi, date e istanti `mean` parte dalla somma esatta e `stddev` dagli
scarti esatti; altrove `sum` somma in `f64` in ordine di riga. `mean` è la somma
divisa per i valori. `min` e `max` sulle colonne intere e `decimal128`
rendono la cella estrema nel tipo della colonna; altrove ignorano i NaN
salvo che la finestra abbia solo NaN, `sum` e `mean` no. `stddev` divide
per `valori - ddof` e dà null con `valori <= ddof`.

### Schema

L'ingresso più la colonna `output_column`, nullabile, senza metadati di
campo, in coda: `int64` con `sum` su una colonna intera, il tipo di
`column` con `min`/`max` su una colonna intera o `decimal128`, `float64`
altrimenti; se il nome esiste già, la colonna è sostituita
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
- `ddof` con una funzione diversa da `stddev`; `ddof`, `group_by` o
  `order_column` `null` espliciti (il parametro si omette);
- `column` assente o non numerica; `group_by` non leggibile come testo;
  `order_column` di tipo non ordinabile; `output_column` non valido;
- funzione fuori elenco, campi sconosciuti.

In esecuzione:

- `Schema`: una cella `utf8` di `column` che non è un numero; una cella di
  `group_by` che non si converte in testo; una chiave di dizionario di
  `order_column` fuori dal proprio dizionario;
- `DataMapping`: un risultato di `sum`, `mean` o `stddev` non finito
  calcolato da valori finiti (overflow di `f64`); con `sum` su una colonna
  intera, una somma oltre la gamma di `int64`;
- `ResourceLimit`: più di `u32::MAX` righe con `order_column`.

### Limiti e deviazioni

Su `decimal128` e testo numerico `sum`, `mean` e `stddev` leggono la cella
come `f64` e arrotondano senza errore, perché il risultato è `float64`;
sulle colonne intere `mean` e `stddev` arrotondano alla fine del calcolo
esatto
([README, «Somme intere esatte e tipi delle riduzioni»](../README.md#somme-intere-esatte-e-tipi-delle-riduzioni)).
Un `NaN` o un infinito già nei dati si propagano senza errore; solo
l'overflow di un calcolo su valori finiti si rifiuta.

### Complessità

Tempo O(n · window): ogni riga riscorre la propria finestra (due volte per
`stddev`), più O(n log n) per l'ordinamento su `order_column`; memoria
O(n). Da 32.768 righe, con più di una partizione, le partizioni si
calcolano in parallelo, con lo stesso risultato. Nessuna variante spilled.

### Esempio

Somma mobile su due righe: il null occupa il suo posto ma non conta, e
la somma di una colonna intera resta `int64`, esatta.

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
    {"nome": "somma_2", "tipo": "int64", "valori": [1, 3, 2, 4]}
  ]}
}
```
