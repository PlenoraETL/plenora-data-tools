### Che cosa fa

Aggiunge una colonna `float64` con la distanza geodetica in metri fra il
punto di ogni riga e un punto fisso della config (`other_wkb`), calcolata
**sull'ellissoide WGS 84** con l'algoritmo di Karney (`Geodesic` di
`geo`, `geographiclib-rs`). Le coordinate sono longitudine e latitudine
in gradi.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `other_wkb` | stringa | obbligatorio | WKB 2D in esadecimale (cifre maiuscole o minuscole, lunghezza pari), longitudine in `[-180, 180]` e latitudine in `[-90, 90]` | secondo operando, nello stesso CRS della colonna |
| `output_column` | stringa | `geodesic_distance` | nome non vuoto e non già nello schema | colonna aggiunta |

### Schema

Aggiunge in coda `output_column`, `float64` nullable. Le altre colonne,
la geometria e i metadati restano; restano anche le proprietà del
contratto (`sorted_by`, `row_count`).

### Righe

1:1 per contratto. Il runner non esegue ancora le operazioni geo e nessun
esecutore chiama il kernel su una tabella. Il kernel
(`extended::geodesic_distance_m`) riceve **due punti**: l'analisi
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

- **Sempre l'ellissoide WGS 84.** Il requisito del catalogo chiede solo
  un CRS geografico: per un CRS su un altro ellissoide (ED50 e Monte
  Mario su Internazionale 1924, NAD27 su Clarke 1866, OSGB36 su Airy) la
  distanza si calcola comunque su WGS 84, senza errore. Per ED50 il
  semiasse maggiore differisce di 251 m (circa 4 parti su centomila:
  dell'ordine di 4 m su 100 km), sopra la precisione di 1 cm. GRS 1980
  (ETRS89, RDN2008, NAD83 e gli altri) differisce da WGS 84 di un decimo
  di millimetro nel semiasse minore: trascurabile.
- Solo punti, vedi «Righe».

### Precisione

Sull'ellissoide WGS 84 l'algoritmo di Karney resta molto sotto 1 cm
(l'errore dichiarato da `geographiclib` è dell'ordine dei nanometri).
Per un CRS su un altro ellissoide la regola di 1 cm **non vale**: vedi
«Limiti e deviazioni». Nessun rifiuto `PrecisionInsufficient`
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
