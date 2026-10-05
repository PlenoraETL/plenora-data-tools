### Che cosa fa

Scrive in una colonna `int64` la lunghezza del testo della colonna
`column`, contata in code point Unicode (non in byte e non in grafemi).
Il null dà null.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa | obbligatorio | colonna `utf8` dell'ingresso | testo da misurare |
| `output_column` | stringa | assente | nome non vuoto, al più 1024 byte; `null` non ammesso | colonna d'uscita; assente, `<column>_length` |

### Schema

La colonna d'uscita è `int64` nullable: se esiste già si sostituisce nella
sua posizione (con il tipo nuovo e senza i metadati di campo di prima),
altrimenti si aggiunge in coda. Le altre colonne e i metadati di schema
restano.

Contratto: il conteggio delle righe resta; l'ordinamento dichiarato resta
solo se l'uscita è una colonna nuova.

### Righe

1:1; il null resta null, il testo vuoto vale 0.

### Ordine

Righe nell'ordine d'ingresso.

### Errori

In validazione, `InvalidPlan`:

- `column` assente o non `utf8`;
- il nome d'uscita (scritto o derivato) vuoto, di soli spazi o oltre 1024
  byte; `output_column` scritto `null` (il parametro si omette);
- config con campi sconosciuti.

In esecuzione, `ResourceLimit`: una lunghezza che non sta in `int64`
(irraggiungibile con le stringhe Arrow `utf8`).

### Limiti e deviazioni

La lunghezza è in code point: `"é"` precomposta conta 1, `e` più accento
combinante conta 2.

### Complessità

Tempo O(b) sui byte della colonna; memoria O(n) per la colonna d'uscita.

### Esempio

```json
{
  "config": {"column": "nome"},
  "ingressi": [
    {"nome": "t", "colonne": [
      {"nome": "nome", "tipo": "utf8", "valori": ["città", "", null]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "nome", "tipo": "utf8", "valori": ["città", "", null]},
    {"nome": "nome_length", "tipo": "int64", "valori": [5, 0, null]}
  ]}
}
```
