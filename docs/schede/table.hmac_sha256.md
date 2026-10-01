### Che cosa fa

Aggiunge a ogni riga l'HMAC-SHA256 (RFC 2104), in esadecimale minuscolo,
dei valori di alcune colonne, con una chiave segreta letta da una variabile
d'ambiente. È una pseudonimizzazione: chi non ha la chiave non può
ricalcolare il valore da un dato noto. Il piano contiene solo il nome della
variabile, mai la chiave.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `columns` | lista di stringhe | obbligatorio | da 1 a `max_columns` colonne leggibili come testo, senza ripetizioni | colonne del messaggio, nell'ordine scritto |
| `key_env` | stringa | obbligatorio | nome di variabile d'ambiente, non vuoto | variabile che contiene la chiave |
| `output_column` | stringa | `hmac` | nome valido | colonna d'uscita |
| `null_policy` | stringa | `empty` | `empty`, `null`, `skip` | come entra una cella nulla |

Leggibile come testo: `utf8`, `int64`, `uint64`, `float64`, `bool`,
`date32`, `date64` (allineato al giorno), `timestamp` di ogni unità, `decimal128` con scala da 0 a 38, `binary`,
`dictionary<utf8>` con chiavi `int32`.

La chiave sono i byte UTF-8 del valore della variabile. Il messaggio di una
riga, con `f(x)` = lunghezza di x in 8 byte big-endian seguita da x:

```text
"plenora-hmac-sha256-v1\0"
per ogni colonna, nell'ordine di columns:
  f(nome) f(tipo Arrow, es. "Utf8") 0x01 f(testo della cella)
```

Una cella nulla con `empty` vale il testo vuoto; con `null` rende nulla
l'uscita della riga; con `skip` la colonna manca dal messaggio (intestazione
compresa). Nessuna politica rifiuta la riga.

### Schema

La colonna d'uscita è `utf8`, 64 cifre esadecimali, nullable solo con
`null_policy` `null`: si aggiunge in coda o sostituisce al suo posto una
colonna con lo stesso nome (perdendone tipo e metadati di campo). Metadati
di schema conservati; `row_count` resta; `sorted_by` resta solo se nessuna
colonna esistente è sovrascritta.

### Righe

1:1.

### Ordine

Invariato.

### Errori

In validazione, `InvalidPlan`:

- `key_env` vuoto o di soli spazi;
- `columns` vuoto, con ripetizioni o con più di `max_columns` colonne;
- una colonna assente o non leggibile come testo;
- `output_column` non valido;
- config con campi sconosciuti o `null_policy` fuori elenco;
- (dal runner) la variabile `key_env` non esiste, è vuota o il suo valore
  non è UTF-8 valido: tre cause distinte, lette dalla stessa funzione che
  usa il kernel. Il messaggio non nomina la variabile e non contiene la
  chiave.

In esecuzione:

- `InvalidPlan`: la chiave non è disponibile (variabile rimossa, svuotata
  o resa non UTF-8 dopo la validazione), con le stesse tre cause. Il
  messaggio non nomina la variabile e non contiene la chiave;
- `Schema`: una cella che non si converte in testo (`binary` non UTF-8,
  data o istante fuori intervallo). Con `null` le colonne dopo la prima
  nulla di una riga non si leggono.

### Limiti e deviazioni

La variabile d'ambiente si controlla in validazione, ma può cambiare prima
dell'esecuzione: allora l'errore arriva al passo
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner)).
Con `empty` una cella nulla e una vuota danno lo stesso valore (con `skip`
no: la colonna nulla manca dal messaggio). Nome e tipo Arrow
entrano nel messaggio: rinominare o cambiare tipo cambia il risultato.

### Complessità

Tempo O(byte delle colonne scelte), in parallelo per blocchi di righe;
memoria O(n) per la colonna d'uscita.

### Esempio

La chiave è il valore della variabile `PLENORA_ESEMPIO_CHIAVE_HMAC`, che la
prova imposta a `chiave-di-esempio`.

```json
{
  "config": {"columns": ["cliente"], "key_env": "PLENORA_ESEMPIO_CHIAVE_HMAC", "null_policy": "null"},
  "ingressi": [
    {"nome": "ordini", "colonne": [
      {"nome": "cliente", "tipo": "utf8", "valori": ["C001", null]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "cliente", "tipo": "utf8", "valori": ["C001", null]},
    {"nome": "hmac", "tipo": "utf8", "valori": ["5a3b8265221dd25c118b9f3d07d05a346737a1a5417ef81b6a60f960e0c44e86", null]}
  ]}
}
```
