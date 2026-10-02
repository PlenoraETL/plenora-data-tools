### Che cosa fa

Espande una colonna lista in una riga per elemento: ogni riga d'ingresso
si ripete una volta per ogni elemento della sua lista, e la colonna
d'uscita contiene l'elemento. Una lista vuota o nulla dà una riga con
elemento nullo, quindi nessuna riga sparisce.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa | obbligatorio | nome di una colonna `list<…>` | colonna da espandere |
| `output_column` | stringa | `column` | nome di colonna valido | colonna con gli elementi |
| `empty_policy` | stringa | `"null"` | `"null"` | liste vuote e nulle danno una riga con null |

`empty_policy: "drop"` si rifiuta: togliere le righe con lista vuota è una
selezione, e si scrive come passo a parte. Solo `list<…>`: non
`large_list` né liste a dimensione fissa.

### Schema

Con `output_column` uguale a `column` (il default), la colonna lista è
sostituita al suo posto dalla colonna degli elementi. Con un nome diverso
la colonna lista resta e la colonna degli elementi va in coda, o sostituisce
al suo posto una colonna esistente con quel nome. La colonna degli elementi
ha il tipo degli elementi, è nullabile e non ha metadati di campo; le altre
colonne e i metadati di schema si conservano. Il contratto non dichiara né
ordinamento né conteggio.

### Righe

Espansione 1:N: per ogni riga, una riga per elemento (un elemento nullo dà
un valore nullo), una sola se la lista è vuota o nulla.

### Ordine

Righe nell'ordine d'ingresso, elementi nell'ordine della lista.

### Errori

In validazione, `InvalidPlan`:

- `empty_policy: "drop"`;
- `column` assente o non di tipo `list<…>`;
- `output_column` non valido o `null` esplicito (il parametro si omette);
  campi sconosciuti.

In esecuzione, `ResourceLimit`: righe d'uscita oltre `max_rows`; un
elemento o una riga d'uscita oltre l'indice `u32::MAX`.

### Limiti e deviazioni

Il runner controlla sui dati righe per arco e fattore di espansione
([Runner, «Esecuzione»](runner.md#esecuzione)).

### Complessità

Tempo e memoria O(n + e) per n righe ed e elementi. Quando la colonna
d'uscita sostituisce la lista, la lista non si copia per ogni riga
d'uscita.

### Esempio

```json
{
  "config": {"column": "tag"},
  "ingressi": [
    {"nome": "articoli", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
      {"nome": "tag", "tipo": "list<utf8>", "valori": [["a", "b"], [], null]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 1, 2, 3]},
    {"nome": "tag", "tipo": "utf8", "valori": ["a", "b", null, null]}
  ]}
}
```
