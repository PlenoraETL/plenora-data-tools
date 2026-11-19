# Offset planare orientato

`plenora_kernels_geo::offset::offset_with_limits` espone GOP-017 del contratto
GEOMETRY-OPERATIONS-1.0: una `LineString` XY aperta semplice, nel modello
planare. CRS risolto, ordine degli assi e unita' sono a carico del chiamante.
Le coordinate geografiche restano angolari; non sono convertite in metri.
Non e' un buffer e non applica parametri o default di servizi esterni.

## Semantica

`OffsetOptions` richiede distanza finita e raccordo. La tolleranza degli
archi deve essere positiva finita solo quando la costruzione contiene archi. `OffsetOptions::new(distance, arc_tolerance)` sceglie `Rounded`, come
`OffsetJoin::default()`; le altre varianti sono
`Bevelled` e `Mitered { limit }`, con limite finito almeno uno. Estremi butt,
positivo a destra e negativo a sinistra seguendo l'ordine dei vertici.
Distanza zero conserva ordine e bit dell'ingresso valido, compreso lo zero
con segno. A distanza non nulla i vertici collineari intermedi possono essere
omessi; il supporto e la direzione del percorso rimangono invariati.

I raccordi interni intersecano le rette di supporto. All'esterno, bevel unisce
le estremita' delle parallele; mitre le interseca, passando a bevel se il
rapporto distanza dal vertice/raggio supera il limite. Rounded segue l'arco
circolare minore centrato sul vertice. Il numero di corde deriva dalla
tolleranza esplicita, senza un numero di segmenti fisso.

L'uscita puo' essere non semplice: gli incroci esistenti sono conservati,
senza semplificazione o noding visibile. Poligoni e altri tipi, linee chiuse,
input non semplice, segmenti nulli, inversioni esatte e lati traslati
collassati/invertiti sono `Unsupported`. Struttura insufficiente o coordinate
non finite sono `InvalidGeometry`; parametri invalidi sono
`InvalidConfiguration`. Errori e diagnostica non contengono coordinate.

## Precisione e certificato

Differenze, parallele e intersezioni usano razionali esatti. Le lunghezze
irrazionali sono racchiuse mediante bisezione razionale con confronti dei
quadrati. Il sistema delle normali combina prima le radici proporzionali,
conservando le cancellazioni esatte. Non si usa una radice floating point.

Ogni ordinata costruita e' pubblicata dalla funzione condivisa GOP-015A
`ordinate`: binary64 piu' vicino, meta' al pari, al massimo mezza ulp per
ordinata. Per un valore irrazionale entrambi gli estremi certificati devono
arrotondarsi allo stesso valore. I punti medi razionali del certificato sono
solo testimoni: non sostituiscono il valore reale da arrotondare.

L'arco usa `t=tan(theta/2)`, con rotazione razionale
`((1-t*t)/(1+t*t), 2*t/(1+t*t))`. Poiche' `abs(d theta/dt)<=2`, una divisione
uniforme del parametro ha freccia al massimo `r*delta_t^2/2`. Il numero di
vertici intermedi e' calcolato e addebitato prima della crescita.

`OffsetAccuracy` separa bound Hausdorff simmetrico di discretizzazione e
bound assoluto di pubblicazione. Il totale e' la somma matematica dei due
campi, nelle unita' dell'ingresso; il bound relativo e' zero. Nessun errore
di trasformazione viene introdotto. L'incertezza del datum non e' compresa.

Il certificato prova orientazioni e ordine degli incroci sugli intervalli,
poi riusa `verify_publication` per coordinate, iniettivita', orientazioni,
ordine dei nodi e identita' dei siti tra lati. Un nuovo contatto, collasso o
cambio di concorrenza dopo rounding e' `InvalidRoundedTopology`.

## Limiti

Il certificato della discretizzazione considera ogni lente fra corda e arco,
contenuta nel triangolo formato dalla corda e dalle tangenti agli estremi.
Certifica la separazione fra queste regioni e i segmenti estranei; ammette
un attraversamento trasverso arco/segmento quando gli estremi dell'arco
sono su lati opposti della retta e gli estremi del segmento fuori dal
triangolo. Certifica al massimo un attraversamento per lente: piu' incroci
richiedono anche una prova del loro ordine. Le lenti di archi distinti sono
confrontate fra loro.
Un contatto o una separazione non dimostrabili producono `AccuracyUnproven`.
Non elimina incroci nella lente arco/corda.

Le bisezioni provano successivamente 80, 160, 320, 640, 1280 e 2560 passi per
radice. Una cancellazione algebrica non riconosciuta, una tangente irrazionale
ambigua o una separazione non certificabile possono terminare con
`AccuracyUnproven` (`GEOMETRY_ACCURACY_UNPROVEN`). Nessun successo viene
restituito per sola convergenza empirica. Valori provati fuori dal dominio
finito sono `CoordinateOutOfRange`.

`RelateLimits` e un unico `Work` coprono validazione, tutti i tentativi,
aritmetica, certificati, output e prenotazioni vive. Le verifiche degli
incroci possono essere quadratiche o maggiori nel numero di siti effettivi:
si contano visite e confronti eseguiti, senza rifiuti preventivi basati sul
prodotto delle dimensioni. Zero consente zero lavoro; soglia inclusiva.

Fra due checkpoint esterni si eseguono al massimo 256 primitive Q su
interi di al massimo 145173 bit. Ogni gcd controlla inoltre la deadline
ad ogni divisione euclidea e termina in meno di 290346 divisioni (limite
di Lame). Il controllo del flag resta al checkpoint esterno. Addizione, sottrazione, prodotto e shift controllano questo tetto
prima di allocare; il tetto interno da' `AccuracyUnproven`, senza
confonderlo con l'esaurimento del budget del chiamante.
Le ricevute degli interi, dei contenitori e dell'output condividono il
bilancio; valgono le esclusioni di scratch privati, capacita' interne di
num-bigint e overhead di `RelateLimits`. Non e' un tetto RSS.

Cancellazione e deadline sono controllate all'ingresso, durante tutte le
fasi e prima del ritorno. Il costruttore comune di `Work` applica GOP-028:
la deadline scaduta precede la cancellazione. Non esiste un risultato parziale dopo un errore.

## Prove

Le prove C1 in `src/offset/tests.rs` riportano GOP-017, GOP-015A e GOP-006
accanto alle attese: lati e inversioni, butt, zero e collinearita', mitre
analitico, bevel e limite, freccia degli archi, radici irrazionali e razionali
non diadici, domini esclusi, uscita non semplice, triplo nodo, lente degli
archi, collasso, soglie e interruzioni durante il lavoro. La radice razionale
e il tetto preventivo degli interi hanno prove in `noding/exact.rs`.

Questa e' un'API Rust diretta additiva; non registra un'operazione di piano,
un adattatore Arrow, un protocollo Z/M/curve o un servizio.
