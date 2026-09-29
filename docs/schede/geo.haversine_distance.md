### Che cosa fa

Aggiunge una colonna `float64` con la distanza in metri fra il punto di
ogni riga e un punto fisso della config (`other_wkb`), lungo il cerchio
massimo di una **sfera** di raggio 6 371 008,8 m (raggio medio di GRS 80,
`Haversine` di `geo`). Le coordinate sono longitudine e latitudine in
gradi. Per la distanza sull'ellissoide c'è
[`geo.geodesic_distance`](#geogeodesic_distance).

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `other_wkb` | stringa | obbligatorio | WKB 2D in esadecimale (cifre maiuscole o minuscole, lunghezza pari), longitudine in `[-180, 180]` e latitudine in `[-90, 90]` | secondo operando, nello stesso CRS della colonna |
| `output_column` | stringa | `haversine_distance` | nome non vuoto e non già nello schema | colonna aggiunta |

### Schema

Aggiunge in coda `output_column`, `float64` nullable. Le altre colonne,
la geometria e i metadati restano; restano anche le proprietà del
contratto (`sorted_by`, `row_count`).

### Righe

1:1 per contratto. Il runner non esegue ancora le operazioni geo e nessun
esecutore chiama il kernel su una tabella. Il kernel
(`extended::haversine_distance_m`) riceve **due punti**: l'analisi
accetta ogni tipo geometrico nella colonna e in `other_wkb`, e che cosa
valga una riga non puntuale, o nulla, non è ancora deciso.

### Ordine

Per contratto quello d'ingresso (forma 1:1).

### Errori

In validazione (analisi del contratto):

- `Schema`: l'ingresso non ha esattamente una colonna geometria, la
  colonna non si dichiara geometria WKB, o `output_column` esiste già;
- `Unsupported`: dimensioni della colonna diverse da XY (anche non
  dichiarate), o `other_wkb` con Z/M o SRID;
- `Crs`: CRS della colonna mancante o non risolto, o non geografico
  (`GEOGRAPHIC_CRS_REQUIRED`); una coordinata di `other_wkb` fuori da
  longitudine e latitudine ammesse (`COORDINATE_OUT_OF_CRS_DOMAIN`);
- `InvalidPlan`: config con campi sconosciuti, `other_wkb` assente, non
  esadecimale o WKB non valido nella struttura, `output_column` vuoto.

In esecuzione il kernel rende `ExtendedError`, che nessun esecutore
traduce ancora in `PlenoraError`: `InvalidGeographicCoordinate` (una
coordinata non finita o fuori da `[-180, 180]` × `[-90, 90]`),
`CalcoloNonConcluso` (il calcolo di `geo` va in panico).

### Limiti e deviazioni

- **Sfera, non ellissoide.** Il raggio è fisso e non dipende dal datum
  del CRS: la distanza differisce dalla geodetica sull'ellissoide di
  qualche millesimo del valore. Dall'origine, un grado di latitudine vale
  111 195,08 m qui e 110 574,39 m sull'ellissoide WGS 84 (5,6 per mille),
  un grado di longitudine 111 195,08 m qui e 111 319,49 m
  sull'ellissoide.
- Solo punti, vedi «Righe».

### Precisione

La regola di 1 cm non riguarda il modello: il risultato è la distanza
sulla sfera, non la distanza vera sull'ellissoide (sopra). Il calcolo
(`2 asin(sqrt(a))` in `f64`) è accurato a pochi ulp relativi lontano
dagli antipodi; vicino agli antipodi `asin` è mal condizionato e
l'errore d'arrotondamento non è misurato né limitato a 1 cm. Nessun
rifiuto `PrecisionInsufficient`
([README, «Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

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
    {"nome": "haversine_distance", "tipo": "float64", "valori": [111195.0802335329, 111195.0802335329]}
  ]}
}
```
