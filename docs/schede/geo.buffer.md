### Che cosa fa

Sostituisce ogni geometria con il suo buffer planare a distanza `distance`,
nelle unità del CRS: l'insieme dei punti che distano al più `distance`
dalla geometria, sempre un `MultiPolygon` (anche vuoto). Con `distance`
negativa il buffer erode: contano solo le parti areali, e punti e linee
spariscono. Con `distance` nulla l'uscita è l'unione delle parti areali
(punti e linee danno un `MultiPolygon` vuoto). Le giunzioni sono sempre
tonde; `cap` sceglie le estremità delle linee.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `distance` | numero | obbligatorio | finito, anche negativo o nullo | distanza del buffer, nelle unità del CRS |
| `cap` | stringa | `round` | `round`, `flat`, `square` | estremità delle linee: arco, taglio netto all'estremo, quadrato che sporge di `distance` |

Con `cap` `flat` i punti non hanno buffer: un ingresso di soli punti dà un
`MultiPolygon` vuoto.

### Schema

La colonna geometria resta al suo posto con lo stesso nome, CRS e
dimensioni (`xy`); le altre colonne, i metadati di schema e le proprietà
del contratto (`sorted_by`, `row_count`) passano invariati. I tipi
dichiarati della colonna diventano `exact` [`MultiPolygon`]: le chiavi
`plenora.geometry.types` e `plenora.geometry.types_declaration` ereditate
si tolgono dal campo.

### Righe

1:1: il runner chiama il kernel (`operations::buffer_with_cap`) su ogni cella non
nulla, in parallelo, e rimette la geometria al suo posto; una cella
nulla resta nulla ([Runner, «Operazioni geo»](runner.md#operazioni-geo)).

### Ordine

L'ordine d'ingresso: l'analisi conserva `sorted_by`.

### Errori

In validazione (analisi del contratto):

- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non si riconosce come WKB (né estensione `geoarrow.wkb` né chiavi
  `plenora.geometry.*`);
- `Unsupported`: dimensioni della geometria diverse da `xy`;
- `InvalidPlan`: `distance` assente o non finita, `cap` fuori elenco,
  campi sconosciuti;
- `Crs`: CRS della colonna assente o non risolto, geografico, o proiettato
  senza unità lineare.

In esecuzione, prima del kernel, su tutta la colonna: `InvalidPlan` per
una cella che non è WKB strutturalmente valido, `Crs` per una coordinata
fuori dal dominio di validità del CRS della colonna, `Schema` per una
geometria di un tipo che il contratto d'ingresso non dichiara, quando li
dichiara con un elenco ([Runner, «Operazioni geo»](runner.md#operazioni-geo)).

Poi dal kernel, per geometria (`OperationError`, che il runner porta in `PlenoraError`:
`Internal` per `Internal`, `ValidazioneNonConclusa` e
`CalcoloNonConcluso`, `Unsupported` per `PrecisionInsufficient`,
`InvalidPlan` per le altre):

- `InvalidInput`: la geometria non supera la validazione OGC;
- `PrecisionInsufficient`: la griglia di `i_overlay`, sommata al rientro
  delle direzioni intere, supererebbe metà della precisione (con 1 cm:
  `|distance|` oltre circa 5.368 km, o coordinate oltre circa `2^39` m);
- `InvalidOutput`: il buffer prodotto non supera la validazione OGC;
- `ValidazioneNonConclusa`, `CalcoloNonConcluso`: la validazione OGC o il
  calcolo di `geo` e `i_overlay` vanno in panico dentro la barriera (il
  messaggio porta solo la forma del payload);
- `InvalidParameter`: `distance` non finita (l'analisi la rifiuta prima);
- `MargineMemoria` (`ResourceLimit`): i punti dei contorni prima di un
  overlay, le parti dopo un'unione dei blocchi o il risultato non
  starebbero nel margine di memoria del passo, per questa geometria
  (guardia che riduce il rischio, non un tetto garantito: [Runner, «Limiti dichiarati del runner»](runner.md#limiti-dichiarati-del-runner), voce «Modelli di costo geo»).

Una geometria prodotta oltre il limite di byte per cella (64 MiB di WKB)
è `ResourceLimit`. Il primo errore è quello della prima riga in ordine di riga, senza
diagnostica per riga.

### Limiti e deviazioni

Nel runner un errore non ha diagnostica per riga e il costo in memoria è
una previsione dalle misure
([Runner, «Limiti dichiarati del runner»](runner.md#limiti-dichiarati-del-runner),
voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).
Il runner passa al kernel la precisione di 1 cm a terra nelle unità del
CRS della colonna (`Precision::from_crs`, calcolata in validazione;
[Runner, «Operazioni geo»](runner.md#operazioni-geo), voce
«Precisione»). Non ci sono i parametri di GEOS e PostGIS per le
giunzioni (`join`, `mitre_limit`), il numero di segmenti per quarto di
cerchio (qui il passo degli archi viene dalla precisione) e il buffer da un
solo lato. Nessun controllo a posteriori del risultato contro la
definizione esatta ([«Limiti dichiarati»](limiti.md#limiti-dichiarati),
voce «Nessun controllo a posteriori»).

### Precisione

Entro la precisione `p` (1 cm a terra) fino a `|distance|` = 500 `p`
(5 m con 1 cm); oltre, **deviazione dichiarata**: gli archi sono corde con
freccia al più `max(p / 2, 0.001 |distance|)`, quindi il buffer può stare
verso l'interno lungo gli archi fino allo 0,1% della distanza (10 cm a
100 m), senza errore. Verso l'esterno il buffer resta entro `p / 2` da
quello esatto: la griglia di `i_overlay` (due passaggi, il buffer di `geo`
e l'unione delle parti) si controlla prima del calcolo e, se non basta, si
rifiuta con `PrecisionInsufficient`. Le componenti che la griglia
ridurrebbe a un punto non spariscono: una linea più corta di due passi
della griglia si bufferizza come il suo primo punto, un poligono minuscolo
o più sottile di due passi come il suo anello esterno. Una linea i cui
offset si sovrappongono su molti segmenti lontani (zig-zag stretto rispetto
alla distanza), con estremità tonde o piatte, si bufferizza a blocchi di 8
segmenti uniti a coppie: l'unione è lo stesso buffer entro la stessa fascia,
e differisce dal calcolo in un solo passaggio di al più `f + p`
([Limiti dichiarati, «Precisione delle operazioni geografiche»](limiti.md#precisione-delle-operazioni-geografiche-1-cm-a-terra),
voci «Buffer», «Deviazione: archi del buffer» e «Buffer delle linee a
blocchi: un secondo algoritmo»).

### Complessità

Dominata da `i_overlay` (offset dei contorni e unione, sulla griglia
intera); non c'è una stima asintotica dichiarata. Il tratto in un solo
passaggio calcola ogni incrocio fra gli offset dei segmenti: su una linea a
zig-zag stretto è quadratico nei vertici, e oltre la soglia si passa ai
blocchi (1.000 vertici a 0,8 m con 200 m: 24 ms invece di 110 s e 21 GiB). Tempi misurati (stella e
linea da 1.000 vertici, da 1 m a 1 km) in Limiti dichiarati, voce «Costo» di
[«Precisione delle operazioni geografiche»](limiti.md#precisione-delle-operazioni-geografiche-1-cm-a-terra).
Più la validazione OGC dell'ingresso e dell'uscita
([Limiti dichiarati, «Validazione OGC»](limiti.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì)).

### Esempio

```json
{
  "config": {"distance": 1, "cap": "flat"},
  "ingressi": [
    {"nome": "tratte", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["LINESTRING(0 0,10 0)", "POINT(5 5)"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["MULTIPOLYGON(((0 1,0 -1,10 -1,10 1,0 1)))", "MULTIPOLYGON EMPTY"]}
  ]}
}
```
