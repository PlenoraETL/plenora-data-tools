### Che cosa fa

Sostituisce la colonna geometria con dieci colonne che la descrivono: tipo,
numero di coordinate, se è vuota, se ha solo coordinate finite, se è valida
per l'OGC e perché no, e il rettangolo d'ingombro. Accetta di proposito le
geometrie non valide, che descrive invece di rifiutare; non esegue alcun
algoritmo su coordinate non finite.

### Parametri

Nessuno: la config è `{}`.

### Schema

La colonna geometria si toglie e al suo posto, nello stesso punto, entrano
dieci colonne nullable senza metadati:

| colonna | tipo | contenuto |
| --- | --- | --- |
| `geometry_type` | `utf8` | `Point`, `LineString`, `Polygon`, `MultiPoint`, `MultiLineString`, `MultiPolygon`, `GeometryCollection` |
| `coordinate_count` | `uint64` | coordinate, duplicati e chiusure degli anelli compresi |
| `is_empty` | `bool` | `coordinate_count` è 0 |
| `is_finite` | `bool` | nessuna coordinata NaN o infinita |
| `is_valid` | `bool` | la geometria supera la validazione OGC; falso con coordinate non finite |
| `validity_reason` | `utf8` | nullo se valida; altrimenti la ragione, senza coordinate |
| `bounds_minx`, `bounds_miny`, `bounds_maxx`, `bounds_maxy` | `float64` | rettangolo d'ingombro; nulli se la geometria è vuota o non finita |

Le ragioni sono `coordinate NaN o infinite`, `punti distinti insufficienti`,
`anello con auto-intersezione`, `anelli che si intersecano`, `anello
interno fuori dal proprio esterno`, `poligoni sovrapposti` e `forma non
valida non ulteriormente distinta`. Il contratto d'uscita non ha più
colonne geometria; le altre colonne, i metadati di schema e le proprietà del
contratto (`sorted_by`, `row_count`) restano.

### Righe

1:1: un referto per riga. Il kernel (`extended_algorithms::geometry_diagnostics`)
descrive una geometria alla volta. Il runner non esegue ancora le
operazioni geo ([README, «Che cosa non c'è ancora»](../README.md#che-cosa-non-cè-ancora)):
che cosa renda una cella nulla lo fisserà l'esecutore geo, come la
decodifica, che per descrivere una geometria non valida non deve
rifiutarla. Dal WKB validato nella struttura non arrivano coordinate non
finite: `is_finite` falso si vede solo con geometrie costruite altrove.

### Ordine

Quello d'ingresso; le dieci colonne nell'ordine della tabella sopra, nella
posizione della colonna geometria.

### Errori

In validazione (analisi del contratto; nell'ordine: forma dell'ingresso,
config, CRS, nomi):

- `InvalidPlan`: più o meno di un ingresso; config non vuota;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come geometria WKB; un'altra colonna ha già
  il nome di una delle dieci;
- `Unsupported`: colonna geometria con dimensioni diverse da `xy`;
- `Crs`: CRS della colonna assente o non risolto (serve un CRS noto, di
  qualunque tipo).

In esecuzione: il runner non esegue l'operazione, e agli errori del kernel
non è ancora assegnata una variante `PlenoraError`. Il kernel non rifiuta
le geometrie non valide; rifiuta solo con `ValidazioneNonConclusa` (la
validazione non conclude: il referto direbbe un verdetto che non esiste) e
`IndexOverflow` (conteggio oltre `u64`).

### Limiti e deviazioni

- La ragione è una classificazione del messaggio di `geo`, senza posizione
  né coordinate: indica il tipo di difetto, non dove sta.
- La validazione è quella di `geo` 0.33.1 con la ricerca rapida delle
  auto-intersezioni ([README, «Validazione OGC: la ricerca delle
  auto-intersezioni non è quella di `geo`, il verdetto sì»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì)),
  con in più il rifiuto degli anelli con una punta
  (`anello con auto-intersezione`).

### Precisione

Esatta: nessuna coordinata si calcola. Il rettangolo è il minimo e il
massimo delle coordinate, con i loro bit; il verdetto di validità è quello
della validazione OGC sulle coordinate come sono, senza tolleranza
([README, «Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

### Complessità

O(n) per geometria, con `n` le coordinate, più la validazione OGC (O(n²)
nel caso peggiore). Memoria O(1) oltre la geometria.

### Esempio

Un quadrato valido, un anello a farfalla e una collezione vuota.

```json
{
  "config": {},
  "ingressi": [
    {"nome": "lotti", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POLYGON((0 0,10 0,10 10,0 10,0 0))", "POLYGON((0 0,10 10,10 0,0 10,0 0))", "GEOMETRYCOLLECTION EMPTY"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
    {"nome": "geometry_type", "tipo": "utf8", "valori": ["Polygon", "Polygon", "GeometryCollection"]},
    {"nome": "coordinate_count", "tipo": "uint64", "valori": [5, 5, 0]},
    {"nome": "is_empty", "tipo": "bool", "valori": [false, false, true]},
    {"nome": "is_finite", "tipo": "bool", "valori": [true, true, true]},
    {"nome": "is_valid", "tipo": "bool", "valori": [true, false, true]},
    {"nome": "validity_reason", "tipo": "utf8", "valori": [null, "anello con auto-intersezione", null]},
    {"nome": "bounds_minx", "tipo": "float64", "valori": [0.0, 0.0, null]},
    {"nome": "bounds_miny", "tipo": "float64", "valori": [0.0, 0.0, null]},
    {"nome": "bounds_maxx", "tipo": "float64", "valori": [10.0, 10.0, null]},
    {"nome": "bounds_maxy", "tipo": "float64", "valori": [10.0, 10.0, null]}
  ]}
}
```
