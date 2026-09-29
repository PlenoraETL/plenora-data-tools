### Che cosa fa

Impila le righe di due tabelle con lo stesso schema: prima tutte le righe
di sinistra, poi tutte quelle di destra. Le colonne si abbinano per
posizione e devono avere, posizione per posizione, lo stesso nome e lo
stesso tipo. Per unire tabelle con colonne diverse o in ordine diverso si
usa [`table.concat_by_name`](#tableconcat_by_name).

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `ignore_index` | booleano | `true` | `true`, `false` | nessun effetto: si accetta per compatibilità, una tabella Arrow non ha indice di riga |

### Schema

Le colonne della sinistra, con nome, tipo e metadati di campo della
sinistra; una colonna è nullable se lo è in almeno un ingresso. I metadati
di schema dei due ingressi si fondono: una chiave presente in un ingresso
solo, o con lo stesso valore, resta. La colonna geometrica è quella della
sinistra. Del contratto resta il conteggio delle righe, somma dei due
ingressi quando entrambi lo dichiarano con la stessa portata e la stessa
confidenza; l'ordinamento dichiarato (`sorted_by`) no.

### Righe

Tutte le righe di entrambi gli ingressi, senza deduplicazione e con i
valori invariati: l'uscita ha `n + m` righe.

### Ordine

Le righe di sinistra nel loro ordine, poi quelle di destra nel loro
ordine.

### Errori

In validazione, `InvalidPlan`:

- numero di colonne diverso, o nome o tipo diversi in una posizione (la
  nullabilità non conta);
- metadati di schema con la stessa chiave e valori diversi;
- config con campi sconosciuti.

Nel runner, `table.concat` con più di due ingressi è `Unsupported`, con
meno di due `InvalidPlan`.

In esecuzione, `ResourceLimit`:

- righe totali oltre `max_rows` (nel runner `max_input_rows`);
- uscita stimata (righe per la larghezza di riga maggiore fra i due
  ingressi) oltre `max_governed_memory_bytes`, prima di copiare.

### Limiti e deviazioni

Il catalogo la dichiara N-aria, ma il runner esegue solo la forma a due
ingressi ([README, «Validazione»](../README.md#validazione)); più tabelle
si impilano con passi in catena. I metadati di campo della destra non si
conservano. `ignore_index` si accetta e non ha effetto, qualunque valore
abbia.

### Complessità

Tempo e memoria O(n + m): ogni colonna si copia nell'uscita.

### Esempio

```json
{
  "config": {},
  "ingressi": [
    {"nome": "gennaio", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2]},
      {"nome": "importo", "tipo": "float64", "valori": [10.5, null]}
    ]},
    {"nome": "febbraio", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [2, 3]},
      {"nome": "importo", "tipo": "float64", "valori": [7.0, 3.25]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2, 2, 3]},
    {"nome": "importo", "tipo": "float64", "valori": [10.5, null, 7.0, 3.25]}
  ]}
}
```
