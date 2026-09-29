### Che cosa fa

Apre una colonna struct: ogni campo dello struct diventa una colonna, con
il nome preceduto da `prefix`. Per default la colonna struct sparisce.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa | obbligatorio | nome di una colonna `struct<…>` | colonna da aprire |
| `prefix` | stringa | `""` | entro `max_string_bytes` | prefisso dei nomi delle colonne nuove |
| `drop_source` | booleano | `true` | `true`, `false` | toglie la colonna struct |

### Schema

Le colonne dell'ingresso nel loro ordine (senza la colonna struct con
`drop_source: true`), poi in coda una colonna per campo dello struct,
nell'ordine dei campi: nome `prefix` + nome del campo, tipo del campo,
nullabile, con i metadati del campo. I metadati di schema si conservano.
Il contratto conserva il conteggio delle righe, e l'ordinamento dichiarato
solo con `drop_source: false`.

### Righe

1:1. Dove lo struct è nullo tutte le colonne nuove sono nulle; dove è
valido portano il valore del campo (null compreso).

### Ordine

Invariato.

### Errori

In validazione, `InvalidPlan`:

- `column` assente o non di tipo `struct<…>`;
- `prefix` oltre `max_string_bytes`;
- colonne d'uscita oltre `max_columns`;
- un nome nuovo non valido, o uguale a una colonna che resta (o a un altro
  campo);
- campi sconosciuti.

In esecuzione, `ResourceLimit`: più di `u32::MAX` righe.

### Limiti e deviazioni

Nessuno oltre ai limiti comuni.

### Complessità

Tempo e memoria O(n · f) per n righe e f campi dello struct.

### Esempio

```json
{
  "config": {"column": "indirizzo", "prefix": "ind_"},
  "ingressi": [
    {"nome": "clienti", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2]},
      {"nome": "indirizzo", "tipo": "struct<via: utf8, civico: int64>", "valori": [{"via": "Roma", "civico": 3}, null]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2]},
    {"nome": "ind_via", "tipo": "utf8", "valori": ["Roma", null]},
    {"nome": "ind_civico", "tipo": "int64", "valori": [3, null]}
  ]}
}
```
