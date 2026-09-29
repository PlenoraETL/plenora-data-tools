### Che cosa fa

Aggiunge una colonna `bool` che dice se la geometria della riga (A) e la geometria costante `other_wkb` (B) si toccano soltanto: hanno punti in comune, ma solo sui confini, mai fra gli interni.

Maschera DE-9IM: interno/interno vuota e almeno una fra interno/confine, confine/interno, confine/confine non vuota (`FT*******`, `F**T*****`, `F***T****`). Due poligoni con un lato in comune si toccano; un punto sul lato di un poligono lo tocca. Fra due punti è sempre falso (un punto non ha confine). Simmetrico. Con una geometria vuota è falso.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `other_wkb` | stringa | obbligatorio | WKB 2D in esadecimale (cifre maiuscole o minuscole, in numero pari), nel CRS dell'input e dentro il suo dominio di validità | la geometria B, uguale per tutte le righe |
| `output_column` | stringa | `predicate_touches` | nome non vuoto e non già nello schema | colonna aggiunta |

`other_wkb` si decodifica e si valida in analisi solo nella struttura
(conteggi, anelli chiusi, coordinate finite) e nel dominio del CRS; la
validità OGC si controlla quando si valuta il predicato.

### Schema

Aggiunge in coda `output_column`, `bool` nullable, senza metadati. Le altre
colonne restano nell'ordine e con i loro metadati; la colonna geometria
resta com'è (tipi dichiarati, CRS, dimensioni `xy`). Metadati di schema e
proprietà del contratto (`sorted_by`, `row_count`) si conservano.

### Righe

1:1: una cella per riga. Per il contratto `other_wkb` è il secondo
operando: la geometria della riga è A, `other_wkb` è B, e il kernel valuta
`predicates::evaluate(A, B, SpatialPredicate::Touches)`. Il runner non
esegue ancora le operazioni geo ([README, «Che cosa non c'è ancora»](../README.md#che-cosa-non-cè-ancora)):
la colonna aggiunta è dichiarata nullable, e che cosa renda una cella
geometria nulla lo fisserà l'esecutore geo.

### Ordine

Quello d'ingresso.

### Errori

In validazione (analisi del contratto; nell'ordine: forma dell'ingresso,
CRS, config):

- `InvalidPlan`: più o meno di un ingresso; config con campi sconosciuti,
  senza `other_wkb` o con un tipo sbagliato; `other_wkb` vuoto, di
  lunghezza dispari o con caratteri non esadecimali; WKB strutturalmente
  non valido (byte o conteggi oltre i limiti, annidamento, anelli non
  chiusi, coordinate NaN o infinite, byte in coda); `output_column` vuoto;
- `Unsupported`: `other_wkb` con coordinate Z/M o con SRID (EWKB); colonna
  geometria con dimensioni diverse da `xy`;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come geometria WKB; `output_column` già
  presente;
- `Crs`: CRS della colonna assente o non risolto; CRS non proiettato o
  senza unità lineare (requisito del catalogo); una coordinata di
  `other_wkb` fuori dal dominio di validità del CRS;
- `Internal`: la decodifica di `other_wkb` non conclude.

In esecuzione: il runner non esegue l'operazione, e agli errori del kernel
non è ancora assegnata una variante `PlenoraError`. Il kernel
(`predicates::evaluate`) rifiuta la coppia con `NonFiniteCoordinate` o
`InvalidGeometry` (geometria non valida per l'OGC; lato `left` la riga,
lato `right` `other_wkb`), `ValidazioneNonConclusa` se la validazione non
conclude, `CalcoloNonConcluso` se `relate` di `geo` va in panico su due
geometrie valide (per esempio una `GeometryCollection` con membri
poligonali che si sovrappongono).

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

Il punto interno non tocca soltanto: entra; B è `POLYGON((0 0,10 0,10 10,0 10,0 0))`.

```json
{
  "config": {"other_wkb": "010300000001000000050000000000000000000000000000000000000000000000000024400000000000000000000000000000244000000000000024400000000000000000000000000000244000000000000000000000000000000000"},
  "ingressi": [
    {"nome": "luoghi", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POINT(10 5)", "POLYGON((10 0,20 0,20 10,10 10,10 0))", "POINT(5 5)"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POINT(10 5)", "POLYGON((10 0,20 0,20 10,10 10,10 0))", "POINT(5 5)"]},
    {"nome": "predicate_touches", "tipo": "bool", "valori": [true, true, false]}
  ]}
}
```
