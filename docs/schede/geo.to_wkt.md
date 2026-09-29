### Che cosa fa

Aggiunge una colonna `utf8` con il testo WKT della geometria di ogni riga.
Il testo è quello del crate `wkt` vendorizzato: nome del tipo attaccato
alla parentesi (`POINT(1.5 -2)`), virgole senza spazi fra i vertici, ogni
coordinata nella forma decimale più breve che rilegge lo stesso `f64`,
senza esponente e con il segno dello zero conservato (`-0`); le geometrie
vuote come `POINT EMPTY`, `MULTIPOLYGON EMPTY`.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `output_column` | stringa | `wkt` | nome non vuoto (non di soli spazi) e non già presente nell'ingresso | colonna aggiunta |

### Schema

Aggiunge in coda `output_column`, `utf8` nullable, senza metadati di
campo. Le altre colonne (geometria compresa), i metadati di schema e le
proprietà del contratto (`sorted_by`, `row_count`) passano invariati.

### Righe

1:1 per contratto. Il kernel (`operations::to_wkt`) lavora su una
geometria alla volta e nessun adapter lo chiama ancora sulle righe: il
trattamento di una cella nulla non è definito da codice eseguito.

### Ordine

L'ordine d'ingresso: l'analisi conserva `sorted_by`.

### Errori

In validazione (analisi del contratto):

- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non si riconosce come WKB (né estensione `geoarrow.wkb` né chiavi
  `plenora.geometry.*`); `output_column` esiste già;
- `Unsupported`: dimensioni della geometria diverse da `xy`;
- `Crs`: CRS della colonna assente o non risolto (ogni CRS risolto è
  ammesso);
- `InvalidPlan`: campi sconosciuti nella config, `output_column` vuoto o
  di soli spazi.

In esecuzione, dal kernel, per geometria (`OperationError`, che nessun
codice traduce ancora in `PlenoraError`):

- `InvalidInput`: la geometria non supera la validazione OGC;
- `ValidazioneNonConclusa`: la validazione OGC va in panico dentro la
  barriera (il messaggio porta solo la forma del payload);
- `WktSerialization`: il writer rifiuta la geometria (un poligono con buchi
  e anello esterno vuoto, anche dentro una collezione), con un testo fisso;
  da una cella WKB non arriva, perché il validatore strutturale vuole
  almeno quattro coordinate per anello.

### Limiti e deviazioni

Il runner non esegue ancora le operazioni geo
([README, «Che cosa non c'è ancora»](../README.md#che-cosa-non-cè-ancora)).
Nessun SRID nel testo (non è EWKT) e nessuna Z/M. I numeri non si
arrotondano: un valore grande o piccolo si scrive per intero
(`1e21` diventa `1000000000000000000000`), dove `ST_AsText` di PostGIS
limita le cifre significative.

### Precisione

Esatta: ogni coordinata si scrive con le cifre che la rileggono bit per bit
([README, «Precisione delle operazioni geografiche»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

### Complessità

Per geometria di n vertici: tempo O(n) e memoria O(n) per il testo, più la
validazione OGC dell'ingresso (sub-quadratica nel caso tipico, O(n²) nel
peggiore:
[README, «Validazione OGC»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì)).

### Esempio

```json
{
  "config": {},
  "ingressi": [
    {"nome": "siti", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2]},
      {"nome": "geometry", "tipo": "geometry", "valori": ["POINT(1.5 -2)", "POLYGON((0 0,4 0,4 4,0 4,0 0))"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2]},
    {"nome": "geometry", "tipo": "geometry", "valori": ["POINT(1.5 -2)", "POLYGON((0 0,4 0,4 4,0 4,0 0))"]},
    {"nome": "wkt", "tipo": "utf8", "valori": ["POINT(1.5 -2)", "POLYGON((0 0,4 0,4 4,0 4,0 0))"]}
  ]}
}
```
