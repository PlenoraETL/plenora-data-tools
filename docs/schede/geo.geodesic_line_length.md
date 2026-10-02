### Che cosa fa

Aggiunge una colonna `float64` con la lunghezza geodetica in metri della
linea di ogni riga: la somma delle geodetiche fra vertici consecutivi
**sull'ellissoide del datum del CRS della colonna**, con l'algoritmo di
Karney di [`geo.geodesic_distance`](#geogeodesic_distance)
([Limiti dichiarati, «Misure geodetiche: l'ellissoide del datum»](limiti.md#misure-geodetiche-lellissoide-del-datum)). Le coordinate sono
longitudine e latitudine in gradi.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `output_column` | stringa | `geodesic_line_length` | nome non vuoto e non già nello schema | colonna aggiunta |

### Schema

Aggiunge in coda `output_column`, `float64`, nullable solo se lo è la colonna geometria (null dove la geometria è null). Le altre colonne,
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
  (`GEOGRAPHIC_CRS_REQUIRED`); CRS senza l'ellissoide del datum (risolto
  dal chiamante, fuori dalla tabella integrata: `ELLIPSOID_REQUIRED`);
- `InvalidPlan`: config con campi sconosciuti, `output_column` vuoto.

In esecuzione ([Runner, «Operazioni geo»](runner.md#operazioni-geo))
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
  `[-180, 180]` × `[-90, 90]`, e `InvalidOutput`, una lunghezza non
  finita (mai attesa);
- `Internal`: dal kernel `CalcoloNonConcluso` (il calcolo di `geo` va in
  panico).

### Limiti e deviazioni

- Ogni tratto è la geodetica **più breve** fra i due vertici: un tratto
  scritto con più di 180 gradi di longitudine (oltre l'antimeridiano) si
  misura dalla parte corta, non come il segmento che il piano lon/lat
  disegna.
- CRS proiettati rifiutati; fino alla versione 2 del catalogo l'ellissoide
  era sempre WGS 84 (come [`geo.geodesic_distance`](#geogeodesic_distance)).
- Solo `LineString`, vedi «Righe»: una `MultiLineString` ferma il passo.
- Errori senza indice di riga della sorgente
([Runner, «Limiti dichiarati del runner»](runner.md#limiti-dichiarati-del-runner),
voce «Geo senza diagnostica per riga»).

### Precisione

Ogni tratto è una geodetica di Karney sull'ellissoide del datum, con
errore dell'ordine dei nanometri; la somma aggiunge un arrotondamento per
tratto, molto sotto 1 cm su ogni linea realistica. Nessun rifiuto
`PrecisionInsufficient`
([Limiti dichiarati, «Precisione delle operazioni geografiche: 1 cm a terra»](limiti.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

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
