### Che cosa fa

Prodotto cartesiano: ogni riga di sinistra affiancata a ogni riga di
destra. Non ci sono chiavi; le colonne con lo stesso nome nei due lati
prendono i suffissi `_x` (sinistra) e `_y` (destra).

### Parametri

Nessuno: la config è `{}`.

### Schema

Prima tutte le colonne di sinistra, poi tutte quelle di destra. Una colonna
di sinistra il cui nome compare anche a destra diventa `<nome>_x`, una di
destra il cui nome compare anche a sinistra `<nome>_y`; le altre tengono il
nome. Tipi e metadati di campo restano quelli d'origine; ogni colonna
d'uscita è nullable. I metadati di schema dei due lati si fondono: una
chiave presente da un lato solo, o con lo stesso valore, resta. L'uscita
ammette una sola colonna geometrica. Il conteggio delle righe del
contratto è il prodotto dei due, quando entrambi lo dichiarano con la
stessa portata e la stessa confidenza; l'ordinamento dichiarato no.

### Righe

`n·m` righe, una per ogni coppia (riga sinistra, riga destra). Con un lato
vuoto l'uscita è vuota.

### Ordine

Comanda la sinistra: per ogni riga di sinistra, nell'ordine d'ingresso,
tutte le righe di destra nel loro ordine.

### Errori

In validazione, `InvalidPlan`:

- nomi d'uscita che collidono dopo i suffissi (a sinistra `a` e `a_x`, a
  destra `a`), nome oltre 1024 byte, più colonne di `max_columns`;
- due colonne geometriche nell'uscita (una per lato);
- metadati di schema con la stessa chiave e valori diversi;
- config non vuota.

In esecuzione, `ResourceLimit`, prima di allocare l'uscita:

- `n·m` oltre `max_rows` (nel runner `max_input_rows`) o non
  rappresentabile;
- uscita stimata oltre `max_governed_memory_bytes`: righe per la somma
  delle larghezze di riga dei due lati più 32 byte di indici per riga.

### Limiti e deviazioni

Nessuno oltre ai limiti comuni.

### Complessità

Tempo e memoria O(n·m).

### Esempio

```json
{
  "config": {},
  "ingressi": [
    {"nome": "taglie", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2]},
      {"nome": "taglia", "tipo": "utf8", "valori": ["S", "M"]}
    ]},
    {"nome": "colori", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [10, 20]},
      {"nome": "colore", "tipo": "utf8", "valori": ["rosso", "blu"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id_x", "tipo": "int64", "valori": [1, 1, 2, 2]},
    {"nome": "taglia", "tipo": "utf8", "valori": ["S", "S", "M", "M"]},
    {"nome": "id_y", "tipo": "int64", "valori": [10, 20, 10, 20]},
    {"nome": "colore", "tipo": "utf8", "valori": ["rosso", "blu", "rosso", "blu"]}
  ]}
}
```
