### Che cosa fa

Aggiunge una colonna `bool` che dice se la geometria della riga (A) sta dentro la geometria costante `other_wkb` (B): nessun punto di A è fuori da B e almeno un punto dell'interno di A è nell'interno di B.

Maschera DE-9IM `T*F**F***`: interno/interno non vuota, interno di A/esterno di B e confine di A/esterno di B vuote. È [`geo.predicate_contains`](#geopredicate_contains) a operandi scambiati: un A tutto sul confine di B (un punto sul lato) non è dentro; per questo c'è [`geo.predicate_covered_by`](#geopredicate_covered_by). Una geometria sta dentro se stessa. Con una geometria vuota è falso.

### Parametri

`input_geometry` (stringa facoltativa) seleziona per nome la colonna geometrica
da elaborare. Omettendo un selettore si usa
la primaria; senza primaria la selezione implicita fallisce con `Schema`.
Il selettore non cambia la primaria. CRS, assi e precisione del calcolo
riguardano solo gli operandi selezionati; le geometrie passanti conservano
i propri metadati e byte, seguendo la stessa lineage delle altre colonne.

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `other_wkb` | stringa | obbligatorio | WKB 2D in esadecimale (cifre maiuscole o minuscole, in numero pari), nel CRS dell'input e dentro il suo dominio di validità | la geometria B, uguale per tutte le righe |
| `output_column` | stringa | `predicate_within` | nome non vuoto e non già nello schema | colonna aggiunta |

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
`predicates::with_crs::evaluate(context, A, B, SpatialPredicate::Within)` su ogni riga. `context` contiene lo SR della colonna per entrambi gli operandi. Una
geometria nulla dà una cella nulla, senza chiamare il kernel; una
geometria vuota dà il verdetto del predicato sul vuoto (vedi sopra). Le
righe sono indipendenti: il runner le calcola in parallelo e rende il
primo errore in ordine di riga
([Runner, «Operazioni geo»](runner.md#operazioni-geo)).

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
- `Schema`: l'ingresso non ha una colonna geometria selezionabile senza ambiguita', o la
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

Poi il kernel (`predicates::with_crs::evaluate`), con l'errore tradotto nella
categoria di `PlenoraError` ([Runner, «Operazioni geo»](runner.md#operazioni-geo),
voce «Errori»):

- `InvalidPlan`: `NonFiniteCoordinate` o `InvalidGeometry` (geometria
  della riga non valida per l'OGC; `other_wkb` l'ha già esclusa
  l'analisi);
- `Internal`: `ValidazioneNonConclusa` se la validazione non conclude,
  `CalcoloNonConcluso` se la costruzione della matrice non conclude;
- `ResourceLimit`: sui percorsi della matrice generale, limite effettivo
  di coppie, passi o prenotazioni strutturali superato, oppure prenotazione
  fallita. I valori predefiniti sono dichiarati nella guida DE-9IM.

### Limiti e deviazioni

- Il secondo operando è una geometria costante della config, non una
  seconda colonna: il secondo operando resta letterale anche con ingressi multigeometria.
- Tutti i predicati usano il percorso razionale esatto di
  [API diretta DE-9IM](topologia.md#api-diretta-de-9im): unione delle aree,
  precedenza dimensionale dichiarata e Mod-2 degli estremi lineari.
  I lati condivisi interni scompaiono dal confine e le sovrapposizioni fra
  membri poligonali di una collezione sono ammesse. Una linea chiusa
  non ha confine.
- Le due geometrie devono essere valide per l'OGC: una geometria non
  valida è un errore, non un verdetto. La validazione di entrambe si
  ripete a ogni valutazione ([Limiti dichiarati, «Validazione OGC: la ricerca delle
  auto-intersezioni non è quella di `geo`, il verdetto sì»](limiti.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì)).
- Si calcola sempre la matrice intera: nessuna scorciatoia per il singolo
  predicato.
- Nessuna diagnostica per riga: un errore è il primo in ordine di riga,
  e il suo indice è una riga dell'ingresso del passo, non della sorgente
  ([Runner, «Limiti dichiarati del runner»](runner.md#limiti-dichiarati-del-runner),
  voce «Geo senza diagnostica per riga»). Il costo in memoria è una
  previsione del modello geo misurato (voce «Modelli di costo geo»).

### Precisione

La matrice usa razionali esatti derivati dai bit delle coordinate `f64`:
anche le intersezioni non rappresentabili in `f64` restano esatte.
Nessuna griglia, tolleranza o spostamento entro 1 cm e' necessario.

### Complessità

La decomposizione verticale esatta ha costo polinomiale, con contatori
espliciti di coppie, passi e prenotazioni strutturali. La validazione OGC
preesistente precede il calcolo e resta O(n² + m²) nel caso peggiore.
Il modello di memoria misurato sul precedente percorso `geo::Relate`
non e' un limite superiore per il motore razionale. Il limite delle
prenotazioni strutturali esclude i buffer aritmetici interni di `BigInt`.

Ordine delle ordinate: dichiarazione esplicita compatibile richiesta dalla 4.0;
assenza o `unknown` causano `crs/validate` senza effetti. Vedi
[migrazione e censimento](limiti.md#ordine-delle-ordinate).

### Esempio

Il punto sul lato non è dentro; il mezzo quadrato, che ne condivide tre lati, sì; B è `POLYGON((0 0,10 0,10 10,0 10,0 0))`.

```json
{
  "config": {"other_wkb": "010300000001000000050000000000000000000000000000000000000000000000000024400000000000000000000000000000244000000000000024400000000000000000000000000000244000000000000000000000000000000000"},
  "ingressi": [
    {"nome": "luoghi", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POINT(5 5)", "POINT(10 5)", "POLYGON((0 0,10 0,10 5,0 5,0 0))"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POINT(5 5)", "POINT(10 5)", "POLYGON((0 0,10 0,10 5,0 5,0 0))"]},
    {"nome": "predicate_within", "tipo": "bool", "valori": [true, false, true]}
  ]}
}
```
