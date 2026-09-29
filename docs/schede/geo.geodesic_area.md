### Che cosa fa

Aggiunge una colonna `float64` con l'area geodetica, in metri quadrati,
dei poligoni e multi-poligoni di ogni riga, sull'ellissoide WGS 84. Le
coordinate sono longitudine (`x`) e latitudine (`y`) in gradi e i lati sono
geodetiche fra vertici consecutivi. Il verso degli anelli non conta: ogni
poligono si orienta prima (esterno antiorario, buchi orari), e l'area è
quella dell'esterno meno quella dei buchi (algoritmo di Karney,
`geodesic_area_unsigned` di `geo`). Un `MultiPolygon` somma le aree dei
suoi poligoni.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `output_column` | stringa | `geodesic_area` | nome non vuoto e non già nello schema | colonna aggiunta |

### Schema

Aggiunge in coda `output_column`, `float64` nullable, senza metadati. Le
altre colonne restano nell'ordine e con i loro metadati; la colonna
geometria resta com'è. Metadati di schema e proprietà del contratto
(`sorted_by`, `row_count`) si conservano.

### Righe

1:1: un'area per riga. Il kernel (`extended_algorithms::geodesic_area_m2`)
accetta solo `Polygon` e `MultiPolygon`; un poligono o un multi-poligono
vuoto dà `-0.0`. Il runner non esegue ancora le operazioni geo ([README,
«Che cosa non c'è ancora»](../README.md#che-cosa-non-cè-ancora)): che cosa
rendano una cella nulla e una riga di altro tipo lo fisserà l'esecutore
geo.

### Ordine

Quello d'ingresso.

### Errori

In validazione (analisi del contratto; nell'ordine: forma dell'ingresso,
CRS, config):

- `InvalidPlan`: più o meno di un ingresso; config con campi sconosciuti o
  `output_column` non stringa; `output_column` vuoto;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come geometria WKB; `output_column` già
  presente;
- `Unsupported`: colonna geometria con dimensioni diverse da `xy`;
- `Crs`: CRS della colonna assente o non risolto; CRS non geografico.

In esecuzione: il runner non esegue l'operazione, e agli errori del kernel
non è ancora assegnata una variante `PlenoraError`. Il kernel rifiuta la
geometria con `InvalidInput` (coordinate non finite o geometria non valida
per l'OGC; `ValidazioneNonConclusa` se la validazione non conclude),
`InvalidGeographicCoordinate` (longitudine fuori da `[-180, 180]` o
latitudine fuori da `[-90, 90]`), `UnsupportedGeometry` (tipo diverso da
`Polygon` e `MultiPolygon`), `CalcoloNonConcluso` (panico di `geo`),
`InvalidOutput` (area non finita).

### Limiti e deviazioni

- **Ellissoide sempre WGS 84**, qualunque sia il datum del CRS geografico
  della colonna. Per la famiglia GRS 80 (ETRS89, RDN2008, NAD83...) la
  differenza è trascurabile; per Monte Mario ed ED50 (ellissoide
  internazionale 1924) l'area è quella delle stesse coordinate su WGS 84,
  circa 8e-5 in meno della vera a 42° di latitudine (stima al primo
  ordine), che supera perimetro per 1 cm già su un quadrato di circa 500 m
  di lato; per OSGB36 (Airy) circa 2e-4 in più. Nessun errore lo segnala.
- L'orientamento si decide nel piano lon/lat, mentre i lati sono
  geodetiche: un poligono che attraversa l'antimeridiano, scritto con
  longitudini da una parte e dall'altra di ±180, non è quello che il
  piano lon/lat disegna.
- Un poligono vuoto dà `-0.0`, non `0.0`.

### Precisione

Nessun controllo e nessun rifiuto di precisione. Su WGS 84 l'area è quella
dell'algoritmo di Karney in `f64`; la regola di 1 cm per le aree ammette
circa perimetro per 1 cm ([README, «Precisione delle operazioni
geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)),
e con un datum su un altro ellissoide lo scarto dell'ellissoide la supera
(vedi sopra).

### Complessità

O(n) per geometria, con `n` i vertici, più la validazione OGC
dell'ingresso (O(n²) nel caso peggiore). Memoria O(n) per la copia
orientata.

### Esempio

Il quadrato di un grado all'equatore, e lo stesso con un buco di mezzo
grado.

```json
{
  "config": {},
  "ingressi": [
    {"nome": "celle", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2]},
      {"nome": "geometry", "tipo": "geometry", "valori": ["POLYGON((0 0,1 0,1 1,0 1,0 0))", "POLYGON((0 0,1 0,1 1,0 1,0 0),(0.25 0.25,0.25 0.75,0.75 0.75,0.75 0.25,0.25 0.25))"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2]},
    {"nome": "geometry", "tipo": "geometry", "valori": ["POLYGON((0 0,1 0,1 1,0 1,0 0))", "POLYGON((0 0,1 0,1 1,0 1,0 0),(0.25 0.25,0.25 0.75,0.75 0.75,0.75 0.25,0.25 0.25))"]},
    {"nome": "geodesic_area", "tipo": "float64", "valori": [12308778361.469452, 9231614224.814873]}
  ]}
}
```
