### Che cosa fa

Verifica che lo schema dell'ingresso sia quello atteso: per ogni voce di
`fields` una colonna con quel nome, un tipo della famiglia indicata e, se
richiesta, quella nullabilità. Se l'asserzione regge, l'uscita è l'ingresso
invariato; se non regge, il passo fallisce e non produce uscita. Guarda solo
lo schema, mai i valori: nel runner l'esito si decide in validazione.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `fields` | lista di oggetti | obbligatorio | almeno una voce, nomi non ripetuti, al più 4096 | colonne attese, ciascuna con `name`, `data_type`, `nullable` |
| `fields[].name` | stringa | obbligatorio | nome non vuoto, al più 1024 byte | nome della colonna attesa |
| `fields[].data_type` | stringa | obbligatorio | vedi sotto (maiuscole e spazi ai lati ignorati) | famiglia di tipo attesa |
| `fields[].nullable` | booleano | assente | `true`, `false` | nullabilità attesa; assente, non si controlla |
| `allow_extra` | booleano | `false` | `true`, `false` | con `false` l'ingresso ha esattamente tante colonne quante voci in `fields` |
| `ordered` | booleano | `true` | `true`, `false` | con `true` la voce *i* descrive la colonna in posizione *i*; con `false` la colonna si cerca per nome |

Valori di `data_type` e tipi che accettano:

- `utf8` o `string`: `utf8`; `int64` o `integer`: `int64`; `float64`,
  `float` o `double`: `float64`; `boolean` o `bool`: `bool`; `uint64` o
  `unsigned`: `uint64`; `date32`; `binary`: tipo identico;
- `timestamp_seconds`, `timestamp_millis`, `timestamp_micros`,
  `timestamp_nanos`: `timestamp` in secondi, millisecondi, microsecondi o
  nanosecondi, con o senza fuso orario (l'unità conta, il fuso no);
- `decimal128`: `decimal128` di qualunque precisione e scala;
- `dictionary_utf8`: dizionario con chiavi `int32` e valori `utf8`;
- `list`: qualunque lista; `struct`: qualunque struct.

Gli altri tipi Arrow (`int32`, `float32`, `date64`…) non si possono
asserire. Con `ordered=true` e `allow_extra=true` le prime colonne devono
essere quelle di `fields`, nell'ordine, e le altre seguono libere.

### Schema

Identico all'ingresso: stesse colonne, tipi, nullabilità, metadati di campo
e di schema; il contratto conserva ordinamento dichiarato (`sorted_by`) e
numero di righe.

### Righe

1:1: tutte le righe dell'ingresso, invariate.

### Ordine

L'ordine d'ingresso.

### Errori

In validazione, `InvalidPlan`:

- `fields` vuoto, con nomi ripetuti o non validi, o oltre 4096 voci;
- `allow_extra=false` e numero di colonne diverso dal numero di voci;
- colonna attesa assente (per posizione con `ordered=true`, per nome con
  `ordered=false`), o in posizione con un nome diverso;
- tipo della colonna fuori dalla famiglia attesa, o `data_type` non in
  elenco;
- `nullable` scritto e diverso da quello della colonna;
- config con campi sconosciuti.

In esecuzione: nel runner nessuno, perché lo schema dei dati è quello del
contratto controllato in validazione. Il kernel chiamato fuori dal runner
rifiuta gli stessi casi con `Schema` (colonna assente o fuori posto, tipo o
nullabilità diversi, numero di colonne) e con `InvalidPlan` (`data_type` non
in elenco).

### Limiti e deviazioni

Le famiglie di tipo sono volutamente larghe: `decimal128`, i `timestamp_*`,
`list` e `struct` non controllano precisione, scala, fuso, tipo degli
elementi né campi.

### Complessità

Tempo O(k) sulle voci di `fields`, indipendente dalle righe; memoria O(1):
l'uscita condivide le colonne dell'ingresso.

### Esempio

```json
{
  "config": {"fields": [
    {"name": "id", "data_type": "int64", "nullable": true},
    {"name": "nome", "data_type": "string"}
  ]},
  "ingressi": [
    {"nome": "clienti", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2]},
      {"nome": "nome", "tipo": "utf8", "valori": ["Anna", null]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2]},
    {"nome": "nome", "tipo": "utf8", "valori": ["Anna", null]}
  ]}
}
```
