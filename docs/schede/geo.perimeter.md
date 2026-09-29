### Che cosa fa

Aggiunge una colonna `float64` con il perimetro planare della geometria di
ogni riga, nelle unità del CRS. Il kernel è quello di
[`geo.length`](#geolength): un poligono vale anello esterno più buchi, ma
anche una linea vale la sua lunghezza (non 0), una multi-geometria o una
collezione la somma delle parti, un punto 0.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `output_column` | stringa | `perimeter` | nome non vuoto (non di soli spazi) e non già presente nell'ingresso | colonna aggiunta |

### Schema

Aggiunge in coda `output_column`, `float64` nullable, senza metadati di
campo. Le altre colonne (geometria compresa), i metadati di schema e le
proprietà del contratto (`sorted_by`, `row_count`) passano invariati.

### Righe

1:1 per contratto. Il kernel (`operations::perimeter`) lavora su una
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
- `Crs`: CRS della colonna assente o non risolto, geografico, o proiettato
  senza unità lineare;
- `InvalidPlan`: campi sconosciuti nella config, `output_column` vuoto o di soli spazi.

In esecuzione, dal kernel, per geometria (`OperationError`, che nessun
codice traduce ancora in `PlenoraError`):

- `InvalidInput`: la geometria non supera la validazione OGC;
- `ValidazioneNonConclusa`, `CalcoloNonConcluso`: la validazione OGC o il
  calcolo di `geo` vanno in panico dentro la barriera (il messaggio porta
  solo la forma del payload).

### Limiti e deviazioni

Il runner non esegue ancora le operazioni geo
([README, «Che cosa non c'è ancora»](../README.md#che-cosa-non-cè-ancora)).
Il perimetro di una linea è la sua lunghezza, come in Manipola
(`GeoSeries.length`), dove `ST_Perimeter` di PostGIS rende 0.

### Precisione

Nessuna griglia e nessun rifiuto `PrecisionInsufficient`: il perimetro è la
somma in `f64` delle lunghezze dei segmenti calcolate da `geo`, senza un
bilancio d'errore dichiarato rispetto alla regola di 1 cm
([README, «Precisione delle operazioni geografiche»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

### Complessità

Per geometria di n vertici: tempo O(n) per la somma, più la validazione OGC
dell'ingresso (sub-quadratica nel caso tipico, O(n²) nel peggiore:
[README, «Validazione OGC»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì));
memoria O(n) per la validazione.

### Esempio

```json
{
  "config": {},
  "ingressi": [
    {"nome": "lotti", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POLYGON((0 0,10 0,10 10,0 10,0 0),(2 2,4 2,4 4,2 4,2 2))", "LINESTRING(0 0,3 4)"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POLYGON((0 0,10 0,10 10,0 10,0 0),(2 2,4 2,4 4,2 4,2 2))", "LINESTRING(0 0,3 4)"]},
    {"nome": "perimeter", "tipo": "float64", "valori": [48.0, 5.0]}
  ]}
}
```
