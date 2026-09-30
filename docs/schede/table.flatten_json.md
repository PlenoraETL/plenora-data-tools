### Che cosa fa

Legge ogni cella di una colonna come documento JSON e ne estrae i valori
nominati in `output_columns`, uno per colonna d'uscita. Il nome di una
colonna d'uscita è `prefix` seguito dal percorso della chiave nel documento,
con i livelli separati da un punto (`doc_indirizzo.citta` legge
`{"indirizzo": {"citta": …}}`). Ogni valore diventa testo.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa | obbligatorio | colonna leggibile come testo | colonna con i documenti JSON |
| `prefix` | stringa | `""`, cioè `<column>_` | qualsiasi | prefisso dei nomi d'uscita; vuoto vale `<column>_` |
| `max_level` | intero | `1` | da 0 a 5 | livelli di annidamento attraversati |
| `output_columns` | lista di stringhe | `[]` | non vuota nei piani; nomi che iniziano con `prefix`, senza ripetizioni, al più `max_columns` | colonne da estrarre |

Leggibile come testo: `utf8`, `int64`, `uint64`, `float64`, `bool`,
`date32`, `timestamp(ms)`, `decimal128` con scala da 0 a 38, `binary`,
`dictionary<utf8>` con chiavi `int32`.

Un percorso con k punti si estrae solo se `k <= max_level`: con il default
`1` si leggono le chiavi della radice e quelle di un oggetto figlio, non
oltre. Il testo estratto è la stringa stessa per una stringa JSON, il testo
JSON per numeri e booleani, il testo JSON compatto per un array
(`[1,2]`), la stringa vuota per un `null` JSON. Un oggetto non si estrae mai
come valore: si attraversa. Un percorso assente nel documento dà una cella
nulla. Una chiave ripetuta nello stesso oggetto vale l'ultima occorrenza. Una
chiave che contiene un punto si confonde con un percorso annidato: se due
derivazioni danno lo stesso percorso vale quella che viene dopo
nell'ordine lessicografico delle chiavi.

### Schema

Le colonne di `output_columns`, nell'ordine scritto, `utf8` nullable: una
colonna con lo stesso nome di una esistente la sostituisce al suo posto
(perdendone tipo e metadati di campo), le altre si aggiungono in coda. La
colonna sorgente resta. Metadati di schema conservati; `row_count` resta;
`sorted_by` resta solo se nessuna colonna esistente è sovrascritta.

### Righe

1:1. Una cella nulla dà null in tutte le colonne estratte.

### Ordine

Invariato.

### Errori

In validazione:

- `InvalidPlan`: `column` assente o non leggibile come testo; `max_level`
  oltre 5; un nome di `output_columns` che non inizia con `prefix`, vuoto,
  oltre 1024 byte o ripetuto; più di `max_columns` nomi; config con campi
  sconosciuti;
- `Unsupported`: `output_columns` vuoto (i nomi delle colonne dipenderebbero
  dai documenti, e lo schema non si inferisce prima dei dati);
- `ResourceLimit`: colonne dell'ingresso più `output_columns` oltre
  `max_columns` (anche quando una colonna prodotta ne sostituisce una
  esistente).

In esecuzione:

- `DataMapping` con diagnostica per riga: una cella non nulla che non è JSON
  valido (`json.invalid_syntax`) o la cui radice non è un oggetto
  (`json.root_not_object`). Il passo non produce uscita; la diagnostica
  conta tutte le righe rifiutate e ne riporta le prime 10;
- `Schema`: una cella che non si converte in testo (`binary` non UTF-8);
- `ResourceLimit`: un testo emesso (valori annidati e numeri riscritti come
  testo JSON) oltre `max_string_bytes` byte.

### Limiti e deviazioni

Un documento si valida come lo valida `serde_json`: numeri oltre il
dominio di `f64`, surrogati isolati e annidamento oltre 128 livelli lo
rendono invalido, anche dove il valore non verrebbe estratto. Il kernel
chiamato fuori dal runner accetta `output_columns` vuoto e crea una colonna
per ogni percorso trovato; il runner lo rifiuta in validazione
([README, «Validazione»](../README.md#validazione)).

### Complessità

Tempo O(byte dei documenti): una passata di parsing per riga, senza
costruire l'albero JSON (salvo le righe con chiavi ambigue o ripetute, che
passano dall'albero); memoria O(n · k) per le k colonne estratte.

### Esempio

```json
{
  "config": {"column": "doc", "output_columns": ["doc_nome", "doc_indirizzo.citta", "doc_tag"]},
  "ingressi": [
    {"nome": "anagrafe", "colonne": [
      {"nome": "doc", "tipo": "utf8", "valori": ["{\"nome\": \"Anna\", \"indirizzo\": {\"citta\": \"Roma\"}, \"tag\": [1, 2]}", "{\"nome\": null}", null]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "doc", "tipo": "utf8", "valori": ["{\"nome\": \"Anna\", \"indirizzo\": {\"citta\": \"Roma\"}, \"tag\": [1, 2]}", "{\"nome\": null}", null]},
    {"nome": "doc_nome", "tipo": "utf8", "valori": ["Anna", "", null]},
    {"nome": "doc_indirizzo.citta", "tipo": "utf8", "valori": ["Roma", null, null]},
    {"nome": "doc_tag", "tipo": "utf8", "valori": ["[1,2]", null, null]}
  ]}
}
```
