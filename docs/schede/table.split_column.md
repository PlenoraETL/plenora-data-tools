### Che cosa fa

Divide il testo della colonna `column` sul delimitatore `delimiter`, da
sinistra, e scrive le parti nelle colonne `new_columns`, una parte per
colonna. Le parti sono al più tante quante le colonne: l'ultima tiene il
resto del testo, delimitatori compresi, e nessun carattere si perde; le
colonne senza parte ricevono null.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa | obbligatorio | colonna `utf8` dell'ingresso | testo da dividere |
| `delimiter` | stringa | `","` | non vuota, al più `max_string_bytes` byte | separatore, testo letterale (non regex) |
| `new_columns` | lista di stringhe | obbligatorio | da 1 a 256 nomi, senza ripetizioni, ciascuno non vuoto e al più 1024 byte | colonne d'uscita, nell'ordine delle parti |
| `max_splits` | intero | `-1` | qualunque intero a 64 bit | se positivo, al più `max_splits` divisioni, cioè `max_splits + 1` parti |

Con `max_splits` non positivo (default) le parti sono al più
`len(new_columns)`. Con `max_splits` positivo sono al più
`min(max_splits + 1, len(new_columns))`: con tre colonne e `max_splits = 1`,
`"a,b,c"` dà `"a"`, `"b,c"`, null.

### Schema

Ogni colonna di `new_columns` è `utf8` nullable: se esiste già (anche
`column` stessa) si sostituisce nella sua posizione, perdendo i metadati di
campo, altrimenti si aggiunge in coda nell'ordine di `new_columns`. Le
altre colonne e i metadati di schema restano.

Contratto: il conteggio delle righe resta; l'ordinamento dichiarato resta
solo se nessuna colonna esistente è stata sostituita.

### Righe

1:1. Una cella null dà null in tutte le colonne d'uscita; il testo vuoto dà
`""` nella prima e null nelle altre.

### Ordine

Righe nell'ordine d'ingresso.

### Errori

In validazione, `InvalidPlan`:

- `column` assente o non `utf8`;
- `delimiter` vuoto o oltre `max_string_bytes`;
- `new_columns` assente, vuota, con più di 256 nomi, con un nome ripetuto,
  vuoto, di soli spazi o oltre 1024 byte;
- config con campi sconosciuti.

In esecuzione: nessun errore che dipenda dai dati.

### Limiti e deviazioni

Al più 256 colonne d'uscita per passo, limite interno dei kernel che il
piano non può cambiare.

### Complessità

Tempo O(b) sui byte della colonna; memoria pari ai byte delle colonne
d'uscita (le parti si copiano).

### Esempio

```json
{
  "config": {"column": "codice", "delimiter": "-", "new_columns": ["area", "resto"]},
  "ingressi": [
    {"nome": "t", "colonne": [
      {"nome": "codice", "tipo": "utf8", "valori": ["MI-2024-07", "RM", null]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "codice", "tipo": "utf8", "valori": ["MI-2024-07", "RM", null]},
    {"nome": "area", "tipo": "utf8", "valori": ["MI", "RM", null]},
    {"nome": "resto", "tipo": "utf8", "valori": ["2024-07", null, null]}
  ]}
}
```
