### Che cosa fa

Tiene le prime `n` righe secondo l'ordinamento di [`table.sort`](#tablesort)
sulle colonne `columns`: l'uscita è quella di `table.sort` seguita dalle
prime `n` righe, calcolata senza ordinare tutto l'ingresso.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `columns` | lista di stringhe | obbligatorio | come `columns` di [`table.sort`](#tablesort) | chiavi d'ordinamento, dalla più significativa |
| `n` | intero | obbligatorio | da `0` a `max_rows` | righe da tenere |
| `descending` | booleano | `false` | `true`, `false` | `true` tiene i valori più grandi |

Il verso si scrive `descending`, non `ascending`: `ascending` è un campo
sconosciuto e si rifiuta.

### Schema

Identico all'ingresso: colonne, tipi, nullabilità e metadati. Il contratto
dichiara l'uscita ordinata sulle colonne di `columns` e un conteggio di
`min(n, righe)` quando quello d'ingresso è noto.

### Righe

Selezione: `min(n, righe)` righe dell'ingresso; `n = 0` dà una tabella
vuota con lo stesso schema.

### Ordine

Quello di [`table.sort`](#tablesort) con `ascending = !descending`: null
dopo ogni valore in ascendente, prima di ogni valore in discendente (con
`descending: true` i null sono quindi le prime righe tenute), pareggi
nell'ordine d'ingresso. Fra righe a pari merito sul confine delle prime
`n` passano quelle che vengono prima nell'ingresso.

### Errori

In validazione, `InvalidPlan`:

- come [`table.sort`](#tablesort) per `columns`;
- `n` oltre `max_rows`;
- config con campi sconosciuti.

In esecuzione:

- `Schema`: una chiave di dizionario fuori dal proprio dizionario;
- `ResourceLimit`: più di `u32::MAX` righe;
- `InvalidPlan`: `n` non rappresentabile come indice della piattaforma
  (solo dove `usize` ha 32 bit).

### Limiti e deviazioni

Nessuno oltre ai limiti comuni.

### Complessità

Tempo O(n_righe) confronti per separare le prime `n` più O(n log n) per
ordinarle; memoria O(n_righe) indici più la copia delle `n` righe. Nessuna
variante spilled.

### Esempio

Con `descending: true` il null è la prima riga tenuta.

```json
{
  "config": {"columns": ["punti"], "n": 2, "descending": true},
  "ingressi": [
    {"nome": "classifica", "colonne": [
      {"nome": "nome", "tipo": "utf8", "valori": ["anna", "bruno", "carla", "dario"]},
      {"nome": "punti", "tipo": "int64", "valori": [7, 3, null, 9]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "nome", "tipo": "utf8", "valori": ["carla", "dario"]},
    {"nome": "punti", "tipo": "int64", "valori": [null, 9]}
  ]}
}
```
