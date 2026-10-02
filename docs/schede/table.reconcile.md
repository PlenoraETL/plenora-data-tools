### Che cosa fa

Confronta due tabelle sulle chiavi `left_keys` e `right_keys` e restituisce
un resoconto di cinque metriche: quante righe si abbinano, quante restano
solo a sinistra o solo a destra, quante sono duplicati di una chiave su
ciascun lato. Le righe si abbinano una a una per chiave: se una chiave
compare 3 volte a sinistra e 2 a destra, 2 righe sono abbinate e 1 resta
solo a sinistra.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `left_keys` | lista di stringhe | obbligatorio | almeno un nome, senza ripetizioni, al più 4096; colonne leggibili come testo scalare | colonne della chiave nella tabella sinistra |
| `right_keys` | lista di stringhe | obbligatorio | tanti nomi quanti `left_keys`, senza ripetizioni; stesse condizioni | colonne della chiave nella tabella destra |
| `nulls_equal` | booleano | `true` | `true`, `false` | con `true` il null è un valore della chiave e due null si abbinano; con `false` una riga con un null nella chiave resta sola sul suo lato |

Le colonne si abbinano per posizione: `left_keys[i]` con `right_keys[i]`, e
ogni coppia ha lo stesso tipo Arrow. Leggibili come testo scalare: `utf8`,
`int64`, `uint64`, `float64`, `bool`, `binary`, `date32`, `date64`,
`timestamp` di ogni unità (con fuso valido), `decimal128` (scala da 0 a 38), dizionario
`int32`→`utf8`.

Due chiavi sono uguali quando lo sono tutte le loro colonne, con
l'uguaglianza della forma testuale: sugli interi, i testi, le date e i
decimali è l'uguaglianza dei valori; su `float64` tutti i `NaN` sono uguali e
`0.0` è diverso da `-0.0`; un `binary` si confronta sui byte.

### Schema

Una tabella nuova, indipendente dagli ingressi: `metric` (`utf8`) e `value`
(`uint64`), non nullabili. Nessuna colonna e nessun metadato di schema degli
ingressi. Il contratto dichiara 5 righe, dimostrate.

### Righe

Sempre 5 righe, una per metrica, anche da due ingressi vuoti (metriche a
zero): il numero di righe non dipende dagli ingressi, e il catalogo esenta
l'operazione dal fattore di espansione. Per ogni chiave, con `L` righe a sinistra
e `R` a destra:

- `matched_rows`: somma di `min(L, R)`;
- `left_only_rows`: somma di `L − min(L, R)`, più le righe sinistre con un
  null nella chiave se `nulls_equal=false`;
- `right_only_rows`: somma di `R − min(L, R)`, più le righe destre con un
  null nella chiave se `nulls_equal=false`;
- `left_duplicate_rows`: somma di `L − 1` sulle chiavi presenti a sinistra;
- `right_duplicate_rows`: somma di `R − 1` sulle chiavi presenti a destra.

Con `nulls_equal=false` le righe con una chiave nulla non contano fra i
duplicati.

### Ordine

Le metriche nell'ordine sopra, sempre lo stesso.

### Errori

In validazione, `InvalidPlan`:

- `left_keys` vuoto, liste di lunghezza diversa, nomi ripetuti o non validi,
  oltre 4096 nomi;
- una colonna chiave non esiste, non è leggibile come testo scalare, o ha
  un tipo diverso da quello della colonna abbinata;
- config con campi sconosciuti.

In esecuzione:

- `ResourceLimit`: le chiavi distinte, contate lato per lato (una chiave
  presente nei due lati conta due volte), superano il margine di memoria
  che il runner passa al kernel (`max_governed_memory_bytes`), a lunghezza
  della forma testuale più 64 byte per chiave; oppure le chiavi distinte di
  un lato superano `max_input_rows`;
- `Schema`: una cella di chiave non si converte in testo (`date32`, `date64` o
  `timestamp` fuori dall'intervallo di calendario, `date64` non allineato al giorno).

### Limiti e deviazioni

L'hash delle chiavi non ha seme
([Limiti dichiarati, «Hash delle chiavi non keyed»](limiti.md#hash-delle-chiavi-non-keyed)).
La memoria contata è quella delle chiavi, non quella delle mappe che le
contengono.

### Complessità

Tempo O(n + m) atteso su righe sinistre e destre; memoria O(d) per le
chiavi distinte dei due lati.

### Esempio

```json
{
  "config": {"left_keys": ["codice"], "right_keys": ["codice"]},
  "ingressi": [
    {"nome": "contabilita", "colonne": [
      {"nome": "codice", "tipo": "utf8", "valori": ["A", "A", "B", "C"]}
    ]},
    {"nome": "banca", "colonne": [
      {"nome": "codice", "tipo": "utf8", "valori": ["A", "B", "B", "D"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "metric", "tipo": "utf8", "valori": ["matched_rows", "left_only_rows", "right_only_rows", "left_duplicate_rows", "right_duplicate_rows"]},
    {"nome": "value", "tipo": "uint64", "valori": [2, 2, 2, 1, 1]}
  ]}
}
```
