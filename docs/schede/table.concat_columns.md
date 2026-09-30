### Che cosa fa

Unisce riga per riga il testo delle colonne `columns`, nell'ordine dato e
con `separator` fra una parte e l'altra, in una colonna `utf8`. I null si
saltano (default) o contano come testo vuoto.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `columns` | lista di stringhe | obbligatorio | colonne `utf8` dell'ingresso, almeno una, senza ripetizioni, al più 4096 | parti da unire, nell'ordine |
| `output_column` | stringa | `"concatenated"` | nome non vuoto, al più 1024 byte | colonna d'uscita |
| `separator` | stringa | `" "` | al più `max_string_bytes` byte, anche vuota; solo con almeno due colonne; `null` non ammesso | testo messo fra due parti |
| `skip_null` | booleano | `true` | `true`, `false` | salta i null invece di trattarli come testo vuoto |

Con `skip_null` il separatore sta solo fra le parti non nulle, e una riga
di soli null dà null. Senza, un null vale `""` e il separatore resta
(`"a"`, null, `"b"` con `"-"` dà `"a--b"`); il risultato non è mai null.

### Schema

La colonna `output_column`, `utf8` nullable: se esiste già si sostituisce
nella sua posizione (perde tipo e metadati di campo di prima), altrimenti
si aggiunge in coda. Le altre colonne e i metadati di schema restano.

Contratto: il conteggio delle righe resta; l'ordinamento dichiarato resta
se `output_column` è una colonna nuova, cade se ne sostituisce una.

### Righe

1:1: una riga d'uscita per riga d'ingresso.

### Ordine

Righe nell'ordine d'ingresso.

### Errori

In validazione, `InvalidPlan`:

- `columns` assente, vuota, con un nome ripetuto o con più di 4096 nomi;
- una colonna di `columns` assente o non `utf8`;
- `separator` oltre `max_string_bytes`, `null` esplicito (il parametro si
  omette), o scritto con una sola colonna in `columns` (non avrebbe
  effetto);
- `output_column` vuoto, di soli spazi o oltre 1024 byte;
- config con campi sconosciuti.

In esecuzione, `ResourceLimit`: un valore unito oltre `max_string_bytes`
byte.

### Limiti e deviazioni

Solo colonne `utf8`: un numero o una data vanno prima convertiti in testo
con [`table.type_cast`](#tabletype_cast).

### Complessità

Tempo O(n·k) per n righe e k colonne, più i byte copiati; memoria pari ai
byte della colonna d'uscita.

### Esempio

```json
{
  "config": {"columns": ["nome", "secondo", "cognome"], "output_column": "completo"},
  "ingressi": [
    {"nome": "persone", "colonne": [
      {"nome": "nome", "tipo": "utf8", "valori": ["Anna", "Luca", null]},
      {"nome": "secondo", "tipo": "utf8", "valori": ["Maria", null, null]},
      {"nome": "cognome", "tipo": "utf8", "valori": ["Rossi", "Bianchi", null]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "nome", "tipo": "utf8", "valori": ["Anna", "Luca", null]},
    {"nome": "secondo", "tipo": "utf8", "valori": ["Maria", null, null]},
    {"nome": "cognome", "tipo": "utf8", "valori": ["Rossi", "Bianchi", null]},
    {"nome": "completo", "tipo": "utf8", "valori": ["Anna Maria Rossi", "Luca Bianchi", null]}
  ]}
}
```
