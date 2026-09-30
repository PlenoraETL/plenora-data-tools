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
dieci colonne senza metadati; `validity_reason` e i quattro `bounds_*`
sono nullable (nulli per una geometria valida o vuota), le altre cinque
solo se lo è la colonna geometria (nulle dove la geometria è null):

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
descrive una geometria alla volta. Una geometria nulla dà dieci celle
nulle. Il runner verifica solo la struttura WKB e il dominio del CRS, non
la validità OGC: una geometria non valida si descrive, non si rifiuta.
Dal WKB validato nella struttura non arrivano coordinate non finite:
`is_finite` falso si vede solo con geometrie costruite altrove.

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

In esecuzione ([README, «Operazioni geo»](../README.md#operazioni-geo))
il passo rende il primo errore in ordine di riga, senza diagnostica per
riga. Prima del kernel, per ogni cella non nulla della colonna geometria:

- `InvalidPlan`: struttura WKB non valida; `Unsupported`: la cella porta
  Z/M o uno SRID; `ResourceLimit`: la cella supera 64 MiB;
- `Crs`: una coordinata fuori dal dominio di validità del CRS della
  colonna;
- `Schema`: il contratto d'ingresso dichiara i tipi geometrici con un
  elenco e la cella è di un altro tipo.

Il kernel (`ExtendedAlgorithmError`) non rifiuta le geometrie non valide;
rifiuta solo con `ValidazioneNonConclusa` (la validazione non conclude: il
referto direbbe un verdetto che non esiste), che diventa `Internal`, e
`IndexOverflow` (conteggio oltre `u64`), che diventa `InvalidPlan`.

### Limiti e deviazioni

- La ragione è una classificazione del messaggio di `geo`, senza posizione
  né coordinate: indica il tipo di difetto, non dove sta.
- La validazione è quella di `geo` 0.33.1 con la ricerca rapida delle
  auto-intersezioni ([README, «Validazione OGC: la ricerca delle
  auto-intersezioni non è quella di `geo`, il verdetto sì»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì)),
  con in più il rifiuto degli anelli con una punta
  (`anello con auto-intersezione`).
- Una coordinata fuori dal dominio del CRS non si descrive: il controllo
  del runner prima del kernel ferma il passo con `Crs`.
- Errori senza indice di riga della sorgente
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voce «Geo senza diagnostica per riga»).

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
