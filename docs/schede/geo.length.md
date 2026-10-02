### Che cosa fa

Aggiunge una colonna `float64` con la lunghezza planare della geometria di
ogni riga, nelle unità del CRS. Una linea vale la somma dei suoi segmenti;
un poligono il suo perimetro, anello esterno più buchi (la semantica di
`length` di Shapely); una multi-geometria o una collezione la somma delle
parti; i punti valgono 0.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `output_column` | stringa | `length` | nome non vuoto (non di soli spazi) e non già presente nell'ingresso | colonna aggiunta |

### Schema

Aggiunge in coda `output_column`, `float64`, nullable solo se lo è la colonna geometria (null dove la geometria è null), senza metadati di
campo. Le altre colonne (geometria compresa), i metadati di schema e le
proprietà del contratto (`sorted_by`, `row_count`) passano invariati.

### Righe

1:1: una lunghezza per riga, dal kernel `operations::length`; una
geometria nulla dà una cella nulla.

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

In esecuzione ([Runner, «Operazioni geo»](runner.md#operazioni-geo))
il passo rende il primo errore in ordine di riga, senza diagnostica per
riga. Prima del kernel, per ogni cella non nulla della colonna geometria:

- `InvalidPlan`: struttura WKB non valida; `Unsupported`: la cella porta
  Z/M o uno SRID; `ResourceLimit`: la cella supera 64 MiB;
- `Crs`: una coordinata fuori dal dominio di validità del CRS della
  colonna;
- `Schema`: il contratto d'ingresso dichiara i tipi geometrici con un
  elenco e la cella è di un altro tipo.

Dal kernel (`OperationError`), per geometria:

- `InvalidPlan` (`InvalidInput`): la geometria non supera la validazione
  OGC;
- `Internal` (`ValidazioneNonConclusa`, `CalcoloNonConcluso`): la
  validazione OGC o il calcolo di `geo` vanno in panico dentro la
  barriera (il messaggio porta solo la forma del payload).

### Limiti e deviazioni

Un poligono ha la lunghezza del suo perimetro, dove `ST_Length` di PostGIS
rende 0. La lunghezza è planare, non geodetica.
Errori senza indice di riga della sorgente
([Runner, «Limiti dichiarati del runner»](runner.md#limiti-dichiarati-del-runner),
voce «Geo senza diagnostica per riga»).

### Precisione

Nessuna griglia e nessun rifiuto `PrecisionInsufficient`: la lunghezza è la
somma in `f64` delle lunghezze dei segmenti calcolate da `geo`, senza un
bilancio d'errore dichiarato rispetto alla regola di 1 cm
([Limiti dichiarati, «Precisione delle operazioni geografiche»](limiti.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

### Complessità

Per geometria di n vertici: tempo O(n) per la somma, più la validazione OGC
dell'ingresso (sub-quadratica nel caso tipico, O(n²) nel peggiore:
[Limiti dichiarati, «Validazione OGC»](limiti.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì));
memoria O(n) per la validazione.

### Esempio

```json
{
  "config": {},
  "ingressi": [
    {"nome": "tratte", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["LINESTRING(0 0,3 4)", "POLYGON((0 0,10 0,10 10,0 10,0 0))", "POINT(1 1)"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["LINESTRING(0 0,3 4)", "POLYGON((0 0,10 0,10 10,0 10,0 0))", "POINT(1 1)"]},
    {"nome": "length", "tipo": "float64", "valori": [5.0, 40.0, 0.0]}
  ]}
}
```
