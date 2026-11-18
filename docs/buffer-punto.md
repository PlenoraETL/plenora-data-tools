# Buffer geodetico di un punto

API additiva `geodesic_buffer::point::geodesic_point_buffer_with_limits`.
Non cambia il profilo vuoto `geodesic_empty_buffer_with_limits`.
Accetta `Point` e `MultiPoint` non vuoto con coordinate tutte identiche.
Non unisce risultati: ordine distanza, poi ingresso, duplicati conservati.
Linee, superfici e multipunti distinti restituiscono `UnsupportedProfile`.
Il CRS geografico risolto fornisce datum ed ellissoide; nessun default Esri.

## Modello e dominio

Il disco esatto e' `{X: d(P,X)<=r}` nella metrica dell'ellissoide.
Il confine campionato usa `direct(P,-360*k/n,r)` di Karney.
Il segno meno produce un esterno antiorario; si copia il primo vertice
alla fine. Nessun buco. Il certificato copre sia i lati come geodetiche
minime sia i segmenti lineari lon/lat consumati dalla 1c-c. La metrica
dell'errore resta geodetica in entrambi i casi; il campo `MODEL` identifica
questa metrica. Il termine `model_change_m` copre il passaggio fra i lati.

Dominio ammesso, deliberatamente piu' piccolo di GOP-005:

- semiasse `6e6 <= a <= 7e6` metri, `250 <= 1/f <= 350`, oblato;
- `1 <= r <= 60000` metri;
- centro con `|lat| <= 80` gradi;
- `(180-|lon|)*15000 > r+1`, con lato sinistro arrotondato in basso
  e destro in alto nel codice.

Fuori dominio: `UnsupportedEllipsoid` o `UnsupportedSurface`.
Anche molti dischi innocui prossimi ai poli o al seam sono rifiutati:
non si pretende un test necessario e sufficiente di attraversamento.
Calotte contenenti poli e attraversamenti dell'antimeridiano sono esclusi.
Raggi non finiti, zero e negativi: `InvalidRadius`; il profilo degenere
resta disponibile separatamente. Non vengono ricostruiti complementi.

## Maggiorante prima della costruzione

Si pone `R=5800000 m`. I due raggi principali di curvatura dell'ellissoide
sono almeno `a(1-f)^2 > R`; dunque `0<K<1/R^2`. Il raggio di iniettivita'
e' almeno `pi R`: il bound di Klingenberg e' il minimo fra il raggio
coniugato (almeno `pi R` per confronto) e meta' della piu' corta geodetica
chiusa. Quest'ultima, come curva nello spazio, ha curvatura al piu' `1/R`;
Fenchel impone curvatura totale almeno `2pi`, quindi lunghezza almeno
`2pi R`. I dischi qui usati sono lontani da entrambi i limiti.
Fonti: [Klingenberg e confronto, capitolo 14, Gallier e Quaintance](https://www.cis.upenn.edu/~cis6100/cis610-18-sl14.pdf),
[Fenchel, richiamato in Miura 2023](https://doi.org/10.1007/s00526-023-02559-7).

La variazione rispetto all'azimut e' un campo di Jacobi trasversale:
`j''=-Kj`, `j(0)=0`, `j'(0)=1`. Prima del punto coniugato `j>0`,
quindi `j''<=0` e `j(s)<=s`. Di conseguenza un tratto di confine
di ampiezza angolare `delta` ha lunghezza al piu' `r*delta`.
La relazione fra variazione di azimut e lunghezza ridotta e' anche
esplicitata da [GeographicLib, Geodesic](https://geographiclib.sourceforge.io/html/java/net/sf/geographiclib/Geodesic.html).

Siano `A,B` campioni esatti consecutivi, `A',B'` quelli numerici,
con `d(A,A'),d(B,B')<=e`. Sia `L<=2pi r/n` la lunghezza del tratto
esatto. Ogni punto di questo tratto dista al piu' `L/2+e` da uno
dei due estremi numerici. Viceversa, il lato minimo `A'B'` ha lunghezza
al piu' `L+2e`; ogni suo punto dista al piu' `L/2+e` da un estremo
numerico, e al piu' `L/2+2e` dal corrispondente estremo esatto.
Unendo gli archi nei due versi:

`d_H(confine esatto, anello numerico) <= pi*r/n + 2e < 4*r/n + 2e`.

Non si usa la freccia di una circonferenza sferica come se l'ellissoide
fosse una sfera. Questo bound e' O(1/n), conservativo ma direttamente
verificabile; non deriva dalla sola convergenza di una prova campionata.
La scelta di `n` precede qualsiasi allocazione delle coordinate.

## Topologia e disco pieno

Per confronto, `j(s)>=R*sin(s/R)>.999s` fino a 60001 metri.
La palla `B(P,60001)` e' fortemente convessa: il suo raggio e' sotto
`min(inj/2, pi/(2 sqrt(Kmax)))`, qui almeno `pi R/2`.
Una geodetica radiale estesa separa la palla in due semipalle convesse:
un lato che uscisse e rientrasse avrebbe due intersezioni con essa,
contraddicendo l'unicita' della geodetica fra quei punti. Quindi
i settori fra geodetiche radiali consecutive, di angolo minore di pi,
sono convessi nella palla. Un lato fra due raggi attraversa ogni raggio
intermedio una sola volta: due intersezioni darebbero due geodetiche
distinte fra gli stessi punti nella palla convessa.

La perturbazione di un vertice di al piu' `e=1.1e-6 m` cambia il suo
azimut di meno di `2e/r` (si integra la metrica polare lungo il piccolo
segmento perturbato, dove il raggio resta almeno `r-e`). Il primo `n`
che soddisfa il budget effettivo `T>=1e-4 m` rispetta
`r/n > 1.2e-5 m`: senza arrotondamenti il fallimento del candidato
precedente darebbe `(T-2.2e-6)/8 >= 1.2225e-5 m`. Con il termine di
cambio modello, se r/n fosse <=1.2e-5, il candidato precedente avrebbe
discretizzazione <=9.6e-5, numerico 2.2e-6 e cambio modello <4e-13 metri:
sarebbe gia' accettato. Gli arrotondamenti diretti a questa scala costano
meno di 1e-17 metri, assorbiti dal margine. Il candidato iniziale n=16 non
puo' riuscire, dato r>=1 e T<=0.01.
Quindi `2pi/n > 4e/r`: l'ordine angolare dei vertici non puo' invertirsi,
e ogni passo e' molto minore di pi. I lati occupano settori consecutivi
disgiunti, l'anello e' semplice, antiorario e stellato rispetto a P.
La chiusura usa una copia esatta, senza una seconda direct.

Per ogni punto del lato il bound precedente implica `|d(P,X)-r|<=B`.
Ogni raggio incontra il lato una sola volta: i due dischi pieni differiscono
solo fra distanze radiali che distano al piu' B. Percorrendo quel raggio
si ottiene anche `d_H(disco esatto, disco numerico)<=B`.
Il certificato non dichiara inclusione unilaterale: gli arrotondamenti
possono collocare vertici da entrambi i lati del confine esatto.

L'area esatta e' al piu' `pi*r^2`, integrando `j<=s` in coordinate polari;
quella del disco numerico e' al piu' `pi*(r+B)^2`. L'area totale
dell'ellissoide e' almeno `4pi*R^2` (Gauss-Bonnet e `K<=1/R^2`).
Entrambi i dischi sono dunque strettamente sotto mezzo ellissoide.

Infine `M>=R` limita lo spostamento in latitudine a meno di 0.6 gradi.
Con |lat|<81, `N*cos(lat)*pi/180 > 15000 m/grado`: la barriera
del dominio impedisce di raggiungere il seam lungo qualunque raggio
di lunghezza al piu' `r+1`. I lati rimangono nella stessa palla convessa.

## Certificato numerico GOP-006

Il certificato riporta CRS/datum, `shortest_geodesic`, metri,
trasformazione zero, discretizzazione `next_up(4*r/n)`,
componente GeographicLib `2e-7 m`, arrotondamenti `2e-6 m` e
`model_change_m`, calcolato come descritto sotto.
La somma assoluta B usa `next_up` dopo ogni addizione; il relativo
e' `next_up(B/r)`. Sono due descrizioni dello stesso bound, non due
budget da sommare; il relativo e' normalizzato al raggio richiesto.

La componente di primitiva usa un inviluppo di 0.1 micrometri per vertice.
[GeographicLib, restrizioni dei parametri](https://geographiclib.sourceforge.io/html/python/interface.html#restrictions-on-the-parameters)
riporta circa 25 nm per |f|=0.01 al semiasse WGS84; la documentazione
Java dichiara meglio di 15 nm per WGS84. Qui f<=0.004 e a<=7e6:
25 nm scalati per 7e6/6378137 sono meno di 28 nm. L'inviluppo 100 nm
e' conservativo rispetto a questa accuratezza della primitiva, con
ulteriore margine di 1 micrometro per vertice nella costruzione.
Il port usato e' `geographiclib-rs =0.2.7`, gia' fissato dal workspace;
la feature `accurate`, disabilitata, riguarda le somme delle aree, non direct.
Le prove includono vettori numerici indipendenti dalla composizione
direct/inverse, oltre al campionamento del disco. Non si afferma una
verifica formale di ogni operazione interna del port o della libm:
la base numerica e' l'accuratezza pubblicata di G1/G2, come per le altre
API geodetiche del crate, non un nuovo algoritmo esatto di geodesia.

`n` e' una potenza di due rappresentabile in u32. Gli interi `k`,
`360*k` e la divisione per n sono esatti in binary64: nessuna
accumulazione incrementale di azimut. L'errore della rappresentazione
lon/lat e delle conversioni della primitiva e' coperto dall'inviluppo;
1 micrometro supera ampiamente una ulp angolare nel dominio terrestre.
Il fattore 4 al posto di pi lascia inoltre margine nella maggiorazione.

Il budget effettivo e' `min(tolerance_m,0.01)`; tolleranze non positive
o non finite sono invalide. Sotto 0.1 mm si restituisce
`AccuracyUnproven`, preservando la separazione angolare usata nella prova.
Se il numero richiesto non entra in u32: lo stesso errore.
Se eccede un budget effettivo del chiamante: `VertexLimit`, `MemoryLimit`
o `WorkLimit`. Il bound conservativo puo' richiedere molti vertici:
1 km a 1 cm usa 524288 lati, piu' la chiusura.

## Risorse e interruzione

Due passate, senza cache intermedia: prevalidazione/conteggio e generazione.
Tutti i conteggi sono checked. La memoria e' la somma degli slot richiesti
per `PointBufferDisk` e `Coord`, come nel contratto di densify;
overhead dell'allocatore, input e stack costante sono esclusi.
Si prenotano capacita' esatte richieste, dopo il controllo dell'intero batch;
nessuna crescita implicita durante i push, ne' copia di anelli alla chiusura.
L'allocatore puo' riservare memoria fisica addizionale, fuori da questo budget.
I fallimenti di prenotazione sono `Allocation`.

Il lavoro conta raggi, letture geometriche, coordinate, controlli dei
settori planari e checkpoint di costruzione/assemblaggio.
La pianificazione ha al piu' 28 raddoppi,
senza primitive geodetiche. Ogni direct e' preceduta da un checkpoint;
ce n'e' uno anche dopo l'ultima direct e prima della pubblicazione.
La deadline precede la cancellazione allo stesso checkpoint.
Un errore distrugge il risultato parziale; nessun prefisso viene restituito.
Le prove iniettano deterministicamente entrambi i segnali nella funzione
di generazione di produzione, dopo 511 vertici, senza gare fra thread.

## Maggiorante laterale per una linea: risultato matematico, non API

Il campionamento dei corridoi e la loro unione non sono implementati.
La seguente derivazione isola la componente di discretizzazione; non
costituisce un certificato numerico o topologico di un buffer di linea.

Sia gamma(s) una geodetica parametrizzata per lunghezza e sia n(s) la
normale unitaria tangente all'ellissoide, trasportata parallelamente.
Si consideri F(s,t)=exp_gamma(s)(t*n(s)). In coordinate di Fermi la
metrica e' dt^2+j(s,t)^2 ds^2. La proposizione 5.1 di
[Urs Lang, Differential Geometry I, pagine 29-30](https://people.math.ethz.ch/~lang/DG1_9Jan2025.pdf)
da' j_tt=-K*j, j(s,0)=1 e j_t(s,0)=0. La costruzione e' locale;
l'assenza di punti focali non implica da sola iniettivita' globale.

Con R=5800000 metri e 0<K<=1/R^2, finche' j e' positivo vale
j_tt<=0 e dunque j<=1. Inoltre
j(t)=1-integrale_0^t (t-u)*K(u)*j(u) du >= 1-t^2/(2R^2).
Un primo zero prima di 60000 metri contraddirebbe questa disuguaglianza.
La stessa prova si applica all'altro verso della normale. Quindi il
tratto laterale s -> F(s,+r), oppure F(s,-r), corrispondente a un
intervallo di lunghezza h ha lunghezza al piu' h.

Supponiamo ora che ciascun campione numerico abbia errore metrico <=e
rispetto a F(s,+r). Ogni punto dell'arco esatto dista <=h/2+e da uno
dei due campioni. La geodetica minima fra i campioni ha lunghezza
<=h+2e; ogni suo punto dista <=h/2+e da un campione e <=h/2+2e
dal corrispondente estremo esatto. Ne segue il bound simmetrico
**d_H(arco laterale, corda geodetica) <= h/2+2e**.
La prova usa soltanto lunghezze e disuguaglianza triangolare, e non
una freccia sferica applicata all'ellissoide.

Questo risultato lascia da verificare nell'implementazione:

- e deve coprire inverse iniziale, posizione e azimut lungo la linea,
  direct normale e arrotondamenti; non si puo' riusare automaticamente
  il margine del disco, che richiede una sola direct;
- il tubo normale deve rappresentare il vicinato metrico del segmento
  nel dominio ammesso, con i dischi terminali e senza identificazioni
  non verificate; i soli estremi entro 80 gradi non provano che l'intera
  geodetica vi rimanga;
- il bound sul confine deve essere esteso alle regioni piene e alla
  pubblicazione, preservando la topologia dichiarata;
- h, numero dei campioni e somme degli errori devono essere maggiorati
  con arrotondamenti diretti e conteggi checked prima delle allocazioni.

## Composizione con l'unione planare: garanzie distinte

Per insiemi non vuoti A_i e B_i nello stesso spazio metrico, se
d_H(A_i,B_i)<=e_i, allora d_H(unione A_i,unione B_i)<=max_i e_i.
Infatti ogni punto di una delle unioni appartiene a un operando e ha
un testimone nel corrispondente operando dell'altra unione. L'argomento
si applica nei due versi. Per un'ulteriore approssimazione U dell'unione
dei B_i con bound E, la disuguaglianza triangolare da' max_i e_i+E.

L'API `polygon_boolean::union_with_limits` certifica un'unione nel
piano delle coordinate. Il suo rounding dei nodi e' nearest-even,
con al piu' mezza ulp per ordinata; la verifica della pubblicazione
riguarda segmenti planari. Non dichiara un E per la sostituzione dei
lati con geodetiche minime. Quella componente deve essere dimostrata
e composta sia in ingresso sia in uscita dall'overlay. Convertire
soltanto mezza ulp da gradi a metri non copre il cambio dei lati.

Un limite di Hausdorff non certifica da solo i contatti. Gia' nel piano,
due dischi esternamente tangenti possono essere approssimati con
poligoni inscritti arbitrariamente accurati ma disgiunti, scegliendo
campioni che non comprendono il punto di tangenza. L'unione esatta
e' connessa; quella approssimata ha due componenti. Occorre quindi
verificare separatamente la topologia promessa e dichiarare l'eventuale
trattamento delle feature sotto la precisione del motore. La validita'
OGC dell'unione planare non e' tale verifica per il modello geodetico.

Il cambio dei lati in ingresso e' coperto dal lemma seguente e dal
certificato del disco. Restano da implementare l'unione cumulativa,
la sua composizione numerica e la qualifica dei contatti. Non viene
ancora esposto un buffer di multipunti distinti.

## Lemma implementato del cambio di modello dei lati

`side_model_error_m(h)` maggiora la distanza fra la geodetica minima
di lunghezza <=h e l'interpolazione lineare delle sue coordinate lon/lat.
Premesse: ellissoide del profilo, h<=60000 metri, estremi entro 81 gradi
e arco senza attraversamento dell'antimeridiano. Questo include il caso
richiesto con estremi entro 80 gradi e tutti i lati generati dal disco.
Il bound e' **8e-6 h^2 metri**, arrotondato in alto a ogni prodotto.
Zero restituisce zero; h negativo, non finito o fuori dominio produce
`AccuracyUnproven`. La funzione e' interna: le premesse geometriche
sono garantite dal dominio del disco, non dedotte dal solo h.

Fonte della metrica ellissoidale e delle equazioni differenziali:
[R. H. Rapp, Geometric Geodesy, Part I, equazioni 4.74-4.77](https://feng.stafpu.bu.edu.eg/Surveying%20Engineering/4251/crs-18763/Files/Rapp_Geom_Geod_Vol_I.pdf).
La seguente stima uniforme e' una derivazione del profilo, non una
costante di accuratezza attribuita a GeographicLib.

In radianti, poniamo q=f(2-f), M=a(1-q)/(1-q sin^2(phi))^(3/2),
N=a/sqrt(1-q sin^2(phi)), L=N cos(phi). La metrica e'
ds^2=M^2 dphi^2+L^2 dlambda^2. Per una geodetica a velocita' unitaria,
le equazioni di Eulero-Lagrange dell'energia danno

```text
phi'' = -(M'/M) phi'^2 + (L L'/M^2) lambda'^2
lambda'' = -2 (L'/L) phi' lambda'
M^2 phi'^2 + L^2 lambda'^2 = 1
```

Gli apici su M e L indicano derivazione rispetto a phi, quelli sulle
coordinate rispetto alla lunghezza. M>=R=5800000 limita la variazione
di latitudine lungo l'arco a h/R<0.6 gradi: l'intero arco sta entro 82.
Nel dominio q<0.008, M,N<8e6, cos(82 gradi)>1/8;
quindi L>R/8, |phi'|<=1/R e |lambda'|<=8/R.
Dalle formule esplicite,
|M'/M|<=3q/(1-q)<0.025 e
|L'/L|<=q/(1-q)+tan(82 gradi)<8.
Usando L^2 lambda'^2<=1 nella prima equazione si ottiene
|phi''|<9/R^2; dalla seconda |lambda''|<=128/R^2.

Per una funzione due volte derivabile su [0,l], il resto rispetto
all'interpolante dei due estremi e' limitato da
sup|u''| s(l-s)/2 <= sup|u''| l^2/8. Si dimostra sottraendo
all'errore le due parabole +/-sup|u''| s(l-s)/2 e applicando
convessita'/concavita' con valori zero agli estremi.
Applicato alle coordinate, limita gli scarti a 9h^2/(8R^2) e
128h^2/(8R^2). Il segmento che collega in coordinate i due punti
allo stesso parametro resta entro 82 gradi e ha lunghezza metrica
<=8e6(|delta_phi|+|delta_lambda|). Ne segue

```text
d(geodetica(t), segmento_lonlat(t))
  <= 8e6 * 137 * h^2 / (8 * 5800000^2)
  < 8e-6 * h^2.
```

La corrispondenza parametrica e' suriettiva su entrambe le curve:
il bound vale per Hausdorff in entrambi i versi. Vale anche per tutti
i punti dell'omotopia lineare in coordinate fra le due curve, perche'
gli scarti intermedi sono frazioni degli stessi scarti maggiorati.
Il fattore 8e-6 e ogni prodotto sono arrotondati con `next_up`;
non occorre valutare funzioni trigonometriche nella funzione Rust.

## Dal lemma al disco planare certificato

La pianificazione usa h=8r/n+2.2e-6, con arrotondamenti verso l'alto:
8>2pi e il termine numerico copre entrambi gli estremi. Somma
`model_change_m=side_model_error_m(h)` alle altre componenti prima di
scegliere n. Il termine decresce con h; non viene aggiunto a posteriori
a un certificato che abbia gia' consumato tutta la tolleranza.

Prima della pubblicazione `verify_planar_fan` verifica con il kernel
di orientamento esatto che ogni coppia consecutiva formi con il centro
un settore strettamente antiorario di angolo minore di pi. Conta inoltre
un unico attraversamento crescente della semiretta est. I settori
percorrono quindi un solo giro, hanno interni disgiunti e ogni lato
interseca una volta ciascuna semiretta interna al proprio settore.
L'anello planare e' semplice, antiorario e stellato. Non basta verificare
solo gli orientamenti: un anello che percorre due giri viene rifiutato.
Ogni lato controllato ha un checkpoint e un addebito di lavoro.

Per estendere il lemma agli insiemi pieni, si usa l'omotopia dei confini.
Un punto a distanza maggiore di D=8e-6 h^2 da uno dei due confini non
puo' essere attraversato dall'omotopia, contenuta nel suo D-intorno.
Il numero di avvolgimento non cambia. Per i due dischi semplici,
un punto interno al primo ed esterno al secondo deve dunque distare
al piu' D dal secondo, e viceversa. La disuguaglianza triangolare con
il bound del disco geodetico da' il certificato totale pubblicato.

Le prove C1 campionano 129 parametri, incluso il punto medio, per
due ellissoidi estremi, cinque latitudini, sei azimut e quattro lunghezze
da 1 metro a 60 km. Inverse di Karney misura la distanza dal testimone
allo stesso parametro sul segmento: maggiora la distanza minima al
segmento e verifica simultaneamente i due versi. Le prove non sostituiscono
la derivazione uniforme; per i lati millimetrici l'errore del misuratore
puo' superare il termine quadratico, gia' coperto dal budget numerico.
Una prova di soglia isola una tolleranza che accetterebbe il vecchio
certificato e verifica che il nuovo termine imponga il raddoppio di n.
