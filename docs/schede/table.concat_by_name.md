### Che cosa fa

Impila le righe di due tabelle abbinando le colonne per nome, non per
posizione: prima tutte le righe di sinistra, poi quelle di destra. Una
colonna che manca in un ingresso vale null nelle sue righe. Con `strict`
gli schemi devono essere identici, come in [`table.concat`](#tableconcat).

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `strict` | booleano | `false` | `true`, `false` | `true`: stesse colonne, con stesso nome e tipo, nello stesso ordine |

### Schema

Senza `strict`, l'unione delle colonne nell'ordine di prima apparizione:
le colonne di sinistra, poi quelle di destra con un nome nuovo. Una colonna
presente in entrambi gli ingressi ha lo stesso tipo nei due (nessuna
conversione) e prende i metadati di campo della sinistra. Una colonna è
nullable se manca in un ingresso o è nullable in almeno uno. Con `strict`
lo schema è quello della sinistra, nullable dove lo è in almeno un
ingresso.

I metadati di schema dei due ingressi si fondono: una chiave presente in un
ingresso solo, o con lo stesso valore, resta. La colonna geometrica della
sinistra si conserva solo se la destra ha una colonna con lo stesso nome e
lo stesso tipo; altrimenti l'uscita non dichiara geometria. Del contratto
resta il conteggio delle righe (somma, come in `table.concat`), non
l'ordinamento dichiarato.

### Righe

Tutte le righe di entrambi gli ingressi, senza deduplicazione: `n + m`
righe. Nelle righe di un ingresso le colonne che quell'ingresso non ha
sono nulle.

### Ordine

Le righe di sinistra nel loro ordine, poi quelle di destra nel loro
ordine.

### Errori

In validazione, `InvalidPlan`:

- una colonna con lo stesso nome e tipi diversi nei due ingressi;
- con `strict`, numero di colonne diverso o nome o tipo diversi in una
  posizione;
- metadati di schema con la stessa chiave e valori diversi;
- config con campi sconosciuti.

Nel runner, più di due ingressi sono `Unsupported`, meno di due
`InvalidPlan`.

In esecuzione, `ResourceLimit`:

- righe totali oltre `max_rows` (nel runner `max_input_rows`);
- uscita stimata oltre `max_governed_memory_bytes`, prima di copiare. La
  stima conta per ogni colonna dell'unione la larghezza maggiore fra gli
  ingressi che la hanno, così anche le colonne di null aggiunte.

### Limiti e deviazioni

Come `table.concat`, il catalogo la dichiara N-aria e il runner esegue solo
la forma a due ingressi ([Runner, «Validazione»](runner.md#validazione)).
Due colonne con lo stesso nome e tipi diversi si rifiutano: nessuna
promozione di tipo.

### Complessità

Tempo e memoria O((n + m)·c), con `c` le colonne dell'unione: ogni colonna
si copia, e quelle mancanti si riempiono di null.

### Esempio

```json
{
  "config": {},
  "ingressi": [
    {"nome": "negozio", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2]},
      {"nome": "importo", "tipo": "float64", "valori": [10.5, 4.0]}
    ]},
    {"nome": "online", "colonne": [
      {"nome": "canale", "tipo": "utf8", "valori": ["web"]},
      {"nome": "id", "tipo": "int64", "valori": [3]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
    {"nome": "importo", "tipo": "float64", "valori": [10.5, 4.0, null]},
    {"nome": "canale", "tipo": "utf8", "valori": [null, null, "web"]}
  ]}
}
```
