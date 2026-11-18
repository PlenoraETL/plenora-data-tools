# Maggioranti delle inverse per preserve_shape

La tabella eseguibile e' il match esaustivo `Proiezione::shape_certificate`
in [proiezioni.rs](../crates/plenora-core/src/crs/riproiezione/proiezioni.rs).
`Riproiettore::inverse_shape_certificate` espone il risultato senza
cambiare datum. Il test in
[forma.rs](../crates/plenora-core/src/crs/riproiezione/forma.rs) costruisce
ogni definizione integrata e verifica l'insieme dei metodi; aggiungere una
variante invertibile senza classificarla causa anche un errore di compilazione.

## Tabella di qualificazione

B1 e B2 sono maggioranti delle derivate prima e seconda della latitudine
rispetto alla coordinata normalizzata psi. La longitudine e' affine.

| Metodo | Scala s | B1 | B2 | Stato e limite |
| --- | --- | --- | --- | --- |
| Geografico (0) | pi/180 | 1 | 0 | Certificato nel dominio geografico integrato |
| Pseudo Mercator (1024) | 1/a | 1 | 1 | Certificato sul ramo aperto, latitudine <85.06 gradi in modulo |
| Mercator A (9804) | 1/(a k0) | 1/(1-e^2) | (1+2e^2)/(1-e^2)^2 | Certificato sullo stesso ramo, controllo del residuo isometrico |
| Transverse Mercator (9807) | - | - | - | AccuracyUnproven: mancano maggioranti delle derivate e del resto della serie inversa di ordine 6 sul dominio dichiarato |
| Lambert Conic Conformal 2SP (9802) | - | - | - | AccuracyUnproven: non certificata la distanza continua dall'apice e dal taglio angolare |
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
inverse proiettate certificate il profilo richiede inoltre |lambda|<pi
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
aggiungervi una tolleranza campionaria. I cinque metodi non certificati
hanno prove del rifiuto; le loro prove numeriche C1 restano da realizzare
insieme ai maggioranti mancanti.
