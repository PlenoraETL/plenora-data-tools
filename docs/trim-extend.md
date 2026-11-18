# Taglio ed estensione planare

`plenora_kernels_geo::trim_extend::trim_extend_with_limits` espone GOP-018
di GEOMETRY-OPERATIONS-1.0 nel profilo planare XY. Il chiamante risolve CRS,
assi e unita'; nessuna riproiezione o conversione implicita in metri.
La funzione restituisce una `LineString` per un singolo target, senza batch,
serializzazione, adattatore Arrow o registrazione di un'operazione di piano.

## Semantica

Target singolo aperto semplice; guida singola rettilinea diretta, interpretata
come retta infinita. Sono ammessi vertici intermedi della guida collineari
nello stesso verso. Un solo attraversamento interno, anche in un vertice
esistente del target, conserva la parte nel semipiano sinistro della guida,
incluso il punto di taglio e mantenendo l'ordine del target.

Senza tagli si considerano i raggi terminali abilitati: start opposto al
primo lato, end lungo l'ultimo. Un colpo a distanza strettamente positiva
estende il target; zero colpi lascia il target invariato, inclusi i bit degli
zeri con segno e tutti i vertici intermedi. Due colpi idonei sono ambigui.
`TrimExtendOptions::default()` seleziona `TrimExtendEnds::Both` e
`TrimExtendMode::AddVertex`. Le alternative sono `Start`, `End`, `Neither`
e `MoveTerminalVertex`. La selezione degli estremi governa solo l'estensione;
il taglio avviene anche con `Neither`. Il modo move sostituisce solo
l'estremo colpito; add conserva anche il vecchio estremo.

Collinearita' con la guida, tangenza, contatto terminale, overlap, due colpi,
piu' tagli, target chiuso/multipart, lati nulli, ingresso o ricostruzione non
semplice e guida non rettilinea sono `Unsupported`. Non si sceglie il colpo
piu' vicino. Struttura senza segmenti o coordinate non finite sono
`InvalidGeometry`. Altri tipi geometrici sono esclusi. Il tipo Rust e' XY:
Z/M e curve non sono rappresentati ne' scartati da questa API.

## Precisione

Segni di semipiano, intersezioni e distanze parametriche sono razionali
esatti, senza epsilon. L'aritmetica fallibile e' quella di `noding`;
`Originals::coordinate` usa l'unico arrotondatore `line_boolean::ordinate`.
Vertici originali invariati; punti costruiti al binary64 piu' vicino,
meta' al pari, entro mezza ulp per ordinata (GOP-015A). Questo e' il
certificato numerico pubblico: modello planare nelle unita' dell'ingresso,
nessuna discretizzazione o trasformazione, bound relativo zero.
L'incertezza del datum non e' inclusa.

La relazione con la guida e il lato originale e' costruita esattamente
prima del rounding; un punto razionale non rappresentabile puo' discostarsi
da tali rette entro il bound per ordinata. `verify_publication`, condiviso
con offset e booleane lineari, verifica la topologia intrinseca pubblicata:
iniettivita' dei siti, assenza di nuovi incroci/contatti/overlap e conservazione
dei settori non nulli. Un collasso o un'alterazione sono
`InvalidRoundedTopology`, mai un risultato plausibile. Coordinate costruite
non pubblicabili finite sono `CoordinateOutOfRange`.

## Risorse e interruzione

Un unico `Work` e `RelateLimits` coprono lettura, validazione, costruzione,
verifica e uscita. `max_segment_pairs` conta test esatti fra lati dopo
pruning, valutazioni punto/retta della guida e test dei raggi abilitati;
la sola scansione di segni gia' calcolati consuma passi. Ripetizioni contate.
`max_nodes` comprende punti costruiti e copie, anche certificati e uscita;
`max_steps` limita visite e iterazioni. Non occorre raffinazione.
I byte vivi includono vettori, capacita', razionali e output, con ricevute
comuni; si addebita prima della crescita. L'ingresso prestato, overhead
allocatore, capacita' e scratch privati di num-bigint sono esclusi, come in
`RelateLimits`. Non e' una misura RSS. Un singolo intero e' limitato a
145173 bit: controlli preventivi dei prodotti/shift e addebito della misura
effettiva dopo costruzione come documentato in [Topologia](topologia.md).

Zero permette zero lavoro, soglie inclusive, overflow rifiutato. Nessun
prodotto delle dimensioni governa l'ammissione, nessun budget cambia la
precisione. Ogni ciclo ha checkpoint, all'ingresso e prima del ritorno;
fra due checkpoint al massimo 256 primitive `Q`: conversione, copia,
confronto (inclusa uguaglianza), segno o operazione aritmetica, espandendo
le operazioni `P` e midpoint. Il ramo piu' lungo e' l'intersezione collineare
condivisa: 8 primitive iniziali, quattro estremi con al massimo 36 primitive
ciascuno e 2 confronti finali, totale 154 prima del checkpoint `nodes`.
Le code di pubblicazione e i confronti di ordinamento restano sotto 256.
La normalizzazione interna di ogni primitiva ha interi limitati a 145173 bit,
indipendentemente dai vertici. Allocazioni e drop non sono unita' di questa
misura; non si garantisce una latenza wall-clock. Le verifiche possono essere quadratiche
nei candidati effettivi. GOP-028: deadline scaduta precede cancellazione
osservata al medesimo checkpoint. La stessa precedenza vale per tutte le API del noder comune.
Errori senza dati, nessun successo parziale o effetto remoto.

## Prove

Le attese C1 in `src/trim_extend/tests.rs` citano GOP-018, GOP-015A e
GOP-026/027/028 accanto alle verifiche. Coprono semipiani, raggi e modi,
selezione, zeri invariati, domini esclusi, ricostruzione non semplice,
razionali non diadici e midpoint nearest-even, collassi, soglia strettamente
positiva, limiti -1/0/+1, cancellazione e deadline durante il lavoro.
Le REST Esri della raccolta GEO-editing non sono fonti normative.
