### Che cosa fa

Verifica un vincolo di chiave esterna: ogni chiave `left_keys` della tabella
sinistra deve comparire fra le chiavi `right_keys` della tabella destra. Se
l'asserzione regge, l'uscita è la tabella sinistra invariata; se una chiave
sinistra non trova riscontro, o è nulla senza `allow_null`, il passo
fallisce con una diagnostica per riga e non produce uscita. La tabella
destra serve solo come riferimento.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `left_keys` | lista di stringhe | obbligatorio | almeno un nome, senza ripetizioni, al più 4096; colonne leggibili come testo scalare | colonne della chiave esterna, nella tabella sinistra |
| `right_keys` | lista di stringhe | obbligatorio | tanti nomi quanti `left_keys`, senza ripetizioni; stesse condizioni | colonne della chiave riferita, nella tabella destra |
| `allow_null` | booleano | `false` | `true`, `false` | con `true` una riga sinistra con un null nella chiave passa senza controllo |

Le colonne si abbinano per posizione: `left_keys[i]` con `right_keys[i]`, e
ogni coppia ha lo stesso tipo Arrow. Leggibili come testo scalare: `utf8`,
`int64`, `uint64`, `float64`, `bool`, `binary`, `date32`, `timestamp(ms)`
(con fuso valido), `decimal128` (scala da 0 a 38), dizionario
`int32`→`utf8`.

Due chiavi sono uguali quando lo sono tutte le loro colonne, con
l'uguaglianza della forma testuale: sugli interi, i testi, le date e i
decimali è l'uguaglianza dei valori; su `float64` tutti i `NaN` sono uguali e
`0.0` è diverso da `-0.0`; un `binary` si confronta sui byte. Una chiave con
un null in una qualunque colonna è nulla: a destra non si registra mai, a
sinistra decide `allow_null`.

### Schema

Quello della tabella sinistra, invariato: colonne, tipi, nullabilità,
metadati di campo e di schema; il contratto conserva ordinamento dichiarato
(`sorted_by`) e numero di righe della sinistra. Nulla della destra entra
nell'uscita.

### Righe

La sinistra 1:1 se l'asserzione regge. Le righe destre duplicate o senza
riscontro a sinistra non contano. Se l'asserzione non regge, nessuna
uscita; nessuna riga viene scartata.

### Ordine

L'ordine della tabella sinistra.

### Errori

In validazione, `InvalidPlan`:

- `left_keys` vuoto, liste di lunghezza diversa, nomi ripetuti o non validi,
  oltre 4096 nomi;
- una colonna chiave non esiste, non è leggibile come testo scalare, o ha
  un tipo diverso da quello della colonna abbinata;
- config con campi sconosciuti.

In esecuzione:

- `DataMapping` con diagnostica per riga: righe sinistre con una chiave
  assente a destra (causa `validation.foreign_key_missing`) e, senza
  `allow_null`, con una chiave nulla (causa `validation.foreign_key_null`),
  senza colonna. La diagnostica dà il conteggio per causa e fino a 10
  esempi in ordine di riga, con l'indice (da zero) della riga della
  sinistra nella base del runner
  ([README, «Diagnostica per riga»](../README.md#diagnostica-per-riga));
  mai i valori;
- `ResourceLimit`: le chiavi distinte della destra superano il margine di
  memoria che il runner passa al kernel (`max_governed_memory_bytes`):
  ogni chiave conta la lunghezza della sua forma testuale più 64 byte;
- `Schema`: una cella di chiave non si converte in testo (`date32` o
  `timestamp(ms)` fuori dall'intervallo di calendario).

### Limiti e deviazioni

L'hash delle chiavi non ha seme
([README, «Hash delle chiavi non keyed»](../README.md#hash-delle-chiavi-non-keyed)).
La memoria contata è quella delle chiavi della destra, non quella della
mappa che le contiene.

### Complessità

Tempo O(n + m) atteso su righe sinistre e destre; memoria O(d) per le
chiavi distinte della destra, più O(r) per le righe rifiutate.

### Esempio

```json
{
  "config": {"left_keys": ["cliente"], "right_keys": ["id"], "allow_null": true},
  "ingressi": [
    {"nome": "ordini", "colonne": [
      {"nome": "ordine", "tipo": "int64", "valori": [101, 102, 103]},
      {"nome": "cliente", "tipo": "int64", "valori": [1, null, 1]}
    ]},
    {"nome": "clienti", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2]},
      {"nome": "nome", "tipo": "utf8", "valori": ["Anna", "Bruno"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "ordine", "tipo": "int64", "valori": [101, 102, 103]},
    {"nome": "cliente", "tipo": "int64", "valori": [1, null, 1]}
  ]}
}
```
