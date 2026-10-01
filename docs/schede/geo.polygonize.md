### Che cosa fa

Costruisce i poligoni racchiusi dalle linee di tutta la tabella: raccoglie
le geometrie lineari non nulle di tutte le righe, le noda nei punti
d'incrocio (salvo `node_input: false`) ed estrae le facce del grafo
planare. Ogni faccia diventa una riga `polygon`; le linee che non chiudono
una faccia escono come residui, classificati come in GEOS: `cut_edge` (lato
con la stessa faccia da entrambe le parti), `dangle` (tratto pendente con un
estremo libero), `invalid_ring` (anello che non forma un poligono valido).
Gli attributi delle righe d'ingresso non passano.

La semantica a livello di tabella è quella dell'esecuzione Arrow
(`rust_backend::arrow::polygonize_batches`), che il runner chiama su tutta
la tabella con la precisione del CRS della colonna e il limite di righe
dell'arco d'uscita ([README, «Operazioni geo»](../README.md#operazioni-geo)).

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `node_input` | booleano | `true` | `true`, `false` | noda le linee negli incroci prima di costruire il grafo |
| `require_complete` | booleano | `false` | `true`, `false` | fallisce se restano residui, invece di emetterli |

Con `node_input: false` il grafo usa i segmenti così come sono: due linee
che si incrociano senza un vertice comune non si dividono, e le linee
duplicate escono come `cut_edge` nell'ordine d'ingresso.

### Schema

Due colonne, in quest'ordine:

- la colonna geometria, con il nome e tutti i metadati del campo
  d'ingresso (CRS, dimensioni, encoding, lineage) tranne la dichiarazione
  dei tipi, non nullable (una riga per faccia o residuo); il contratto
  dichiara i tipi `exact`
  `LineString` e `Polygon`;
- `__class`, `utf8` non nullable: `polygon`, `cut_edge`, `dangle`,
  `invalid_ring`.

Le altre colonne dell'ingresso spariscono; i metadati di schema restano.
Nessuna proprietà del contratto sopravvive (`sorted_by` e `row_count`
cadono).

### Righe

Aggregazione dell'intera tabella: da 0 righe (ingresso vuoto o tutto
nullo) a una per faccia e per residuo. Le celle nulle si saltano; nessuna
riga d'uscita ha geometria nulla. Un poligono
con buchi resta una riga sola.

### Ordine

Prima i poligoni, nell'ordine di estrazione delle facce (chiavi di
coordinata ordinate), con l'anello esterno antiorario; poi i residui per
classe (`cut_edge`, `dangle`, `invalid_ring`), ognuna nell'ordine del grafo.
Il contenuto coincide con GEOS sui casi qualificati, l'ordine no.
Deterministico: stesso ingresso, stesse righe nello stesso ordine.

### Errori

In validazione (analisi del contratto):

- `InvalidPlan`: config con campi sconosciuti o valori non booleani;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come WKB;
- `Unsupported`: dimensioni della geometria diverse da `xy`;
- `Crs`: colonna senza CRS risolto, o CRS non proiettato.

In esecuzione, prima del kernel, su ogni cella non nulla ([README,
«Operazioni geo»](../README.md#operazioni-geo)): `InvalidPlan` per un WKB
malformato o con coordinate non finite, `Unsupported` per dimensioni Z/M o
SRID, `Crs` per una coordinata fuori dal dominio di validità del CRS della
colonna, `Schema` per una geometria di un tipo che il contratto
dell'ingresso dichiara con un elenco e che non vi compare. La validità OGC non si controlla qui: la
controlla l'esecuzione Arrow.

Poi l'esecuzione Arrow:

- `Schema`: colonna geometria assente o non `Binary`;
- `ResourceLimit`: cella oltre il limite di byte per cella; prenotazione di
  memoria fallita; più di 100.000.000 coordinate in ingresso o in uscita;
  più di 100.000.000 coppie di segmenti esaminate dal noding; righe
  d'uscita oltre il limite di righe dell'arco (`max_output_rows` per un
  output del piano, `max_rows_per_edge` altrimenti; contato anche sulle
  facce intermedie);
- `InvalidPlan`: una cella che non è `LineString`, `MultiLineString` o
  collezione di linee; WKB malformato o OGC-invalido;
  `require_complete` con residui (il messaggio riporta il numero di residui
  per classe); una faccia che non supera la validazione;
- `Unsupported`: WKB con dimensioni Z/M o SRID; noding non convergente;
  segno o confronto d'area non decidibile su coordinate fuori da
  `[2^-450, 2^450]`; `PrecisionInsufficient` (sotto, «Precisione»);
- `Internal`: panico di `geo` dentro il kernel, invariante violata.

### Limiti e deviazioni

- Kernel del laboratorio qualificato contro GEOS per equivalenza semantica
  (stesse facce, stessi residui, stessa area), non per identità
  ([README, «`geo.make_valid`, `geo.polygonize`, `geo.split`: equivalenza a
  GEOS verificata, non dimostrata»](../README.md#geomake_valid-geopolygonize-geosplit-equivalenza-a-geos-verificata-non-dimostrata)).
- Forma canonica solo quando gli incroci sono esattamente rappresentabili:
  altrimenti, su ingresso permutato o invertito, classi e aree coincidono ma
  i bit di un incrocio, il segno di uno zero e l'ordine delle linee
  duplicate seguono l'ordine d'ingresso.
- Il lavoro di noding conta le coppie di segmenti esaminate, addebitate
  mentre accadono (GEOS stimava prima il quadrato dei segmenti); i limiti
  d'uscita valgono anche sulle facce intermedie.
- Segni esatti dove GEOS non lo è: una faccia degenere solo per la
  precisione di GEOS resta un poligono.
- Elenco completo: [README, «Differenze da GEOS»](../README.md#differenze-da-geos).
- Nessuna diagnostica per riga: il passo rende il primo errore ([README,
  «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
  voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).

### Precisione

Nessuna griglia di `i_overlay`: il solo calcolo che sposta punti è il
noding
([README, «Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)):

- coordinate troppo rade (unità in ultima posizione del modulo massimo
  oltre `p / 64`): `PrecisionInsufficient`, prima del noding;
- ogni punto d'incrocio, calcolato in doppia-doppia e arrotondato in `f64`,
  deve stare entro `p / 5` da entrambi i segmenti che divide; il noding si
  ripete al più cinque volte, quindi il grafo dista dalle linee d'ingresso
  al più `p`. Oltre, `PrecisionInsufficient`;
- orientamento delle facce e annidamento per area sono esatti; restano
  decisioni in `f64` entro la precisione il punto medio dei lati che
  ricuce i buchi, il punto interno di `geo` per l'annidamento e lo
  spareggio fra archi sovrapposti (solo senza noding).

Con `node_input: false` nessun punto è calcolato: le facce hanno le
coordinate d'ingresso. `p` è la precisione del CRS della colonna
(`Precision::from_crs`, 1 cm a terra).

### Complessità

`n` segmenti in tutto: noding con filtro sugli inviluppi (coppie trovate
su una griglia di celle, nella sequenza e col budget della scansione su
`x`), O(n²) coppie nel caso peggiore, entro il tetto di 100.000.000
coppie; estrazione delle facce O(e log e) sui lati nodati `e`, col punto
interno di una faccia calcolato solo se un'altra faccia ne contiene il
rettangolo. Memoria O(n) per tutte le linee della
tabella, raccolte prima del calcolo (classe bloccante), più le righe
d'uscita.

### Esempio

Un quadrato chiuso da tre linee, con un tratto pendente.

```json
{
  "config": {},
  "ingressi": [
    {"nome": "linee", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2, 3, 4]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["LINESTRING(0 0,10 0)", "LINESTRING(10 0,10 10)", "LINESTRING(10 10,0 10,0 0)", "LINESTRING(10 10,15 10)"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POLYGON((0 10,0 0,10 0,10 10,0 10))", "LINESTRING(10 10,15 10)"]},
    {"nome": "__class", "tipo": "utf8", "valori": ["polygon", "dangle"]}
  ]}
}
```
