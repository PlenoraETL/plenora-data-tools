### Che cosa fa

Spezza ogni geometria con più di `max_vertices` vertici in parti che ne
hanno al più `max_vertices`, una riga per parte con gli attributi della
riga d'origine e il suo indice in `__parent_index`, come `ST_Subdivide` di
PostGIS. I poligoni si tagliano a metà del rettangolo d'ingombro, sul lato
lungo, finché ogni pezzo sta sotto la soglia; linee e `MultiPoint` si
dividono a blocchi. Una geometria sotto la soglia passa invariata, in una
riga sola.

Il calcolo per cella è `extensions2::subdivide_wkb`, che lascia l'indice
della riga d'origine al chiamante; il runner non esegue ancora
l'operazione.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `max_vertices` | intero | obbligatorio | `>= 4` | vertici massimi per parte (contati come `coords_count`: il vertice di chiusura e i buchi compresi) |
| `output_column` | stringa | nome della colonna geometria | nome non vuoto e libero | rinomina la colonna geometria dell'uscita |

### Schema

Le colonne dell'ingresso, nelle stesse posizioni; la colonna geometria
prende il nome `output_column` se dato (stesso tipo, metadati e
nullabilità). In coda `__parent_index`, `uint64` non nullable. Tipi
dichiarati, se l'ingresso li dichiara: `Point`, `LineString`, `Polygon` e
`MultiPoint` restano tali; `MultiLineString` dà `LineString` o
`MultiLineString`, `MultiPolygon` dà `Polygon` o `MultiPolygon`,
`GeometryCollection` uno qualunque dei sette tipi. Resta `sorted_by`,
`row_count` cade.

### Righe

Espansione 1:N, per tipo:

- `LineString`: blocchi di `max_vertices` vertici, in cui l'ultimo vertice
  di un blocco è il primo del successivo;
- `MultiLineString`: ogni linea sotto la soglia intera, le altre a blocchi;
  le parti sono `LineString`;
- `MultiPoint`: blocchi di `max_vertices` punti, ciascuno un `MultiPoint`;
- `Polygon`, `MultiPolygon`: taglio ricorsivo di ogni poligono,
  intersecandolo con le due metà del suo rettangolo d'ingombro (la metà sul
  lato più lungo, sull'asse x a parità), fino a 32 livelli; i pezzi di area
  nulla lungo la linea di taglio si scartano. La somma delle aree resta
  quella del poligono, entro la precisione;
- `GeometryCollection`: ogni membro per sé.

Ogni parte si valida (OGC). Che cosa esca per una geometria nulla non è
ancora fissato da nessuna esecuzione a livello di tabella.

### Ordine

Le parti di una riga seguono la riga; dentro una riga, i blocchi di una
linea dall'inizio alla fine, e i pezzi di un poligono in profondità, prima
la metà sinistra (o inferiore), e dentro una metà nell'ordine delle parti
restituite da `i_overlay`.

### Errori

In validazione (analisi del contratto):

- `InvalidPlan`: config con campi sconosciuti; `max_vertices` mancante,
  non intero non negativo o minore di 4; `output_column` vuota;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come WKB; `output_column` o `__parent_index`
  già presenti;
- `Unsupported`: dimensioni della geometria diverse da `xy`;
- `Crs`: colonna senza CRS o con un'incoerenza CRS non risolta.

In esecuzione (calcolo per cella, messaggi con prefisso `geo.subdivide:`):

- `InvalidPlan`: WKB malformato o OGC-invalido; taglio che non converge in
  32 livelli; parte prodotta non valida;
- `Unsupported`: `PrecisionInsufficient` (sotto, «Precisione»); WKB con
  dimensioni Z/M o SRID;
- `ResourceLimit`: cella d'ingresso o parte oltre il limite di byte per
  cella;
- `Internal`: panico di `geo` o `i_overlay`, validazione che non conclude.

### Limiti e deviazioni

Le parti di un poligono non sono uniche: due versioni di `i_overlay`
possono scegliere tagli diversi, con la stessa area totale
([README, «Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra),
«Hazard»). Il taglio si ferma a 32 livelli con un errore, non con parti
sopra la soglia.

### Precisione

Linee e `MultiPoint` sono esatti: le parti copiano i vertici. I tagli dei
poligoni passano dalla griglia di `i_overlay` e sono in catena, un livello
sul risultato del precedente: prima di ogni taglio il controllo a priori
dà a ognuno dei 32 livelli `1/32` di `p / 2` sull'ingombro del pezzo, e
se lo spostamento della griglia lo supera, o le coordinate sono troppo rade
per `p`, il taglio non si esegue (`PrecisionInsufficient`). Le foglie non
sono confrontate con il poligono di partenza: sotto la precisione parti più
sottili di 1 cm possono fondersi o sparire
([README, «Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

### Complessità

Linee e punti: O(v) per riga. Poligoni: a ogni livello due intersezioni
per pezzo, O(v log v) sui vertici del pezzo, per al più 32 livelli;
memoria O(v) più le parti prodotte.

### Esempio

Una linea di 7 vertici con `max_vertices` 4 e una linea sotto la soglia.

```json
{
  "config": {"max_vertices": 4},
  "ingressi": [
    {"nome": "reti", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["LINESTRING(0 0,1 0,2 0,3 0,4 0,5 0,6 0)", "LINESTRING(0 5,1 5)"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 1, 2]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["LINESTRING(0 0,1 0,2 0,3 0)", "LINESTRING(3 0,4 0,5 0,6 0)", "LINESTRING(0 5,1 5)"]},
    {"nome": "__parent_index", "tipo": "uint64", "valori": [0, 0, 1]}
  ]}
}
```
