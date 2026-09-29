### Che cosa fa

Riga per riga, prende il primo valore non nullo fra le colonne `columns`,
nell'ordine in cui sono elencate, e lo scrive in `output_column`. Se tutte
sono nulle, il risultato è null. Le colonne devono avere lo stesso tipo.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `columns` | lista di stringhe | obbligatorio | almeno un nome, senza ripetizioni, al più 4096; colonne dell'ingresso con tipi Arrow identici | colonne da cui prendere il valore, in ordine di precedenza |
| `output_column` | stringa | obbligatorio | nome non vuoto, al più 1024 byte | colonna del risultato |

Il null è quello logico: una voce di dizionario che punta a un valore nullo
conta come null e si passa alla colonna successiva.

### Schema

`output_column` ha il tipo comune delle colonne `columns` (anche un
dizionario, una lista o una struct) ed è sempre nullabile. Se il nome esiste
già, anche se è una delle colonne di `columns`, la colonna si sostituisce
nella sua posizione e perde i propri metadati di campo; altrimenti si
aggiunge in coda. Le altre colonne restano invariate, e così i metadati di
schema. Se la colonna sostituita è la geometria, l'uscita non ha più una
colonna geometrica.

Il contratto conserva il numero di righe; l'ordinamento dichiarato
(`sorted_by`) resta solo se nessuna colonna è stata sostituita.

### Righe

1:1: una riga d'uscita per riga d'ingresso.

### Ordine

L'ordine d'ingresso.

### Errori

In validazione, `InvalidPlan`:

- `columns` vuoto, con nomi ripetuti o non validi, o oltre 4096 nomi;
- una colonna non esiste, o i tipi Arrow non sono identici (anche
  precisione, scala, fuso orario);
- `output_column` vuoto o oltre 1024 byte;
- config con campi sconosciuti.

In esecuzione: nessuno che dipenda dai dati. `ResourceLimit` se gli indici
interni del percorso generico traboccano (righe per colonne oltre
`usize`), e `DataMapping` (`arrow error`) se Arrow non riesce a concatenare
le colonne: entrambi fuori dai volumi ammessi dai limiti.

### Limiti e deviazioni

Nessuno oltre ai limiti comuni.

### Complessità

Tempo O(n·c) su righe e colonne elencate; memoria O(n) per la colonna
prodotta. Per i tipi fuori da `int64`, `uint64`, `float64`, `bool` e `utf8`
il percorso generico concatena le colonne: memoria O(n·c).

### Esempio

```json
{
  "config": {"columns": ["cellulare", "fisso"], "output_column": "telefono"},
  "ingressi": [
    {"nome": "contatti", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
      {"nome": "cellulare", "tipo": "utf8", "valori": ["333 1", null, null]},
      {"nome": "fisso", "tipo": "utf8", "valori": ["02 9", "06 5", null]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
    {"nome": "cellulare", "tipo": "utf8", "valori": ["333 1", null, null]},
    {"nome": "fisso", "tipo": "utf8", "valori": ["02 9", "06 5", null]},
    {"nome": "telefono", "tipo": "utf8", "valori": ["333 1", "06 5", null]}
  ]}
}
```
