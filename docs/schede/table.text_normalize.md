### Che cosa fa

Normalizza il testo delle colonne `columns` con una regola sola, scelta in
`operations`: toglie gli spazi ai lati, cambia maiuscole e minuscole, toglie
gli accenti, riduce gli spazi multipli, o fa tutto insieme (`full`, il
default). Il risultato sostituisce la colonna o va in `<colonna>_norm`.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `columns` | lista di stringhe | obbligatorio | colonne `utf8` dell'ingresso, almeno una, senza ripetizioni, al più 4096 | colonne da normalizzare |
| `operations` | stringa | `"full"` | `trim`, `lower`, `upper`, `title`, `strip_accents`, `strip_double_spaces`, `full` | la regola da applicare (una sola, nonostante il plurale) |
| `overwrite` | booleano | `true` | `true`, `false` | sostituisce la colonna; con `false` scrive `<colonna>_norm` |

Le regole, su ogni cella non nulla:

- `trim`: toglie gli spazi Unicode ai due lati;
- `lower`, `upper`: minuscole e maiuscole Unicode, che possono cambiare la
  lunghezza (`"straße"` in maiuscolo dà `"STRASSE"`);
- `title`: maiuscola la prima lettera o cifra di ogni parola e minuscole le
  altre; una parola comincia dopo ogni carattere che non è una lettera né
  una cifra (`"d'ÉCOLE"` dà `"D'École"`, `"rust_lang"` dà `"Rust_Lang"`);
- `strip_accents`: decomposizione NFKD e rimozione dei segni combinanti;
  NFKD scompone anche i caratteri di compatibilità (la legatura `ﬁ` diventa
  `fi`, lo spazio non separabile diventa uno spazio, `²` diventa `2`);
- `strip_double_spaces`: spezza sugli spazi Unicode e riunisce con uno
  spazio solo, quindi toglie anche gli spazi ai lati e trasforma tabulazioni
  e a capo in spazi;
- `full`: `trim`, poi `lower`, poi `strip_accents`, poi
  `strip_double_spaces`.

### Schema

Con `overwrite` ogni colonna si sostituisce nella sua posizione, `utf8`
nullable, senza i metadati di campo di prima. Senza, ogni
`<colonna>_norm` è `utf8` nullable, sostituita se esiste già, altrimenti
aggiunta in coda nell'ordine di `columns`. Le altre colonne e i metadati di
schema restano.

Contratto: il conteggio delle righe resta; l'ordinamento dichiarato resta
solo se nessuna colonna esistente è stata sostituita (quindi mai con
`overwrite`).

### Righe

1:1; il null resta null.

### Ordine

Righe nell'ordine d'ingresso.

### Errori

In validazione, `InvalidPlan`:

- `columns` assente, vuota, con un nome ripetuto o con più di 4096 nomi;
- una colonna assente o non `utf8`;
- un nome d'uscita `<colonna>_norm` oltre 1024 byte;
- `operations` fuori elenco, config con campi sconosciuti.

In esecuzione, `ResourceLimit`: un valore normalizzato oltre
`max_string_bytes` byte.

### Limiti e deviazioni

`strip_accents` e `full` usano NFKD, non NFD: oltre agli accenti cambiano
i caratteri di compatibilità, e un testo con legature o apici non torna
uguale.

### Complessità

Tempo O(b) sui byte delle colonne; memoria pari ai byte delle colonne
d'uscita.

### Esempio

```json
{
  "config": {"columns": ["citta"], "overwrite": false},
  "ingressi": [
    {"nome": "t", "colonne": [
      {"nome": "citta", "tipo": "utf8", "valori": ["  Forlì ", "SAN   Donà", null]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "citta", "tipo": "utf8", "valori": ["  Forlì ", "SAN   Donà", null]},
    {"nome": "citta_norm", "tipo": "utf8", "valori": ["forli", "san dona", null]}
  ]}
}
```
