### Che cosa fa

Sostituisce ogni geometria con un poligono concavo che ne racchiude tutte
le coordinate (vertici degli anelli interni e punti ripetuti compresi).
È l'algoritmo di `geo`, porting di `concaveman`: parte dall'inviluppo
convesso e scava verso i punti interni ogni lato più lungo di
`length_threshold`, se il punto candidato sta entro la lunghezza del lato
divisa per `concavity`. Più `concavity` è piccola, più il poligono è
concavo; molto grande, è l'inviluppo convesso.

I confronti di distanza usano i lati adiacenti del bordo corrente, anche
dopo uno scavo. La coda visita ciascun lato una volta e accoda solo i due
figli dell'inserimento; non riparte dai lati gia' visitati.

Il percorso dimensionale 6.2.0 e' opt-in: `degeneracy_threshold: 0`
riconosce la dimensione esatta dei binary64, senza tolleranza implicita.
Un punto o punti coincidenti producono `Point`; due punti distinti o
punti collineari producono `LineString` fra minimo e massimo lessicografico
XY, indipendentemente dall'ordine e dai duplicati. Lo zero con segno viene
normalizzato. Nessuna coordinata viene costruita. Il vuoto resta
`Polygon EMPTY`; negli altri casi si usa lo stesso percorso poligonale.
L'oracolo dimensionale e' il determinante razionale nullo e gli estremi
della retta, non una presunta specifica Esri di concaveHull.

### Parametri

`input_geometry` (stringa facoltativa) seleziona per nome la colonna geometrica
da elaborare. Omettendo un selettore si usa
la primaria; senza primaria la selezione implicita fallisce con `Schema`.
Il selettore non cambia la primaria. CRS, assi e precisione del calcolo
riguardano solo gli operandi selezionati; le geometrie passanti conservano
i propri metadati e byte, seguendo la stessa lineage delle altre colonne.

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `degeneracy_threshold` | numero | assente: percorso legacy | finito, non negativo | opt-in dimensionale, separato dallo scavo |
| `concavity` | numero | obbligatorio | finito, maggiore di zero | concavità relativa: più piccola, più concavo |
| `length_threshold` | numero | `0` | finito, non negativo | lati più corti di così non si scavano; `0` scava ogni lato |

Il kernel (`extended::concave_hull`) riceve `length_threshold` sempre
esplicito, insieme al limite di coordinate `max_coordinates`: il runner
passa `0` quando manca (ogni lato si può scavare) e `MAX_CELL_COORDINATES`
(4 194 304) come limite.

### Schema

Stesse colonne, tipi, nullabilità e metadati di schema. La colonna
geometria resta al suo posto con lo stesso CRS e le stesse dimensioni
(XY), ma dichiara ora il solo tipo `Polygon` (dichiarazione esatta): le
chiavi `plenora.geometry.types` e `plenora.geometry.types_declaration`
ereditate si tolgono dal campo. Le proprietà del contratto (`sorted_by`,
`row_count`) restano.

Con `degeneracy_threshold` presente l'elenco esatto dei tipi d'uscita e'
`point,linestring,polygon`. Concavita' e soglia di scavo si validano anche
per vuoti e degeneri, ma non alterano il punto o segmento risultante.
Soglie di degenerazione negative o non finite sono `InvalidPlan`.
Con soglia positiva si restituisce il minimo lessicografico XY se ogni
coordinata dista da esso al massimo la soglia; altrimenti il segmento
minimo-massimo XY se ogni coordinata dista dal segmento al massimo la
soglia. Negli altri casi si esegue concaveman. La distanza e' nelle unita'
del CRS, la soglia e' inclusiva e non e' una precisione numerica implicita.
Il solo adattatore `metric_within` usa `operations::distance` di main:
confronto binary64 provvisorio, da adeguare alla primitiva esatta PR #196.
Distanza assente, non finita o negativa produce errore esplicito.
In attesa della metrica esatta, nel confronto punto-segmento ogni delta
non nullo fra le tre coordinate deve avere valore assoluto fra `2^-128`
e `2^20`; fuori da questo dominio si rende `Unsupported`, evitando
overflow/underflow interni che la distanza legacy potrebbe nascondere.
Anche i delta punto-punto devono essere al massimo `2^20` in valore
assoluto. Il limite superiore restringe gli errori assoluti della formula
legacy; non qualifica confronti esatti a ridosso della soglia.
Ogni distanza zero e' inoltre verificata dalla topologia razionale comune.

### Righe

1:1: il runner chiama il kernel (`extended::concave_hull`) su ogni cella non
nulla, in parallelo, e rimette la geometria al suo posto; una cella
nulla resta nulla ([Runner, «Operazioni geo»](runner.md#operazioni-geo)).
Una geometria senza coordinate dà un `POLYGON EMPTY`.

### Ordine

Per contratto quello d'ingresso (forma 1:1).

### Errori

In validazione (analisi del contratto):

- `Schema`: l'ingresso non ha una colonna geometria selezionabile senza ambiguita', o la
  colonna non si dichiara geometria WKB;
- `Unsupported`: dimensioni della colonna diverse da XY (anche non
  dichiarate);
- `InvalidPlan`: config con campi sconosciuti, `concavity` assente, non
  finita o non positiva, `length_threshold` scritto e non finito o
  negativo;
- `Crs`: CRS della colonna mancante o non risolto, non proiettato
  (`PROJECTED_CRS_REQUIRED`) o senza unità lineare
  (`LINEAR_UNIT_REQUIRED`).

In esecuzione, prima del kernel, su tutta la colonna: `InvalidPlan` per
una cella che non è WKB strutturalmente valido, `Crs` per una coordinata
fuori dal dominio di validità del CRS della colonna, `Schema` per una
geometria di un tipo che il contratto d'ingresso non dichiara, quando li
dichiara con un elenco ([Runner, «Operazioni geo»](runner.md#operazioni-geo)).

Poi dal kernel, per geometria (`ExtendedError`, che il runner porta in `PlenoraError`: `Internal` per
`ValidazioneNonConclusa` e `CalcoloNonConcluso`, `ResourceLimit` per
`CoordinateLimit`, `InvalidPlan` per le altre):

- `InvalidInput`: coordinate NaN o infinite o geometria non valida OGC;
- `CoordinateLimit`: più coordinate di `max_coordinates` (il runner passa
  `MAX_CELL_COORDINATES`, 4 194 304);
- `InvalidOutput`: il poligono prodotto non è valido OGC, come per un
  punto solo, due punti distinti o punti tutti allineati;
- `CalcoloNonConcluso`: `geo` va in panico, per esempio con coordinate
  vicine al massimo di `f64`; `ValidazioneNonConclusa`: la validazione
  OGC non conclude.

Una geometria prodotta oltre il limite di byte per cella (64 MiB di WKB)
è `ResourceLimit`. Il primo errore è quello della prima riga in ordine di riga, senza
diagnostica per riga.

### Limiti e deviazioni

Nel runner un errore non ha diagnostica per riga e il costo in memoria è
una previsione dalle misure
([Runner, «Limiti dichiarati del runner»](runner.md#limiti-dichiarati-del-runner),
voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).
Il lavoro è limitato da `max_coordinates`, un argomento del kernel e non
della config: nel runner `MAX_CELL_COORDINATES`. Il poligono non è quello
di `ST_ConcaveHull` di PostGIS, che
ha altri parametri (frazione dell'area convessa, buchi ammessi).

### Precisione

Nessuna coordinata calcolata: i vertici del poligono sono coordinate
dell'ingresso, con i loro bit. Quali punti entrano nel bordo lo decidono
distanze e confronti in `f64` di `geo`, non predicati esatti: per punti
più vicini di 1 cm al bordo la scelta segue l'arrotondamento, il caso
fuori ambito della regola
([Limiti dichiarati, «Precisione delle operazioni geografiche: 1 cm a terra»](limiti.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).
Nessun rifiuto `PrecisionInsufficient`.

### Complessità

Tempo almeno O(n log n) sulle n coordinate (inviluppo convesso e R-tree
dei punti di `geo`; il costo dello scavo nel caso peggiore non è
dichiarato), più la validazione OGC di ingresso e uscita; memoria O(n).

Ordine delle ordinate: dichiarazione esplicita compatibile richiesta dalla 4.0;
assenza o `unknown` causano `crs/validate` senza effetti. Vedi
[migrazione e censimento](limiti.md#ordine-delle-ordinate).

L'API Rust `extended::dimensional::concave_hull_dimensional_with_limits`
conta le coordinate prima di duplicarle. Le scansioni e le verifiche
razionali consumano passi reali; la memoria aritmetica usa le ricevute
del noder (GOP-027). Deadline e annullamento usano il controllo condiviso
GOP-028; deadline diventa Timeout nella pipeline. I limiti forniti governano
la classificazione dimensionale e i buffer propri del concaveman:
coda dei lati, heap dei candidati, punti scartati, bordo e uscita. Ogni
visita o operazione di coda consuma lavoro effettivo, senza prodotti
preventivi; le coppie contano candidati metrici e confronti geometrici.
Le ricevute precedono la crescita e comprendono la sovrapposizione dei
buffer durante la riallocazione. L'ordine RTree/BinaryHeap resta legacy.
Quickhull, gli interni degli RTree e la validazione OGC restano opachi:
checkpoint ai confini, scratch interni non coperti, GOP-027 non integrale.
La pipeline propaga scadenza e token alla classificazione, con limiti standard
del noder e memoria per invocazione limitata anche dal margine del passo;
non e' un tetto aggregato sulle invocazioni parallele. La qualifica
cooperativa e di memoria delle fasi opache resta lavoro necessario
per completare il contratto 9; le soglie positive attendono la qualifica
metrica esatta della PR #196.

### Esempio

Con `concavity` 1 il bordo scava fino al punto `(1.5, 1)`; con 3 il
risultato sarebbe il quadrato convesso.

```json
{
  "config": {"concavity": 1.0, "length_threshold": 0.0},
  "ingressi": [
    {"nome": "rilievi", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["MULTIPOINT((0 0),(2 0),(1.5 1),(2 2),(0 2))"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POLYGON((2 0,1.5 1,2 2,0 2,0 0,2 0))"]}
  ]}
}
```
