### Che cosa fa

Riempie le celle null di una colonna, o di tutte se `column` manca: con un
valore fisso (`method = "value"`), con l'ultimo valore non nullo che le
precede (`ffill`) o con il primo che le segue (`bfill`), nell'ordine delle
righe. Il tipo delle colonne non cambia.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa | assente | colonna dell'ingresso di tipo `utf8`, `int64`, `float64` o `bool`; `null` non ammesso | colonna da riempire; assente, le riempie tutte |
| `method` | stringa | `"value"` | `value`, `ffill`, `bfill` | come si riempie |
| `value` | JSON | assente | convertibile nel tipo di ogni colonna da riempire (sotto); testo al più `max_string_bytes` byte | valore di riempimento, solo con `method = "value"` |

Senza `column` ogni colonna dell'ingresso deve essere di uno dei quattro
tipi, e `value` deve convertirsi nel tipo di ognuna.

`value` si converte così:

- `utf8`: una stringa com'è; ogni altro valore JSON diventa il suo testo
  JSON (`5` dà `"5"`, `true` dà `"true"`);
- `int64`: un intero JSON nel dominio di `int64`, o una stringa che lo è
  (senza spazi); `1.5` si rifiuta;
- `float64`: un numero JSON, o una stringa con la virgola decimale ammessa
  (`"2,5"`, senza spazi);
- `bool`: `true`/`false` JSON, o le stringhe `"true"`/`"false"` in
  qualunque combinazione di maiuscole.

`value` assente o `null` con `method = "value"` è accettato e non cambia
niente. `value` scritto, anche `null`, con `ffill` o `bfill` si rifiuta.

### Schema

Stesse colonne, stessi tipi, stesse posizioni e stessi metadati di campo e
di schema; le colonne riempite diventano nullable (possono restare null
all'inizio con `ffill` e alla fine con `bfill`).

Contratto: il conteggio delle righe resta; l'ordinamento dichiarato cade.

### Righe

1:1. Si riempie solo il null: un `NaN` in un `float64` non è null e
resta. Con `ffill` i null prima del primo valore restano null; con `bfill`
quelli dopo l'ultimo.

### Ordine

Righe nell'ordine d'ingresso, che è anche l'ordine in cui `ffill` e `bfill`
cercano il valore.

### Errori

In validazione, `InvalidPlan`:

- `column` assente dall'ingresso, o scritto `null` (per riempire tutte le
  colonne si omette);
- una colonna da riempire di tipo diverso da `utf8`, `int64`, `float64`,
  `bool` (senza `column`: una qualunque colonna dell'ingresso);
- `value` non convertibile nel tipo di una colonna da riempire;
- il testo di `value` oltre `max_string_bytes` byte;
- `value` scritto con `ffill` o `bfill`;
- `method` fuori elenco, config con campi sconosciuti.

In esecuzione: nessun errore che dipenda dai dati.

### Limiti e deviazioni

Solo i quattro tipi elencati: per date, decimali o `uint64` il riempimento
non c'è. `ffill` e `bfill` seguono l'ordine delle righe, quindi hanno senso
dopo un `table.sort`.

### Complessità

Tempo O(n) per colonna riempita; memoria pari a una copia di ciascuna
colonna che contiene null (quelle senza null restano condivise).

### Esempio

```json
{
  "config": {"column": "prezzo", "method": "ffill"},
  "ingressi": [
    {"nome": "listino", "colonne": [
      {"nome": "giorno", "tipo": "int64", "valori": [1, 2, 3, 4]},
      {"nome": "prezzo", "tipo": "float64", "valori": [null, 10.5, null, 11.0]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "giorno", "tipo": "int64", "valori": [1, 2, 3, 4]},
    {"nome": "prezzo", "tipo": "float64", "valori": [null, 10.5, 10.5, 11.0]}
  ]}
}
```
