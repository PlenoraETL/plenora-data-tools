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
| `other_wkb` | stringa | obbligatorio | WKB 2D in esadecimale (cifre maiuscole o minuscole, lunghezza pari), coordinate nel dominio di validità del CRS dell'ingresso | secondo operando, nello stesso CRS della colonna |
| `output_column` | stringa | `hausdorff_distance` | nome non vuoto e non già nello schema | colonna aggiunta |

### Schema

Aggiunge in coda `output_column`, `float64` nullable. Le altre colonne,
la geometria e i metadati restano; restano anche le proprietà del
contratto (`sorted_by`, `row_count`).

### Righe

1:1 per contratto. Il runner non esegue ancora le operazioni geo e nessun
esecutore chiama il kernel su una tabella. Il kernel rende «nessun
valore» (`None`) quando una delle due geometrie non ha coordinate; la
cella che ne nascerà, come quella di una geometria nulla, non è ancora
decisa da nessun esecutore.

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
  coordinate non finite, byte residui), `output_column` vuoto.

`other_wkb` non passa dalla validazione OGC in analisi: la fa il kernel.

In esecuzione il kernel (`extended::hausdorff_distance`) rende
`ExtendedError`, che nessun esecutore traduce ancora in `PlenoraError`:
`InvalidInput` (coordinate non finite o geometria non valida OGC, da
una parte o dall'altra), `WorkLimit` (più coppie di vertici di
`max_coordinate_pairs`), `IndexOverflow`, `ValidazioneNonConclusa` e
`CalcoloNonConcluso` (validazione o calcolo interrotti).

### Limiti e deviazioni

- **Solo vertici.** Diversa da `ST_HausdorffDistance` di PostGIS (GEOS
  `DiscreteHausdorffDistance`), che misura dai vertici di una geometria ai
  lati dell'altra: qui un vertice che sta su un lato dell'altra geometria
  ma lontano dai suoi vertici pesa per la distanza da quei vertici, e il
  risultato può essere maggiore (vedi l'esempio). Nessuna densificazione.
- Il lavoro è limitato da `max_coordinate_pairs` (prodotto dei vertici
  delle due geometrie), un argomento del kernel e non della config.

### Precisione

Le distanze fra vertici sono calcolate in `f64` (differenze e `hypot`),
con errore relativo di pochi ulp: sotto 1 cm per ogni distanza sotto
circa `1e13` unità del CRS. Il massimo e il minimo non arrotondano.
Nessun rifiuto `PrecisionInsufficient`
([README, «Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

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
