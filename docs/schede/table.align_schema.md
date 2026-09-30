### Che cosa fa

Porta la tabella allo schema dichiarato in `columns`: le colonne escono
nell'ordine dichiarato; una colonna che esiste deve avere già il tipo
dichiarato (nessuna conversione implicita); una che manca si aggiunge,
tutta null oppure costante col `default`. Le colonne non dichiarate si
scartano, o con `keep_extra` si tengono in coda.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `columns` | lista di oggetti | obbligatorio | da 1 a 4096 colonne, nomi senza ripetizioni | schema d'uscita, nell'ordine d'uscita |
| `columns[].name` | stringa | obbligatorio | nome non vuoto, al più 1024 byte | nome della colonna |
| `columns[].type` | stringa | obbligatorio | `Utf8`, `Int64`, `UInt64`, `Float64`, `Boolean`, `Date32`, `Timestamp`, `Decimal128`, `Binary` | tipo della colonna (tabella sotto) |
| `columns[].default` | JSON | assente | valore convertibile nel tipo (sotto), solo su una colonna che manca nell'ingresso; `null` vale assente | valore di ogni cella di una colonna aggiunta |
| `keep_extra` | booleano | `false` | `true`, `false`; scritto solo se almeno una colonna d'ingresso non è dichiarata | tiene in coda, nell'ordine d'ingresso, le colonne non dichiarate |

I tipi: `Utf8` → `utf8`, `Int64` → `int64`, `UInt64` → `uint64`,
`Float64` → `float64`, `Boolean` → `bool`, `Date32` → `date32`,
`Timestamp` → `timestamp(ms)` senza fuso, `Decimal128` →
`decimal128(38, 10)`, `Binary` → `binary`. Il nome del tipo si scrive
esattamente così.

Il `default` si converte così, e ciò che non si converte si rifiuta:

- `Utf8`, `Binary`: solo una stringa JSON di al più `max_string_bytes`
  byte (per `Binary`, i suoi byte UTF-8);
- `Int64`, `UInt64`: un intero JSON nel dominio del tipo, o una stringa
  che lo è (spazi ai lati ignorati); `1.0` si rifiuta;
- `Float64`: un numero JSON, o una stringa con la virgola decimale
  ammessa (`"2,5"` vale 2,5);
- `Boolean`: `true`/`false` JSON, o le stringhe `"true"`/`"false"` in
  qualunque combinazione di maiuscole;
- `Date32`: una stringa `AAAA-MM-GG`;
- `Timestamp`: una stringa RFC 3339 con fuso (`"2026-07-25T00:00:00Z"`),
  convertita all'istante in millisecondi (la parte sotto il millisecondo si
  scarta);
- `Decimal128`: un numero JSON o una stringa, senza esponente e con al più
  10 cifre decimali (nessun arrotondamento).

Il `default` di una colonna che esiste già non avrebbe effetto e si
rifiuta. `keep_extra` scritto, con qualunque valore, quando ogni colonna
d'ingresso è dichiarata non avrebbe effetto e si rifiuta.

### Schema

Le colonne dichiarate, nell'ordine di `columns`, poi, con `keep_extra`, le
altre colonne dell'ingresso nel loro ordine. Una colonna che esiste resta
identica (tipo, nullabilità, metadati di campo); una aggiunta senza
`default` è nullable e tutta null; una aggiunta con `default` non è
nullable. I metadati di schema restano.

Contratto: il conteggio delle righe resta; l'ordinamento dichiarato resta
solo se nessuna colonna è stata aggiunta né scartata. Se la colonna
geometrica è scartata il contratto diventa tabellare.

### Righe

1:1: stesse righe; le colonne esistenti hanno gli stessi valori.

### Ordine

Righe nell'ordine d'ingresso; colonne come descritto sopra.

### Errori

In validazione, `InvalidPlan`:

- `columns` assente, vuota o con più di 4096 colonne;
- un nome ripetuto, vuoto, di soli spazi o oltre 1024 byte;
- una colonna esistente di tipo diverso da quello dichiarato (anche un
  `timestamp(ms)` con fuso, o un decimale di precisione o scala diverse);
- un `default` non convertibile nel tipo, o un `default` `Utf8` o
  `Binary` oltre `max_string_bytes` byte;
- un `default` su una colonna che esiste già nell'ingresso;
- `keep_extra` scritto (`true` o `false`) quando ogni colonna d'ingresso
  è dichiarata;
- un `type` fuori elenco, config con campi sconosciuti.

In esecuzione: nessun errore che dipenda dai dati.

### Limiti e deviazioni

- **Nessuna conversione implicita**: per cambiare il tipo di una colonna
  esistente serve [`table.type_cast`](#tabletype_cast) prima.
- **Segni ripetuti nel `default` `Decimal128`**: i segni iniziali si
  accettano tutti e conta solo il primo carattere (`"--5"` vale -5,
  `"+-5"` vale 5).

### Complessità

Tempo O(c) sulle colonne più O(n) per ogni colonna aggiunta; memoria O(n)
per ogni colonna aggiunta, nessuna per quelle esistenti (array condivisi).

### Esempio

```json
{
  "config": {"columns": [
    {"name": "id", "type": "Int64"},
    {"name": "stato", "type": "Utf8", "default": "nuovo"},
    {"name": "sconto", "type": "Float64"}
  ]},
  "ingressi": [
    {"nome": "ordini", "colonne": [
      {"nome": "note", "tipo": "utf8", "valori": ["urgente", null]},
      {"nome": "id", "tipo": "int64", "valori": [1, 2]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2]},
    {"nome": "stato", "tipo": "utf8", "valori": ["nuovo", "nuovo"]},
    {"nome": "sconto", "tipo": "float64", "valori": [null, null]}
  ]}
}
```
