### Che cosa fa

Aggiunge una colonna `float64` con la posizione, lungo la linea di ogni
riga, del punto della linea più vicino a un punto fisso dato nella config:
la frazione della lunghezza totale da 0 (inizio) a 1 (fine), come
`ST_LineLocatePoint` di PostGIS. Le geometrie che non sono `LineString`
danno null.

Il calcolo per geometria è `extensions::line_locate_point`, che il
runner chiama su ogni riga non nulla.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `point_wkb` | stringa | obbligatorio | WKB esadecimale di un `Point` valido, nel dominio del CRS della colonna | il punto da proiettare, nello stesso CRS della colonna |
| `output_column` | stringa | `fraction` | nome non vuoto e libero | colonna aggiunta |

### Schema

Aggiunge in coda `output_column`, `float64` nullable. Le altre colonne, i
metadati e le proprietà del contratto (`sorted_by`, `row_count`) restano.

### Righe

1:1. Il valore è:

- per una `LineString` di almeno due punti, la frazione in `[0, 1]`: un
  punto oltre un estremo si proietta sull'estremo; se due segmenti sono
  alla stessa distanza dal punto vince il primo nel verso della linea;
- null per ogni altro tipo, `MultiLineString` compresa, e per la linea
  vuota.

Una linea con un solo punto distinto non supera la validazione OGC: errore,
non un valore.

Una geometria nulla dà una cella nulla.

### Ordine

Quello d'ingresso.

### Errori

In validazione (analisi del contratto):

- `InvalidPlan`: config con campi sconosciuti; `point_wkb` mancante, non
  esadecimale, malformato, OGC-invalido o non `Point`; `output_column`
  vuota;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come WKB; `output_column` già presente;
- `Unsupported`: dimensioni della geometria diverse da `xy`; `point_wkb`
  con dimensioni Z/M o SRID;
- `Crs`: colonna senza CRS o con un'incoerenza CRS non risolta; punto fuori
  dal dominio del CRS.

In esecuzione ([README, «Operazioni geo»](../README.md#operazioni-geo))
il passo rende il primo errore in ordine di riga, senza diagnostica per
riga. Prima del kernel, per ogni cella non nulla della colonna geometria:

- `InvalidPlan`: struttura WKB non valida; `Unsupported`: la cella porta
  Z/M o uno SRID; `ResourceLimit`: la cella supera 64 MiB;
- `Crs`: una coordinata fuori dal dominio di validità del CRS della
  colonna;
- `Schema`: il contratto d'ingresso dichiara i tipi geometrici con un
  elenco e la cella è di un altro tipo.

Dal kernel (`ExtensionError`), per geometria: una geometria che non
supera la validazione OGC (`InvalidInput`) è `InvalidPlan`; un panico di
`geo` o una validazione che non conclude sono `Internal`.

### Limiti e deviazioni

Una `MultiLineString` dà null, non la frazione lungo la parte più vicina.
Errori senza indice di riga della sorgente
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voce «Geo senza diagnostica per riga»).

### Precisione

Nessun controllo dedicato: distanze e frazione sono calcolate in `f64` da
`geo` (distanza dai segmenti, proiezione sul segmento più vicino, lunghezze
cumulate), con errori d'arrotondamento relativi alla lunghezza della linea,
molto sotto 1 cm per le lunghezze dei CRS reali. Dove due segmenti sono alla
stessa distanza dal punto entro l'arrotondamento, la scelta fra i due, e
con essa la frazione, può cambiare con l'ultimo bit delle coordinate
([README, «Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

### Complessità

O(v) per riga sui suoi `v` vertici, più la validazione OGC della geometria
(O(v²) nel caso peggiore); memoria O(n) per la colonna aggiunta.

### Esempio

`point_wkb` è `POINT(5 3)`.

```json
{
  "config": {"point_wkb": "010100000000000000000014400000000000000840"},
  "ingressi": [
    {"nome": "tratte", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["LINESTRING(0 0,10 0)", "LINESTRING(0 0,10 0,10 10)", "POLYGON((0 0,4 0,4 4,0 4,0 0))"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["LINESTRING(0 0,10 0)", "LINESTRING(0 0,10 0,10 10)", "POLYGON((0 0,4 0,4 4,0 4,0 0))"]},
    {"nome": "fraction", "tipo": "float64", "valori": [0.5, 0.25, null]}
  ]}
}
```
