### Che cosa fa

Aggiunge una colonna `float64` con la distanza di Hausdorff discreta fra
la geometria di ogni riga e una geometria fissa della config
(`other_wkb`), nelle unità del CRS. È la distanza **fra vertici**
(`HausdorffDistance` di `geo`): il massimo, nei due versi, della distanza
euclidea fra un vertice di una geometria e il vertice più vicino
dell'altra. I lati non contano.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `other_wkb` | stringa | obbligatorio | WKB 2D in esadecimale (cifre maiuscole o minuscole, lunghezza pari) di una geometria valida OGC, coordinate nel dominio di validità del CRS dell'ingresso | secondo operando, nello stesso CRS della colonna |
| `output_column` | stringa | `hausdorff_distance` | nome non vuoto e non già nello schema | colonna aggiunta |

### Schema

Aggiunge in coda `output_column`, `float64` nullable. Le altre colonne,
la geometria e i metadati restano; restano anche le proprietà del
contratto (`sorted_by`, `row_count`).

### Righe

1:1: una distanza per riga, dal kernel `extended::hausdorff_distance`
(riga, `other_wkb`). Una geometria nulla dà una distanza nulla, e anche
una geometria senza coordinate, da una parte o dall'altra (il kernel
rende «nessun valore»).

### Ordine

Per contratto quello d'ingresso (forma 1:1).

### Errori

In validazione (analisi del contratto):

- `Schema`: l'ingresso non ha esattamente una colonna geometria, la
  colonna non si dichiara geometria WKB, o `output_column` esiste già;
- `Unsupported`: dimensioni della colonna diverse da XY (anche non
  dichiarate), o `other_wkb` con Z/M o SRID;
- `Crs`: CRS della colonna mancante o non risolto, non proiettato
  (`PROJECTED_CRS_REQUIRED`) o senza unità lineare
  (`LINEAR_UNIT_REQUIRED`); una coordinata di `other_wkb` fuori dal
  dominio di validità del CRS (`COORDINATE_OUT_OF_CRS_DOMAIN`);
- `InvalidPlan`: config con campi sconosciuti, `other_wkb` assente, non
  esadecimale o WKB non valido nella struttura (anelli aperti,
  coordinate non finite, byte residui) o nella validità OGC,
  `output_column` vuoto;
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

Dal kernel (`extended::hausdorff_distance`, `ExtendedError`), per riga:

- `InvalidPlan`: `InvalidInput` (coordinate non finite o geometria della
  riga non valida OGC; `other_wkb` è già validata in analisi),
  `IndexOverflow`;
- `ResourceLimit`: `WorkLimit` (il prodotto dei vertici delle due
  geometrie supera `10^8` coppie, il tetto che il runner passa al
  kernel);
- `Internal`: `ValidazioneNonConclusa` e `CalcoloNonConcluso`
  (validazione o calcolo interrotti).

### Limiti e deviazioni

- **Solo vertici.** Diversa da `ST_HausdorffDistance` di PostGIS (GEOS
  `DiscreteHausdorffDistance`), che misura dai vertici di una geometria ai
  lati dell'altra: qui un vertice che sta su un lato dell'altra geometria
  ma lontano dai suoi vertici pesa per la distanza da quei vertici, e il
  risultato può essere maggiore (vedi l'esempio). Nessuna densificazione.
- Il lavoro è limitato da `max_coordinate_pairs` (prodotto dei vertici
  delle due geometrie): il runner passa `10^8` per riga, l'ordine di
  `MAX_NODING_WORK` dei kernel; non è un parametro della config.
- Errori senza indice di riga della sorgente
([Runner, «Limiti dichiarati del runner»](runner.md#limiti-dichiarati-del-runner),
voce «Geo senza diagnostica per riga»).

### Precisione

Le distanze fra vertici sono calcolate in `f64` (differenze e `hypot`),
con errore relativo di pochi ulp: sotto 1 cm per ogni distanza sotto
circa `1e13` unità del CRS. Il massimo e il minimo non arrotondano.
Nessun rifiuto `PrecisionInsufficient`
([Limiti dichiarati, «Precisione delle operazioni geografiche: 1 cm a terra»](limiti.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

### Complessità

Tempo O(n·m) sui vertici delle due geometrie, limitato da
`max_coordinate_pairs`, più la validazione OGC dei due ingressi; memoria
O(1) oltre agli ingressi.

### Esempio

`other_wkb` è `LINESTRING(0 0,10 0)`. Per la seconda riga il vertice
`(5, 1)` dista 1 dal segmento, ma `sqrt(26)` dai suoi vertici.

```json
{
  "config": {"other_wkb": "0102000000020000000000000000000000000000000000000000000000000024400000000000000000"},
  "ingressi": [
    {"nome": "tracce", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["LINESTRING(0 3,10 3)", "LINESTRING(0 1,5 1,10 1)"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["LINESTRING(0 3,10 3)", "LINESTRING(0 1,5 1,10 1)"]},
    {"nome": "hausdorff_distance", "tipo": "float64", "valori": [3.0, 5.0990195135927845]}
  ]}
}
```
