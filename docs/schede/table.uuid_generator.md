### Che cosa fa

Aggiunge una colonna con un UUID versione 4 casuale per riga, in forma
testuale minuscola con i trattini (36 caratteri,
`xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx`). I valori cambiano a ogni
esecuzione: il risultato non dipende solo dall'ingresso.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `output_column` | stringa | `"uuid"` | nome non vuoto, al più 1024 byte | colonna d'uscita |

### Schema

La colonna `output_column`, `utf8` non nullable: se esiste già si
sostituisce nella sua posizione (senza i metadati di campo di prima),
altrimenti si aggiunge in coda. Le altre colonne e i metadati di schema
restano.

Contratto: il conteggio delle righe resta; l'ordinamento dichiarato resta
solo se `output_column` è una colonna nuova.

### Righe

1:1: un UUID per riga, anche per le righe con valori null.

### Ordine

Righe nell'ordine d'ingresso.

### Errori

In validazione, `InvalidPlan`: `output_column` vuoto, di soli spazi o
oltre 1024 byte; config con campi sconosciuti.

In esecuzione: nessun errore che dipenda dai dati.

### Limiti e deviazioni

- **Non deterministica per contratto**: due esecuzioni dello stesso piano
  danno UUID diversi. L'esempio qui sotto confronta solo colonne, tipi e
  numero di righe.
- L'unicità è probabilistica (122 bit casuali), non verificata.

### Complessità

Tempo O(n); memoria O(n) per la colonna d'uscita (36 byte per riga).

### Esempio

```json
{
  "config": {"output_column": "chiave"},
  "ingressi": [
    {"nome": "t", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2]},
    {"nome": "chiave", "tipo": "utf8", "valori": ["9b2f4c1e-3a7d-4e2b-8f6a-0c5d1e2f3a4b", "e1d2c3b4-a5f6-4789-9abc-def012345678"]}
  ]},
  "valori_confrontati": false
}
```
