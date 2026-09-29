### Che cosa fa

Aggiunge quattro colonne `float64` con il rettangolo d'ingombro della
geometria di ogni riga: coordinate minime e massime in x e in y, nelle
unità del CRS. Per una geometria vuota il kernel non dà un rettangolo.

### Parametri

Nessuno: la config è `{}`.

### Schema

Aggiunge in coda, in quest'ordine, `<geometria>_minx`, `<geometria>_miny`,
`<geometria>_maxx`, `<geometria>_maxy` (`<geometria>` è il nome della
colonna geometria), `float64` nullable, senza metadati di campo. Le altre
colonne (geometria compresa), i metadati di schema e le proprietà del
contratto (`sorted_by`, `row_count`) passano invariati.

### Righe

1:1 per contratto. Il kernel (`operations::bounds`) lavora su una
geometria alla volta e nessun adapter lo chiama ancora sulle righe: il
trattamento di una cella nulla, e la resa del caso senza rettangolo
(geometria vuota), non sono definiti da codice eseguito.

### Ordine

L'ordine d'ingresso: l'analisi conserva `sorted_by`.

### Errori

In validazione (analisi del contratto):

- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non si riconosce come WKB (né estensione `geoarrow.wkb` né chiavi
  `plenora.geometry.*`); una delle quattro colonne esiste già;
- `Unsupported`: dimensioni della geometria diverse da `xy`;
- `InvalidPlan`: config diversa da `{}`;
- `Crs`: CRS della colonna assente o non risolto, geografico, o proiettato
  senza unità lineare.

In esecuzione, dal kernel, per geometria (`OperationError`, che nessun
codice traduce ancora in `PlenoraError`):

- `InvalidInput`: la geometria non supera la validazione OGC;
- `ValidazioneNonConclusa`: la validazione OGC va in panico dentro la
  barriera (il messaggio porta solo la forma del payload).

### Limiti e deviazioni

Il runner non esegue ancora le operazioni geo
([README, «Che cosa non c'è ancora»](../README.md#che-cosa-non-cè-ancora)).
Il catalogo chiede un CRS proiettato, anche se il rettangolo non dipende
dalla metrica.

### Precisione

Esatta: i quattro valori sono coordinate d'ingresso, copiate senza calcolo
([README, «Precisione delle operazioni geografiche»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

### Complessità

Per geometria di n vertici: tempo O(n) per l'ingombro, più la validazione
OGC dell'ingresso (sub-quadratica nel caso tipico, O(n²) nel peggiore:
[README, «Validazione OGC»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì));
memoria O(n) per la validazione.

### Esempio

```json
{
  "config": {},
  "ingressi": [
    {"nome": "lotti", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POLYGON((0 0,4 1,2 3,0 0))", "POINT(5 6)"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POLYGON((0 0,4 1,2 3,0 0))", "POINT(5 6)"]},
    {"nome": "geometry_minx", "tipo": "float64", "valori": [0.0, 5.0]},
    {"nome": "geometry_miny", "tipo": "float64", "valori": [0.0, 6.0]},
    {"nome": "geometry_maxx", "tipo": "float64", "valori": [4.0, 5.0]},
    {"nome": "geometry_maxy", "tipo": "float64", "valori": [3.0, 6.0]}
  ]}
}
```
