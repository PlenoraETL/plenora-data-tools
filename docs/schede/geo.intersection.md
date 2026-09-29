### Che cosa fa

Sostituisce la geometria della sinistra con la sua intersezione con la
geometria della destra, `sinistra ∩ destra`, calcolata dal kernel
`topology::boolean_operation`. Lavora solo su `Polygon` e `MultiPolygon` e
rende sempre un `MultiPolygon`. Il runner non esegue ancora le operazioni
geo ([README, «Che cosa non c'è ancora»](../README.md#che-cosa-non-cè-ancora)):
lo schema qui descritto è quello dell'analisi del contratto, il calcolo
quello del kernel su una coppia di geometrie.

### Parametri

Nessuno: la config è `{}`.

### Schema

Quello della sinistra: stesse colonne, nello stesso ordine, con gli stessi
tipi e la stessa nullabilità; le colonne della destra non passano. La
colonna geometria resta al suo posto, con lo stesso nome e lo stesso CRS
della sinistra (uguale a quello della destra), in XY; i tipi geometrici
dichiarati diventano `MultiPolygon` (`exact`) e le chiavi dei tipi
ereditate dal campo si tolgono. Gli altri metadati di campo restano. I
metadati di schema sono la fusione dei due lati: una chiave presente da un
solo lato o uguale sui due passa. Le proprietà del contratto della sinistra
(`sorted_by`, `row_count`) restano.

### Righe

Il contratto dichiara una riga d'uscita per ogni riga della sinistra.
Come le righe della destra si abbinano a quelle della sinistra (e che
cosa succede a una cella nulla) non è ancora fissato da codice di questo
repository: il kernel calcola la booleana di una coppia di geometrie. Dove
le due geometrie non si intersecano il kernel rende un `MULTIPOLYGON
EMPTY`, non una geometria nulla.

### Ordine

Quello della sinistra.

### Errori

In validazione (analisi del contratto):

- `InvalidPlan`: un numero di ingressi diverso da due; config non vuota;
  un metadato di schema presente sui due lati con valori diversi;
- `Schema`: un lato senza esattamente una colonna geometria, o con una
  colonna non riconoscibile come geometria WKB (né estensione
  `geoarrow.wkb` né chiavi canoniche `plenora.geometry.*`);
- `Unsupported`: una colonna geometria non XY;
- `Crs`: un lato senza CRS risolto, un CRS non proiettato (o senza unità
  lineare), CRS dei due lati non equivalenti.

In esecuzione (kernel `topology::boolean_operation`, errore
`TopologyError`; nessun codice di questo repository lo traduce ancora in
`PlenoraError`):

- `UnsupportedGeometry`: una geometria non è `Polygon`/`MultiPolygon`;
- `InvalidGeometry`: una geometria d'ingresso o il risultato non supera la
  validazione OGC;
- `ValidazioneNonConclusa`: la validazione OGC non ha concluso;
- `PrecisionInsufficient`: la griglia dell'overlay sposterebbe il risultato
  oltre la precisione (sotto, «Precisione»);
- `CalcoloNonConcluso`: l'overlay di `geo` è andato in panico.

### Limiti e deviazioni

Solo poligoni: un'intersezione che si riduce a linee o punti (due
quadrati che si toccano su un lato) è un `MultiPolygon` vuoto, dove GEOS e
PostGIS renderebbero la linea o il punto. Il catalogo dichiara forma 1:N e
vincolo `max(uscita / sinistra, uscita / destra)`, ma il kernel rende una
geometria per coppia. Nessun controllo a posteriori del risultato contro gli
ingressi ([README, «Precisione delle operazioni geografiche: 1 cm a
terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

### Precisione

Entro 1 cm a terra: un solo overlay di `i_overlay` su interi `i64`, con la
griglia controllata prima del calcolo sull'ingombro dei due operandi. Se lo
spostamento a priori supera mezzo centimetro, o le coordinate sono troppo
rade per il centimetro, `PrecisionInsufficient` e nessun calcolo. Parti
più sottili di 1 cm possono sparire o fondersi senza errore; vedi
[README, «Precisione delle operazioni geografiche: 1 cm a
terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra).

### Complessità

Per coppia l'overlay a scansione di `i_overlay`, di norma O((v + k) log v)
con `v` i vertici dei due operandi e `k` gli incroci fra i lati, più la
validazione OGC di ingressi e risultato (di norma O(v log v), nel caso
peggiore O(v²): [README, «Validazione OGC: la
ricerca delle auto-intersezioni non è quella di `geo`, il verdetto
sì»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì)).
Memoria O(v + k).

### Esempio

Una riga per lato, perché l'abbinamento delle righe non è ancora fissato.

```json
{
  "config": {},
  "ingressi": [
    {"nome": "lotti", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POLYGON((0 0,2 0,2 2,0 2,0 0))"]}
    ]},
    {"nome": "vincoli", "colonne": [
      {"nome": "zona", "tipo": "utf8", "valori": ["A"]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POLYGON((1 0,3 0,3 2,1 2,1 0))"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["MULTIPOLYGON(((1 2,1 0,2 0,2 2,1 2)))"]}
  ]}
}
```
