# Buffer con proiezione locale

API additiva `geodesic_buffer::projected::geodesic_line_buffer_with_limits`.
Accetta LineString, MultiLineString, Polygon e MultiPolygon in un CRS
geografico risolto, con ellissoide e datum del chiamante. I punti conservano
le API geodetiche di [buffer-punto](buffer-punto.md). Sono implementate
dilatazioni, raggio zero ed erosioni certificate mediante differenza dai
corridoi del bordo e copertura continua di stabilita'.

## Scelta e dominio

Si sceglie deterministicamente Transverse Mercator, scala centrale uno,
origine equatoriale e false origini nulle; il meridiano centrale e' il
punto medio dell'inviluppo longitudinale del gruppo. Per unione per
distanza il gruppo comprende tutti gli ingressi per raggi positivi o nulli;
per raggi negativi si costruisce ciascuna erosione nel proprio gruppo. Questa scelta minimizza
la massima distanza longitudinale dal meridiano fra i vertici, fra i
centri rappresentabili in binary64. Non si
dichiara ottimalita' fra le famiglie di proiezioni. LAEA e stereografica
non sono candidate di questo profilo.

Il core riusa esattamente la propria TM, senza introdurre un CRS EPSG
fittizio, una trasformazione di datum o un motore esterno. Le conversioni
sono verificate mediante il residuo indipendente di preserveShape,
anche in avanti: l'inversa esatta della coordinata proiettata deve
coincidere con la coordinata geografica originale entro il residuo.

Dominio conservativo: 6e6<=a<=7e6, 250<=1/f<=350; vertici con
|lat|<50 gradi, |lon|<139 gradi, ampiezza longitudinale del gruppo <1
grado, ogni lato geodetico <=60000 m. Si controlla inoltre tutto il box
proiettato allargato: |y/a|<0.9 e |x/a|<0.03. Poli e antimeridiano sono
esclusi. Il dominio ristretto non copre 70 gradi.

Raggi finiti di modulo <=60000 metri. Zero produce Polygon vuoti per
linee e ingressi vuoti, mentre conserva le superfici non vuote validate.
Raggi negativi non sono ammessi per le linee. La tolleranza del chiamante
deve essere almeno 0.001 m e non ha un tetto artificiale di 1 cm:
la distorsione certificata e' una proprieta' dichiarata del metodo.
Il limite di 1 cm sul rounding delle booleane resta invariato.
Essere nel dominio non garantisce
che un raggio sia certificabile: il termine di distorsione puo' da solo
superare la tolleranza. In quel caso si restituisce AccuracyUnproven.

## Fattori metrici continui

Si riusano definizioni, fonti e derivate della
[TM esatta di preserveShape](maggioranti-forma.md#transverse-mercator-epsg-9807).
Qui z=y/a+i*x/a, w=m^-1(z), chi=psi(w), q=e^2. La conformalita' e
la metrica ellissoidale danno il fattore inverso terreno/piano

```text
b = cos(phi)/sqrt(1-q sin(phi)^2) * |sqrt(1-q sin(w)^2)/cos(w)|.
log b = l(phi) + Re F(w)
l(phi) = log cos(phi) - log(1-q sin(phi)^2)/2
F(w) = log(1-q sin(w)^2)/2 - log cos(w)
```

Sull'asse reale |phi|=|w|<0.9/0.978<0.921. Integrando il maggiorante
|phi_x|<4 fino a |Im z|<0.03 segue |phi|<1.041. Gli indici x in
questa derivazione indicano la coordinata normalizzata x_piano/a.
Per |phi|<1.041 e q<=0.008, |l'|<2 e |l''|<4: cos(1.041)>0.505,
e sec(phi)^2+q/(1-q)+2q^2/(1-q)^2<4 maggiora |l''|.

```text
F' = tan(w) - q sin(w)cos(w)/(1-q sin(w)^2)
F'' = sec(w)^2 - q(cos(w)^2-sin(w)^2)/(1-q sin(w)^2)
      - 2q^2 sin(w)^2 cos(w)^2/(1-q sin(w)^2)^2
```

Sul rettangolo esterno |Re w|<=1.21, |Im w|<=0.21 gia' qualificato,
cos(1.21)>0.35, cosh(0.21)<1.023 danno |F'|<3 e |F''|<8.2.
I maggioranti dell'arco meridiano danno |w'|<1.023, |w''|<0.028,
dunque |(F composto w)''|<9. Con le derivate 4 e 20 di phi:

```text
|(log b)_xx| < 4*16 + 2*20 + 9 = 113 < 128.
```

La simmetria rispetto all'asse implica b_x(y,0)=0; la scala centrale
unitaria implica b(y,0)=1. Per Taylor, |log b|<=64*(x/a)^2=t.
Per 0<=t<1, exp(-t)>=1-t e exp(t)<=1/(1-t), quindi

```text
b_min = 1-t; b_max = 1/(1-t).
```

Il codice racchiude z con intervalli e arrotonda t verso l'alto,
b_min verso il basso e b_max verso l'alto. Il dominio e' convesso e
il limite riguarda ogni punto del box, non un campionamento.
Inoltre |lambda-lambda0|<=3*|x/a|<0.09 radianti: l'immagine e i
segmenti lon/lat pubblicati stanno in una striscia larga meno di 0.181
radianti, inclusi i margini numerici, lontana da poli e seam. Tale striscia
contiene meno di mezzo ellissoide; non si costruisce un complemento
globale GOP-005.

I fattori diretti sono i reciproci. Questo particolare intervallo e'
chiuso rispetto ai reciproci, percio' maggiora anche i fattori diretti;
in generale non si possono usare indistintamente fattori avanti e inversi
nel termine radiale.

## Regione e confronto dei buffer pieni

Si allarga il bbox delle corde proiettate di R=2*r+1 metri in ciascuna
direzione e si impone b_min*R>r+input_model_m+0.0002. Un cammino geodetico lungo al massimo
r, partendo dall'operando, non puo' uscire dall'immagine del box: fino
al primo attraversamento costerebbe almeno b_min*R. L'inversa e'
iniettiva sul rettangolo per la prova di preserveShape. Per il buffer
planare esatto C_r dell'operando C vale

```text
(F(C))_(b_min*r) subset F(C_r) subset (F(C))_(b_max*r).
```

Il limite di Hausdorff degli insiemi pieni e' quindi
r*max(|1-b_min|,|b_max-1|). Questa prova non e' una garanzia della
distanza fra confini o dell'identita' dei contatti. Le modifiche sotto
il centimetro seguono la precisione geografica del motore. L'erosione usa invece la verifica continua descritta sotto.

## Cambi di modello e archi

Per una corda piana di lunghezza L1=|dx|+|dy|, le derivate inverse 9/20
e il resto d'interpolazione danno lo scarto metrico rispetto al segmento
lon/lat dei suoi estremi:

```text
8e6*(9+20)/(8*a^2)*L1^2 < 1e-6*L1^2.
```

Le coordinate intermedie sono nel dominio terrestre ristretto. Per
l'ingresso si aggiunge il [lemma geodetico](buffer-punto.md#lemma-implementato-del-cambio-di-modello-dei-lati)
8e-6*h^2, dove h maggiora la distanza fra campioni. L'ingresso viene
suddiviso con direct di Karney; il limite usa la lunghezza divisa per n
piu' un margine per gli estremi, senza dividere quel margine per n.
La pianificazione dell'ingresso usa min(tolleranza,0.5 m); si verifica
inoltre input_model_m<0.01 m per ogni risultato non nullo. Il margine
geometrico del box rimane quindi maggiore dell'errore di ingresso anche
quando il chiamante ammette una grande distorsione. Il massimo degli
errori dei lati maggiora il cambio dell'intero insieme.
Gli intorni metrici di insiemi vicini conservano tale limite di Hausdorff.

Il disco planare usa la parametrizzazione razionale
((1-t^2)/(1+t^2),2t/(1+t^2)), t=i/n, e quattro rotazioni esatte di
quadrante. L'angolo e' 2 atan(t), con derivata <=2: ogni intervallo ha
ampiezza <=2/n, anche alle giunzioni. La freccia e' al piu' r/(2*n^2).
Il poligono inscrive il disco, e il bound vale anche sui pieni.
Si sceglie una potenza di due n prima di costruire i dischi. I rettangoli
dei lati e i dischi dei vertici sono uniti con la 1c-c.

L'output planare viene suddiviso finche' il limite 1e-6*L1^2 e' entro
il budget assegnato. Dopo inverse certificate si pubblicano segmenti
lon/lat. L'omotopia d'interpolazione dei confini e il numero di avvolgimento
estendono lo scarto agli insiemi pieni; la 1c-c valida la superficie
pubblicata. Una topologia non certificabile viene rifiutata.

## Numerica e composizione

La base numerica Karney resta quella qualificata per il disco:
[accuratezza delle primitive](https://geographiclib.sourceforge.io/html/python/interface.html#restrictions-on-the-parameters),
con inviluppo di 0.1 micrometri per direct o inverse nel dominio
ellissoidale del profilo. Non si presume una precisione dell'azimut
dall'errore della sola distanza inverse.

Per ogni lato, direct(A,azimut_inverse,L_inverse) viene confrontata con
B tramite inverse. Un residuo misurato >1 micrometro rifiuta. Gli errori
di entrambe le primitive portano lo scarto dell'estremo esatto sotto
1.2 micrometri. Nella palla locale di raggio 60001 metri il campo di
Jacobi soddisfa 0.999*s<j(s)<=s, come nella prova del disco. La mappa
exp_A(t log_A(B)), per 0<=t<=1, ha norma differenziale <1.002 rispetto
all'estremo B: nella direzione radiale vale t, in quella trasversale
j(tL)/j(L)<1/0.999. Integrando sul breve segmento fra i due estremi,
anche i campioni intermedi restano entro 3 micrometri, inclusa direct.
Questo certifica il contributo dell'azimut tramite un residuo verificato.

Il residuo TM limita ciascuna ordinata angolare a 2048 epsilon rad:
8e6*4096 epsilon <8 micrometri per punto. Il margine fisso 200 micrometri
copre i campioni geodetici, due estremi di ciascun cambio di modello,
conversioni, interpolazioni, normali dei rettangoli e cerchi razionali.
Per questi ultimi |XY|<7e6, r<=60000, n<=2^31 e operazioni binary64
normali danno errori molto inferiori al micrometro; la norma dei lati
rifiuta quadrati subnormali o non finiti. Il margine include anche
l'unica validazione/ricomposizione finale in lon/lat (<4 nanometri).
Non costituisce una prova formale di tutta la libm o del port Karney.

Ogni unione piana aggiunge 2e-9 metri al numerico: |XY|<7e6 implica
mezzo ulp <=2^-31 per ordinata, e b_max<1.062. I contributi si sommano
verso l'alto. Gli errori degli operandi si compongono con il massimo,
come nel lemma dell'unione del disco. Il certificato finale somma
distorsione, cambio in ingresso, archi, cambio in uscita, numerico e
`stability_m` (zero nelle dilatazioni);
verifica di nuovo la tolleranza prima della pubblicazione. Il relativo
e' il totale/r, arrotondato in alto e finito.

## Risorse e interruzione

Un solo Work con precedenza della deadline governa prevalidazione,
generazione, residui, booleane, esportazioni e distanze multiple. Ogni
residuo ha un numero fisso e limitato di operazioni, con checkpoint
intorno alla conversione; nessuna promessa di latenza wall-clock.
Le raccolte usano CountedVec e le ricevute di esportazione restano
addebitate fino al termine del batch. I limiti ed esclusioni della
memoria razionale sono quelli della 1c-c. Nessun prefisso viene pubblicato.

Le distanze sono esterne nell'ordine del batch; l'unione avviene solo
fra ingressi della medesima distanza. Il conteggio dei risultati e'
checked. Il limite dei vertici conta campioni proiettati, primitive
planari e campioni pubblicati, incluse chiusure; nodi e lati creati dalla
1c-c hanno i propri contatori cumulativi nello stesso Work.

## Superfici e validazione locale

Le superfici richiedono anche ampiezza latitudinale <1 grado e il filtro
continuo di `projected/surface.rs`. Si valida la topologia planare originale
con la 1c-c e si dimostra che i lati geodetici hanno la stessa topologia.
Per ciascun lato di lunghezza maggiorata h<=60000 m si usa
D=8e-6*h^2+20 micrometri, con arrotondamenti verso l'esterno. Il lemma del
cambio di modello copre tutta l'omotopia fra geodetica e segmento lon/lat.
Per ogni coppia di lati non adiacenti, e per ogni coppia di anelli distinti,
la separazione degli AABB in coordinate deve superare D_i+D_j.
Il minorante metrico e' 50000 metri per grado per il massimo dei gap sugli
assi. Non si inferisce separazione da campioni o dalla sola validita' OGC.

Il bbox <1 grado per 1 grado offre fra due suoi punti un cammino lon/lat
di lunghezza <198 km, usando M,N<8e6 m. La minima geodetica resta quindi
in |lat|<53 gradi, dove M e N*cos(phi), convertiti in metri/grado, sono
entrambi >50000. Le geodesiche dei lati adiacenti sono nella stessa palla
fortemente convessa di raggio <200 km e non possono incontrarsi due volte.
Si esclude la sovrapposizione usando gli azimut dal vertice comune,
qualificati mediante residui direct/inverse e stabilita' del campo di Jacobi.
Il margine angolare e' 8 micrometri/min(lunghezze)+1e-8 gradi, con il primo
termine convertito da radianti. Le omotopie disgiunte fra anelli conservano
la collocazione dei buchi; il dominio locale esclude complementi globali.

Il filtro e' deliberatamente sufficiente, non necessario: AABB sovrapposti
possono far rifiutare anche un poligono valido. Non si aggiusta la geometria.
La validazione usa CountedVec e il Work del batch, con checkpoint per lato,
confronto e primitiva geodetica. Non usa un secondo bilancio di risorse.

La dilatazione costruisce i corridoi di tutti gli anelli, buchi compresi,
e unisce anche gli operandi proiettati. Il pieno originale non viene perso.
Per r=0 si restituiscono le coordinate e il tipo della superficie non vuota,
validati, senza roundtrip nelle coordinate pubblicate; certificato assoluto
e relativo zero. `preserves_geodesic_operand=true` dichiara che i lati restano
geodetici: la costante OUTPUT_MODEL descrive invece i lati costruiti per
raggio positivo. Gli ingressi vuoti producono sempre Polygon vuoto GOP-008.
A zero si validano separatamente le parti e si uniscono con le booleane
poligonali, sempre con lo stesso Work. Le unioni intermedie si applicano
tra Polygon: le componenti a contatto puntuale non vengono reinserite
come un MultiPolygon che il profilo booleano rifiuterebbe. Nessun roundtrip
proiettivo e nessun nuovo modello dei lati viene imposto all'operando.
Per un singolo operando non ricomposto, coordinate e tipo restano identici.

Il filtro di separazione ammette anche estremi comuni e sottoarchi esatti
su meridiani o sull'equatore. Al contatto, i raggi incidenti distinti devono
conservare l'ordine circolare fra vettori lon/lat e azimut geodetici.
Gli azimut sono qualificati da residui direct/inverse e dal margine
8 micrometri/lunghezza gia' usato per gli adiacenti. Il margine additivo
1e-8 gradi copre atan2, conversioni e sottrazioni angolari binary64 nel
dominio dei lati qualificati; non e' una prova formale di libm.
Raggi coincidenti sono deduplicati solo per lo stesso altro estremo oppure
per la stessa direzione su meridiano/equatore. Gli intervalli delle altre
direzioni devono essere disgiunti. Questo conserva i settori al contatto;
la convessita' forte esclude ulteriori incontri fra archi con estremo comune.

Ogni lato dell'unione deve coincidere con un lato originale (eventualmente
invertito), o essere un suo sottoarco meridiano/equatoriale esatto. Sono
quindi ammessi contatti puntuali, lati comuni, inclusioni, duplicati e
contatti a T qualificati su tali assi. Nuove intersezioni o sottoarchi
obliqui che richiedano una ricostruzione geodetica restano AccuracyUnproven:
una booleana lon/lat non viene dichiarata esatta per archi geodetici diversi.
Questi sono limiti del certificatore locale, non un'interpretazione diversa
di GOP-008. Il certificato a zero resta zero e il modello geodetico originale
resta dichiarato da `preserves_geodesic_operand`.

## Erosione e collasso certificati

Per r negativo, con rho=abs(r), si sottrae dall'operando proiettato l'unione
dei corridoi di tutti gli anelli, buchi compresi. La differenza e tutte le
sue fasi usano le booleane con i limiti del chiamante e lo stesso Work.
Si pubblica soltanto dopo la verifica di stabilita' descritta sotto.
Per unione per distanza si erode ciascun ingresso separatamente, poi si
uniscono i risultati: erosione e unione non commutano. Le componenti del
certificato si combinano con il massimo, aggiungendo il rounding dell'unione.

Resta disponibile la scorciatoia D=b_max*diametro_L1(box)+input_model_m+0.0002,
con arrotondamenti in alto. Il confine geodetico e il pieno locale stanno
nella palla fortemente convessa di raggio D centrata su un vertice. Se
D<rho<=60000, ogni punto dista meno di rho da quel vertice del confine:
l'erosione e' vuota, con certificato zero. Gli altri collassi passano
attraverso la differenza e la copertura, come le erosioni non vuote.

### Involucri mediante distanza firmata

Sia P il pieno proiettato, R il risultato effettivo della differenza
binary64 e s_P la distanza euclidea firmata dal bordo, positiva nel pieno.
Le parti proiettate sono validate come MultiPolygon disgiunto prima della
differenza: nessuna frontiera interna puo' sparire per fusione delle corde.
Il bound delle frontiere segue quindi anello per anello.
L'omotopia dei bordi conserva l'orientamento del pieno. Posto
eta=input_model_m+numerical_m, lo scarto dei bordi in metri a terra e' <=eta.
Quando i segni coincidono, le distanze dal bordo differiscono al piu' eta;
quando differiscono, ogni distanza e' <=eta per la separazione dell'omotopia.
Si usa dunque conservativamente e=2*eta, non il solo bound Hausdorff dei pieni.
I fattori metrici continui e la barriera del box danno, con arrotondamenti
verso l'esterno:

```text
lower = (rho-e)/b_max
upper = (rho+e)/b_min
{s_P > upper} subset F^-1(erosione_esatta) subset {s_P >= lower}.
```

Si richiede lower>0; altrimenti AccuracyUnproven. Ne segue che l'erosione
esatta e' contenuta in P, quindi nel bbox radice della verifica. La radice
comprende anche R. La distanza firmata e' 1-Lipschitz in coordinate piane.
La soglia superiore stretta produce testimoni interni all'erosione esatta.
Non si assume la stabilita' dell'erosione rispetto a perturbazioni arbitrarie.

### Copertura continua e testimoni comuni

`projected/erosion.rs` calcola intervalli diretti IEEE delle distanze ai
segmenti. Prodotto scalare, parametro clamped, proiezione e radice quadrata
sono arrotondati all'esterno. Il ray crossing usa intervalli del determinante;
un segno incerto allarga l'intervallo firmato a entrambi i segni, mai sceglie
un lato. Denominatori non positivi, reciproci non finiti e distanze non
finite vengono rifiutati. Il vuoto ha distanza firmata meno infinito.

Ogni cella rettangolare ha centro c e maggiorante euclideo h arrotondato in alto della distanza da c.
Gli intervalli di s_P(c) e s_R(c), allargati di h, consentono tre prove:

- entrambi gli insiemi sono assenti dalla cella (s_P+h<lower e s_R+h<0);
- tutta la cella e' interna a entrambi (s_P-h>upper e s_R-h>0);
- un testimone w, con s_P(w)>upper e s_R(w)>0, copre tutta la cella entro
  stability_m/b_max. Si usa la distanza massima dai vertici della cella a w, con quadrati,
  somma e radice arrotondati verso l'alto.

Nel terzo caso ogni punto di ciascuno dei due insiemi e' vicino a un punto
dell'altro. Il trasporto con b_max prova Hausdorff<=stability_m sui pieni.
Le celle non decise si bisecano sull'asse maggiore; a scala
stability_m/(128*b_max), o se non e' possibile bisecare in binary64, si
rifiuta esplicitamente. La ricerca usa 256 direzioni distinte nel piano anche nei settori acuti.
La normalizzazione dei candidati non e' un assunto numerico del certificato:
si verifica la distanza effettiva del punto binary64 costruito.
La ricerca finita di testimoni non e' la prova:
la prova e' la disuguaglianza continua sull'intera cella. Il limite puo'
rifiutare geometrie valide, specialmente vicino a un collasso critico.

Lo stack e' CountedVec; confronti, lati, testimoni e suddivisioni hanno
checkpoint sul medesimo Work. Se R e' vuoto, non esistono testimoni comuni:
il successo richiede che ogni cella sia esclusa dall'erosione esatta. Cio'
certifica anche il collasso totale. Lo stesso criterio impedisce che una
componente remota sparisca senza controllo nel collasso parziale.

Si assegna stability_m=3*tolleranza/4 e si limita il budget degli archi a
tolleranza/32. Il certificato somma anche distorsione, cambio di modello,
archi e numerico: alcuni termini sono gia' assorbiti dalla verifica, ma
la somma e' conservativa. L'output viene poi pubblicato con il consueto
bound di cambio di modello e rounding. La tolleranza totale si ricontrolla
prima della restituzione atomica; nessun prefisso del batch viene pubblicato.

## Raggi grandi e latitudini escluse

A 10 km il tetto precedente di 1 cm sulla tolleranza totale impediva il
successo pur essendo il box nel dominio. A 60 km il margine 2*r+1 superava
anche |x/a|<0.01. Il dominio dimostrato <0.03 e la tolleranza esplicita
risolvono questi impedimenti per le geometrie locali delle prove.
Le prove usano fino a 2000 m a r=60000 m per questo maggiorante conservativo;
non dichiarano un'accuratezza centimetrica del buffer a tale raggio.

A 70 gradi la TM resta fuori dai maggioranti della coordinata meridiana.
Partizionare cambiando meridiano centrale non riduce tale coordinata;
cambiare origine latitudinale non aggira il limite, perche' il target del
certificato aggiunge m(phi0). Anche LAEA e stereografica qualificate hanno
origini ristrette. Senza ulteriori dimostrazioni i casi a 70 gradi restano
UnsupportedSurface. Non e' implementata una ricomposizione multi-proiezione.
