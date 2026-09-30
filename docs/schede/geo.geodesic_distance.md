### Che cosa fa

Aggiunge una colonna `float64` con la distanza geodetica in metri fra il
punto di ogni riga e un punto fisso della config (`other_wkb`), calcolata
**sull'ellissoide del datum del CRS della colonna** (Internazionale 1924
per ED50 e Monte Mario, Clarke 1866 per NAD27, Airy per OSGB36, GRS 80,
WGS 84...) con l'algoritmo di Karney (`geographiclib-rs`), mai su un
ellissoide di comodo ([README, «Misure geodetiche: l'ellissoide del datum»](../README.md#misure-geodetiche-lellissoide-del-datum)). Le coordinate sono longitudine e
latitudine in gradi.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `other_wkb` | stringa | obbligatorio | WKB 2D in esadecimale (cifre maiuscole o minuscole, lunghezza pari) di un `Point`, longitudine in `[-180, 180]` e latitudine in `[-90, 90]` | secondo operando, nello stesso CRS della colonna |
| `output_column` | stringa | `geodesic_distance` | nome non vuoto e non già nello schema | colonna aggiunta |

### Schema

Aggiunge in coda `output_column`, `float64`, nullable solo se lo è la colonna geometria (null dove la geometria è null). Le altre colonne,
la geometria e i metadati restano; restano anche le proprietà del
contratto (`sorted_by`, `row_count`).

### Righe

1:1: una distanza per riga, dal kernel (`extended::geodesic_distance_m`), che
riceve **due punti**. `other_wkb` deve essere un `Point` (l'analisi lo
verifica); una geometria nulla dà una distanza nulla, e una riga che non è
un `Point` ferma il passo con un errore (vedi «Errori»): l'analisi accetta
ogni tipo nella colonna, perché non conosce le celle.

### Ordine

Per contratto quello d'ingresso (forma 1:1).

### Errori

In validazione (analisi del contratto):

- `Schema`: l'ingresso non ha esattamente una colonna geometria, la
  colonna non si dichiara geometria WKB, o `output_column` esiste già;
- `Unsupported`: dimensioni della colonna diverse da XY (anche non
  dichiarate), o `other_wkb` con Z/M o SRID;
- `Crs`: CRS della colonna mancante o non risolto, o non geografico
  (`GEOGRAPHIC_CRS_REQUIRED`); CRS senza l'ellissoide del datum (risolto
  dal chiamante, fuori dalla tabella integrata: `ELLIPSOID_REQUIRED`); una coordinata di `other_wkb` fuori da
  longitudine e latitudine ammesse (`COORDINATE_OUT_OF_CRS_DOMAIN`);
- `InvalidPlan`: config con campi sconosciuti, `other_wkb` assente, non
  esadecimale, WKB non valido nella struttura o nella validità OGC, o che
  non è un `Point`; `output_column` vuoto;
- `Internal`: la decodifica o la validazione OGC di `other_wkb` non
  conclude.

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

- `InvalidPlan`: la geometria della riga non è un `Point` (errore del
  runner, «tipo geometria non supportato»); dal kernel (`ExtendedError`)
  `InvalidGeographicCoordinate`, una coordinata non finita o fuori da
  `[-180, 180]` × `[-90, 90]`, e `InvalidOutput`, una distanza non finita
  (mai attesa);
- `Internal`: dal kernel `CalcoloNonConcluso` (il calcolo di `geo` va in
  panico).

### Limiti e deviazioni

- **CRS proiettati rifiutati**, non riportati al loro CRS geografico di
  base: la distanza si chiede sulle coordinate geografiche
  (`geo.reproject` al CRS geografico del datum, prima).
- Fino alla versione 1 del catalogo l'ellissoide era sempre WGS 84: su
  ED50 circa 4 m ogni 100 km di troppo o di meno, senza errore.
- Solo punti, vedi «Righe»: una `MultiPoint` nella colonna ferma il
  passo.
- Errori senza indice di riga della sorgente
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voce «Geo senza diagnostica per riga»).

### Precisione

Sull'ellissoide del datum l'algoritmo di Karney resta molto sotto 1 cm
(l'errore dichiarato da `geographiclib` è dell'ordine dei nanometri;
l'oracolo `tests/geodetica_oracolo.rs` lo confronta con GeographicLib e
PROJ su ogni ellissoide della tabella entro un micrometro). Nessun
rifiuto `PrecisionInsufficient`
([README, «Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

### Complessità

Tempo e memoria O(1) per riga (poche iterazioni del problema inverso).

### Esempio

`other_wkb` è `POINT(0 0)`: sull'ellissoide un grado di latitudine è più
corto di uno di longitudine all'equatore.

```json
{
  "config": {"other_wkb": "010100000000000000000000000000000000000000"},
  "ingressi": [
    {"nome": "stazioni", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:4326", "valori": ["POINT(0 1)", "POINT(1 0)"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:4326", "valori": ["POINT(0 1)", "POINT(1 0)"]},
    {"nome": "geodesic_distance", "tipo": "float64", "valori": [110574.38855779878, 111319.49079327357]}
  ]}
}
```
