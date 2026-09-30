### Che cosa fa

Aggiunge una colonna `float64` con l'area geodetica, in metri quadrati,
dei poligoni e multi-poligoni di ogni riga, sull'ellissoide del datum del
CRS della colonna ([README, «Misure geodetiche: l'ellissoide del datum»](../README.md#misure-geodetiche-lellissoide-del-datum)). Le
coordinate sono longitudine (`x`) e latitudine (`y`) in gradi e i lati sono
geodetiche fra vertici consecutivi. Il verso degli anelli non conta: ogni
poligono si orienta prima (esterno antiorario, buchi orari), e l'area è
quella dell'esterno meno quella dei buchi (algoritmo di Karney, il
calcolo di `geodesic_area_unsigned` di `geo` sul `PolygonArea` di
`geographiclib-rs` dell'ellissoide del datum). Un `MultiPolygon` somma le aree dei
suoi poligoni.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `output_column` | stringa | `geodesic_area` | nome non vuoto e non già nello schema | colonna aggiunta |

### Schema

Aggiunge in coda `output_column`, `float64`, nullable solo se lo è la colonna geometria (null dove la geometria è null), senza metadati. Le
altre colonne restano nell'ordine e con i loro metadati; la colonna
geometria resta com'è. Metadati di schema e proprietà del contratto
(`sorted_by`, `row_count`) si conservano.

### Righe

1:1: un'area per riga. Il kernel (`extended_algorithms::geodesic_area_m2`)
accetta solo `Polygon` e `MultiPolygon`; un poligono o un multi-poligono
vuoto dà `-0.0`. Una geometria nulla dà un'area nulla; una riga di altro
tipo ferma il passo con un errore (vedi «Errori»): l'analisi accetta ogni
tipo nella colonna, perché non conosce le celle.

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
- `Crs`: CRS della colonna assente o non risolto; CRS non geografico;
  CRS senza l'ellissoide del datum (`ELLIPSOID_REQUIRED`).

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

- `InvalidPlan`: una geometria diversa da `Polygon` e `MultiPolygon`
  (errore del runner, «tipo geometria non supportato», prima del kernel);
  dal kernel (`ExtendedAlgorithmError`) `InvalidInput` (coordinate non
  finite o geometria non valida per l'OGC; un lato con almeno 180 gradi
  di longitudine o un anello che sul globo gira al contrario o copre mezzo
  ellissoide, o un poligono che non passa la verifica di topologia delle
  geodetiche, vedi «Limiti e deviazioni»), `InvalidGeographicCoordinate`
  (longitudine fuori da `[-180, 180]` o latitudine fuori da `[-90, 90]`),
  `InvalidOutput` (area non finita);
- `Internal`: dal kernel `ValidazioneNonConclusa` (la validazione non
  conclude) e `CalcoloNonConcluso` (panico di `geo`).

### Limiti e deviazioni

- L'interno si decide orientando nel piano lon/lat, mentre i lati sono
  geodetiche. Un lato con almeno 180 gradi di longitudine (un poligono
  sull'antimeridiano, scritto con longitudini da una parte e dall'altra di
  ±180, o un anello attorno a un polo) la geodetica lo percorre
  dall'altra parte, e l'area sarebbe quella del complemento sul globo: si
  rifiuta (`InvalidInput`), invece di rendere un'area sbagliata come fino
  alla versione 2 del catalogo. Un poligono sull'antimeridiano va diviso
  in due. Lo stesso per ogni anello che, letto come geodetiche, gira al
  contrario del piano (un lato lungo che passa dall'altra parte di un
  vertice: il triangolo `0 30, 170 30, 85 31` è antiorario nel piano ma
  orario sul globo, perché il lato fra i primi due vertici sale oltre 80°)
  o copre mezzo ellissoide o più: l'area di ogni anello si calcola con
  segno e un segno non positivo si rifiuta.
- **Topologia delle geodetiche.** La validità OGC si verifica nel piano
  lon/lat, i lati sono geodetiche: un lato lungo può passare dall'altra
  parte di un buco (`POLYGON((-80 30,80 30,80 80,-80 80,-80 30),(-1 40,-1
  41,1 41,1 40,-1 40))`: il lato inferiore sale oltre 70° a longitudine 0,
  e il buco a 40° è fuori dall'esterno geodetico) o di un'altra parte.
  L'area si calcola solo se la topologia delle geodetiche è dimostrabilmente
  quella del piano ([README, «Misure geodetiche: l'ellissoide del
  datum»](../README.md#misure-geodetiche-lellissoide-del-datum)): ogni lato
  deve avere una lunghezza maggiorata di al più 1000 km, una latitudine
  maggiorata sotto 90° e una rotazione `K L` al più 1 (maggioranti
  certificati, senza problema inverso), e gli anelli devono stare più
  lontani degli scarti fra geodetiche e corde. Poligoni catastali e
  comunali non ne sono toccati.
  Altrimenti `InvalidInput`. La verifica è prudente: rifiuta anche poligoni
  corretti con lati oltre 1000 km (in lunghezza maggiorata) o anelli
  vicini ai lati lunghi.
- CRS proiettati rifiutati; fino alla versione 2 del catalogo l'ellissoide
  era sempre WGS 84 (su ED50, a 42° di latitudine, circa 8e-5 di area in
  meno, oltre perimetro per 1 cm già su un quadrato di 500 m).
- Un poligono vuoto dà `-0.0`, non `0.0`.
- Errori senza indice di riga della sorgente
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voce «Geo senza diagnostica per riga»).

### Precisione

Nessun controllo e nessun rifiuto di precisione. L'area è quella
dell'algoritmo di Karney in `f64` sull'ellissoide del datum; la regola di
1 cm per le aree ammette circa perimetro per 1 cm ([README, «Precisione
delle operazioni geografiche: 1 cm a
terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).
La somma non è compensata (come in `geo`): su un poligono piccolo lontano
dall'equatore lo scarto da GeographicLib è dell'ordine di 1e-5 m²
(l'oracolo ammette 1e-3 m²).

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
