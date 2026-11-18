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
alla fine. Nessun buco. I lati sono **geodetiche minime**, come in
`geodesic_densify.rs` e `geodesic_distance.rs`, non corde nel piano lon/lat.
Consumatori e serializzatori devono mantenere questa interpretazione.

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
precedente darebbe `(T-2.2e-6)/8 >= 1.2225e-5 m`; i tre `next_up`
e le addizioni a questa scala costano meno di `1e-17 m`, largamente
assorbiti dalla riduzione a `1.2e-5`. Il candidato iniziale n=16 non
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
componente GeographicLib `2e-7 m`, arrotondamenti `2e-6 m`.
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

Il lavoro conta raggi, letture geometriche, coordinate e checkpoint
di costruzione/assemblaggio. La pianificazione ha al piu' 28 raddoppi,
senza primitive geodetiche. Ogni direct e' preceduta da un checkpoint;
ce n'e' uno anche dopo l'ultima direct e prima della pubblicazione.
La deadline precede la cancellazione allo stesso checkpoint.
Un errore distrugge il risultato parziale; nessun prefisso viene restituito.
Le prove iniettano deterministicamente entrambi i segnali nella funzione
di generazione di produzione, dopo 511 vertici, senza gare fra thread.
