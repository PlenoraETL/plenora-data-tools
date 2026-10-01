### Che cosa fa

Verifica che la chiave formata dalle colonne `columns` sia unica nella
tabella: nessuna coppia di righe ha gli stessi valori in tutte le colonne
della chiave. Se l'asserzione regge, l'uscita è l'ingresso invariato; se
ci sono duplicati, il passo fallisce con una diagnostica che indica tutte le
righe dei gruppi duplicati, senza produrre uscita.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `columns` | lista di stringhe | obbligatorio | almeno un nome, senza ripetizioni, al più 4096; colonne leggibili come testo scalare | colonne della chiave |
| `nulls_equal` | booleano | `true` | `true`, `false` | con `true` il null è un valore della chiave e due null sono uguali; con `false` le righe con un null in una colonna della chiave non si controllano |

Leggibili come testo scalare: `utf8`, `int64`, `uint64`, `float64`, `bool`,
`binary`, `date32`, `date64`,
`timestamp` di ogni unità (con fuso valido), `decimal128` (scala
da 0 a 38), dizionario `int32`→`utf8`.

Due valori sono uguali quando lo è la loro forma testuale: sugli interi, i
testi, le date e i decimali è l'uguaglianza dei valori; su `float64` tutti
i `NaN` sono uguali fra loro e `0.0` è diverso da `-0.0`; un `binary` si
confronta sui byte.

### Schema

Identico all'ingresso: stesse colonne, tipi, nullabilità, metadati di campo
e di schema; il contratto conserva ordinamento dichiarato (`sorted_by`) e
numero di righe.

### Righe

1:1 se l'asserzione regge: tutte le righe, invariate. Altrimenti nessuna
uscita; nessuna riga viene scartata.

### Ordine

L'ordine d'ingresso.

### Errori

In validazione, `InvalidPlan`:

- `columns` vuoto, con nomi ripetuti o non validi, o oltre 4096 nomi;
- una colonna non esiste o non è leggibile come testo scalare;
- config con campi sconosciuti.

In esecuzione:

- `DataMapping` con diagnostica per riga: almeno una chiave compare in più
  righe. Sono rifiutate tutte le righe di ogni gruppo duplicato, la prima
  compresa, con causa `validation.duplicate_key` e senza colonna. La
  diagnostica dà il conteggio delle righe e fino a 10 esempi in ordine di
  riga, con l'indice (da zero) della riga nella base del runner
  ([README, «Diagnostica per riga»](../README.md#diagnostica-per-riga));
  mai i valori;
- `Schema`: una cella della chiave non si converte in testo (`date32`, `date64` o
  `timestamp` fuori dall'intervallo di calendario, `date64` non allineato al giorno, chiave di dizionario
  fuori dal dizionario). Le righe saltate con `nulls_equal=false` non si
  convertono.

### Limiti e deviazioni

La mappa delle chiavi non si conta su `max_governed_memory_bytes`
([README, «Memoria delle chiavi dei kernel in memoria non governata»](../README.md#memoria-delle-chiavi-dei-kernel-in-memoria-non-governata));
l'hash delle chiavi non ha seme
([README, «Hash delle chiavi non keyed»](../README.md#hash-delle-chiavi-non-keyed)).

### Complessità

Tempo O(n·c) atteso su righe e colonne della chiave; memoria O(d) per le
chiavi distinte, più O(r) per le righe rifiutate.

### Esempio

Con `nulls_equal=false` i due null non sono un duplicato.

```json
{
  "config": {"columns": ["codice"], "nulls_equal": false},
  "ingressi": [
    {"nome": "prodotti", "colonne": [
      {"nome": "codice", "tipo": "utf8", "valori": ["A1", null, "B2", null]},
      {"nome": "prezzo", "tipo": "float64", "valori": [9.5, 3.0, 12.0, 3.0]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "codice", "tipo": "utf8", "valori": ["A1", null, "B2", null]},
    {"nome": "prezzo", "tipo": "float64", "valori": [9.5, 3.0, 12.0, 3.0]}
  ]}
}
```
