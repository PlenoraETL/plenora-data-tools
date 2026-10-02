### Che cosa fa

Porta la tabella da larga a lunga: ogni colonna valore diventa un blocco di
righe, con il nome della colonna in `var_name` e la cella in `value_name`;
le colonne id si ripetono su ogni blocco.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `id_columns` | lista di stringhe | obbligatorio | nomi di colonne dell'ingresso, senza ripetizioni; anche vuota | colonne ripetute su ogni riga d'uscita |
| `value_columns` | lista di stringhe | `[]` | nomi di colonne dell'ingresso, senza ripetizioni | colonne da portare in righe; vuota vale tutte le colonne non id |
| `var_name` | stringa | `"variable"` | nome di colonna valido, diverso da `value_name` | colonna con il nome della colonna valore |
| `value_name` | stringa | `"value"` | nome di colonna valido | colonna con la cella |
| `type_policy` | stringa | `"reject"` | `"reject"`, `"string"`; `null` non ammesso | colonne valore di tipi diversi: rifiuto, o conversione in testo |

`var_name` e `value_name` non collidono con nessuna colonna dell'ingresso,
nemmeno con quelle che spariscono: un nome già preso riceve il primo
suffisso libero fra `_1` e `_99`, prima `var_name` poi `value_name`
(sciogliere una colonna di nome `value` dà una colonna `value_1`).

`type_policy` con colonne valore tutte dello stesso tipo si accetta e non
cambia niente: dipende dall'ingresso, e lo stesso piano gira su tabelle
diverse.

### Schema

Le colonne di `id_columns` nel loro ordine, con tipo, nullabilità e
metadati di campo; poi `var_name`, `utf8` non nullabile; poi `value_name`,
nullabile e senza metadati di campo, del tipo comune se le colonne valore
hanno tutte lo stesso tipo Arrow, altrimenti `utf8` con
`type_policy: "string"`. I metadati di schema si conservano. Una colonna
geometrica resta geometrica solo se è una colonna id. Il contratto dichiara
righe × colonne valore quando il conteggio d'ingresso è noto, e nessun
ordinamento.

Con `type_policy: "string"` le celle diventano il loro testo: `1.0` è
`"1"`, `-0.0` è `"-0"`, le date `AAAA-MM-GG`, i timestamp in RFC 3339, i
booleani `true`/`false`, i `decimal128` con tutte le cifre della scala.

### Righe

Espansione: righe × colonne valore. Una cella nulla dà una riga con valore
nullo: nessuna riga si scarta.

### Ordine

Per colonna valore, nell'ordine di `value_columns` (o delle colonne
dell'ingresso): prima tutte le righe per la prima colonna, in ordine
d'ingresso, poi tutte per la seconda, e così via.

### Errori

In validazione:

- `InvalidPlan`: `var_name` uguale a `value_name`; una lista con un nome
  ripetuto o non valido, o oltre il limite di colonne; una colonna assente;
  nessuna colonna valore; colonne valore di tipi diversi con
  `type_policy: "reject"`; `type_policy: null` esplicito (il parametro si
  omette); nessun suffisso libero per un nome d'uscita; campi sconosciuti;
- `Schema`: con `type_policy: "string"` e tipi diversi, una colonna valore
  di tipo non convertibile in testo o con una timezone non valida.

In esecuzione:

- `ResourceLimit`: righe d'uscita oltre `max_rows`; stima dei byte
  d'uscita oltre `max_governed_memory_bytes`; un testo oltre
  `max_string_bytes`;
- `Schema`: con la conversione in testo, una cella che non si converte
  (`binary` non UTF-8, date fuori intervallo).

### Limiti e deviazioni

Prima di allocare, il kernel stima i byte dell'uscita (colonne id
ripetute, nome della colonna valore più lungo, colonna valore più larga,
misurata in testo con la conversione) e rifiuta oltre
`max_governed_memory_bytes`. L'espansione (righe × colonne valore) è
fissata da config e schema: il runner non le applica il fattore di
espansione e dopo il passo controlla invece il numero esatto di righe,
oltre alle righe per arco
([Runner, «Esecuzione»](runner.md#esecuzione)).

### Complessità

Tempo e memoria O(n · k) per n righe e k colonne valore.

### Esempio

```json
{
  "config": {"id_columns": ["id"], "value_columns": ["q1", "q2"], "var_name": "trimestre", "value_name": "vendite"},
  "ingressi": [
    {"nome": "larga", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2]},
      {"nome": "q1", "tipo": "int64", "valori": [10, null]},
      {"nome": "q2", "tipo": "int64", "valori": [20, 30]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2, 1, 2]},
    {"nome": "trimestre", "tipo": "utf8", "valori": ["q1", "q1", "q2", "q2"]},
    {"nome": "vendite", "tipo": "int64", "valori": [10, null, 20, 30]}
  ]}
}
```
