# Misure a forma conservata

Le API Rust additive nel modulo `preserve_shape_measure` misurano la retta
nello SR originale sull'ellissoide del datum. Non modificano le operazioni
planari o geodetiche esistenti. Non sono registrate nel catalogo, in analyze,
nella CLI o negli adapter Arrow. Non introducono parametri di servizi esterni.

## API e dominio

`preserve_shape_length_with_limits` accetta una slice di `LineString<f64>`
e restituisce la somma delle lunghezze in metri. Si possono passare anche
anelli esplicitamente chiusi per misurare contorni. Parti vuote, vertici
duplicati e percorsi costanti al polo contribuiscono zero.

`preserve_shape_area_with_limits` accetta una slice di `Polygon<f64>` e
restituisce la somma delle aree in metri quadrati. Per ciascun poligono:
area assoluta del guscio meno aree assolute dei buchi. Il calcolo interno
degli anelli e' firmato, positivo antiorario; invertire il verso non cambia
il ruolo di guscio o buco. Le parti non sono unite: eventuali sovrapposizioni
fra poligoni distinti si contano piu' volte. Un poligono vuoto vale zero.

Entrambe richiedono `ResolvedCrs`, `ShapeTolerance`, `ShapeMeasureLimits`
e un flag `AtomicBool` opzionale. Il risultato `ShapeMeasure` contiene valore,
bound assoluto, ellissoide e lavoro consumato. Il CRS del certificato e'
esattamente l'argomento della chiamata; il modello e' `preserve_shape`, le
unita' sono fissate dalla funzione. Nessun cambio di datum: le conversioni
riusano `Riproiettore::geografiche_sorgente` con piano identita'.

Il dominio certificato comprende i CRS geografici integrati, EPSG:3857, Mercator A (EPSG:3395)
e Transverse Mercator EPSG 9807, LCC 2SP EPSG 9802, LAEA EPSG 9820 e Oblique Stereographic EPSG 9809 nei rettangoli qualificati in [maggioranti-forma](maggioranti-forma.md),
con ellissoide terrestre `6e6 <= a <= 7e6` metri e `250 <= 1/f <= 350`.
**Il supporto a qualunque CRS riproiettabile non e' completo:** Hotine
restituisce `AccuracyUnproven`. Anche TM, LCC, LAEA e stereografica obliqua rifiutano
fuori dal proprio dominio qualificato o quando il residuo a intervalli non
dimostra la precisione numerica richiesta. Una convergenza campionaria non li
sostituisce. CRS senza inversa integrata e punti fuori dominio danno
`CrsDomain`. Non si ripiega su WGS 84.

Il segmento geografico interpola linearmente longitudine e latitudine.
Da 179 a -179 attraversa zero e copre 358 gradi; non c'e' unwrap implicito.
Un attraversamento corto del seam va fornito come parti separate. Una
retta proiettata viene campionata prima dell'inversa, non trasformata in
una geodetica fra i soli vertici.

Ogni poligono deve essere semplice, con anelli chiusi non degeneri, buchi
strettamente interni, senza contatti o incroci fra anelli. La validazione
usa i predicati esatti del kernel geometrico, anche per i test di
contenimento. Tutti gli anelli di un poligono devono stare in una striscia
aperta di longitudine larga meno di 180 gradi, senza poli. Per geografico
e per i due metodi Mercator la trasformazione e' iniettiva in questa striscia e conserva
la topologia. Anche TM, LCC, LAEA e stereografica obliqua sono iniettive nei rispettivi rettangoli qualificati. La superficie e' strettamente minore di mezzo ellissoide;
complementi, cappe polari e regioni piu' larghe sono `UnsupportedSurface`.
Z, M e curve non appartengono a questa API XY.

## Accuratezza

La tolleranza e' separata per ciascuna chiamata: assoluta in m o m² e
relativa adimensionale. Entrambe finite e non negative, almeno una positiva;
il relativo e' minore di uno. Dati valore calcolato v, valore matematico t
e bound B, l'accettazione richiede

```text
B <= absolute + relative * max(0, abs(v) - B)
```

Poiche' `abs(t) >= max(0, abs(v)-B)`, questo implica il contratto
`abs(v-t) <= absolute + relative*abs(t)`. Non si confronta semplicemente
l'errore relativo con il valore approssimato. Il bound B e' restituito,
non solo la tolleranza richiesta. Non e' l'incertezza del rilievo o della
realizzazione del datum, che queste API non quantificano. Non c'e'
quantizzazione, repair o generalizzazione. Il centimetro posizionale non
e' interpretato come un centimetro quadrato di errore sulle aree.

Il campionamento parte da una divisione per lato e raddoppia la risoluzione
globale finche' il bound soddisfa la tolleranza. I punti intermedi sono
sempre interpolati sul lato originale, poi convertiti tramite il core CRS.
Il massimo rappresentabile e' `2^31` divisioni per lato. Ogni tentativo
scartato entra nel budget; nessuna dipendenza dall'ordine degli hash.

La [tabella dei maggioranti](maggioranti-forma.md) censisce ogni metodo
invertibile, i motivi dei rifiuti e la regola comune usata anche per Mercator A.

### Lunghezza

Per una curva ECEF r(t), t in [0,1], poniamo `norm(r''(t)) <= A` e lunghezza
L. Se c e' la corda ECEF e d la distanza geodetica fra gli estremi:

```text
L² - c² <= (1/2) integral integral norm(r'(t)-r'(s))² dt ds <= A²/12
c <= d <= L
0 <= L-d <= A²/(12d)
```

Questo maggiorante deriva dalla varianza della velocita', non dalla
differenza fra due campionamenti. Per la metrica ellissoidica G1, nel
dominio terrestre qualificato, le norme delle derivate ECEF prime e
seconde rispetto a longitudine e latitudine sono minori di `1.1a`.
Con differenze angolari in radianti, sul singolo sottotratto:

```text
geografico: A = 1.1a (abs(delta_lambda) + abs(delta_phi))²
Web Mercator: A = 1.1a ((abs(delta_x/a) + abs(delta_y/a))² + (delta_y/a)²)
```

Per Web Mercator `phi=atan(sinh(y/a))`, quindi la prima derivata e'
limitata da `abs(delta_y/a)` e la seconda da `(delta_y/a)²`. G2 dichiara
errore inverse terrestre inferiore a 15 nm. Il kernel usa un margine di
1 micrometro per sottotratto, comprendendo conversioni e interpolazione:
nel denominatore usa `d-1e-6`, poi aggiunge `1e-6` al bound. Se il
denominatore non e' positivo usa il maggiorante della velocita'
`1.1a (abs(delta_lambda)+abs(delta_phi))` (coordinate normalizzate nel caso
Web Mercator), quindi non scambia gli estremi coincidenti per una curva
costante. La somma usa compensazione di Neumaier e un margine relativo
aggiuntivo di `32 epsilon`.

### Area

G1/G2 forniscono la somma delle aree geodetiche dei sottotratti tramite
`PolygonArea`. Il certificato confronta quel risultato con la quadratura
al punto medio dell'integrale esatto della superficie originale:

```text
area_firmata = -integral Q(phi) d_lambda
Q(phi) = a²(1-e²)/2 [sin(phi)/(1-e²sin²(phi)) + atanh(e sin(phi))/e]
```

Il kernel valuta Q con nove termini della serie
`a²(1-e²) sum_{n=0..8} (n+1)/(2n+1) e^(2n) sin(phi)^(2n+1)`;
il resto e' minore di `1e-18 a²` per `e²<0.008`. Nel dominio qualificato,
`abs(Q')<1.1a²` e `abs(Q'')<1.1a²`. Il resto della quadratura per sottotratto
e' quindi limitato da

```text
geografico: abs(delta_lambda) * 1.1a² * delta_phi² / 24
Web Mercator: abs(delta_x/a) * 2.2a² * (delta_y/a)² / 24
```

Si aggiunge `512 epsilon a² abs(delta_lambda)` per valutazione della serie,
conversioni angolari, interpolazione e accumulazione compensata. Per anello:

```text
B = abs(area_GeographicLib - integrale_calcolato) + resto + margine_numerico
```

Il residuo include anche l'errore di accumulazione GeographicLib con le
feature effettive del workspace (`default-features=false`). Non si assume
che la sua somma sia esatta. I bound degli anelli si sommano anche per i
buchi; la cancellazione delle aree non riduce il bound. Ulteriori margini
di `32 epsilon` coprono il residuo e la somma finale.

Una differenza sorgente non nulla che diventa zero o subnormale durante
normalizzazione o divisione e' `AccuracyUnproven`. Il numero complessivo
di unita' di lavoro e' limitato anche a `2^40` per mantenere trascurabile
il termine `n epsilon²` nelle somme compensate. Tolleranze sotto il margine
numerico possono non essere raggiungibili: il lavoro non prosegue oltre
il budget e non restituisce un valore plausibile privo di certificato.

## Risorse ed errori

Non si allocano vettori di campioni: la memoria aggiuntiva rispetto al piano
CRS e agli input e' costante. Non c'e' stack ricorsivo. Il piano identita'
riusa le piccole strutture del core CRS; non e' un limite all'heap del processo.

`max_work` conta visite di coordinate e parti, conversioni, primitive
geodetiche, confronti di segmenti e passi del test di contenimento.
Controllo e carica precedono ogni unita'. La validazione topologica e'
quadratica nel numero di lati nel caso peggiore e viene contata, non
ammessa attraverso una stima. Somme intere controllate; nessun prodotto
fra cardinalita' decide l'ammissione. Un errore non restituisce un prefisso.

Checkpoint all'ingresso, per parte, per coordinata e per primitiva, nei
confronti topologici, e prima del ritorno. Cancellazione prima della
deadline, poi budget. Le singole primitive core/GeographicLib sono
sincrone; non si promette interruzione all'interno di una loro chiamata.

Errori distinti: `InvalidTolerance`, `CrsDomain`, `UnsupportedEllipsoid`,
`UnsupportedTopology`, `UnsupportedSurface`, `AccuracyUnproven`,
`WorkLimit`, `Cancelled`, `DeadlineExceeded`. Il testo non contiene dati.

## Fonti e prove

- Contratto locale `GEOMETRY-OPERATIONS-1.0`, GOP-003--007: modello,
  accuratezza, datum, segni, dominio e percorso originale senza unwrap.
- [G1, Karney 2013, sezioni 2, 6, 7](https://arxiv.org/html/1109.4448v2):
  metrica e primitive geodetiche. I maggioranti delle curve composte sono
  derivati sopra, non attribuiti a una garanzia generale di GeographicLib.
- [G2, GeographicLib Python 2.1](https://geographiclib.sourceforge.io/html/python/geodesics.html):
  inverse, area e accuratezza delle primitive terrestri.
- Inventario `GEO-geodetica.md`, voce 26: campionare nello SR originale.

Le prove C1 sono in
[`preserve_shape_measure/tests.rs`](../crates/plenora-kernels-geo/src/preserve_shape_measure/tests.rs).
Ogni attesa porta la propria fonte: parallelo analitico diverso dalla
geodetica, rettangolo analitico, buco, orientazione, retta Web Mercator,
convergenza raddoppiando la precisione, datum NAD27, seam lungo, vuoti,
multipart, soglie esatte di lavoro, interruzione, underflow e domini rifiutati.
Il driver indipendente
[`reference.py`](../crates/plenora-kernels-geo/tests/fixtures/preserve_shape/reference.py)
genera i riferimenti GeographicLib Python 2.1 a 32768 e 65536 divisioni
per lato. La differenza fra i due riferimenti e' circa 2.7 micrometri sulla
linea e 0.77 m² sul rettangolo; le prove usano margini rispettivamente
di 0.1 mm e 1 m² per la fixture. La prova analitica dell'area non dipende
da quel margine campionario.
