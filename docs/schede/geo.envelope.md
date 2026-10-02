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

1:1: il runner chiama il kernel (`transform_geometry` con
`Operation::Envelope`) su ogni cella non nulla, in parallelo, e rimette la
geometria al suo posto; una cella nulla resta nulla ([Runner, «Operazioni geo»](runner.md#operazioni-geo)).

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

In esecuzione, prima del kernel, su tutta la colonna: `InvalidPlan` per
una cella che non è WKB strutturalmente valido, `Crs` per una coordinata
fuori dal dominio di validità del CRS della colonna, `Schema` per una
geometria di un tipo che il contratto d'ingresso non dichiara, quando li
dichiara con un elenco ([Runner, «Operazioni geo»](runner.md#operazioni-geo)).

Poi dal kernel, per geometria:

- `InvalidPlan`: la geometria d'ingresso non supera la validazione OGC, o
  è vuota (nessun rettangolo);
- `Internal`: la validazione OGC non conclude (panico dentro la barriera;
  il messaggio porta solo la forma del payload).

Una geometria prodotta oltre il limite di byte per cella (64 MiB di WKB)
è `ResourceLimit`. Il primo errore è quello della prima riga in ordine
di riga, senza diagnostica per riga.

### Limiti e deviazioni

Nel runner un errore non ha diagnostica per riga e il costo in memoria è
una previsione dalle misure
([Runner, «Limiti dichiarati del runner»](runner.md#limiti-dichiarati-del-runner),
voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).
Una geometria vuota è un errore, dove PostGIS rende la geometria vuota.

### Precisione

Esatta: le coordinate d'uscita sono i minimi e i massimi delle coordinate
d'ingresso, copiati senza calcolo
([Limiti dichiarati, «Precisione delle operazioni geografiche»](limiti.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

### Complessità

Per geometria di n vertici: tempo O(n) per l'ingombro, più la validazione
OGC dell'ingresso (sub-quadratica nel caso tipico, O(n²) nel peggiore:
[Limiti dichiarati, «Validazione OGC»](limiti.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì));
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
