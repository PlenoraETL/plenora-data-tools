### Che cosa fa

Aggiunge una colonna `bool` che dice se la geometria della riga (A) contiene la geometria costante `other_wkb` (B): nessun punto di B è fuori da A e almeno un punto dell'interno di B è nell'interno di A.

Maschera DE-9IM `T*****FF*`: interno/interno non vuota, esterno di A/interno di B ed esterno di A/confine di B vuote. Un B tutto sul confine di A (un punto sul lato di un poligono, un lato dell'anello) non è contenuto: per questo c'è [`geo.predicate_covers`](#geopredicate_covers). Una geometria contiene se stessa. Il contrario di [`geo.predicate_within`](#geopredicate_within) a operandi scambiati. Con una geometria vuota è falso.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `other_wkb` | stringa | obbligatorio | WKB 2D in esadecimale (cifre maiuscole o minuscole, in numero pari), nel CRS dell'input e dentro il suo dominio di validità | la geometria B, uguale per tutte le righe |
| `output_column` | stringa | `predicate_contains` | nome non vuoto e non già nello schema | colonna aggiunta |

`other_wkb` si decodifica e si valida in analisi, una volta per piano:
struttura (conteggi, anelli chiusi, coordinate finite), validità OGC e
dominio del CRS. Il kernel la rivaluta comunque a ogni riga.

### Schema

Aggiunge in coda `output_column`, `bool`, nullable solo se lo è la colonna geometria (null dove la geometria è null), senza metadati. Le altre
colonne restano nell'ordine e con i loro metadati; la colonna geometria
resta com'è (tipi dichiarati, CRS, dimensioni `xy`). Metadati di schema e
proprietà del contratto (`sorted_by`, `row_count`) si conservano.

### Righe

1:1: una cella per riga. Per il contratto `other_wkb` è il secondo
operando: la geometria della riga è A, `other_wkb` è B, e il kernel valuta
`predicates::evaluate(A, B, SpatialPredicate::Contains)` su ogni riga. Una
geometria nulla dà una cella nulla, senza chiamare il kernel; una
geometria vuota dà il verdetto del predicato sul vuoto (vedi sopra). Le
righe sono indipendenti: il runner le calcola in parallelo e rende il
primo errore in ordine di riga
([README, «Operazioni geo»](../README.md#operazioni-geo)).

### Ordine

Quello d'ingresso.

### Errori

In validazione (analisi del contratto; nell'ordine: forma dell'ingresso,
CRS, config):

- `InvalidPlan`: più o meno di un ingresso; config con campi sconosciuti,
  senza `other_wkb` o con un tipo sbagliato; `other_wkb` vuoto, di
  lunghezza dispari o con caratteri non esadecimali; WKB strutturalmente
  non valido (byte o conteggi oltre i limiti, annidamento, anelli non
  chiusi, coordinate NaN o infinite, byte in coda); `other_wkb` non valida
  per l'OGC; `output_column` vuoto;
- `Unsupported`: `other_wkb` con coordinate Z/M o con SRID (EWKB); colonna
  geometria con dimensioni diverse da `xy`;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come geometria WKB; `output_column` già
  presente;
- `Crs`: CRS della colonna assente o non risolto; CRS non proiettato o
  senza unità lineare (requisito del catalogo); una coordinata di
  `other_wkb` fuori dal dominio di validità del CRS;
- `Internal`: la decodifica o la validazione OGC di `other_wkb` non
  conclude.

In esecuzione, il primo errore in ordine di riga, senza diagnostica per
riga. Prima del kernel, su ogni cella non nulla:

- `InvalidPlan` o `Unsupported`: la cella non è WKB strutturalmente
  valido (`Unsupported` per coordinate Z/M o SRID);
- `ResourceLimit`: la cella supera il limite di byte per cella;
- `Crs`: una coordinata fuori dal dominio di validità del CRS della
  colonna;
- `Schema`: il contratto dell'ingresso dichiara un elenco di tipi
  geometrici e la cella è di un altro tipo.

Poi il kernel (`predicates::evaluate`), con l'errore tradotto nella
categoria di `PlenoraError` ([README, «Operazioni geo»](../README.md#operazioni-geo),
voce «Errori»):

- `InvalidPlan`: `NonFiniteCoordinate` o `InvalidGeometry` (geometria
  della riga non valida per l'OGC; `other_wkb` l'ha già esclusa
  l'analisi);
- `Internal`: `ValidazioneNonConclusa` se la validazione non conclude,
  `CalcoloNonConcluso` se `relate` di `geo` va in panico su due geometrie
  valide (per esempio una `GeometryCollection` con membri poligonali che
  si sovrappongono).

### Limiti e deviazioni

- Il secondo operando è una geometria costante della config, non una
  seconda colonna: un input ha una sola colonna geometria.
- La matrice è quella di `relate` di `geo` 0.33.1, non di GEOS. Il confine
  segue la regola mod-2 dell'OGC: l'estremo condiviso da un numero pari di
  linee di una `MultiLineString` è interno, una linea chiusa non ha
  confine. Le collezioni non si uniscono prima del confronto: il lato
  condiviso da due membri poligonali adiacenti di una `GeometryCollection`
  resta confine, e membri poligonali che si sovrappongono possono far
  fallire il calcolo.
- Le due geometrie devono essere valide per l'OGC: una geometria non
  valida è un errore, non un verdetto. La validazione di entrambe si
  ripete a ogni valutazione ([README, «Validazione OGC: la ricerca delle
  auto-intersezioni non è quella di `geo`, il verdetto sì»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì)).
- Si calcola sempre la matrice intera: nessuna scorciatoia per il singolo
  predicato.
- Nessuna diagnostica per riga: un errore è il primo in ordine di riga,
  e il suo indice è una riga dell'ingresso del passo, non della sorgente
  ([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
  voce «Geo senza diagnostica per riga»). Il costo in memoria è una
  previsione del modello geo misurato (voce «Modelli di costo geo»).

### Precisione

Nessuna tolleranza e nessun rifiuto `PrecisionInsufficient`: il verdetto
viene dalla matrice DE-9IM calcolata sulle coordinate `f64` così come
sono, con orientazioni esatte. Due geometrie distanti meno di 1 cm non si
toccano. I punti d'incrocio fra lati che `relate` calcola sono arrotondati
in `f64`: su lati distinti più vicini della precisione (quasi coincidenti,
un vertice a pochi ulp da un lato) il verdetto può dipendere da
quell'arrotondamento, fuori dalla garanzia di 1 cm ([README, «Precisione
delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

### Complessità

Per riga, con `n` e `m` i vertici della riga e di `other_wkb`: `relate`
indicizza i lati con un R-tree, O((n + m) log(n + m)) tipico e O(n·m) nel
caso peggiore (molti lati che si incrociano); in più la validazione OGC di
entrambe, O(n²) e O(m²) nel caso peggiore. Memoria O(n + m).

### Esempio

Il punto `(10 5)` è interno al quadrato grande e sul lato di quello piccolo; B è `POINT(10 5)`.

```json
{
  "config": {"other_wkb": "010100000000000000000024400000000000001440"},
  "ingressi": [
    {"nome": "luoghi", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POLYGON((0 0,20 0,20 20,0 20,0 0))", "POLYGON((0 0,10 0,10 10,0 10,0 0))", "POLYGON((20 0,30 0,30 10,20 10,20 0))"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POLYGON((0 0,20 0,20 20,0 20,0 0))", "POLYGON((0 0,10 0,10 10,0 10,0 0))", "POLYGON((20 0,30 0,30 10,20 10,20 0))"]},
    {"nome": "predicate_contains", "tipo": "bool", "valori": [true, false, false]}
  ]}
}
```
