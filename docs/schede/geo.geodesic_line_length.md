### Che cosa fa

Aggiunge una colonna `float64` con la lunghezza geodetica in metri della
linea di ogni riga: la somma delle geodetiche fra vertici consecutivi
**sull'ellissoide WGS 84**, con l'algoritmo di Karney di
[`geo.geodesic_distance`](#geogeodesic_distance). Le coordinate sono
longitudine e latitudine in gradi.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `output_column` | stringa | `geodesic_line_length` | nome non vuoto e non già nello schema | colonna aggiunta |

### Schema

Aggiunge in coda `output_column`, `float64` nullable. Le altre colonne,
la geometria e i metadati restano; restano anche le proprietà del
contratto (`sorted_by`, `row_count`).

### Righe

1:1: una lunghezza per riga, dal kernel
(`extended::geodesic_line_length_m`), che riceve **una `LineString`**
(vuota o di un solo vertice: 0). Una geometria nulla dà una lunghezza
nulla; una multilinea, un poligono o un punto fermano il passo con un
errore (vedi «Errori»): l'analisi accetta ogni tipo nella colonna, perché
non conosce le celle.

### Ordine

Per contratto quello d'ingresso (forma 1:1).

### Errori

In validazione (analisi del contratto):

- `Schema`: l'ingresso non ha esattamente una colonna geometria, la
  colonna non si dichiara geometria WKB, o `output_column` esiste già;
- `Unsupported`: dimensioni della colonna diverse da XY (anche non
  dichiarate);
- `Crs`: CRS della colonna mancante o non risolto, o non geografico
  (`GEOGRAPHIC_CRS_REQUIRED`);
- `InvalidPlan`: config con campi sconosciuti, `output_column` vuoto.

In esecuzione ([README, «Operazioni geo»](../README.md#operazioni-geo))
il passo rende il primo errore in ordine di riga, senza diagnostica per
riga. Prima del kernel, per ogni cella non nulla della colonna geometria:

- `InvalidPlan`: struttura WKB non valida; `Unsupported`: la cella porta
  Z/M o uno SRID; `ResourceLimit`: la cella supera 64 MiB;
- `Crs`: una coordinata fuori dal dominio di validità del CRS della
  colonna;
- `Schema`: il contratto d'ingresso dichiara i tipi geometrici con un
  elenco e la cella è di un altro tipo.

Poi, per riga:

- `InvalidPlan`: la geometria non è una `LineString` (errore del runner,
  «tipo geometria non supportato»); dal kernel (`ExtendedError`)
  `InvalidGeographicCoordinate`, un vertice non finito o fuori da
  `[-180, 180]` × `[-90, 90]`;
- `Internal`: dal kernel `CalcoloNonConcluso` (il calcolo di `geo` va in
  panico).

### Limiti e deviazioni

- **Sempre l'ellissoide WGS 84**, qualunque sia l'ellissoide del CRS
  geografico: per ED50, Monte Mario, NAD27 o OSGB36 la lunghezza si
  scosta di parti su centomila, sopra 1 cm su linee di qualche centinaio
  di metri, senza errore (come [`geo.geodesic_distance`](#geogeodesic_distance)).
- Solo `LineString`, vedi «Righe»: una `MultiLineString` ferma il passo.
- Errori senza indice di riga della sorgente
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voce «Geo senza diagnostica per riga»).

### Precisione

Ogni tratto è una geodetica di Karney su WGS 84, con errore dell'ordine
dei nanometri; la somma aggiunge un arrotondamento per tratto, molto
sotto 1 cm su ogni linea realistica. Per un CRS su un altro ellissoide la
regola di 1 cm **non vale** (sopra). Nessun rifiuto
`PrecisionInsufficient`
([README, «Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

### Complessità

Tempo O(n) sui vertici della linea; memoria O(1) oltre all'ingresso.

### Esempio

```json
{
  "config": {},
  "ingressi": [
    {"nome": "rotte", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:4326", "valori": ["LINESTRING(0 0,1 0)", "LINESTRING(0 0,0 1,1 1)"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:4326", "valori": ["LINESTRING(0 0,1 0)", "LINESTRING(0 0,0 1,1 1)"]},
    {"nome": "geodesic_line_length", "tipo": "float64", "valori": [111319.49079327357, 221877.0378972296]}
  ]}
}
```
