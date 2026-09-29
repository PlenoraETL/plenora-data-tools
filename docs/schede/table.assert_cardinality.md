### Che cosa fa

Verifica il numero di righe della tabella: esattamente `exact_rows`, oppure
almeno `min_rows` e al più `max_rows`. Se l'asserzione regge, l'uscita è
l'ingresso invariato; se non regge, il passo fallisce e non produce uscita.
Quando il numero di righe dell'ingresso è già attestato nel contratto,
l'esito si decide in validazione.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `exact_rows` | intero | assente | da 0 a `max_input_rows` | numero esatto di righe |
| `min_rows` | intero | assente | da 0 a `max_input_rows`, non oltre `max_rows` | minimo di righe, incluso |
| `max_rows` | intero | assente | da 0 a `max_input_rows` | massimo di righe, incluso |

Almeno uno dei tre; `exact_rows` non si combina con `min_rows` o
`max_rows`. `max_input_rows` è il limite di righe del piano.

### Schema

Identico all'ingresso: stesse colonne, tipi, nullabilità, metadati di campo
e di schema; il contratto conserva ordinamento dichiarato (`sorted_by`) e
numero di righe.

### Righe

1:1 se l'asserzione regge: tutte le righe, invariate. Altrimenti nessuna
uscita.

### Ordine

L'ordine d'ingresso.

### Errori

In validazione, `InvalidPlan`:

- nessuno dei tre vincoli, o `exact_rows` insieme a `min_rows`/`max_rows`;
- `min_rows` maggiore di `max_rows`, o un vincolo oltre `max_input_rows`;
- il contratto d'ingresso attesta il numero di righe (`row_count`
  dimostrato, per esempio dopo `table.reconcile`) e questo viola il vincolo;
- config con campi sconosciuti o valori negativi.

In esecuzione, `InvalidPlan` (`N righe fuori contratto`): il numero di righe
viola il vincolo. Non c'è diagnostica per riga: il difetto è della tabella,
non di una riga.

### Limiti e deviazioni

L'errore in esecuzione è `InvalidPlan`, non `DataMapping` come le altre
asserzioni sui dati: chi lo classifica per categoria lo vede come un difetto
del piano.

### Complessità

Tempo e memoria O(1): conta le righe della tabella, l'uscita condivide le
colonne dell'ingresso.

### Esempio

```json
{
  "config": {"min_rows": 1, "max_rows": 3},
  "ingressi": [
    {"nome": "lotto", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [10, 20]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [10, 20]}
  ]}
}
```
