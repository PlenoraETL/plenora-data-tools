### Che cosa fa

Aggiunge una colonna `uint64` con il numero di coordinate della geometria
di ogni riga, sommato su tutte le parti. Ogni anello conta anche il vertice
di chiusura (un quadrato ne ha 5); una geometria vuota ne ha 0.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `output_column` | stringa | `vertex_count` | nome non vuoto (non di soli spazi) e non già presente nell'ingresso | colonna aggiunta |

### Schema

Aggiunge in coda `output_column`, `uint64` nullable, senza metadati di
campo. Le altre colonne (geometria compresa), i metadati di schema e le
proprietà del contratto (`sorted_by`, `row_count`) passano invariati.

### Righe

1:1 per contratto. Il kernel (`operations::vertex_count`) lavora su una
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
- `Internal`: il conteggio non entra in `u64` (mai sulle piattaforme
  supportate).

### Limiti e deviazioni

Il runner non esegue ancora le operazioni geo
([README, «Che cosa non c'è ancora»](../README.md#che-cosa-non-cè-ancora)).
Il conteggio è quello di `ST_NPoints` di PostGIS: le coordinate ripetute
contano tutte.

### Precisione

Esatta: è un conteggio
([README, «Precisione delle operazioni geografiche»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

### Complessità

Per geometria di n vertici: tempo O(n) per il conteggio, più la
validazione OGC dell'ingresso (sub-quadratica nel caso tipico, O(n²) nel
peggiore:
[README, «Validazione OGC»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì));
memoria O(n) per la validazione.

### Esempio

```json
{
  "config": {},
  "ingressi": [
    {"nome": "forme", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
      {"nome": "geometry", "tipo": "geometry", "valori": ["POLYGON((0 0,4 0,4 4,0 4,0 0))", "LINESTRING(0 0,1 1,2 0)", "POINT(1 1)"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
    {"nome": "geometry", "tipo": "geometry", "valori": ["POLYGON((0 0,4 0,4 4,0 4,0 0))", "LINESTRING(0 0,1 1,2 0)", "POINT(1 1)"]},
    {"nome": "vertex_count", "tipo": "uint64", "valori": [5, 3, 1]}
  ]}
}
```
