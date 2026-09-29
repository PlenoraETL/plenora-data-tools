### Che cosa fa

Unisce tutte le geometrie della tabella in una sola (kernel
`topology::dissolve`, `unary_union` di `geo`): le parti che si toccano o
si sovrappongono si fondono, quelle disgiunte restano poligoni distinti
dello stesso `MultiPolygon`. Lavora solo su `Polygon` e `MultiPolygon`. Le
colonne attributo non passano e non ci sono gruppi. Il runner non esegue
ancora le operazioni geo ([README, «Che cosa non c'è
ancora»](../README.md#che-cosa-non-cè-ancora)): lo schema qui descritto è
quello dell'analisi del contratto, i valori quelli del kernel.

### Parametri

Nessuno: la config è `{}`.

### Schema

Una sola colonna: la colonna geometria dell'ingresso, con lo stesso nome e
lo stesso CRS, `Binary` GeoArrow-WKB, nullable, in XY. I tipi geometrici
dichiarati diventano `MultiPolygon` (`exact`) e le chiavi dei tipi
ereditate dal campo si tolgono; gli altri metadati di campo e i metadati
di schema restano. Le proprietà del contratto (`sorted_by`, `row_count`)
si perdono.

### Righe

Aggregazione: una riga. Il kernel riceve solo geometrie: come si trattano
le celle nulle a livello di tabella non è ancora fissato. Senza geometrie
il kernel rende un `MULTIPOLYGON EMPTY`; l'analisi dichiara la colonna
nullable pensando a una geometria nulla per un ingresso vuoto, e quale dei
due valga si fisserà con il runner.

### Ordine

Una riga sola. Le parti del `MultiPolygon` e i loro vertici sono
nell'ordine che `i_overlay` produce, lo stesso a ogni esecuzione.

### Errori

In validazione (analisi del contratto):

- `InvalidPlan`: config non vuota;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come geometria WKB (né estensione
  `geoarrow.wkb` né chiavi canoniche `plenora.geometry.*`);
- `Unsupported`: una colonna geometria non XY;
- `Crs`: CRS non risolto, o non proiettato (o senza unità lineare).

In esecuzione (kernel `topology::dissolve`, errore `TopologyError`; nessun
codice di questo repository lo traduce ancora in `PlenoraError`):

- `UnsupportedGeometry`: una geometria non è `Polygon`/`MultiPolygon`;
- `InvalidGeometry`: una geometria d'ingresso o il risultato non supera la
  validazione OGC;
- `ValidazioneNonConclusa`: la validazione OGC non ha concluso;
- `PrecisionInsufficient`: la griglia dell'overlay sposterebbe il risultato
  oltre la precisione (sotto, «Precisione»);
- `CalcoloNonConcluso`: l'overlay di `geo` è andato in panico.

### Limiti e deviazioni

Nessun raggruppamento per attributo: tutta la tabella diventa una
geometria. Gli ingressi si orientano (anello esterno antiorario) prima
dell'unione, perché `unary_union` di `geo` sceglie la regola di
riempimento dal verso del primo anello e un poligono valido di verso
opposto sparirebbe ([README, «Precisione delle operazioni geografiche: 1 cm
a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).
Nessun controllo a posteriori del risultato contro gli ingressi.

### Precisione

Entro 1 cm a terra: un solo overlay di `i_overlay` su interi `i64`, con la
griglia controllata prima del calcolo sull'ingombro di tutti gli ingressi.
Se lo spostamento a priori supera mezzo centimetro, o le coordinate sono
troppo rade per il centimetro, `PrecisionInsufficient` e nessun calcolo.
Due poligoni separati da meno di 1 cm possono fondersi, e parti più
sottili di 1 cm sparire, senza errore; vedi [README, «Precisione delle
operazioni geografiche: 1 cm a
terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra).

### Complessità

Un overlay a scansione su tutti i vertici `v` della tabella, di norma
O((v + k) log v) con `k` gli incroci fra i lati, più la validazione OGC di
ingressi e risultato. Memoria O(v + k): l'intera tabella in memoria.

### Esempio

```json
{
  "config": {},
  "ingressi": [
    {"nome": "lotti", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POLYGON((0 0,2 0,2 2,0 2,0 0))", "POLYGON((1 0,3 0,3 2,1 2,1 0))", "POLYGON((10 10,11 10,11 11,10 11,10 10))"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["MULTIPOLYGON(((0 2,0 0,3 0,3 2,0 2)),((10 11,10 10,11 10,11 11,10 11)))"]}
  ]}
}
```
