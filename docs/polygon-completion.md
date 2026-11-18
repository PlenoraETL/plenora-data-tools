# Completamento dei poligoni

`plenora_kernels_geo::polygon_completion_with_limits` riceve
`&[geo::Polygon<f64>]`, un `&geo::Geometry<f64>` lineare, `RelateLimits`
e un flag opzionale `AtomicBool`. Restituisce `Vec<Polygon<f64>>`.
L'errore e' `polygon_completion::PolygonCompletionError`.
API Rust diretta: nessun nuovo kernel Arrow, CLI o runtime.

## Dominio e risultato

Profilo GOP-020 planare XY: poligoni OGC validi con interni disgiunti,
LineString o MultiLineString semplice, anche vuoto. Il chiamante risolve
CRS, unita' e ordine degli assi comuni. Z, M e curve non sono rappresentati
da questi tipi; nessuna riproiezione, riparazione o convenzione Esri.

Il noder comune conserva l'indice sorgente di ogni lato atomico. Una tabella
associa ciascun indice al ruolo nuovo oppure all'identita' del poligono.
L'enumeratore razionale comune produce i cicli e assegna i buchi. Una faccia
qualifica soltanto se non occupata e se guscio o buchi contengono almeno
un lato positivo di ciascun ruolo. La classificazione usa il punto medio
razionale di un lato e una perturbazione simbolica a sinistra; nessun epsilon.
I buchi preesistenti non adiacenti e le facce di sole linee sono esclusi.
I residui aperti non producono facce. Nessuna faccia significa lista vuota.

Intersezioni puntuali fra poligoni sono ammesse. Interni sovrapposti,
compresi poligoni duplicati, danno `OverlappingInteriors`. Confini condivisi
fra poligoni esistenti sono ammessi; la coincidenza nuovo/esistente da'
`AmbiguousProvenance`. Linework non semplice, inclusi duplicati nuovi, o tipo escluso danno
`UnsupportedLinework`. Geometria invalida/non finita conserva il
`PredicateError` di validazione. Nessun errore contiene dati degli operandi.

Gusci antiorari e buchi orari iniziano dal minimo XY pubblicato. Facce e
buchi sono ordinati per minimo XY, area razionale positiva e sequenza
canonica. L'area delle facce sottrae i buchi. Gli zeri con segno sono lo stesso
sito XY; il primo vertice originale conserva i propri bit.

## Precisione e verifica

Noding e selezione usano razionali esatti. Il verso viene dalla svolta al
minimo lessicografico; i buchi vengono assegnati per annidamento esatto. `Originals`, `ordinate` e
`verify_publication` sono quelli delle booleane lineari: un solo arrotondatore
nearest-even e un solo verificatore. Errore <= mezza ulp per ordinata
costruita, nelle unita' delle coordinate; nessuno snap metrico.

Il certificato contiene i lati delle facce e i confini occupati, suddivisi
soltanto ai siti effettivamente pubblicati. I nodi esclusi non impongono
arrotondamenti. Sui confini originali si verifica anche l'aderenza al supporto:
un nodo arrotondato fuori dalla retta viene rifiutato. Si controllano
iniettivita', incidenze, settori, validita' OGC, disgiunzione fra gli interni delle facce e dall'area
originale dopo il rounding. Un fallimento da' `InvalidRoundedTopology`,
mai un risultato parziale.

## Limiti

Un unico `RelateLimits` copre validazione, noding, selezione, ordinamenti,
verifica e output. Coppie effettive, nodi e passi sono cumulativi. Lati,
facce, vettori, coordinate e certificati hanno prenotazioni vive prima della
crescita; gli export mantengono le ricevute fino al ritorno. Passi e byte
limitano anche il numero di lati e facce. Nessuna ammissione per prodotto
stimato delle dimensioni. Nessuna serializzazione in questa API.

L'aritmetica usa interi di massimo 145173 bit. Prima di ogni somma/sottrazione
si verifica `max(bits(a),bits(b))+1`, prima di un prodotto `bits(a)+bits(b)`,
prima di uno shift `bits(a)+k`. Divisione, resto, segno e copia non crescono.
Il confronto d'area viene eseguito solo a parita' di minimo XY. Un filtro
su una griglia binaria a 128 bit frazionari conta rigorosamente i resti.
Se non decide, la divisione binaria prosegue mantenendo un resto limitato
per ciascun termine; non costruisce mai il denominatore comune. La somma
dei bit dei denominatori fornisce un limite di separazione che permette di
certificare anche l'uguaglianza esatta. Cresce il lavoro, contato dal budget
del chiamante, non la larghezza dei singoli interi. Il tetto interno degli
interi produce `AccuracyUnproven`; `ReservationLimit` indica invece il
budget di memoria esaurito. La derivazione e' in `noding/bounds.rs`.

Il massimo sforamento contabile dopo la creazione di un singolo intero e'
`RelateLimits::MAX_VALUE_BYTES`. Input preso in prestito, scratch privati
BigInt e overhead dell'allocatore sono esclusi: non e' un tetto RSS.
Le copie strutturali e aritmetiche restano contate secondo `RelateLimits`.

## Cancellazione e qualifica

`Work::new` applica GOP-028: a uno stesso checkpoint,
deadline scaduta precede cancellazione in tutte le API del noder comune. Checkpoint all'ingresso, nei cicli e prima del ritorno.
Non viene introdotta una seconda deadline nel payload di un runtime.

La prova pubblica interrompe lavoro gia' iniziato con flag e deadline;
le soglie esatte sono provate su passi, nodi, coppie e byte. La matrice
funzionale include buchi, residui, duplicati, ordine, rounding e un oracolo
indipendente di cammini semplici su grafi razionali, esteso a componenti
annidate e residui non ciclici. L'oracolo non usa l'enumeratore del kernel.

La qualifica integrale GOP-028 resta aperta: non sono disponibili prove
black-box che isolino l'interruzione in ciascuna fase, ne' un audit completo
del massimo intervallo numerico fra checkpoint. In particolare
`Polygon::new` visita i buchi in una primitiva esterna senza callback;
il loro numero e' limitato dai passi gia' contati, ma non e' un intervallo
costante indipendente dai limiti. Non si dichiara una latenza wall-clock o
una certificazione integrale GOP-028.
