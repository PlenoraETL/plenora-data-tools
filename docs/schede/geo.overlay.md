### Che cosa fa

Sovrappone due tabelle di poligoni e ne produce i pezzi, ognuno con la
riga sinistra e la riga destra da cui viene (kernel
`topology::polygon_overlay_validated`; [README, «Operazioni
geo»](../README.md#operazioni-geo)): le intersezioni delle coppie che si
sovrappongono e, secondo `mode`, i resti di ciascun lato fuori dall'altro.
Lavora solo su `Polygon` e `MultiPolygon`.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `mode` | stringa | obbligatorio | `intersection`, `union`, `identity`, `symmetric_difference` | quali pezzi emettere |

Pezzi per `mode`:

- `intersection`: le intersezioni delle coppie;
- `union`: le intersezioni, i resti della sinistra e i resti della destra;
- `identity`: le intersezioni e i resti della sinistra (la sinistra resta
  coperta per intero, la destra solo dove la tocca);
- `symmetric_difference`: i resti della sinistra e quelli della destra.

Il resto di una riga è la riga meno l'unione di tutte le righe dell'altro
lato; se l'altro lato non ha righe, è la riga invariata.

### Schema

Tre colonne, in quest'ordine: la colonna geometria della sinistra (stesso
nome e CRS, `Binary` GeoArrow-WKB, nullable, XY, tipi dichiarati `Polygon`
e `MultiPolygon` `exact`, senza le chiavi dei tipi ereditate), poi
`__left_index` e `__right_index`, `uint64` nullable: le posizioni delle
righe d'origine, nulle dove il pezzo è un resto dell'altro lato. Le
colonne attributo dei due lati non passano: si ricollegano con gli indici.
I metadati di schema sono la fusione dei due lati; le proprietà del
contratto si perdono.

### Righe

Una riga per pezzo, da 0 a molte per riga d'ingresso. Le coppie candidate
si trovano con il join spaziale `intersects` sui rettangoli d'ingombro e
si confermano con il predicato esatto; una coppia che si tocca solo sul
bordo dà un'intersezione vuota, che non si emette. Nessun pezzo vuoto esce.
Una riga sinistra coperta del tutto dalla destra non ha resto. Le righe
con la geometria nulla, da un lato o dall'altro, non entrano nel kernel e
non danno pezzi; `__left_index` e `__right_index` sono le posizioni delle
righe negli ingressi, contando anche le nulle. Le coppie candidate e i
pezzi sono ciascuno al più il limite di righe dell'arco d'uscita
(`max_output_rows` se il passo è un output del piano, `max_rows_per_edge`
altrimenti).

### Ordine

Prima le intersezioni, in ordine `(sinistra, destra)` crescente; poi i
resti della sinistra in ordine di riga; poi i resti della destra in ordine
di riga.

### Errori

In validazione (analisi del contratto):

- `InvalidPlan`: un numero di ingressi diverso da due; config con campi
  sconosciuti, `mode` assente o fuori elenco; un metadato di schema
  presente sui due lati con valori diversi;
- `Schema`: un lato senza esattamente una colonna geometria, o con una
  colonna non riconoscibile come geometria WKB (né estensione
  `geoarrow.wkb` né chiavi canoniche `plenora.geometry.*`);
- `Unsupported`: una colonna geometria non XY;
- `Crs`: un lato senza CRS risolto, un CRS non proiettato (o senza unità
  lineare), CRS dei due lati non equivalenti.

In esecuzione, prima del kernel, su ogni cella non nulla dei due lati
([README, «Operazioni geo»](../README.md#operazioni-geo)):

- `Schema`: il contratto di un lato dichiara i tipi geometrici con un
  elenco e la cella è di un altro tipo;
- `Crs`: una coordinata fuori dal dominio di validità del CRS della
  colonna;
- `InvalidPlan`: la cella viola il contratto WKB o non supera la
  validazione OGC (`Unsupported` per dimensioni Z o M, `Internal` se la
  validazione non conclude, `ResourceLimit` per una cella oltre il limite
  di byte).

Dal kernel (`topology::polygon_overlay_validated`, errore
`TopologyError`, sulle geometrie già validate: restano le validazioni OGC
delle unioni e dei pezzi), nella categoria del passo geo indicata fra
parentesi:

- `UnsupportedGeometry` (`InvalidPlan`): una geometria non è
  `Polygon`/`MultiPolygon`;
- `InvalidGeometry` (`InvalidPlan`): un'unione o un pezzo non supera la
  validazione OGC; oppure il join delle coppie candidate fallisce, anche
  solo perché le coppie superano il limite di righe dell'arco;
- `ResourceLimit` (`overlay_results`; `InvalidPlan`, non la categoria
  `ResourceLimit`): i pezzi superano il limite di righe dell'arco;
- `IndexOverflow` (`InvalidPlan`): un indice non entra in `u64`;
- `PrecisionInsufficient` (`Unsupported`): la griglia di un overlay
  sposterebbe il risultato oltre la precisione (sotto, «Precisione»);
- `ValidazioneNonConclusa`, `CalcoloNonConcluso` (`Internal`): una
  validazione, un predicato o un overlay di `geo` non ha concluso.

Un indice di pezzo che non corrisponde a una riga d'ingresso è
`Internal`.

Il primo errore è quello della prima riga, in ordine di riga, senza
diagnostica per riga ([README, «Limiti dichiarati del
runner»](../README.md#limiti-dichiarati-del-runner), voce «Geo senza
diagnostica per riga»).

### Limiti e deviazioni

Solo parti poligonali: le intersezioni che si riducono a linee o punti
sono escluse, come `keep_geom_type=True` di GeoPandas. Il superamento del
limite delle coppie candidate esce come `InvalidGeometry`, non come
`ResourceLimit`, e nessuno dei due limiti ha la categoria `ResourceLimit`
nel runner: entrambi escono come `InvalidPlan`. I pezzi dipendono dai
dati: il modello di costo non li prevede, e li limita solo il limite di
righe dell'arco ([README, «Limiti dichiarati del
runner»](../README.md#limiti-dichiarati-del-runner), voce «Modelli di
costo geo»). Nessun controllo a posteriori del risultato contro
gli ingressi ([README, «Precisione delle operazioni geografiche: 1 cm a
terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

### Precisione

La precisione è 1 cm a terra nelle unità del CRS della sinistra
(`Precision::from_crs`, calcolata in validazione). Entro 1 cm a terra.
Ogni intersezione è un overlay con la griglia entro mezzo centimetro; ogni
resto sono due overlay in catena (l'unione dell'altro lato, poi la
differenza), ognuno entro un quarto di centimetro. Ogni griglia è
controllata prima del calcolo sull'ingombro dei suoi operandi; oltre, o
con coordinate troppo rade per il centimetro, `PrecisionInsufficient` e
nessun calcolo. Parti più sottili di 1 cm possono sparire o fondersi senza
errore; vedi [README, «Precisione delle operazioni geografiche: 1 cm a
terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra).

### Complessità

Join delle coppie candidate con un R-tree, O((n + m) log m) più le coppie
trovate (nel caso peggiore O(n · m)); un overlay per coppia; per i resti
un'unione di ciascun lato e una differenza per riga contro l'unione intera
dell'altro lato. Più la validazione OGC di ingressi, unioni e pezzi.
Memoria: i due lati, le unioni e i pezzi.

### Esempio

```json
{
  "config": {"mode": "union"},
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
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["MULTIPOLYGON(((1 2,1 0,2 0,2 2,1 2)))", "MULTIPOLYGON(((0 2,0 0,1 0,1 2,0 2)))", "MULTIPOLYGON(((2 2,2 0,3 0,3 2,2 2)))"]},
    {"nome": "__left_index", "tipo": "uint64", "valori": [0, 0, null]},
    {"nome": "__right_index", "tipo": "uint64", "valori": [0, null, 0]}
  ]}
}
```
