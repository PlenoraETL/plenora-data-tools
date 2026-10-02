### Che cosa fa

Controlla che i poligoni della tabella formino una copertura senza
sovrapposizioni (le particelle di un catasto, le stanze di una pianta):
per ogni coppia di righe i cui poligoni si sovrappongono con un'area
maggiore di `tolerance` scrive una riga `overlap`, con le posizioni delle
due righe, l'area e la geometria della zona sovrapposta. I buchi fra i
poligoni (gap) non si cercano: se un buco sia atteso dipende dal dominio.

La conversione di colonna è `extensions3::coverage_validate_rows`, che il
runner chiama su tutta la colonna con i default della tabella sotto e la
precisione del CRS della colonna ([Runner, «Operazioni geo»](runner.md#operazioni-geo)).

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `tolerance` | numero | `0` | finito, `>= 0` | area minima, nelle unità del CRS al quadrato, perché una sovrapposizione sia segnalata (strettamente maggiore) |
| `max_issues` | intero | `1000` | `>= 1` | sovrapposizioni massime: oltre, l'operazione fallisce invece di troncare |

### Schema

Schema nuovo, in quest'ordine: `issue_type` (`utf8`), `index_a` e
`index_b` (`uint64`), `area` (`float64`), `geometry` (`binary`, CRS della
colonna d'ingresso, dimensioni `xy`, senza dichiarazione dei tipi), tutte
non nullable. Le colonne dell'ingresso spariscono, i metadati di schema
restano; nessuna proprietà del contratto sopravvive.

### Righe

Una riga per coppia di righe d'ingresso `(a, b)`, con `a < b`, i cui
poligoni si sovrappongono per un'area maggiore di `tolerance`:

- `issue_type`: sempre `overlap`;
- `index_a`, `index_b`: posizioni delle due righe nell'ingresso, da 0; le
  righe nulle e le geometrie vuote non partecipano ma contano nella
  posizione;
- `area`: area della sovrapposizione;
- `geometry`: la zona sovrapposta, `Polygon` se è una sola, altrimenti
  `MultiPolygon`.

Poligoni che si toccano lungo un lato o in un punto non danno righe (salvo
le schegge descritte in «Precisione»).
Un ingresso senza sovrapposizioni dà 0 righe.

### Ordine

Per `index_a`, poi per `index_b`, crescenti.

### Errori

In validazione (analisi del contratto):

- `InvalidPlan`: config con campi sconosciuti; `tolerance` negativa o non
  finita; `max_issues` pari a zero o non intero;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come WKB;
- `Unsupported`: dimensioni della geometria diverse da `xy`;
- `Crs`: colonna senza CRS risolto o CRS non proiettato.

In esecuzione, prima del kernel, su ogni cella non nulla ([Runner,
«Operazioni geo»](runner.md#operazioni-geo)): `InvalidPlan` per un WKB
malformato o con coordinate non finite, `Unsupported` per dimensioni Z/M o
SRID, `Crs` per una coordinata fuori dal dominio di validità del CRS della
colonna, `Schema` per una geometria di un tipo che il contratto
dell'ingresso dichiara con un elenco e che non vi compare.

Poi la conversione di colonna (messaggi del calcolo con prefisso
`geo.coverage_validate:`):

- `InvalidPlan`: WKB malformato o OGC-invalido; una geometria che non è
  `Polygon` o `MultiPolygon` (il messaggio riporta la posizione della riga,
  non i dati); zona sovrapposta non valida;
- `Unsupported`: `PrecisionInsufficient` (sotto, «Precisione»); WKB con
  dimensioni Z/M o SRID;
- `ResourceLimit`: cella oltre il limite di byte per cella; più di
  `max_issues` sovrapposizioni (`IssueLimit`); coppie candidate e
  sovrapposizioni (stimate dalla zona, con la sua codifica) oltre il
  margine di memoria del passo (`MargineMemoria`; guardia che riduce il rischio, non un tetto garantito: [Runner, «Limiti dichiarati del runner»](runner.md#limiti-dichiarati-del-runner), voce «Modelli di costo geo»);
- `Internal`: panico di `geo`, `i_overlay` o `rstar`, validazione che non
  conclude.

### Limiti e deviazioni

- Solo sovrapposizioni: i gap non sono rilevati.
- Oltre `max_issues` l'operazione fallisce, non tronca l'elenco.
- La decisione sull'area è presa sul risultato passato dalla griglia di
  `i_overlay` ([Limiti dichiarati, «Precisione delle operazioni geografiche: 1 cm a
  terra»](limiti.md#precisione-delle-operazioni-geografiche-1-cm-a-terra),
  «Hazard»).
- Nessuna diagnostica per riga: il passo rende il primo errore ([Runner,
  «Limiti dichiarati del runner»](runner.md#limiti-dichiarati-del-runner),
  voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).

### Precisione

Ogni intersezione di una coppia passa dalla griglia di `i_overlay`, con il
controllo a priori sull'ingombro della coppia: guardia di spaziatura delle
coordinate e spostamento della griglia entro `p / 2`, altrimenti
`PrecisionInsufficient`; `p` è 1 cm a terra nelle unità del CRS della
colonna (`Precision::from_crs`). Nessun controllo a posteriori: una sovrapposizione
più sottile della griglia può sparire (segnalazione mancata), e vertici
diversi su lati collineari possono lasciare una scheggia (segnalazione
spuria), con area entro la precisione per il perimetro della zona
([Limiti dichiarati, «Precisione delle operazioni geografiche: 1 cm a terra»](limiti.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).
`area` è calcolata in `f64` sulla zona restituita dalla griglia.

### Complessità

R-tree sui rettangoli d'ingombro, O(n log n); poi un'intersezione per ogni
coppia di rettangoli che si toccano, O(v log v) sui vertici `v` della
coppia (O(n²) coppie nel caso peggiore, tutte sovrapposte). Memoria O(n)
per tutte le geometrie, decodificate prima del calcolo (classe bloccante).

### Esempio

Due rettangoli sovrapposti per un'area di 8 e un terzo che tocca il secondo
lungo un lato.

```json
{
  "config": {},
  "ingressi": [
    {"nome": "particelle", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POLYGON((0 0,4 0,4 4,0 4,0 0))", "POLYGON((2 0,6 0,6 4,2 4,2 0))", "POLYGON((6 0,8 0,8 4,6 4,6 0))"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "issue_type", "tipo": "utf8", "valori": ["overlap"]},
    {"nome": "index_a", "tipo": "uint64", "valori": [0]},
    {"nome": "index_b", "tipo": "uint64", "valori": [1]},
    {"nome": "area", "tipo": "float64", "valori": [8.0]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POLYGON((2 4,2 0,4 0,4 4,2 4))"]}
  ]}
}
```
