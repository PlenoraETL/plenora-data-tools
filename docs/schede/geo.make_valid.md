### Che cosa fa

Ripara la geometria di ogni riga che non supera la validazione OGC del
workspace (un anello che si auto-interseca, un buco che esce dalla shell,
parti di un multipoligono che si sovrappongono) con la regola `MakeValid`
`LINEWORK` di GEOS, eseguita in Rust puro sui lati nodati del bordo. Le
geometrie già valide escono invariate, byte per byte; i null restano null.
La riparazione può cambiare tipo: un poligono a farfalla diventa un
`MultiPolygon`, e le parti collassate (lati ripercorsi, anelli ridotti a un
punto) escono come linee o punti accanto all'area, in una
`GeometryCollection`.

La semantica a livello di tabella è quella dell'esecuzione Arrow
(`rust_backend::arrow::make_valid_batches`), che il runner chiama su tutta
la tabella con la precisione del CRS della colonna ([README, «Operazioni
geo»](../README.md#operazioni-geo)).

### Parametri

Nessuno: la config è `{}`. Metodo e parti collassate non si scelgono dal
piano: l'esecuzione Arrow usa sempre `LINEWORK` con le parti collassate
conservate. `STRUCTURE` (la regola di `GeometryFixer`) è raggiungibile solo
dall'API dei kernel (`rust_backend::make_valid_wkb`).

### Schema

Le colonne restano quelle dell'ingresso, nelle stesse posizioni, con gli
stessi tipi e la stessa nullabilità. Il campo geometria conserva tutti i
suoi metadati (CRS, dimensioni, encoding, chiavi di lineage) tranne la
dichiarazione dei tipi geometrici, che la riparazione riscrive: il
contratto dichiara `mixed` senza elenco. I metadati di schema e le
proprietà del contratto (`sorted_by`, `row_count`) restano.

### Righe

1:1. Una cella null resta null; una cella valida esce con gli stessi byte;
una cella invalida esce riparata e rivalidata.

### Ordine

Quello d'ingresso.

### Errori

In validazione (analisi del contratto):

- `InvalidPlan`: config con un campo qualunque;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come WKB (né estensione `geoarrow.wkb` né
  chiavi `plenora.geometry.*`);
- `Unsupported`: dimensioni della geometria diverse da `xy`;
- `Crs`: colonna senza CRS o con un'incoerenza CRS non risolta.

In esecuzione, prima del kernel, su ogni cella non nulla (runner, [README,
«Operazioni geo»](../README.md#operazioni-geo)): la decodifica strutturale
(`InvalidPlan` per un WKB malformato o con coordinate non finite,
`Unsupported` per dimensioni Z/M o SRID), `Crs` per una coordinata fuori
dal dominio di validità del CRS della colonna, `Schema` per una geometria
di un tipo che il contratto dell'ingresso dichiara con un elenco e che non
vi compare. La validità OGC non si controlla qui: è il lavoro del kernel.

Poi l'esecuzione Arrow (vince la prima cella che fallisce in ordine di
riga):

- `Schema`: colonna geometria assente o non `Binary`;
- `ResourceLimit`: cella oltre il limite di byte per cella; prenotazione di
  memoria fallita; input invalido con più di 10.000 segmenti (`WorkLimit`:
  il lavoro stimato è il quadrato dei segmenti); coordinate o componenti
  d'uscita oltre i limiti per cella;
- `InvalidPlan`: WKB malformato o con coordinate non finite; riparazione
  che resta invalida; precisione del CRS non valida;
- `Unsupported`: WKB con dimensioni Z/M o SRID; noding non convergente;
  segno d'area non decidibile su coordinate fuori da `[2^-450, 2^450]`
  (`NumericRange`); `PrecisionInsufficient` (sotto, «Precisione»);
- `Internal`: panico di `geo` dentro il kernel, invariante violata,
  validazione OGC che non conclude.

### Limiti e deviazioni

- Il kernel è quello del laboratorio, qualificato contro GEOS per
  equivalenza semantica, non byte per byte: ordine delle parti, punto
  iniziale e verso degli anelli e scelta fra `Polygon`, `MultiPolygon` e
  `GeometryCollection` possono differire da GEOS a parità di geometria
  ([README, «`geo.make_valid`, `geo.polygonize`, `geo.split`: equivalenza a
  GEOS verificata, non dimostrata»](../README.md#geomake_valid-geopolygonize-geosplit-equivalenza-a-geos-verificata-non-dimostrata)).
- `LINEWORK` è una funzione dell'insieme dei lati d'ingresso: anelli e parti
  permutati, ruotati o invertiti danno la stessa geometria. Non è il
  pari-dispari su tutti gli anelli: un buco che condivide un lato con la
  shell e ne sporge, o parti sovrapposte, seguono la regola di GEOS.
- «Valido» è la validazione OGC del workspace (quella di `geo` più il
  controllo degli anelli con punta), non `IsValid` di GEOS: dove le due
  divergono, una geometria può essere riparata qui e restituita invariata
  da GEOS, o viceversa.
- Oltre 10.000 segmenti un input invalido si rifiuta (GEOS non aveva
  limite); un input valido passa a ogni dimensione. Il caso peggiore dei
  giri di `LINEWORK` non ha un budget di tempo proprio.
- Elenco completo: [README, «Differenze da GEOS»](../README.md#differenze-da-geos)
  e [README, «Operazioni topologiche in Rust puro»](../README.md#operazioni-topologiche-in-rust-puro).
- Nel runner il passo rende il primo errore, senza diagnostica per riga
  ([README, «Limiti dichiarati del
  runner»](../README.md#limiti-dichiarati-del-runner), voci «Geo senza
  diagnostica per riga» e «Modelli di costo geo»).

### Precisione

`LINEWORK` non passa dalla griglia di `i_overlay`: l'unico calcolo che
arrotonda è il noding del bordo, e ogni passo successivo è un'operazione
esatta sull'insieme dei lati nodati. Le regole di 1 cm
([README, «Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)):

- coordinate troppo rade (unità in ultima posizione del modulo massimo
  oltre `p / 64`): `PrecisionInsufficient`, prima di ogni calcolo;
- ogni punto d'incrocio arrotondato del noding sta entro `p / 5` da
  entrambi i segmenti che divide (al più cinque giri, quindi entro `p`);
  oltre, `PrecisionInsufficient`;
- un incrocio arrotondato a meno di `p` da un altro vertice o da un lato
  non incidente: `PrecisionInsufficient`, perché lì la topologia delle
  facce si deciderebbe sotto la precisione. Due incroci arrotondati sullo
  stesso vertice non sono riconosciuti;
- segni e confronti d'area sono esatti; una geometria valida non si tocca.

Sotto la precisione feature d'ingresso più vicine di 1 cm possono fondersi
o cambiare la topologia senza errore, come dichiarato. `p` è la precisione
del CRS della colonna (`Precision::from_crs`).

### Complessità

Per riga, con `v` i vertici della cella: la validazione OGC del workspace
(scansione sui rettangoli d'ingombro, O(v²) nel caso peggiore). Solo per le
celle invalide, in più: la validazione interna del kernel
(`check_validation` di `geo`, O(v²)), il noding (coppie di segmenti con
filtro sugli inviluppi, quadratico nel caso peggiore), un polygonize per
giro (i giri sono al più i lati nodati: il caso peggiore, anelli
concentrici collegati, è quadratico nei lati per il logaritmo, entro il
tetto di 10.000 segmenti) e la rivalidazione dell'uscita. Memoria O(v) per
la cella in lavorazione, più le celle d'uscita.

### Esempio

Una farfalla, un quadrato valido e un null.

```json
{
  "config": {},
  "ingressi": [
    {"nome": "lotti", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POLYGON((0 0,2 2,2 0,0 2,0 0))", "POLYGON((0 0,10 0,10 10,0 10,0 0))", null]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["MULTIPOLYGON(((0 2,0 0,1 1,0 2)),((1 1,2 0,2 2,1 1)))", "POLYGON((0 0,10 0,10 10,0 10,0 0))", null]}
  ]}
}
```
