### Che cosa fa

Riproietta la colonna geometria dal suo CRS a `target_crs`, fra i CRS
della tabella integrata, in Rust puro: proiezione inversa, cambio di
datum lungo un percorso di trasformazioni EPSG, proiezione diretta. Ogni
lato si densifica perché resti entro mezza precisione dall'immagine
esatta del lato sorgente; tipo e struttura di ogni geometria non
cambiano. Il quadro completo è in
[«Riproiezione»](riproiezione.md#riproiezione).

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `target_crs` | stringa | obbligatorio | identificatore della tabella integrata (`EPSG:<codice>`, `OGC:CRS84`, forme URN) | CRS d'arrivo |
| `accuratezza_accettata_m` | numero | nessuna | finito, non negativo, con effetto | accuratezza in metri accettata per un cambio di datum oltre 1 cm |
| `trasformazioni` | lista di interi | nessuna | codici EPSG che formano un percorso fra i due datum | percorso imposto, nell'ordine, per tutte le geometrie |
| `griglie` | lista di oggetti | `[]` | `{"trasformazione": <codice EPSG NTv2>, "file": <percorso>}`, codici distinti, `file` non vuoto, al più 4096 byte, senza NUL | griglie NTv2 fornite dall'utente |
| `convenzione_wgs84_etrs89` | booleano | `true` | `true`, `false`, solo se un percorso passa da EPSG:1149 | WGS 84 e la famiglia ETRS89 equivalenti per convenzione |

Parametri scritti senza effetto si rifiutano: `accuratezza_accettata_m`
quando ogni percorso ammesso sta già entro 1 cm, una griglia che nessun
percorso ammesso usa, `convenzione_wgs84_etrs89` (con l'uno o l'altro
valore) su una coppia che non passa da ETRS89 to WGS 84 (1). I percorsi
imposti da `trasformazioni` restano soggetti alla regola dell'accuratezza
([Riproiezione, «La regola dell'accuratezza»](riproiezione.md#la-regola-dellaccuratezza)).
L'analisi non legge i file delle griglie: li legge l'esecuzione.

### Schema

Stesse colonne, tipi, nullabilità e posizione; stessi tipi geometrici
dichiarati, dimensioni (XY) e `FieldId`; metadati di schema e proprietà
del contratto (`sorted_by`, `row_count`) conservati. Cambia il campo
geometria: il CRS del contratto diventa il target, il metadato `geo`
dichiara il target (dimensioni ed encoding invariati), le chiavi
canoniche CRS della sorgente (`plenora.geometry.crs_*`,
`plenora.geometry.srid`, `plenora.geometry.axis_order`) si tolgono, e `axis_order` si riscrive con l'ordine GIS
normalizzato del target. Gli altri metadati del campo restano.

### Righe

1:1. Il runner chiama l'adapter di tabella dei kernel
(`riproiezione::reproject_batches`) con i parametri letti in validazione:
ogni cella non nulla si decodifica, si riproietta e si ricodifica; una
cella nulla resta nulla; le altre colonne non cambiano
([Runner, «Operazioni geo»](runner.md#operazioni-geo)). Ogni geometria usa un solo percorso fra i datum, il primo
dell'ordine di preferenza la cui area d'uso contiene tutti i suoi punti.

### Ordine

Quello d'ingresso, righe e colonne. Il primo errore in ordine di riga è
quello che si riporta.

### Errori

In validazione (analisi del contratto):

- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non si dichiara geometria WKB;
- `Unsupported`: dimensioni della colonna diverse da XY (anche non
  dichiarate);
- `Crs`: CRS della sorgente mancante o non risolto; `axis_order`
  dichiarato diverso dall'ordine GIS normalizzato della sorgente (anche
  `unknown`); `target_crs` non risolvibile; sorgente o target fuori dalla
  tabella (`CRS_NOT_BUILTIN`); `REPROJECTION_CONFIG_INVALID` (parametri
  senza effetto o fuori dominio, griglia ripetuta o non NTv2,
  `trasformazioni` che non formano un percorso);
  `REPROJECTION_PATH_UNAVAILABLE` (nessun percorso fra i datum);
  `REPROJECTION_ACCURACY_NOT_ACCEPTED` (il percorso migliore supera 1 cm
  e l'accuratezza accettata);
- `InvalidPlan`: config non leggibile (campi sconosciuti, `target_crs`
  assente, tipi sbagliati), `file` di una griglia vuoto, oltre 4096 byte
  o con NUL. Fino alla versione 1 dell'analisi del contratto era
  `InvalidConfiguration`, unica fra le operazioni geo.

In esecuzione, prima dell'adapter, su tutta la colonna: `InvalidPlan` per
una cella che non è WKB strutturalmente valido, `Crs` per una coordinata
fuori dal dominio di validità del CRS sorgente, `Schema` per una geometria
di un tipo che il contratto d'ingresso non dichiara, quando li dichiara con
un elenco. Poi `reproject_batches`:

- `Schema`: colonna geometria assente, non `Binary` o non dichiarata WKB;
- `Crs`: `axis_order` non normalizzato; CRS sorgente diverso da quello
  con cui il piano è stato deciso; precisione del target non definita;
  `NTV2_GRID_UNREADABLE`, `NTV2_GRID_INVALID` (file di griglia); per
  geometria, `COORDINATE_OUT_OF_CRS_DOMAIN` (dominio o regione lon/lat
  della sorgente o del target), `REPROJECTION_NOT_CONVERGED` (un'inversa
  iterativa, di proiezione o di griglia, che non converge), `REPROJECTION_OUTSIDE_TRANSFORMATION_AREA`
  (nessun percorso ammesso copre tutta la geometria),
  `REPROJECTION_MIXED_TRANSFORMATION_AREAS` (punti o lati che
  preferiscono un percorso precedente), `REPROJECTION_EDGE_NOT_CONVERGED`
  (un lato non converge in 48 divisioni, per esempio attraverso
  l'antimeridiano del target);
- `ResourceLimit`: cella d'ingresso o d'uscita oltre 64 MiB, o
  densificazione oltre `MAX_CELL_COORDINATES` (4 194 304) coordinate per
  geometria;
- `InvalidPlan`: chiavi canoniche del campo non valide, WKB o geometria
  d'ingresso non validi, geometria riproiettata non valida OGC;
- `Unsupported`: dimensioni del campo diverse da XY, celle con Z/M;
- `Internal`: calcolo o validazione OGC interrotti (barriera dei panici).

Nessun messaggio riporta coordinate. Il primo errore è quello della prima
riga in ordine di riga, senza diagnostica per riga.

### Limiti e deviazioni

- Il cambio di datum vale quanto l'accuratezza EPSG del percorso: oltre 1
  cm solo con `accuratezza_accettata_m` dichiarata
  ([Limiti dichiarati, «`geo.reproject`: il cambio di datum vale quanto l'accuratezza accettata»](limiti.md#georeproject-il-cambio-di-datum-vale-quanto-laccuratezza-accettata));
  WGS 84 ed ETRS89 sono equivalenti per convenzione
  ([Riproiezione, «WGS 84 = ETRS89 per convenzione»](riproiezione.md#wgs-84--etrs89-per-convenzione)).
- Solo i CRS della tabella integrata
  ([«CRS integrati»](crs.md#crs-integrati)); aree d'uso come
  riquadri, accuratezze sommate, griglie non verificate contro il
  registro, densificazione a campioni e gli altri limiti in
  [Riproiezione, «Limiti dichiarati della riproiezione»](riproiezione.md#limiti-dichiarati-della-riproiezione).
- Nel runner un errore non ha diagnostica per riga e il costo in memoria è
  una previsione dalle misure
  ([Runner, «Limiti dichiarati del runner»](runner.md#limiti-dichiarati-del-runner),
  voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).

### Precisione

La matematica resta entro la precisione del target: proiezioni e
trasformazioni entro circa `1e-8` m da PROJ
([Riproiezione, «Oracolo»](riproiezione.md#oracolo)), lati densificati entro mezza
precisione del target, cioè 5 mm a terra (per un target geografico metà
di 1 cm in gradi all'equatore;
[Riproiezione, «Densificazione dei lati»](riproiezione.md#densificazione-dei-lati)).
Stesso CRS, o CRS che differiscono solo per l'ordine d'autorità degli
assi: coordinate invariate al bit. Il cambio di datum invece vale quanto
l'accuratezza del percorso, che oltre 1 cm si accetta solo dichiarandola
(sopra): è una garanzia indebolita per scelta esplicita
([Limiti dichiarati, «Precisione delle operazioni geografiche: 1 cm a terra»](limiti.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

### Complessità

Per geometria, tempo O(P² · k) nel caso peggiore, con k coordinate
prodotte (vertici più punti di densificazione, al più 4 194 304) e P
percorsi ammessi: i percorsi si provano in ordine finché uno copre la
geometria, e ogni punto si confronta con i percorsi precedenti. Memoria
O(k) per geometria, più le griglie NTv2 lette (al più 256 MiB per file);
`reproject_batches` tiene in memoria l'intera uscita.

### Esempio

Da WGS 84 a Pseudo Mercator (stesso datum): un parallelo resta una retta e
non riceve punti di densificazione; la cella nulla resta nulla.

```json
{
  "config": {"target_crs": "EPSG:3857"},
  "ingressi": [
    {"nome": "luoghi", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:4326", "valori": ["POINT(1 0)", "LINESTRING(9 45,10 45)", null]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POINT(111319.49079327357 0)", "LINESTRING(1001875.4171394621 5621521.486192066,1113194.9079327357 5621521.486192066)", null]}
  ]}
}
```
