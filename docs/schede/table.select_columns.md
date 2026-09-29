### Che cosa fa

Tiene solo le colonne elencate in `columns`, nell'ordine in cui sono
elencate, e scarta le altre. È il contrario di
[`table.drop_columns`](#tabledrop_columns): qui un nome che non esiste è un
errore. I dati non si copiano.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `columns` | lista di stringhe | obbligatorio | colonne dell'ingresso, almeno una, senza ripetizioni | colonne da tenere, nell'ordine d'uscita |

### Schema

Le colonne elencate, nell'ordine di `columns`, con tipo, nullabilità e
metadati di campo; i metadati di schema restano.

Contratto: il conteggio delle righe resta; l'ordinamento dichiarato resta
solo se non si scarta nessuna colonna (una semplice permutazione), e cade
altrimenti. Se la colonna geometrica non è fra quelle tenute il contratto
diventa tabellare.

### Righe

1:1: stesse righe, stessi valori.

### Ordine

Righe nell'ordine d'ingresso; colonne nell'ordine di `columns`.

### Errori

In validazione, `InvalidPlan`:

- `columns` assente o vuota;
- un nome ripetuto o che non è una colonna dell'ingresso;
- config con campi sconosciuti.

In esecuzione: nessun errore che dipenda dai dati.

### Limiti e deviazioni

Nessuno oltre ai limiti comuni.

### Complessità

Tempo O(k) sulle colonne scelte, indipendente dalle righe; nessuna memoria
per i dati.

### Esempio

```json
{
  "config": {"columns": ["importo", "id"]},
  "ingressi": [
    {"nome": "ordini", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2]},
      {"nome": "note", "tipo": "utf8", "valori": ["urgente", null]},
      {"nome": "importo", "tipo": "float64", "valori": [5.5, null]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "importo", "tipo": "float64", "valori": [5.5, null]},
    {"nome": "id", "tipo": "int64", "valori": [1, 2]}
  ]}
}
```
