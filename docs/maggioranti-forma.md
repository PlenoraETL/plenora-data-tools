# Maggioranti delle inverse per preserve_shape

La tabella eseguibile e' il match esaustivo `Proiezione::shape_certificate`
in [proiezioni.rs](../crates/plenora-core/src/crs/riproiezione/proiezioni.rs).
`Riproiettore::inverse_shape_certificate` espone il risultato senza
cambiare datum. Il test in
[forma.rs](../crates/plenora-core/src/crs/riproiezione/forma.rs) costruisce
ogni definizione integrata e verifica l'insieme dei metodi; aggiungere una
variante invertibile senza classificarla causa anche un errore di compilazione.

## Tabella di qualificazione

Per le righe separabili B1 e B2 sono maggioranti delle derivate prima e seconda della latitudine
rispetto alla coordinata normalizzata psi. La longitudine e' affine.

| Metodo | Scala s | B1 | B2 | Stato e limite |
| --- | --- | --- | --- | --- |
| Geografico (0) | pi/180 | 1 | 0 | Certificato nel dominio geografico integrato |
| Pseudo Mercator (1024) | 1/a | 1 | 1 | Certificato sul ramo aperto, latitudine <85.06 gradi in modulo |
| Mercator A (9804) | 1/(a k0) | 1/(1-e^2) | (1+2e^2)/(1-e^2)^2 | Certificato sullo stesso ramo, controllo del residuo isometrico |
| Transverse Mercator (9807) | 1/(a k0) | phi: 4; lambda: 3 | phi: 20; lambda: 9 | Non separabile; certificato esatto sul rettangolo z sotto definito, resto di Krueger incluso nel residuo a intervalli |
| Lambert Conic Conformal 2SP (9802) | 1/a | phi: 4; lambda: 3 | phi: 20; lambda: 9 | Composizione polare e inversa isometrica separabile; certificato sul rettangolo LCC sotto definito |
| Lambert Azimuthal Equal Area (9820) | - | - | - | AccuracyUnproven: non maggiorate le derivate al bordo del disco e dell'inversa autalica |
| Oblique Stereographic (9809) | - | - | - | AccuracyUnproven: non maggiorata la composizione della rotazione con l'inversa conforme |
| Hotine Oblique Mercator B (9815) | - | - | - | AccuracyUnproven: non maggiorata la rotazione obliqua con i suoi tagli; il core implementa solo il caso svizzero, angoli 90 gradi |

Le righe non certificate dichiarano lavoro mancante, non impossibilita'
matematica. Non ricevono coefficienti sostitutivi. Anche gli ingressi vuoti
sono rifiutati per questi metodi. Il supporto di tutti i CRS rimane incompleto.

## Derivazione Mercator e regola comune

Fonte delle formule: [Snyder 1987, USGS Professional Paper 1395,
capitolo 7, equazioni 7-6 e 7-7](https://pubs.usgs.gov/publication/pp1395),
Mercator ellissoidica, equivalente al metodo EPSG 9804 della Guidance Note
7-2. I maggioranti seguenti sono una derivazione dalla formula, non
coefficienti attribuiti alla fonte.

Posti psi=(y-FN)/(a k0), lambda=lambda0+(x-FE)/(a k0),

```text
psi(phi) = asinh(tan(phi)) - e atanh(e sin(phi))
dpsi/dphi = (1-e^2) / [cos(phi)(1-e^2 sin^2(phi))]
F(phi) = dphi/dpsi = cos(phi)(1-e^2 sin^2(phi))/(1-e^2)
```

Per |phi|<pi/2, F e' positiva. Usando |sin|,|cos|<=1 e 0<=e^2<1:

```text
|F| <= 1/(1-e^2) = B1
F' = [-sin(phi)(1-e^2sin^2(phi)) - 2e^2sin(phi)cos^2(phi)]/(1-e^2)
|F'| <= (1+2e^2)/(1-e^2)
|d^2phi/dpsi^2| = |F' F| <= (1+2e^2)/(1-e^2)^2 = B2
```

Queste disuguaglianze valgono in tutto il dominio aperto, non solo ai
campioni. Ponendo e=0 si ottiene Pseudo Mercator: B1=B2=1 e s=1/a.
Per coordinate geografiche phi e lambda sono entrambe affini: B1=1, B2=0.

Per un sottosegmento parametrizzato su [0,1], dx=s delta_x e dy=s delta_y.
Il kernel usa esclusivamente la regola della catena comune:

```text
S = |dx| + B1 |dy|
|r''| <= 1.1 a [S^2 + B2 dy^2]
|(Q(phi))''| <= 1.1 a^2 (B1^2+B2) dy^2
resto_area <= |dx| 1.1 a^2 (B1^2+B2) dy^2 / 24
```

Le disuguaglianze per la metrica ECEF e Q, la varianza della velocita'
e il confronto con PolygonArea sono descritti in
[misure-forma-conservata.md](misure-forma-conservata.md#accuratezza).
Non ci sono formule di integrazione diverse per ciascun metodo.
I maggioranti del core includono arrotondamento verso l'alto; i margini
numerici del kernel coprono le operazioni di composizione.

## Dominio e numerica

Il dominio del core e' controllato prima di ogni conversione. Per le
inverse proiettate separabili certificate il profilo richiede inoltre |lambda|<pi
prima di qualsiasi riduzione modulo 2pi, e |phi|<85.06 gradi. I bordi
sono rifiutati con CrsDomain. Monotonia separata e affinita' implicano che
il rettangolo determinato dagli estremi contiene l'intero segmento inverso;
il controllo non inferisce la validita' del segmento dal campionamento.
Per l'area resta la striscia aperta di longitudine <180 gradi. La mappa
separabile strettamente monotona preserva la topologia in tale striscia.

Per Mercator A il residuo e' valutato con intervalli arrotondati verso
l'esterno in [certificate.rs](../crates/plenora-kernels-geo/src/preserve_shape_measure/certificate.rs).
Ogni addizione, sottrazione, prodotto e divisione IEEE 754 include i due
vicini del risultato arrotondato. Pi e' racchiuso tra i vicini della
costante binary64. Il divisore a*k0 include il suo arrotondamento; FN e
i parametri ellissoidici sono quelli esatti del registro nella loro
rappresentazione. La radice quadrata IEEE 754, correttamente arrotondata,
ha estremi ampliati di un ulp. Nessuna funzione trigonometrica o logaritmica
libm viene usata per attestare il residuo.

La latitudine restituita dal core, convertita da gradi con pi intervallo,
si valuta mediante queste identita' e resti:

```text
sin(phi): Taylor alternata fino al grado 25
|phi| < 1.5 => |resto| <= 1.5^27/27! < 6e-24
ln(z) = 2 sum(n=0..24) t^(2n+1)/(2n+1) + resto, t=(z-1)/(z+1)
|t| <= 0.334 => |resto| <= 2*0.334^51/[51*(1-0.334^2)] < 1e-24
atanh(s) = [ln(1+s)-ln(1-s)]/2
psi(phi) = atanh(sin(phi)) - e atanh(e sin(phi))
```

La riduzione di ln moltiplica o divide z per due e conserva l'esponente;
ln(2) si valuta con la stessa serie, non con una costante approssimata.
Tutti i passi e i resti sono propagati negli intervalli. Gli intervalli
che escono dalle ipotesi dichiarate falliscono con AccuracyUnproven.
Il numero di iterazioni del Newton del core non entra nel certificato.
La valutazione e' conservativa: presso 85 gradi il certificato residuale
rifiuta anche un punto matematicamente valido (prova dedicata), poiche'
atanh(sin(phi)) amplifica l'ampiezza dell'intervallo. Il supporto numerico
di Mercator A non copre quindi ogni punto del dominio dichiarato del core;
i maggioranti analitici, invece, valgono sull'intero dominio aperto.

La stabilita' inversa e' |errore_phi|<=B1 |errore_psi|. Il massimo modulo
dell'intervallo residuo, moltiplicato per B1 e arrotondato verso l'alto,
deve essere <=128 epsilon radianti: altrimenti AccuracyUnproven. Questa
soglia e' un criterio verificato, non un coefficiente d'errore presunto.
L'errore ECEF per punto e' quindi <=1.1a*128epsilon <0.22 micrometri
per a<=7e6. Due estremi piu' l'errore geodetico di 15 nm e le conversioni
di longitudine/interpolazione sono coperti dal micrometro per sottosegmento.
La perturbazione di Q e' <=1.1a^2*128epsilon; insieme al precedente budget
128epsilon per Q, interpolazione e accumulazione, e' <512epsilon a^2.
Il kernel usa questo maggiorante comune per l'area. La garanzia non
dipende dall'accuratezza delle funzioni trascendenti usate dal core: un
risultato impreciso del core deve superare questo certificato indipendente.

## Prove indipendenti

[mercator.py](../crates/plenora-kernels-geo/tests/fixtures/preserve_shape/mercator.py)
usa bisezione della formula di Snyder per l'inversa (nessun Newton del
core e nessun import Rust), poi GeographicLib Python 2.1 per linea e
poligono obliqui. Il pacchetto e' confinato in target/preserve-oracle;
pyproj non e' disponibile offline. Le fixture a 16384 e 32768 suddivisioni
sono controlli indipendenti, non una dimostrazione del bound. Le prove C1
confrontano direttamente la differenza con il bound restituito, senza
aggiungervi una tolleranza campionaria. I tre metodi non certificati
hanno prove del rifiuto; le loro prove numeriche C1 restano da realizzare
insieme ai maggioranti mancanti.

## Transverse Mercator EPSG 9807

Fonti: [Karney 2011, Transverse Mercator with an accuracy of a few nanometers](https://arxiv.org/pdf/1002.1417),
equazioni 14, 16, 18, 35 e 36 per la serie di Krueger del motore;
[EPSG Guidance Note 7-2, settembre 2019](https://www.iogp.org/wp-content/uploads/2019/09/373-07-02.pdf),
capitolo 3.2.3.1, per metodo, scala e false origini. Il motore usa
l'estensione di Karney al sesto ordine. Il dato numerico di Karney
sull'errore non viene assunto come dimostrazione delle derivate della coda.
La costruzione seguente e' una derivazione: confronta la serie finita con
la TM esatta mediante la continuazione analitica dell'arco meridiano.

### Dominio continuo e derivate esatte

Siano q=e^2, m(w)=M(w)/a e psi la latitudine isometrica, prolungati
analiticamente dalla latitudine reale. Si lavora sul ramo contenente zero:

```text
m(0)=0
m'(w)=(1-q)/(1-q sin(w)^2)^(3/2)
psi(w)=atanh(sin(w))-e atanh(e sin(w))
z=(y-FN)/(a k0)+m(phi0) + i (x-FE)/(a k0)
w=m^-1(z)
chi(z)=psi(w)=psi(phi)+i(lambda-lambda0)
```

L'identita' sull'asse reale esprime la scala costante del meridiano
centrale; il proseguimento conforme definisce la TM esatta. Il certificato
usa i parametri originali del registro: non i coefficienti arrotondati del
core, ne' il suo raggio rettificante o la sua origine approssimata.

Il dominio qualificato e' |Re z|<1.1, |Im z|<0.1,
|phi0|<1.1 e |lambda0|<2.5 radianti, 0<=q<=0.008, oltre ai limiti
terrestri comuni. E' intenzionalmente piu' stretto del dominio CRS e
non copre tutte le zone UTM, le latitudini elevate o tutte le origini
nazionali. Il controllo intervallare rifiuta con AccuracyUnproven anche
un bordo la cui appartenenza stretta non e' dimostrata. I segmenti sono
contenuti nel dominio per affinita' di z e convessita' del rettangolo;
non si deduce questa proprieta' dai campioni interni.

Per includere anche gli arrotondamenti del box iniziale, si usa il
rettangolo esterno |Re w|<=U=1.21, |Im w|<=V=0.21, |w|<1.23.
Posti rho=q cosh(V)^2 e d=q+(1-q)((1-rho)^(-3/2)-1):

```text
|m'-1| <= d < 0.022
|m(w2)-m(w1)| >= (1-d)|w2-w1|
```

La seconda disuguaglianza segue integrando m'-1 sul segmento. La mappa
w -> z-(m(w)-w) e' una contrazione e manda il box di semilato 0.1
centrato in z in se': |m(w)-w|<=d|w|<0.028<0.1.
Esistenza e unicita' seguono dal teorema della contrazione. L'intersezione
delle immagini intervallari conserva quindi sempre la radice.

Maggioranti ottenuti per differenziazione, con denominatori positivi:

```text
M2 = 3q(1-q)cosh(V)^2/(1-rho)^(5/2)
P1 = (1-q)/(cos(U)(1-rho))
P2 = P1 [cosh(V)/cos(U) + 2q cosh(V)^2/(1-rho)]
|chi'| <= P1/(1-d) < 3
|chi''| <= P2/(1-d)^2 + P1 M2/(1-d)^3 < 9
```

Le costanti larghe 3 e 9 si verificano anche sostituendo tutti i fattori
numeratori 1-q con 1 e q con 0.008; non richiedono una ricerca numerica
del massimo. La composizione con l'inversa isometrica reale gia'
dimostrata, B1<1.01 e B2<1.033, da' per una retta z(t),
v=|z'(t)| (si puo' maggiorare con la norma 1):

```text
|lambda'| <= 3v; |lambda''| <= 9v^2
|phi'| <= 4v;    |phi''| <= 20v^2
|r''| <= 1.1 a [(7v)^2+29v^2] = 1.1 a 78v^2
```

Sono le costanti pubbliche di TransverseShapeBounds. Non sono B1/B2
separabili: entrambe le coordinate dipendono da x e y.
La topologia e' conservata: m e' univalente; per cos(w)=A+iB,
Re[cos(w)(1-q sin(w)^2)]=A[(1-q)+q(A^2-3B^2)]>0 sul rettangolo.
Segue Re psi'>0 e quindi univalenza di psi per integrazione sul segmento.
La successiva inversa isometrica reale e' monotona. Inoltre
|Im chi|<=3|Im z|<0.3, partendo dall'asse reale: la longitudine resta
in una striscia di ampiezza <0.6 radianti, senza seam o poli.

### Resto di Krueger e certificato numerico

Per la sola serie finita F6(zeta)=zeta-sum(beta_j sin(2j zeta)),
sul dominio |Im zeta|<=H, la disuguaglianza triangolare darebbe
|F6'|<=1+sum(2j|beta_j|cosh(2jH)) e
|F6''|<=sum((2j)^2|beta_j|cosh(2jH)). Questi maggioranti da soli
non includono il resto infinito. Il certificato implementato evita questa
lacuna: confronta direttamente l'uscita di Krueger con chi esatta.
Comprende cosi' troncamento di ordine 6, origine, raggio rettificante,
coefficienti e arrotondamenti del core nello stesso residuo verificato.
Le derivate sono quelle esatte sopra dimostrate, non derivate numeriche
della serie troncata.

[transverse.rs](../crates/plenora-kernels-geo/src/preserve_shape_measure/certificate/transverse.rs)
riusa ShapeInterval di Mercator A. L'arco meridiano si valuta attraverso
una serie convergente con resto esplicito, senza funzioni ellittiche:

```text
c_k=(3/2)_k/k!; J_0(w)=w
J_k(w)=((2k-1)J_(k-1)(w)-sin(w)^(2k-1)cos(w))/(2k)
m(w)-w=-qw+(1-q)sum(k=1..12) c_k q^k J_k(w)
|coda_N| <= (1-q)|w| c_(N+1) rho^(N+1)
             / [1-rho(1+1/(2N+4))]
```

Infatti |J_k(w)|<=|w|cosh(V)^(2k) lungo il segmento da zero a w;
il rapporto dei termini maggioranti dopo N+1 e' al piu'
rho(1+1/(2N+4)). Per N=12 sul rettangolo esterno la coda e'
<1e-24. Ogni componente complessa viene ampliata di questo valore.
L'iterazione intervallare valuta m-w nella forma esplicita, evitando
la cancellazione dipendente fra due intervalli che contengono w.
Il numero di iterazioni non prova la precisione: decide il residuo finale.

Seno, coseno, sinh e cosh complessi sono composti dalle identita' reali.
Il seno usa il resto Mercator; il coseno fino al grado 24 ha resto
<1e-21 per |Re w|<1.5; sinh/cosh fino ai gradi 25/24 hanno resto
<1e-30 per |Im w|<=0.21. Il logaritmo complesso, sul semipiano destro,
usa ln(re^2+im^2)/2 e atan(im/re). Per atan, due dimezzamenti con
x/(1+sqrt(1+x^2)) riducono |x|<=8 a |x|<0.4; la serie alternata
fino al grado 81 ha resto <1e-32 prima della moltiplicazione per 4.
Logaritmo reale e radice riusano le garanzie IEEE 754 del certificato
Mercator. Nessuna trascendente libm certifica il risultato.

L'intervallo chi viene confrontato con psi(phi_core)+i(lambda_core-lambda0).
La stabilita' B1<1.01 trasforma il residuo reale in errore di latitudine.
Ogni errore angolare deve essere <=2048 epsilon; altrimenti si restituisce
AccuracyUnproven. E' un bound puntuale verificato sulla TM esatta,
comprensivo del resto di Krueger, non una stima sperimentale del resto.
Le conversioni dei gradi e i parametri sono anch'essi intervallari.

### Lunghezza e area non separabili

Per la lunghezza si usa il maggiorante ECEF 1.1a78v^2 nella regola
comune di varianza della velocita'. Il margine di 30 micrometri per
sottosegmento copre due errori ECEF <=1.1a*4096epsilon, la primitiva
geodetica e le conversioni/interpolazioni sul dominio limitato.

Per l'area, posto g(t)=Q(phi(t)), si integra
-g(1/2)(lambda(1)-lambda(0)). Espandendo g attorno al centro, il termine
lineare integrato contro lambda'(1/2) si annulla. Rimangono:

```text
|g'| <= 4.4 a^2 v
|g''| <= 39.6 a^2 v^2
resto <= |g''| |lambda'|/24 + |g'| |lambda''|/12
       <= 8.25 a^2 v^3
```

Si aggiungono 16384epsilon a^2 per sottosegmento: comprendono gli errori
angolari di entrambi gli estremi e del centro, Q e le operazioni
aritmetiche. Il confronto finale con PolygonArea resta quello comune.
Questo margine e' conservativo e cresce col numero di sottosegmenti:
non si promette di soddisfare ogni tolleranza, anche entro il rettangolo.
Un budget o una precisione insufficienti producono un errore esplicito.

### Prove TM indipendenti

[tm.py](../crates/plenora-kernels-geo/tests/fixtures/preserve_shape/tm.py)
implementa autonomamente Krueger di ordine 6 con bisezione isometrica;
GeographicLib Python 2.1, confinata in target/preserve-oracle, misura le
linee e i poligoni densificati. EPSG:32632 usa WGS84, EPSG:27700 usa
Airy 1830 e latitudine d'origine non nulla, senza cambio di datum.
La fixture contiene i risultati a 16384 e 32768 divisioni. Il confronto
C1 usa direttamente il bound restituito: nessuna tolleranza aggiuntiva
di convergenza. Questo oracolo controlla l'implementazione e non sostituisce
la prova del resto esatto. Sono verificati anche i bordi rifiutati del
rettangolo normalizzato per lunghezza e area.

## Lambert Conic Conformal 2SP EPSG 9802

Fonti: [Snyder 1987, capitolo 15, pp. 107-109](https://pubs.usgs.gov/publication/pp1395),
equazioni 15-7--15-11 e 14-9--14-11 (anche nella
[copia del volume](https://gis-lab.info/docs/books/snyder87/snyder87.pdf));
[EPSG GN 7-2, settembre 2019, capitolo 3.1.1.1](https://www.iogp.org/wp-content/uploads/2019/09/373-07-02.pdf)
per la formula del motore. Il segno del cono si applica ad entrambe le
coordinate dell'atan2, come precisa Snyder per l'emisfero sud.
Le disuguaglianze che seguono sono derivate qui, non attribuite alle fonti.

### Composizione e dominio continuo

Posti q=e^2, psi la latitudine isometrica e m(phi)=cos(phi)/sqrt(1-q sin(phi)^2):

```text
n = [ln m(phi1)-ln m(phi2)]/[psi(phi2)-psi(phi1)]
ln|F| = ln m(phi1)-ln|n|+n psi(phi1)
r0/a = F exp(-n psi(phi0)); sigma = sign(n)
u = sigma (x-FE)/a
v = sigma [r0/a-(y-FN)/a]
R = sqrt(u^2+v^2); theta = atan2(u,v)
lambda = lambda0+theta/n
psi = [ln|F|-ln R]/n; phi = inverse_isometric(psi)
```

Il certificato riusa i maggioranti separabili B1<1.01 e B2<1.033
dell'inversa isometrica, ma non dichiara separabile la mappa da XY:
la trasformazione polare accoppia entrambe le coordinate. Come TM usa
il percorso comune non separabile, senza una formula di misura specifica.

Parametri qualificati: 0<=q<=0.008, 0.5<=|n|<=0.9,
|phi0|,|phi1|,|phi2|<1.2 radianti, |lambda0|<2 radianti,
|FE|,|FN|<=20000000 m e 6000000<=a<=7000000 m.
I paralleli coincidenti o una differenza isometrica il cui intervallo
contiene zero vengono rifiutati. Il dominio dei punti e':

```text
|u| < 1/4; 3/4 < v < 3/2
```

Si interseca con il dominio CRS del motore. E' un profilo intenzionalmente
ristretto: non si promette l'intero dominio di ogni definizione LCC.
Bordi, esterno o appartenenza stretta non dimostrata producono
AccuracyUnproven prima dell'inversa ordinaria. Non si cambia il registro
ne' il dominio dell'operazione di riproiezione.

u e v sono affini in XY, con r0 esatto costante. Il rettangolo e' convesso:
il controllo degli estremi include ogni punto di ogni segmento, senza
inferenze dal campionamento. Distanza continua dall'apice >3a/4;
distanza dal taglio atan2 (u=0,v<=0) >3a/4;
distanza angolare dal taglio >pi-atan(1/3). Inoltre
|lambda-lambda0|<2 atan(1/3)<0.644, dunque |lambda|<2.644<pi:
nessun modulo o seam. La formula avanti riduce lambda-lambda0: anche
i raggi theta=+/-|n|pi restano a distanza angolare almeno
pi/2-atan(1/3)>1.249. La distanza euclidea da questi raggi e' almeno
(3a/4)*3/sqrt(10)>0.71a: per angoli oltre pi/2 il punto piu'
vicino del raggio e' l'apice. L'intero dominio ha ampiezza di longitudine <1.288<pi.
Il logaritmo polare e' iniettivo nel semipiano v>0; divisione per n e
inversa isometrica monotona conservano la topologia. Anche gli interni
degli anelli restano nel rettangolo convesso.

### Maggioranti delle derivate

Per z=v+i u, lungo una retta normalizzata e con h=|z'|,
(log z)'=z'/z e (log z)''=-(z'/z)^2. Ne segue:

```text
|lambda'|, |psi'| <= h/(|n| R) < (8/3)h < 3h
|lambda''|, |psi''| <= h^2/(|n| R^2) < (32/9)h^2 < 9h^2
|phi'| <= B1 (8/3)h < 4h
|phi''| <= [B2 (8/3)^2+B1 (32/9)]h^2 < 12h^2 < 20h^2
```

Le costanti esatte 3/9/4/20 valgono per l'intero segmento, incluse le
parti che si avvicinano all'apice. Il kernel maggiora h con
(|delta_x|+|delta_y|)/a. Riusa quindi i maggioranti TM gia' dimostrati:
|r''|<=1.1a78h^2, resto area <=8.25a^2h^3. Le costanti piu' larghe
permettono la stessa regola comune di integrazione e gli stessi budget.

### Certificato numerico a intervalli

[lambert.rs](../crates/plenora-kernels-geo/src/preserve_shape_measure/certificate/lambert.rs)
ricostruisce n e r0/a dai parametri originali del metodo, non dalle
costanti libm del core. Comprende cosi' l'errore delle costanti, delle
false origini, della conversione in radianti e dell'inversa iterativa.
Riusa ShapeInterval, seno, coseno, logaritmo, atan e radice gia' descritti.
Si usa la forma r0/a = m(phi1)/n * exp(n*(psi(phi1)-psi(phi0))).
Le restrizioni |n*(psi(phi1)-psi(phi0))|<2 e 3/4<|r0/a|<3/2
sono verificate con intervalli. Quest'ultima include l'origine nel
rettangolo ed e' un ulteriore limite del profilo certificato.
Per exp usa Taylor al grado 40 e, per |x|<2, resto
exp(2)*2^41/41!<1e-35. Tutte le operazioni e il resto sono arrotondati
verso l'esterno; le ipotesi delle serie sono verificate, mai presunte.
Il residuo usa U=u/|r0/a| e V=1-sigma*(y-FN)/(a*|r0/a|):
psi=psi(phi0)-ln(U^2+V^2)/(2n), theta=atan(U/V).
Questa forma include la correlazione esatta all'origine ed evita di
sottrarre due logaritmi larghi. Si valuta ln((U^2+V^2)/2)+ln(2);
l'argomento e' fra 0.125 e 2.056, nel dominio della serie. La divisione per n
nega numeratore e denominatore quando n<0; un intervallo contenente zero
non e' divisibile. Nessuna trascendente libm attesta il residuo.

La differenza fra psi esatta e psi(phi_core), moltiplicata verso l'alto
per 1.01, deve provare un errore <=2048epsilon radianti. Lo stesso limite
si impone alla differenza fra lambda esatta e lambda_core. NaN o un
intervallo troppo largo rifiutano esplicitamente. Il numero di iterazioni
del core non decide l'accettazione; la disponibilita' numerica puo'
restringere ulteriormente il rettangolo qualificato.

Si riusano i margini TM: 30 micrometri per sottosegmento nella lunghezza
e 16384epsilon a^2 per sottosegmento nell'area. Due errori angolari per
punto danno errore ECEF <=1.1a4096epsilon; i due estremi, la primitiva
geodetica e l'aritmetica restano sotto 30 micrometri. Nel dominio finito,
|x|<21750000 m e |y|<41000000 m (|r0/a|<1.5); interpolazione e
sottrazioni hanno errore normalizzato inferiore a 64epsilon. Composto
con 3/4/9/20 questo lascia margine nei budget comuni. Q, delta_lambda,
conversioni e somme sono coperti dal margine area comune. Come TM il
termine numerico cresce col raffinamento: tolleranze non dimostrabili
producono un errore, senza restituzione parziale.


### Prove LCC indipendenti e limite del risolutore WKT

[lcc.py](../crates/plenora-kernels-geo/tests/fixtures/preserve_shape/lcc.py)
implementa le formule di Snyder con bisezione isometrica e GeographicLib
Python 2.1, senza importare il motore Rust. La copia eseguibile e il pacchetto
sono sotto target/preserve-oracle. La fixture contiene una linea e un
poligono Lambert-93 EPSG:2154, a 16384 e 32768 divisioni; il confronto usa
il bound restituito senza tolleranze aggiuntive. Sono provati anche i
quattro bordi del rettangolo, apice e taglio, e il rifiuto di angoli alterati.

Manca la prova end-to-end su un secondo CRS LCC. Il risolutore WKT del
repository riconosce solo definizioni equivalenti al registro integrato:
`definizioni::riconosci` richiede un ID esistente e ne confronta tutti i
parametri; `Lato::nuovo` richiede poi la definizione statica del registro.
Una WKT arbitraria non fornisce quindi un secondo CRS riproiettabile.
Il registro EPSG non e' stato ampliato. Le prove locali dei due segni del
cono qualificano il certificato numerico, non sostituiscono quella prova
end-to-end mancante.
