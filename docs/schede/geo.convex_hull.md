### Che cosa fa

Sostituisce ogni geometria con il suo inviluppo convesso, un `Polygon`,
nella stessa colonna: il più piccolo poligono convesso che contiene tutti i
vertici. Un inviluppo con meno di tre punti non allineati (un punto, un
segmento, punti collineari) non è un poligono valido e si rifiuta; una
geometria vuota dà `POLYGON EMPTY`.

### Parametri

Nessuno: la config è `{}`.

### Schema

La colonna geometria resta al suo posto con lo stesso nome, CRS e
dimensioni (`xy`); le altre colonne, i metadati di schema e le proprietà
del contratto (`sorted_by`, `row_count`) passano invariati. I tipi
dichiarati della colonna diventano `exact` [`Polygon`]: le chiavi
`plenora.geometry.types` e `plenora.geometry.types_declaration` ereditate
si tolgono dal campo.

### Righe

1:1: il runner chiama il kernel (`transform_geometry` con
`Operation::ConvexHull`) su ogni cella non nulla, in parallelo, e rimette la
geometria al suo posto; una cella nulla resta nulla ([README, «Operazioni geo»](../README.md#operazioni-geo)).

### Ordine

L'ordine d'ingresso: l'analisi conserva `sorted_by`. L'anello
dell'inviluppo è antiorario, con il primo vertice scelto da `geo`.

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
dichiara con un elenco ([README, «Operazioni geo»](../README.md#operazioni-geo)).

Poi dal kernel, per geometria:

- `InvalidPlan`: la geometria d'ingresso non supera la validazione OGC, o
  l'inviluppo è degenere (punto, segmento, punti collineari: «punti
  distinti insufficienti» sull'uscita);
- `Internal`: la validazione OGC o il calcolo di `geo` non concludono
  (panico dentro la barriera; il messaggio porta solo la forma del
  payload).

Una geometria prodotta oltre il limite di byte per cella (64 MiB di WKB)
è `ResourceLimit`. Il primo errore è quello della prima riga in ordine
di riga, senza diagnostica per riga.

### Limiti e deviazioni

Nel runner un errore non ha diagnostica per riga e il costo in memoria è
una previsione provvisoria
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voci «Geo senza diagnostica per riga» e «Modelli di costo geo provvisori»).
Dove GEOS e PostGIS rendono un `Point` o una `LineString` per l'inviluppo
degenere, qui c'è un errore: l'uscita dichiarata è sempre `Polygon`.
Le coordinate si dividono per il loro modulo massimo prima del calcolo e si
rimoltiplicano dopo, sempre (non solo vicino ai limiti di `f64`, dove gli
orientamenti di `geo` traboccherebbero).

### Precisione

I vertici dell'inviluppo sono vertici dell'ingresso passati per la
divisione e la moltiplicazione per il modulo massimo: due arrotondamenti,
al più qualche ulp per coordinata, molto sotto 1 cm in ogni dominio di un
CRS reale; esatti quando il modulo massimo è una potenza di 2. Nessuna
griglia e nessun rifiuto `PrecisionInsufficient`
([README, «Precisione delle operazioni geografiche»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

### Complessità

Per geometria di n vertici: quickhull di `geo`, O(n log n) atteso e O(n²)
nel caso peggiore, più la validazione OGC dell'ingresso e dell'uscita
(sub-quadratica nel caso tipico, O(n²) nel peggiore:
[README, «Validazione OGC»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì));
memoria O(n) per la copia scalata delle coordinate.

### Esempio

```json
{
  "config": {},
  "ingressi": [
    {"nome": "siti", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["MULTIPOINT((0 0),(4 0),(4 4),(0 4),(2 2))", "LINESTRING(0 0,4 0,2 3)"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POLYGON((4 0,4 4,0 4,0 0,4 0))", "POLYGON((0 0,4 0,2 3,0 0))"]}
  ]}
}
```
