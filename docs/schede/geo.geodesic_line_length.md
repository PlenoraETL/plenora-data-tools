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

1:1 per contratto. Il runner non esegue ancora le operazioni geo e nessun
esecutore chiama il kernel su una tabella. Il kernel
(`extended::geodesic_line_length_m`) riceve **una `LineString`** (vuota o
di un solo vertice: 0): l'analisi accetta ogni tipo geometrico, e che
cosa valgano una multilinea, un poligono, un punto o una riga nulla non è
ancora deciso.

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

In esecuzione il kernel rende `ExtendedError`, che nessun esecutore
traduce ancora in `PlenoraError`: `InvalidGeographicCoordinate` (un
vertice non finito o fuori da `[-180, 180]` × `[-90, 90]`),
`CalcoloNonConcluso` (il calcolo di `geo` va in panico).

### Limiti e deviazioni

- **Sempre l'ellissoide WGS 84**, qualunque sia l'ellissoide del CRS
  geografico: per ED50, Monte Mario, NAD27 o OSGB36 la lunghezza si
  scosta di parti su centomila, sopra 1 cm su linee di qualche centinaio
  di metri, senza errore (come [`geo.geodesic_distance`](#geogeodesic_distance)).
- Solo `LineString`, vedi «Righe».

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
