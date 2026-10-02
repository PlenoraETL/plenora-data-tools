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
| `columns[].default` | JSON | assente | valore convertibile nel tipo (sotto); `null` vale assente | valore di ogni cella di una colonna aggiunta |
| `keep_extra` | booleano | `false` | `true`, `false`; `null` non ammesso | tiene in coda, nell'ordine d'ingresso, le colonne non dichiarate |

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
  convertita all'istante in millisecondi; una parte sotto il millisecondo,
  una frazione oltre il nanosecondo e un secondo intercalare si rifiutano
  (il lettore di [Limiti dichiarati, «Colonne temporali e formati di data»](limiti.md#colonne-temporali-e-formati-di-data));
- `Decimal128`: un numero JSON o una stringa, senza esponente, con un
  segno facoltativo (uno solo: `"--5"` e `"+-5"` si rifiutano) e con al
  più 10 cifre decimali (nessun arrotondamento).

Si accetta, perché l'effetto dipende dall'ingresso e lo stesso piano gira
su tabelle diverse: il `default` di una colonna che esiste già (non si
legge) e `keep_extra` quando ogni colonna d'ingresso è dichiarata (non
tiene niente). `keep_extra: null` esplicito si rifiuta: il parametro si
omette.

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
- `keep_extra` `null` esplicito;
- un `type` fuori elenco, config con campi sconosciuti.

In esecuzione: nessun errore che dipenda dai dati.

### Limiti e deviazioni

- **Nessuna conversione implicita**: per cambiare il tipo di una colonna
  esistente serve [`table.type_cast`](#tabletype_cast) prima.

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
