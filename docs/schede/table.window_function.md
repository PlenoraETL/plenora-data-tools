### Che cosa fa

Calcola per ogni riga una funzione finestra sulla colonna `column` —
rango, somma cumulata, valore precedente o successivo, variazione
percentuale, quantile di posizione — dentro la sua partizione
(`group_by`), e la aggiunge come colonna `float64`. Con `order_column` le
righe si riordinano prima su quella colonna.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa | obbligatorio | colonna numerica (sotto) | colonna su cui si calcola |
| `function` | stringa | `"rank"` | `rank`, `dense_rank`, `percent_rank`, `cume_dist`, `cumsum`, `running_mean`, `cumcount`, `lag`, `lead`, `pct_change`, `ntile` | funzione |
| `group_by` | stringa | nessuno | colonna leggibile come testo | partizione; senza, una partizione sola |
| `order_column` | stringa | nessuno | colonna di tipo ordinabile | ordinamento ascendente prima del calcolo |
| `offset` | intero | `1` | da `1`; solo con `lag` e `lead`; `null` non ammesso | distanza in righe |
| `buckets` | intero | nessuno | da `1` a `max_rows`; obbligatorio con `ntile`, solo con `ntile`; `null` non ammesso | numero di gruppi di `ntile` |
| `output_column` | stringa | `<column>_<function>` | nome di colonna valido | colonna d'uscita |

Colonne numeriche: `int64`, `uint64`, `float64`, `decimal128`, `date32`,
`timestamp(ms)` e `utf8` il cui testo è un numero; le funzioni di rango
(`rank`, `dense_rank`, `percent_rank`, `cume_dist`) non accettano `utf8`.
`group_by` legge i tipi di [`table.distinct`](#tabledistinct);
`order_column` quelli di [`table.sort`](#tablesort).

Funzioni, per partizione, con le righe nell'ordine descritto sotto:

- `rank`: posizione del valore della cella fra i valori non nulli della
  partizione in ordine crescente (non la posizione della riga), da 1; a
  pari merito la media delle posizioni (due valori in testa: `1.5`);
- `dense_rank`: 1, 2, 3… sui valori distinti;
- `percent_rank`: valori minori diviso (valori non nulli − 1), `0` con un
  solo valore;
- `cume_dist`: valori minori o uguali diviso valori non nulli;
- `cumsum`, `running_mean`: somma e media dei valori non nulli fin qui;
- `cumcount`: posizione della riga nella partizione, da 0;
- `lag`, `lead`: il valore `offset` righe prima o dopo nella partizione;
- `pct_change`: `(corrente − precedente) / precedente` sulla riga subito
  prima (`offset` non si usa);
- `ntile`: `posizione * min(buckets, righe) / righe + 1` in divisione
  intera, con la posizione da 0.

Le funzioni di rango confrontano il valore nativo con l'ordine di
[`table.sort`](#tablesort) (su `float64`, `-0.0` prima di `0.0`); le altre
leggono la cella come `f64`.

### Schema

L'ingresso più la colonna `output_column`, `float64` nullabile, senza
metadati di campo, in coda; se il nome esiste già, la colonna è sostituita
al suo posto. Gli altri metadati si conservano. Il contratto dichiara
l'uscita ordinata in ascendente su `order_column` se c'è (altrimenti
conserva l'ordinamento dell'ingresso) e conserva il conteggio.

### Righe

1:1. Null nell'uscita: sulla riga di un valore nullo per rango, `cumsum`,
`running_mean` e `pct_change`; per `lag`/`lead` quando la riga a distanza
`offset` non c'è o è nulla; per `pct_change` anche senza riga precedente,
con il precedente nullo o uguale a zero. `cumcount` e `ntile` contano
tutte le righe, null compresi.

### Ordine

Con `order_column`, le righe escono ordinate su quella colonna come
[`table.sort`](#tablesort) in ascendente (stabile, null in coda), senza
raggrupparle per partizione; senza, nell'ordine d'ingresso. Le partizioni
si formano sul testo di `group_by` (il null è una partizione a sé) e le
loro righe sono in quest'ordine.

### Errori

In validazione, `InvalidPlan`:

- `offset` uguale a 0, o scritto per una funzione diversa da `lag`/`lead`;
- `ntile` senza `buckets` positivo; `buckets` oltre `max_rows` o scritto
  per un'altra funzione;
- `column` assente o non numerica; una funzione di rango su `utf8`;
- `group_by` non leggibile come testo; `order_column` di tipo non
  ordinabile; un nome d'uscita non valido;
- `offset` o `buckets` `null` espliciti (un parametro facoltativo si
  omette);
- funzione fuori elenco, campi sconosciuti.

In esecuzione:

- `Schema`: una cella `utf8` di `column` che non è un numero (anche per
  `cumcount` e `ntile`, che non ne usano il valore); una cella di
  `group_by` che non si converte in testo; una chiave di dizionario di
  `order_column` fuori dal proprio dizionario;
- `DataMapping`: un risultato di `cumsum`, `running_mean` o `pct_change`
  non finito calcolato da valori finiti (overflow di `f64`);
- `ResourceLimit`: più di `u32::MAX` righe con `order_column`.

### Limiti e deviazioni

Le funzioni che rendono un valore (`cumsum`, `running_mean`, `lag`,
`lead`, `pct_change`) leggono la cella come `f64`: un intero oltre `2^53`
o un `decimal128` si arrotondano senza errore. Un `NaN` o un infinito già
nei dati si propagano senza errore; solo l'overflow di un calcolo su valori
finiti si rifiuta. Le funzioni di rango non arrotondano, e per questo
rifiutano il testo numerico.

### Complessità

Tempo O(n log n) per l'ordinamento su `order_column`, O(n) per le
partizioni e, per le funzioni di rango, O(p log p) per ogni partizione di p
righe; le altre funzioni O(p). Memoria O(n). Da 32.768 righe, con più di
una partizione, le partizioni si calcolano in parallelo, con lo stesso
risultato. Nessuna variante spilled.

### Esempio

Due `10` nella squadra `a` sono a pari merito: rango `1.5`.

```json
{
  "config": {"column": "punti", "function": "rank", "group_by": "squadra"},
  "ingressi": [
    {"nome": "partite", "colonne": [
      {"nome": "squadra", "tipo": "utf8", "valori": ["a", "a", "b", "a"]},
      {"nome": "punti", "tipo": "int64", "valori": [10, 30, 5, 10]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "squadra", "tipo": "utf8", "valori": ["a", "a", "b", "a"]},
    {"nome": "punti", "tipo": "int64", "valori": [10, 30, 5, 10]},
    {"nome": "punti_rank", "tipo": "float64", "valori": [1.5, 3.0, 1.0, 1.5]}
  ]}
}
```
