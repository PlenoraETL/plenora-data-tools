### Che cosa fa

Costruisce una colonna geometria di punti da due colonne numeriche: per
ogni riga il `Point` (x, y), nel CRS dato dalla config o dal piano. È un
produttore: l'ingresso non ha colonne geometria, l'uscita ne ha una.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `x_column` | stringa | `x` | colonna `float64` o `int64` dell'ingresso | coordinata x (est) |
| `y_column` | stringa | `y` | colonna `float64` o `int64` dell'ingresso | coordinata y (nord) |
| `geometry_column` | stringa | `geometry` | nome non vuoto (non di soli spazi) e non già presente | colonna aggiunta |
| `crs` | stringa | il CRS di piano | un identificatore della tabella dei CRS integrati, o la definizione del CRS di piano scritta uguale; proiettato | CRS della colonna prodotta |

Senza `crs` e senza CRS di piano il passo si rifiuta: il CRS non si
inventa.

### Schema

Le colonne d'ingresso restano tutte, `x_column` e `y_column` comprese. In
coda si aggiunge `geometry_column`: `binary` GeoArrow-WKB (estensione
`geoarrow.wkb`, metadato `geo` con il CRS e dimensioni `xy`), non
nullable, colonna geometria attiva con un nuovo identificatore di campo e
senza tipi dichiarati. I metadati di schema e le proprietà del contratto
(`sorted_by`, `row_count`) passano invariati.

### Righe

1:1 per contratto. Il kernel (`construction::point_from_lon_lat`) riceve
due `f64` e nessun adapter lo chiama ancora sulle righe: una x o una y
nulla, e la conversione di una colonna `int64` in `f64`, non sono definite
da codice eseguito (la colonna prodotta è dichiarata non nullable).

### Ordine

L'ordine d'ingresso: l'analisi conserva `sorted_by`.

### Errori

In validazione (analisi del contratto):

- `Schema`: l'ingresso ha già una colonna geometria; `x_column` o
  `y_column` non esiste o non è `float64` né `int64`; `geometry_column`
  esiste già;
- `InvalidPlan`: campi sconosciuti; `x_column`, `y_column` o
  `geometry_column` vuoti o di soli spazi;
- `Crs`: nessun `crs` e nessun CRS di piano; `crs` vuoto, fuori dalla
  tabella integrata o scritto come definizione (WKT, PROJJSON…) diversa da
  quella del piano; CRS geografico, o proiettato senza unità lineare.

In esecuzione, dal kernel, per riga (`ConstructionError`, che nessun codice
traduce ancora in `PlenoraError`):

- `NonFiniteCoordinate`: x o y è NaN o infinita (il messaggio chiama le due
  coordinate `lon` e `lat`, i nomi del kernel d'origine).

### Limiti e deviazioni

Il runner non esegue ancora le operazioni geo
([README, «Che cosa non c'è ancora»](../README.md#che-cosa-non-cè-ancora)).
Il kernel controlla solo che le coordinate siano finite, non che stiano
nel dominio del CRS. La conversione da `int64` a `f64` sarebbe esatta solo
fino a `2^53` in modulo. Il catalogo chiede un CRS proiettato: punti in
longitudine e latitudine non si costruiscono qui
([README, «CRS integrati»](../README.md#crs-integrati)).

### Precisione

Esatta: le coordinate `float64` diventano il punto senza calcolo
([README, «Precisione delle operazioni geografiche»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

### Complessità

Tempo O(n) sulle righe, O(1) per punto; memoria O(n) per la colonna
prodotta (21 byte di WKB per punto).

### Esempio

```json
{
  "config": {"crs": "EPSG:3857"},
  "ingressi": [
    {"nome": "rilievi", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2]},
      {"nome": "x", "tipo": "float64", "valori": [500000.0, 0.0]},
      {"nome": "y", "tipo": "float64", "valori": [4000000.0, 0.0]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2]},
    {"nome": "x", "tipo": "float64", "valori": [500000.0, 0.0]},
    {"nome": "y", "tipo": "float64", "valori": [4000000.0, 0.0]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POINT(500000 4000000)", "POINT(0 0)"]}
  ]}
}
```
