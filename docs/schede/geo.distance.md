### Che cosa fa

Aggiunge una colonna `float64` con la distanza euclidea planare fra la
geometria di ogni riga e una geometria fissa scritta nella config
(`other_wkb`), nelle unità del CRS: la distanza minima fra i due insiemi di
punti, 0 quando si intersecano (anche quando uno contiene l'altro). Con una
geometria vuota, da una parte o dall'altra, il kernel non dà una distanza.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `other_wkb` | stringa | obbligatorio | WKB in esadecimale (cifre maiuscole o minuscole) di una geometria valida OGC, solo XY, senza SRID, coordinate nel dominio del CRS dell'ingresso | secondo operando, nel CRS della colonna geometria |
| `output_column` | stringa | `distance` | nome non vuoto (non di soli spazi) e non già presente nell'ingresso | colonna aggiunta |

L'ingresso ha una sola colonna geometria: il secondo operando arriva dalla
config ed è assunto nello stesso CRS, che nessun dato può confermare.
L'analisi ne verifica la struttura WKB, la validità OGC e il dominio delle
coordinate.

### Schema

Aggiunge in coda `output_column`, `float64` nullable, senza metadati di
campo. Le altre colonne (geometria compresa), i metadati di schema e le
proprietà del contratto (`sorted_by`, `row_count`) passano invariati.

### Righe

1:1: una distanza per riga, dal kernel `operations::distance` (riga,
`other_wkb`). Una geometria nulla dà una distanza nulla; una geometria
vuota, da una parte o dall'altra, anche: il kernel non ha una distanza da
rendere.

### Ordine

L'ordine d'ingresso: l'analisi conserva `sorted_by`.

### Errori

In validazione (analisi del contratto):

- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non si riconosce come WKB (né estensione `geoarrow.wkb` né chiavi
  `plenora.geometry.*`); `output_column` esiste già;
- `Unsupported`: dimensioni della geometria diverse da `xy`; `other_wkb`
  porta dimensioni Z/M o uno SRID EWKB;
- `Crs`: CRS della colonna assente o non risolto, geografico, o proiettato
  senza unità lineare; una coordinata di `other_wkb` fuori dal dominio del
  CRS;
- `InvalidPlan`: config con campi sconosciuti o senza `other_wkb`;
  `other_wkb` vuoto, di lunghezza dispari o con caratteri non esadecimali,
  o con una struttura WKB non valida (conteggi, anelli non chiusi, byte in
  coda, coordinate non finite, oltre 64 MiB o 64 livelli d'annidamento),
  o che non supera la validazione OGC; `output_column` vuoto o di soli
  spazi;
- `Internal`: la decodifica o la validazione OGC di `other_wkb` non
  conclude.

In esecuzione ([Runner, «Operazioni geo»](runner.md#operazioni-geo))
il passo rende il primo errore in ordine di riga, senza diagnostica per
riga. Prima del kernel, per ogni cella non nulla della colonna geometria:

- `InvalidPlan`: struttura WKB non valida; `Unsupported`: la cella porta
  Z/M o uno SRID; `ResourceLimit`: la cella supera 64 MiB;
- `Crs`: una coordinata fuori dal dominio di validità del CRS della
  colonna;
- `Schema`: il contratto d'ingresso dichiara i tipi geometrici con un
  elenco e la cella è di un altro tipo.

Dal kernel (`OperationError`), per geometria:

- `InvalidPlan` (`InvalidInput`): la geometria della riga non supera la
  validazione OGC (`other_wkb` l'ha già superata in analisi);
- `Internal` (`ValidazioneNonConclusa`, `CalcoloNonConcluso`): la
  validazione OGC o il calcolo di `geo` vanno in panico dentro la
  barriera (il messaggio porta solo la forma del payload).

### Limiti e deviazioni

Il secondo operando è uno solo per tutto il passo: la distanza fra due
colonne o fra due tabelle non c'è. La distanza è planare, non geodetica. Errori senza indice di riga
della sorgente ([Runner, «Limiti dichiarati del runner»](runner.md#limiti-dichiarati-del-runner),
voce «Geo senza diagnostica per riga»).

### Precisione

Nessuna griglia e nessun rifiuto `PrecisionInsufficient`: la distanza è il
calcolo in `f64` di `geo` (punto-segmento sulle coppie più vicine), senza un
bilancio d'errore dichiarato rispetto alla regola di 1 cm; per linee e
poligoni che si intersecano lo zero viene dal predicato d'intersezione di
`geo`, non da una differenza di coordinate
([Limiti dichiarati, «Precisione delle operazioni geografiche»](limiti.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

### Complessità

Per una riga di n vertici e `other_wkb` di m vertici: il test
d'intersezione di `geo` confronta coppie di segmenti, O(n·m) nel caso
peggiore; la ricerca della distanza minima che segue usa R-tree o
proiezioni ordinate dei segmenti. Più la
validazione OGC dei due operandi (sub-quadratica nel caso tipico, O(n²)
nel peggiore:
[Limiti dichiarati, «Validazione OGC»](limiti.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì)),
ripetuta per `other_wkb` a ogni riga.

### Esempio

La geometria fissa è `POINT(0 0)`.

```json
{
  "config": {"other_wkb": "010100000000000000000000000000000000000000"},
  "ingressi": [
    {"nome": "siti", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POINT(3 4)", "LINESTRING(1 -1,1 1)", "POLYGON((-1 -1,1 -1,1 1,-1 1,-1 -1))"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POINT(3 4)", "LINESTRING(1 -1,1 1)", "POLYGON((-1 -1,1 -1,1 1,-1 1,-1 -1))"]},
    {"nome": "distance", "tipo": "float64", "valori": [5.0, 1.0, 0.0]}
  ]}
}
```
