### Che cosa fa

Cambia il nome delle colonne secondo le coppie `old_name` → `new_name`.
Le rinomine valgono tutte insieme, quindi si possono scambiare due nomi. Le
colonne non nominate restano come sono; un `old_name` che non è una
colonna dell'ingresso si ignora. I dati non si copiano.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `renames` | lista di oggetti | obbligatorio | al più 4096 coppie | rinomine da applicare |
| `renames[].old_name` | stringa | obbligatorio | nome non vuoto, al più 1024 byte, mai ripetuto fra le coppie | colonna da rinominare |
| `renames[].new_name` | stringa | obbligatorio | nome non vuoto, al più 1024 byte, mai ripetuto fra le coppie | nuovo nome |

Una lista vuota è accettata e non cambia niente. I nomi dell'uscita devono
essere tutti diversi: un `new_name` uguale al nome di una colonna che resta
com'è si rifiuta.

### Schema

Stesse colonne, nello stesso ordine, con tipo, nullabilità e metadati di
campo; cambiano solo i nomi. I metadati di schema restano.

Contratto: conteggio delle righe e ordinamento dichiarato restano, anche
quando una chiave dell'ordinamento è stata rinominata; la colonna
geometrica resta tale col nuovo nome.

### Righe

1:1: stesse righe, stessi valori.

### Ordine

Righe e colonne nell'ordine d'ingresso.

### Errori

In validazione, `InvalidPlan`:

- lo stesso `old_name` in due coppie, o lo stesso `new_name` in due coppie;
- un nome vuoto, di soli spazi o oltre 1024 byte (in entrambe le
  posizioni), o più di 4096 coppie;
- l'uscita avrebbe due colonne con lo stesso nome;
- config con campi sconosciuti.

In esecuzione: nessun errore che dipenda dai dati.

### Limiti e deviazioni

Nessuno oltre ai limiti comuni.

### Complessità

Tempo O(c + r) sulle colonne e sulle coppie, indipendente dalle righe;
nessuna memoria per i dati.

### Esempio

```json
{
  "config": {"renames": [
    {"old_name": "a", "new_name": "b"},
    {"old_name": "b", "new_name": "a"},
    {"old_name": "cod", "new_name": "codice"}
  ]},
  "ingressi": [
    {"nome": "t", "colonne": [
      {"nome": "a", "tipo": "int64", "valori": [1, 2]},
      {"nome": "b", "tipo": "utf8", "valori": ["x", null]},
      {"nome": "cod", "tipo": "utf8", "valori": ["K1", "K2"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "b", "tipo": "int64", "valori": [1, 2]},
    {"nome": "a", "tipo": "utf8", "valori": ["x", null]},
    {"nome": "codice", "tipo": "utf8", "valori": ["K1", "K2"]}
  ]}
}
```
