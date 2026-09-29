### Che cosa fa

Sostituisce ogni geometria con il suo rettangolo d'ingombro, nella stessa
colonna. Il rettangolo è un `Polygon`; se larghezza e altezza sono entrambe
nulle diventa il `Point` comune, se solo una è nulla la `LineString` di due
vertici dall'angolo minimo al massimo.

### Parametri

Nessuno: la config è `{}`.

### Schema

La colonna geometria resta al suo posto con lo stesso nome, CRS e
dimensioni (`xy`); le altre colonne, i metadati di schema e le proprietà
del contratto (`sorted_by`, `row_count`) passano invariati. I tipi
dichiarati della colonna diventano `exact` [`Point`, `LineString`,
`Polygon`]: le chiavi `plenora.geometry.types` e
`plenora.geometry.types_declaration` ereditate si tolgono dal campo.

### Righe

1:1 per contratto. Il kernel (`transform_geometry` con
`Operation::Envelope`) lavora su una geometria alla volta e nessun adapter
lo chiama ancora sulle righe: il trattamento di una cella nulla non è
definito da codice eseguito.

### Ordine

L'ordine d'ingresso: l'analisi conserva `sorted_by`. L'anello del
rettangolo parte da (`max x`, `min y`) e gira in senso antiorario.

### Errori

In validazione (analisi del contratto):

- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non si riconosce come WKB (né estensione `geoarrow.wkb` né chiavi
  `plenora.geometry.*`);
- `Unsupported`: dimensioni della geometria diverse da `xy`;
- `InvalidPlan`: config diversa da `{}`;
- `Crs`: CRS della colonna assente o non risolto, geografico, o proiettato
  senza unità lineare.

In esecuzione, dal kernel, per geometria:

- `InvalidPlan`: la geometria d'ingresso non supera la validazione OGC, o
  è vuota (nessun rettangolo);
- `Internal`: la validazione OGC non conclude (panico dentro la barriera;
  il messaggio porta solo la forma del payload).

### Limiti e deviazioni

Il runner non esegue ancora le operazioni geo
([README, «Che cosa non c'è ancora»](../README.md#che-cosa-non-cè-ancora)).
Una geometria vuota è un errore, dove PostGIS rende la geometria vuota.

### Precisione

Esatta: le coordinate d'uscita sono i minimi e i massimi delle coordinate
d'ingresso, copiati senza calcolo
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
      {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POLYGON((0 0,4 1,2 3,0 0))", "LINESTRING(1 4,5 4)", "POINT(2 3)"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POLYGON((4 0,4 3,0 3,0 0,4 0))", "LINESTRING(1 4,5 4)", "POINT(2 3)"]}
  ]}
}
```
