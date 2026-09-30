### Che cosa fa

Allunga il testo della colonna `column` fino a `width` caratteri,
aggiungendo `fill_char` a sinistra (default) o a destra. Un testo lungo
già almeno `width` caratteri resta com'è, senza troncamento. I caratteri
sono code point Unicode, non byte e non grafemi.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa | obbligatorio | colonna `utf8` dell'ingresso | testo da allungare |
| `width` | intero | `5` | da 1 a `max_string_bytes` | lunghezza minima in caratteri |
| `side` | stringa | `"left"` | `left`, `right` | lato su cui si aggiunge il riempimento |
| `fill_char` | stringa | `"0"` | esattamente un code point | carattere di riempimento |
| `output_column` | stringa o `null` | `null` | nome non vuoto, al più 1024 byte | colonna d'uscita; `null` sostituisce `column` |

### Schema

La colonna d'uscita è `utf8` nullable: se esiste già (anche `column`
stessa, il caso di default) si sostituisce nella sua posizione, perdendo i
metadati di campo, altrimenti si aggiunge in coda. Le altre colonne e i
metadati di schema restano.

Contratto: il conteggio delle righe resta; l'ordinamento dichiarato resta
solo se l'uscita è una colonna nuova.

### Righe

1:1; il null resta null.

### Ordine

Righe nell'ordine d'ingresso.

### Errori

In validazione, `InvalidPlan`:

- `column` assente o non `utf8`;
- `fill_char` vuoto o di più di un code point;
- `width` oltre `max_string_bytes` (o negativo, che la config non legge);
- `width = 0`: nessun testo si allungherebbe, e `side` e `fill_char` non
  avrebbero effetto;
- `output_column` vuoto, di soli spazi o oltre 1024 byte;
- `side` fuori elenco, config con campi sconosciuti.

In esecuzione, `ResourceLimit`: un valore allungato oltre
`max_string_bytes` byte (possibile con un `fill_char` di più byte).

### Limiti e deviazioni

La lunghezza si conta in code point: un carattere composto da lettera e
accento combinante conta due, un emoji composto conta quanti code point ha.

### Complessità

Tempo O(b) sui byte della colonna più il riempimento; memoria pari ai
byte della colonna d'uscita.

### Esempio

```json
{
  "config": {"column": "cap", "width": 5, "fill_char": "0"},
  "ingressi": [
    {"nome": "indirizzi", "colonne": [
      {"nome": "cap", "tipo": "utf8", "valori": ["186", "20121", null, "123456"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "cap", "tipo": "utf8", "valori": ["00186", "20121", null, "123456"]}
  ]}
}
```
