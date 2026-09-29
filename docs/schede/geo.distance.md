### Che cosa fa

Aggiunge una colonna `float64` con la distanza euclidea planare fra la
geometria di ogni riga e una geometria fissa scritta nella config
(`other_wkb`), nelle unità del CRS: la distanza minima fra i due insiemi di
punti, 0 quando si intersecano (anche quando uno contiene l'altro). Con una
geometria vuota, da una parte o dall'altra, il kernel non dà una distanza.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `other_wkb` | stringa | obbligatorio | WKB in esadecimale (cifre maiuscole o minuscole), solo XY, senza SRID, coordinate nel dominio del CRS dell'ingresso | secondo operando, nel CRS della colonna geometria |
| `output_column` | stringa | `distance` | nome non vuoto (non di soli spazi) e non già presente nell'ingresso | colonna aggiunta |

L'ingresso ha una sola colonna geometria: il secondo operando arriva dalla
config ed è assunto nello stesso CRS, che nessun dato può confermare.
L'analisi ne verifica la struttura WKB e il dominio delle coordinate, non
la validità OGC, che controlla il kernel.

### Schema

Aggiunge in coda `output_column`, `float64` nullable, senza metadati di
campo. Le altre colonne (geometria compresa), i metadati di schema e le
proprietà del contratto (`sorted_by`, `row_count`) passano invariati.

### Righe

1:1 per contratto. Il kernel (`operations::distance`) lavora su una coppia
di geometrie alla volta e nessun adapter lo chiama ancora sulle righe: il
trattamento di una cella nulla, e la resa del caso senza distanza (geometria
vuota), non sono definiti da codice eseguito.

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
  coda, coordinate non finite, oltre 64 MiB o 64 livelli d'annidamento);
  `output_column` vuoto o di soli spazi.

In esecuzione, dal kernel, per geometria (`OperationError`, che nessun
codice traduce ancora in `PlenoraError`):

- `InvalidInput`: la geometria della riga o `other_wkb` non supera la
  validazione OGC;
- `ValidazioneNonConclusa`, `CalcoloNonConcluso`: la validazione OGC o il
  calcolo di `geo` vanno in panico dentro la barriera (il messaggio porta
  solo la forma del payload).

### Limiti e deviazioni

Il runner non esegue ancora le operazioni geo
([README, «Che cosa non c'è ancora»](../README.md#che-cosa-non-cè-ancora)).
Il secondo operando è uno solo per tutto il passo: la distanza fra due
colonne o fra due tabelle non c'è. La distanza è planare, non geodetica.

### Precisione

Nessuna griglia e nessun rifiuto `PrecisionInsufficient`: la distanza è il
calcolo in `f64` di `geo` (punto-segmento sulle coppie più vicine), senza un
bilancio d'errore dichiarato rispetto alla regola di 1 cm; per linee e
poligoni che si intersecano lo zero viene dal predicato d'intersezione di
`geo`, non da una differenza di coordinate
([README, «Precisione delle operazioni geografiche»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

### Complessità

Per una riga di n vertici e `other_wkb` di m vertici: il test
d'intersezione di `geo` confronta coppie di segmenti, O(n·m) nel caso
peggiore; la ricerca della distanza minima che segue usa R-tree o
proiezioni ordinate dei segmenti. Più la
validazione OGC dei due operandi (sub-quadratica nel caso tipico, O(n²)
nel peggiore:
[README, «Validazione OGC»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì)),
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
