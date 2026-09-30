### Che cosa fa

Aggiunge una colonna `int64` con il numero progressivo di ogni riga
nell'ordine d'ingresso, a partire da `start`. Con `partition_column` la
numerazione riparte da `start` per ogni valore distinto di quella colonna.
L'ordine è quello delle righe: per numerare secondo un ordinamento serve
prima un `table.sort`.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `output_column` | stringa | `"row_number"` | nome non vuoto, al più 1024 byte | colonna d'uscita |
| `start` | intero | `1` | intero a 64 bit | numero della prima riga (di ogni partizione) |
| `partition_column` | stringa o `null` | `null` | colonna dell'ingresso leggibile come testo | colonna le cui righe uguali formano una partizione |
| `order_column` | stringa o `null` | `null` | solo `null` | non supportato: scritto, si rifiuta |
| `ascending` | booleano o `null` | `null` | solo `null` | vale solo con `order_column`: scritto, si rifiuta |

Le partizioni si distinguono per il testo della cella (lo stesso di
[`table.type_cast`](#tabletype_cast) verso `str`); tutte le celle null
formano una partizione sola. Per un `float64` `0.0` e `-0.0` hanno testi
diversi e sono partizioni diverse.

### Schema

La colonna `output_column`, `int64` non nullable: se esiste già si
sostituisce nella sua posizione (senza i metadati di campo di prima),
altrimenti si aggiunge in coda. Le altre colonne e i metadati di schema
restano.

Contratto: il conteggio delle righe resta; l'ordinamento dichiarato resta
solo se `output_column` è una colonna nuova.

### Righe

1:1: ogni riga riceve un numero.

### Ordine

Righe nell'ordine d'ingresso; i numeri crescono di 1 nell'ordine delle
righe, dentro ogni partizione.

### Errori

In validazione, `InvalidPlan`:

- `output_column` vuoto, di soli spazi o oltre 1024 byte;
- `order_column` scritto (non nullo);
- `ascending` scritto;
- `partition_column` assente o di un tipo che non si legge come testo;
- config con campi sconosciuti.

In esecuzione:

- `ResourceLimit`: un numero oltre `i64::MAX` (con `start` vicino al
  massimo). Con e senza `partition_column` il numero vale `start` più la
  posizione della riga (nella partizione): `i64::MAX` si raggiunge, solo
  il numero successivo fallisce;
- `Schema`: una cella di `partition_column` che non si legge come testo.

### Limiti e deviazioni

Nessuna numerazione ordinata: `order_column` e `ascending` restano nella
config per compatibilità, ma si rifiutano.

### Complessità

Tempo O(n); memoria O(n) per la colonna d'uscita, più con
`partition_column` un contatore per partizione distinta con il testo della
sua chiave.

### Esempio

```json
{
  "config": {"partition_column": "reparto", "start": 1, "output_column": "n"},
  "ingressi": [
    {"nome": "dipendenti", "colonne": [
      {"nome": "reparto", "tipo": "utf8", "valori": ["A", "B", "A", null, "A"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "reparto", "tipo": "utf8", "valori": ["A", "B", "A", null, "A"]},
    {"nome": "n", "tipo": "int64", "valori": [1, 1, 2, 1, 3]}
  ]}
}
```
