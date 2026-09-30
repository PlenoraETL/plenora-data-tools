### Che cosa fa

Trova i confini in comune fra i poligoni della tabella (i muri fra stanze
adiacenti, i confini fra particelle): per ogni coppia di righe i cui bordi
condividono tratti collineari scrive una riga con le posizioni delle due
righe, la lunghezza condivisa totale e i tratti, sul modello di
`ST_SharedPaths` di PostGIS. I contatti in un punto solo non contano.

La conversione di colonna è `extensions3::shared_paths_rows`, che il
runner chiama su tutta la colonna con i default della tabella sotto
([README, «Operazioni geo»](../README.md#operazioni-geo)).

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `tolerance` | numero | `0` | finito, `>= 0` | lunghezza sotto cui (compresa) un singolo tratto condiviso si scarta, nelle unità del CRS |
| `min_length` | numero | `0` | finito, `>= 0` | lunghezza condivisa totale minima perché la coppia dia una riga |

`tolerance` è una soglia di lunghezza dei tratti, non una distanza: due
bordi che corrono paralleli a meno di 1 cm senza essere collineari non
condividono nulla.

### Schema

Schema nuovo, in quest'ordine: `index_a` e `index_b` (`uint64`),
`shared_length` (`float64`), `geometry` (`binary`, CRS della colonna
d'ingresso, dimensioni `xy`, senza dichiarazione dei tipi), tutte non
nullable. Le colonne dell'ingresso spariscono, i metadati di schema
restano; nessuna proprietà del contratto sopravvive.

### Righe

Una riga per coppia `(a, b)`, con `a < b`, i cui rettangoli d'ingombro si
toccano e i cui bordi (anelli esterni e buchi) condividono tratti
collineari di lunghezza totale non nulla e almeno `min_length`:

- `index_a`, `index_b`: posizioni delle due righe nell'ingresso, da 0; le
  righe nulle e le geometrie vuote non partecipano ma contano nella
  posizione;
- `shared_length`: somma delle lunghezze dei tratti tenuti;
- `geometry`: un `LineString` di due punti se il tratto è uno, altrimenti
  una `MultiLineString` di segmenti, uno per coppia di lati sovrapposti
  (non fusi in catene).

Anche le coppie di poligoni che si sovrappongono danno una riga, se i loro
bordi hanno tratti collineari.

### Ordine

Per `index_a`, poi per `index_b`, crescenti. Dentro una riga, i segmenti
seguono anelli e lati del primo poligono, poi quelli del secondo.

### Errori

In validazione (analisi del contratto):

- `InvalidPlan`: config con campi sconosciuti; `tolerance` o `min_length`
  negativi o non finiti;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come WKB;
- `Unsupported`: dimensioni della geometria diverse da `xy`;
- `Crs`: colonna senza CRS risolto o CRS non proiettato.

In esecuzione, prima del kernel, su ogni cella non nulla ([README,
«Operazioni geo»](../README.md#operazioni-geo)): `InvalidPlan` per un WKB
malformato o con coordinate non finite, `Unsupported` per dimensioni Z/M o
SRID, `Crs` per una coordinata fuori dal dominio di validità del CRS della
colonna, `Schema` per una geometria di un tipo che il contratto
dell'ingresso dichiara con un elenco e che non vi compare.

Poi la conversione di colonna (messaggi del calcolo con prefisso
`geo.shared_paths:`):

- `InvalidPlan`: WKB malformato o OGC-invalido; una geometria che non è
  `Polygon` o `MultiPolygon` (il messaggio riporta la posizione della riga,
  non i dati); tratti prodotti non validi;
- `Unsupported`: WKB con dimensioni Z/M o SRID;
- `ResourceLimit`: cella oltre il limite di byte per cella;
- `Internal`: panico di `geo` o `rstar`, validazione che non conclude.

### Limiti e deviazioni

Solo tratti esattamente collineari: nessuna tolleranza di distanza, nessuna
fusione dei segmenti consecutivi in una linea sola. Nessuna diagnostica per riga: il passo rende il primo errore ([README,
«Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voci «Geo senza diagnostica per riga» e «Modelli di costo geo
provvisori»).

### Precisione

Esatta sulla geometria: la collinearità si decide con il predicato
`orient2d` esatto di `geo`, e gli estremi di un tratto condiviso sono
vertici d'ingresso. Solo le lunghezze (`shared_length`, e il confronto con
`tolerance` e `min_length`) si calcolano in `f64`. Nessuna griglia, nessun
controllo di precisione
([README, «Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

### Complessità

R-tree sui rettangoli d'ingombro, O(n log n); per ogni coppia candidata un
confronto di tutti i lati dell'uno con tutti i lati dell'altro, O(a · b),
con un filtro sui rettangoli dei segmenti. Memoria O(n) per tutte le
geometrie, decodificate prima del calcolo (classe bloccante).

### Esempio

Due stanze con un muro in comune e una terza che le tocca in un punto.

```json
{
  "config": {},
  "ingressi": [
    {"nome": "stanze", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POLYGON((0 0,4 0,4 4,0 4,0 0))", "POLYGON((4 0,8 0,8 4,4 4,4 0))", "POLYGON((8 4,10 4,10 6,8 6,8 4))"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "index_a", "tipo": "uint64", "valori": [0]},
    {"nome": "index_b", "tipo": "uint64", "valori": [1]},
    {"nome": "shared_length", "tipo": "float64", "valori": [4.0]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["LINESTRING(4 4,4 0)"]}
  ]}
}
```
