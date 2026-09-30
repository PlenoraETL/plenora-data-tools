### Che cosa fa

Cambia l'ordine delle colonne senza toglierne né aggiungerne: prima quelle
elencate in `columns`, nell'ordine dato, poi tutte le altre, nell'ordine
d'ingresso oppure, con `alphabetical`, in ordine alfabetico. I dati non si
copiano.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `columns` | lista di stringhe | `[]` | colonne dell'ingresso, senza ripetizioni; vuota solo con `alphabetical = true` | colonne da mettere in testa, nell'ordine dato |
| `alphabetical` | booleano | `false` | `true`, `false`; alias `sort_alphabetical`; solo con almeno due colonne d'ingresso non elencate | ordina alfabeticamente le colonne non elencate |

`alphabetical` riguarda solo le colonne non elencate in `columns`. L'ordine
alfabetico confronta i nomi in minuscolo (minuscole Unicode) byte per byte
in UTF-8, quindi le lettere accentate vanno dopo la `z`; due nomi uguali in
minuscolo restano nell'ordine d'ingresso.

Un parametro senza effetto si rifiuta: `columns` vuota senza
`alphabetical = true` non sposta niente; `alphabetical` scritto (con
qualunque valore) quando al più una colonna d'ingresso non è elencata in
`columns` non ordina niente.

### Schema

Stesse colonne, con tipo, nullabilità e metadati di campo; cambia solo la
posizione. I metadati di schema restano. Il contratto conserva il
conteggio delle righe e l'ordinamento dichiarato.

### Righe

1:1: stesse righe, stessi valori.

### Ordine

Righe nell'ordine d'ingresso; colonne come descritto sopra.

### Errori

In validazione, `InvalidPlan`:

- un nome di `columns` ripetuto o che non è una colonna dell'ingresso;
- `columns` vuota senza `alphabetical = true`;
- `alphabetical` (o `sort_alphabetical`) scritto quando al più una colonna
  d'ingresso non è elencata in `columns`;
- config con campi sconosciuti.

In esecuzione: nessun errore che dipenda dai dati.

### Limiti e deviazioni

Nessuno oltre ai limiti comuni.

### Complessità

Tempo O(c log c) sulle colonne, indipendente dalle righe; nessuna memoria
per i dati.

### Esempio

```json
{
  "config": {"columns": ["id"], "alphabetical": true},
  "ingressi": [
    {"nome": "t", "colonne": [
      {"nome": "zona", "tipo": "utf8", "valori": ["N", "S"]},
      {"nome": "Anno", "tipo": "int64", "valori": [2023, 2024]},
      {"nome": "id", "tipo": "int64", "valori": [1, 2]},
      {"nome": "citta", "tipo": "utf8", "valori": ["Roma", null]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2]},
    {"nome": "Anno", "tipo": "int64", "valori": [2023, 2024]},
    {"nome": "citta", "tipo": "utf8", "valori": ["Roma", null]},
    {"nome": "zona", "tipo": "utf8", "valori": ["N", "S"]}
  ]}
}
```
