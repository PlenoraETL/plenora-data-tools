# Reshape planare

`plenora_kernels_geo::reshape::reshape_with_limits` espone GOP-019 di
GEOMETRY-OPERATIONS-1.0 nel profilo planare XY. Riceve target, reshaper,
`RelateLimits` e segnale di cancellazione; restituisce una `LineString`.
Il chiamante risolve CRS, assi e unita'. Non e' un'operazione di piano,
un adattatore Arrow o una rotta di servizio. Le REST Esri non sono normative.

## Semantica

Il target e' una singola polilinea aperta semplice. Il reshaper e' una
singola polilinea semplice; servono esattamente due intersezioni distinte,
trasversali e interne a entrambi i percorsi. Si conserva il prefisso e il
suffisso del target e si sostituisce il tratto intermedio con quello del
reshaper. Quest'ultimo viene invertito quando necessario per conservare la
direzione del target. Tutti i vertici intermedi del tratto scelto restano.

Gli incroci possono essere interni ai lati o coincidere con vertici interni.
La trasversalita' e' l'alternanza ciclica dei quattro rami incidenti, con
ordinamento angolare razionale esatto. Non si usa un epsilon. Per un reshaper
chiuso il sottopercorso e' l'intervallo nell'ordine memorizzato fra i due
incroci, senza attraversare la giunzione iniziale/finale; un contatto su tale
giunzione e' escluso come contatto terminale del percorso memorizzato.

Zero, uno o piu' di due incroci, overlap, tangenze, contatti terminali,
target chiuso/multiparte, altri tipi, lati nulli, input non semplice e
ricostruzione non semplice restituiscono `ReshapeError::Unsupported`.
Struttura senza segmenti o coordinate non finite sono `InvalidGeometry`.
Non esistono scelta di regione poligonale, semplificazione o riparazione
implicita. Z/M e curve non sono rappresentati dal confine Rust XY e non
vengono eliminati da questa API.

## Precisione

Intersezioni e confronti sono razionali esatti del noder comune. Il solo
pubblicatore e il solo verificatore GOP-015A sono quelli condivisi con
trimExtend, offset e booleane lineari. Vertici originali invariati, nodi
costruiti al binary64 piu' vicino, ties-to-even, errore al massimo mezza ulp
per ordinata. Nei siti originali coincidenti prevale il target, anche per
gli zeri con segno. Modello planare nelle unita' delle coordinate, bound
relativo zero, nessuna trasformazione/discretizzazione; incertezza del datum
esclusa. Le incidenze sugli input sono quelle della costruzione esatta:
il rounding puo' discostare un nodo dalle rette entro il bound dichiarato.

La topologia intrinseca pubblicata conserva siti distinti, assenza di nuovi
incroci/contatti/overlap e orientazioni non nulle dei rami. Se il rounding
rompe queste proprieta', `InvalidRoundedTopology`. Nessuno snap sostitutivo.
`CoordinateOutOfRange` conserva il rifiuto tipizzato del pubblicatore comune.

## Risorse e limiti

Un unico `Work` copre lettura, validazione, incroci, ricostruzione,
pubblicazione e verifica. `max_segment_pairs` conta ciascun test esatto fra
lati dopo pruning, anche ripetuto; visite degli inviluppi, confronti di siti
e ordinamenti consumano passi. `max_nodes` comprende punti e copie,
certificati e uscita; `max_steps` limita tutti i cicli. Non c'e' raffinazione.
I contenitori prenotano prima della crescita; le ricevute dei razionali e
dell'uscita restano vive fino al rilascio della memoria posseduta.

Valgono le esclusioni di `RelateLimits`: input prestato, overhead allocatore,
capacita' e scratch privati del backend numerico. Non e' un limite RSS.
Il massimo per intero e' 145173 bit con controlli preventivi condivisi;
si veda [Topologia](topologia.md). Zero permette zero lavoro, soglie esatte
inclusive, overflow rifiutato prima dell'addebito. Nessun prodotto delle
dimensioni o stima per vertice governa l'ammissione.

Checkpoint all'ingresso, in ogni ciclo e prima del ritorno. Fra checkpoint
al massimo 256 primitive Q (conversioni, copie, confronti, segni e
aritmetica), espandendo P e midpoint. Il ramo collineare condiviso costa
al massimo 154 primitive prima di `nodes`; classificazione angolare e
localizzazione dei tagli restano sotto tale massimo. Il tetto degli interi
non dipende dal numero di vertici. Allocazioni e drop esclusi da questa
misura, nessuna garanzia di latenza wall-clock. Le scansioni possono avere
costo quadratico nei candidati effettivi.

GOP-028: al medesimo checkpoint deadline scaduta precede cancellazione;
un errore di validazione gia' riconosciuto conserva la propria categoria.
Errori bounded senza dati, nessun effetto remoto o successo parziale.

## Prove

Le attese C1 in `src/reshape/tests.rs` citano GOP-019, GOP-015A e
GOP-026/027/028 accanto alle verifiche: direzioni concordi/discordi,
vertici e lati, domini esclusi, ricostruzione, nodi razionali e midpoint,
collasso dopo rounding, soglie esatte e interruzioni durante il lavoro.
Con input semplici e soli due incroci, il sottopercorso esatto e' semplice
per costruzione; il gate finale e' comunque esercitato direttamente con una
ricostruzione non semplice. Un arco che incrocia il prefisso del target
viene gia' rifiutato come terzo incontro.
