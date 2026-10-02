### Che cosa fa

Aggiunge colonne che descrivono la geometria di ogni riga: il tipo, il
numero di parti, il numero di anelli interni, il primo e l'ultimo punto di
una linea aperta e se la geometria è chiusa. Si sceglie quali colonne con
`fields`; la geometria non cambia.

Il calcolo per geometria è `extensions::geometry_accessors`, che il
runner chiama su ogni riga non nulla.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `fields` | lista di stringhe | tutte e sei | `geometry_type`, `num_geometries`, `num_interior_rings`, `start_point`, `end_point`, `is_closed`; non vuota, senza ripetizioni | colonne da aggiungere |
| `output_prefix` | stringa | `""` | qualunque | prefisso dei nomi delle colonne aggiunte |

Significato dei campi:

- `geometry_type`: `Point`, `LineString`, `Polygon`, `MultiPoint`,
  `MultiLineString`, `MultiPolygon`, `GeometryCollection`;
- `num_geometries`: 1 per le geometrie semplici, il numero di membri per
  multi-geometrie e collezioni (0 se vuote);
- `num_interior_rings`: i buchi di un `Polygon`, la loro somma su un
  `MultiPolygon`, 0 per gli altri tipi;
- `start_point`, `end_point`: `POINT(x y)` in WKT, solo per una
  `LineString` aperta con almeno due punti; null per ogni altro caso
  (linee chiuse, poligoni, multi-geometrie);
- `is_closed`: per una `LineString` se il primo punto coincide con
  l'ultimo (`true` anche per la linea vuota, come in `geo`), `true` per `Polygon` e `MultiPolygon`, `false` per gli altri
  tipi (`MultiLineString` compresa).

### Schema

Le colonne dell'ingresso restano; in coda si aggiungono, nell'ordine fisso
della lista sopra e non in quello di `fields`, le colonne richieste, con
nome `output_prefix` + campo: `geometry_type` `utf8`, `num_geometries`
`uint64`, `num_interior_rings` `uint64`, `start_point` `utf8`, `end_point`
`utf8`, `is_closed` `bool`. `start_point` ed `end_point` sono nullable
(nulli per una geometria che non è una linea aperta); le altre solo se lo
è la colonna geometria (nulle dove la geometria è null). Colonna geometria, metadati e
proprietà del contratto (`sorted_by`, `row_count`) restano.

### Righe

1:1. Una geometria nulla dà una cella nulla in ogni colonna aggiunta;
`start_point` e `end_point` sono nulli anche nei casi detti sopra.

### Ordine

Quello d'ingresso.

### Errori

In validazione (analisi del contratto):

- `InvalidPlan`: config con campi sconosciuti; `fields` vuota, con
  ripetizioni o con un nome fuori elenco;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come WKB; una colonna da aggiungere esiste
  già;
- `Unsupported`: dimensioni della geometria diverse da `xy`;
- `Crs`: colonna senza CRS o con un'incoerenza CRS non risolta.

In esecuzione ([Runner, «Operazioni geo»](runner.md#operazioni-geo))
il passo rende il primo errore in ordine di riga, senza diagnostica per
riga. Prima del kernel, per ogni cella non nulla della colonna geometria:

- `InvalidPlan`: struttura WKB non valida; `Unsupported`: la cella porta
  Z/M o uno SRID; `ResourceLimit`: la cella supera 64 MiB;
- `Crs`: una coordinata fuori dal dominio di validità del CRS della
  colonna;
- `Schema`: il contratto d'ingresso dichiara i tipi geometrici con un
  elenco e la cella è di un altro tipo.

Dal kernel (`ExtensionError`), per geometria: una geometria che non
supera la validazione OGC (`InvalidInput`) è `InvalidPlan`; una
validazione che non conclude è `Internal`.

### Limiti e deviazioni

Errori senza indice di riga della sorgente
([Runner, «Limiti dichiarati del runner»](runner.md#limiti-dichiarati-del-runner),
voce «Geo senza diagnostica per riga»).

### Precisione

Esatta: conteggi e tipi non calcolano nulla, e `start_point`/`end_point`
riportano le coordinate del vertice nel testo più corto che, riletto, dà lo
stesso `f64`
([Limiti dichiarati, «Precisione delle operazioni geografiche: 1 cm a terra»](limiti.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

### Complessità

O(v) per riga sui suoi `v` vertici, più la validazione OGC della geometria
(O(v²) nel caso peggiore); memoria O(n) per le colonne aggiunte.

### Esempio

`fields` in un ordine qualunque: le colonne escono nell'ordine fisso.

```json
{
  "config": {"fields": ["is_closed", "geometry_type", "start_point"]},
  "ingressi": [
    {"nome": "oggetti", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["LINESTRING(0 0,3 4)", "POLYGON((0 0,8 0,8 8,0 8,0 0),(2 2,4 2,4 4,2 2))"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["LINESTRING(0 0,3 4)", "POLYGON((0 0,8 0,8 8,0 8,0 0),(2 2,4 2,4 4,2 2))"]},
    {"nome": "geometry_type", "tipo": "utf8", "valori": ["LineString", "Polygon"]},
    {"nome": "start_point", "tipo": "utf8", "valori": ["POINT(0 0)", null]},
    {"nome": "is_closed", "tipo": "bool", "valori": [false, true]}
  ]}
}
```
