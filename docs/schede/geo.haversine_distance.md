### Che cosa fa

Aggiunge una colonna `float64` con la distanza in metri fra il punto di
ogni riga e un punto fisso della config (`other_wkb`), lungo il cerchio
massimo di una **sfera** con il raggio medio IUGG `R1 = a (1 - f / 3)`
dell'ellissoide del datum del CRS della colonna (per WGS 84 6 371 008,771 m,
per Internazionale 1924 6 371 229,315 m;
[Limiti dichiarati, «Misure geodetiche: l'ellissoide del datum»](limiti.md#misure-geodetiche-lellissoide-del-datum)). Le coordinate sono longitudine e latitudine in gradi. Il
nome resta `haversine`, ma il calcolo non è la formula dell'emiseno: è il
problema inverso di `geographiclib-rs` a schiacciamento nullo, ben
condizionato anche agli antipodi. Per la distanza sull'ellissoide c'è
[`geo.geodesic_distance`](#geogeodesic_distance).

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `other_wkb` | stringa | obbligatorio | WKB 2D in esadecimale (cifre maiuscole o minuscole, lunghezza pari) di un `Point`, longitudine in `[-180, 180]` e latitudine in `[-90, 90]` | secondo operando, nello stesso CRS della colonna |
| `output_column` | stringa | `haversine_distance` | nome non vuoto e non già nello schema | colonna aggiunta |

### Schema

Aggiunge in coda `output_column`, `float64`, nullable solo se lo è la colonna geometria (null dove la geometria è null). Le altre colonne,
la geometria e i metadati restano; restano anche le proprietà del
contratto (`sorted_by`, `row_count`).

### Righe

1:1: una distanza per riga, dal kernel (`extended::haversine_distance_m`), che
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

- `InvalidPlan`: la geometria della riga non è un `Point` (errore del
  runner, «tipo geometria non supportato»); dal kernel (`ExtendedError`)
  `InvalidGeographicCoordinate`, una coordinata non finita o fuori da
  `[-180, 180]` × `[-90, 90]`, e `InvalidOutput`, una distanza non finita
  (mai attesa);
- `Internal`: dal kernel `CalcoloNonConcluso` (il calcolo di `geo` va in
  panico).

### Limiti e deviazioni

- **Sfera, non ellissoide.** La distanza differisce dalla geodetica
  sull'ellissoide di qualche millesimo del valore. Dall'origine, su
  WGS 84, un grado di latitudine vale 111 195,08 m qui e 110 574,39 m
  sull'ellissoide (5,6 per mille), un grado di longitudine 111 195,08 m
  qui e 111 319,49 m sull'ellissoide.
- Fino alla versione 1 del catalogo il raggio era fisso (6 371 008,8 m,
  quello di `Haversine` di `geo`) qualunque fosse il datum, e il calcolo
  era la formula dell'emiseno: vicino agli antipodi, con l'emiseno
  arrotondato sopra 1, rendeva NaN senza errore, e poco prima perdeva
  decimetri per arrotondamento.
- CRS proiettati rifiutati.
- Solo punti, vedi «Righe»: una `MultiPoint` nella colonna ferma il
  passo.
- Errori senza indice di riga della sorgente
([Runner, «Limiti dichiarati del runner»](runner.md#limiti-dichiarati-del-runner),
voce «Geo senza diagnostica per riga»).

### Precisione

La regola di 1 cm non riguarda il modello: il risultato è la distanza
sulla sfera, non la distanza vera sull'ellissoide (sopra). Sulla sfera il
calcolo è accurato ai nanometri ovunque, antipodi compresi (l'oracolo
`tests/geodetica_oracolo.rs` lo confronta con GeographicLib entro un
micrometro su ogni ellissoide della tabella); il risultato è sempre
finito. Nessun rifiuto `PrecisionInsufficient`
([Limiti dichiarati, «Precisione delle operazioni geografiche: 1 cm a terra»](limiti.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

### Complessità

Tempo e memoria O(1) per riga.

### Esempio

`other_wkb` è `POINT(0 0)`: un grado di latitudine e uno di longitudine
all'equatore valgono lo stesso arco sulla sfera.

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
    {"nome": "haversine_distance", "tipo": "float64", "valori": [111195.07973463158, 111195.07973463158]}
  ]}
}
```
