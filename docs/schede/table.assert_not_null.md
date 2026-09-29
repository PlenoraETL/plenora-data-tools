### Che cosa fa

Verifica che nessuna delle colonne `columns` contenga un null. Se
l'asserzione regge, l'uscita è l'ingresso invariato; se una riga ha un null
in una di quelle colonne, il passo fallisce con una diagnostica che indica
quali righe e per quale colonna, senza produrre uscita. Conta il null logico:
anche una voce di dizionario che punta a un valore nullo.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `columns` | lista di stringhe | obbligatorio | almeno un nome, senza ripetizioni, al più 4096; colonne dell'ingresso di qualunque tipo | colonne che non devono contenere null |

### Schema

Identico all'ingresso: stesse colonne, tipi, nullabilità, metadati di campo
e di schema; il contratto conserva ordinamento dichiarato (`sorted_by`) e
numero di righe. La nullabilità dichiarata non cambia: l'asserzione non la
restringe a `false`.

### Righe

1:1 se l'asserzione regge: tutte le righe, invariate. Altrimenti nessuna
uscita; nessuna riga viene scartata.

### Ordine

L'ordine d'ingresso.

### Errori

In validazione, `InvalidPlan`:

- `columns` vuoto, con nomi ripetuti o non validi, o oltre 4096 nomi;
- una colonna non esiste nell'ingresso;
- l'ingresso viene da un passo che cambia numero o ordine delle righe
  (`filter`, `sort`, `aggregate`…): la diagnostica per riga ha bisogno
  degli indici della sorgente;
- config con campi sconosciuti.

In esecuzione, `DataMapping` con diagnostica per riga: una o più righe hanno
un null in una colonna di `columns`. Ogni riga conta una volta, con la
prima colonna di `columns` in cui è nulla; causa
`validation.required_value_missing`. La diagnostica dà il conteggio per
causa e fino a 10 esempi in ordine di riga, con l'indice (da zero) della
riga nella sorgente e il nome della colonna; mai i valori.

### Limiti e deviazioni

Nessuno oltre ai limiti comuni.

### Complessità

Tempo O(n·c) su righe e colonne controllate; memoria O(r) per le righe
rifiutate, l'uscita condivide le colonne dell'ingresso.

### Esempio

```json
{
  "config": {"columns": ["id"]},
  "ingressi": [
    {"nome": "clienti", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
      {"nome": "email", "tipo": "utf8", "valori": ["a@x.it", null, "c@x.it"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
    {"nome": "email", "tipo": "utf8", "valori": ["a@x.it", null, "c@x.it"]}
  ]}
}
```
