### Che cosa fa

Toglie dalla tabella le colonne elencate in `columns` e lascia le altre
come sono, nello stesso ordine. Un nome che non è una colonna dell'ingresso
si ignora. Le colonne che restano non si copiano: l'uscita condivide gli
array dell'ingresso.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `columns` | lista di stringhe | obbligatorio | nomi non vuoti (non di soli spazi), al più 1024 byte ciascuno, senza ripetizioni, al più 4096 | colonne da togliere |

Una lista vuota è accettata e non toglie niente.

### Schema

Le colonne non elencate restano nell'ordine d'ingresso, con tipo,
nullabilità e metadati di campo; i metadati di schema restano. Se le
colonne tolte sono tutte, l'uscita ha zero colonne e lo stesso numero di
righe dell'ingresso.

Contratto: il conteggio delle righe resta; l'ordinamento dichiarato
(`sorted_by`) cade se almeno una colonna è stata tolta davvero, anche se
non era una chiave. Se si toglie la colonna geometrica il contratto diventa
tabellare.

### Righe

1:1: stesse righe, stessi valori.

### Ordine

Righe nell'ordine d'ingresso; colonne rimaste nell'ordine d'ingresso.

### Errori

In validazione, `InvalidPlan`:

- `columns` assente, con un nome ripetuto, vuoto, di soli spazi o oltre
  1024 byte, o con più di 4096 nomi;
- config con campi sconosciuti.

In esecuzione: nessun errore che dipenda dai dati.

### Limiti e deviazioni

Nessuno oltre ai limiti comuni.

### Complessità

Tempo O(c) sulle colonne dello schema, indipendente dalle righe; nessuna
memoria per i dati (array condivisi).

### Esempio

```json
{
  "config": {"columns": ["note", "assente"]},
  "ingressi": [
    {"nome": "ordini", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2]},
      {"nome": "note", "tipo": "utf8", "valori": ["urgente", null]},
      {"nome": "importo", "tipo": "float64", "valori": [5.5, 12.0]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2]},
    {"nome": "importo", "tipo": "float64", "valori": [5.5, 12.0]}
  ]}
}
```
