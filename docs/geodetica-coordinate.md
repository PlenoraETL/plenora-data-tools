# Geodetica e coordinate: API dirette

Queste API Rust non aggiungono operazioni al catalogo Arrow o alla CLI.
Il contratto selezionato e' GEOMETRY-OPERATIONS-1.0.

## Densify e certificazione delle superfici

Il conteggio iniziale e' `ceil(L/h)`, almeno uno. Quando occorre inserire
punti, il calcolo salta a `ceil(L/(h-4e-6))`, almeno due, e aumenta ancora
finche' `L/n + 4e-6 <= h`. Ogni pezzo emesso viene verificato con inverse e
margine di 1 micrometro. Il massimo e' `u32::MAX` divisioni per lato;
i punti e i byte effettivi sono addebitati prima della riserva. Un passo
non certificabile rispetto al round-off fallisce esplicitamente; nessun ciclo
senza limite. Deadline precede cancellazione a ogni checkpoint.

La validazione comune a densify e distanza usa due certificati di orientamento.
Il primo integra un ventaglio sulla mappa normale dell'ellissoide. Il resto
fra archi normali e corde e' `16 sum((L_i/5.9e6)^3)` steradianti; si aggiungono
`512 P margin/5.9e6^2` e i margini di calcolo e accumulazione. Densificare riduce
il termine cubico; non aggiunge un metro quadrato di soglia per ogni vertice.
Il ventaglio deve avere condizionamento `hypot(determinante, denominatore)>0.5`.
L'area normale ridotta modulo `4*pi`, incluso il bound, deve avere modulo
inferiore a 6 sr: per `f<=1/250` questo certifica anche la regione ellissoidica
minore di mezzo ellissoide. Il valore interno e' solo un orientamento,
non una misura d'area pubblicata.

Se il ventaglio e' quasi antipodale, il passo troppo lungo per il resto cubico,
o la regione normale e' prossima a un emisfero, viene tentato il certificato
globale GeographicLib. Esso include l'errore assoluto conservativo delle
primitive d'area, la somma dei valori assoluti S12 con fattore `gamma_n`, e
il perimetro per il margine posizionale. Questo secondo certificato resta
conservativo per anelli globali molto sottili e densi; non si promette successo
per ogni geometria valida e ogni budget. Il limite residuo e' la mancata prova
di entrambi i certificati, che produce `AccuracyUnproven`, mai un'area plausibile.

La separazione non usa la soglia fissa di 4 mm della ricerca di distanza:
usa `16*margin` per il lower bound e `32*margin` per fermare una ricerca non
risolta. Il margine base e' 1 micrometro e cresce sui lati lunghi in funzione
del ridotto geodetico. Sotto il margine residuo la separazione puo' non essere
certificata. Il limite di lavoro resta applicato a ogni raffinamento.

Le prove di incrocio trasverso e i contatti esatti danno `UnsupportedSurface`;
separazione, orientamento o precisione non dimostrati danno `AccuracyUnproven`
(`execution`, codice `GEOMETRY_ACCURACY_UNPROVEN`), sia all'ingresso sia in
uscita. Gli emisferi ambigui sono `unsupported`. Buchi annidati e buchi
esterni violano i ruoli espliciti di GOP-002/005: `InvalidGeometry`, categoria
`data_mapping`. Parti sovrapposte non sono ammesse come superficie geodetica.

## Coordinate testuali

MGRS usa UTM per `-80 <= lat < 84` e UPS fuori da questo intervallo.
UTM come formato autonomo mantiene 84 N incluso. USNG mantiene il proprio
dominio NAD83/NAD27, incluso 84 N, senza passare a UPS.
L'equatore UTM ha solo la designazione N: la lettura di northing 10000000
nell'emisfero S e' rifiutata. In scrittura un nodo che arrotonda all'equatore
usa N e northing zero, anche per latitudini negative subnormali. La proprieta'
delle celle MGRS/USNG immediatamente a sud resta distinta.

La validazione delle opzioni segue un ordine fisso: profilo e numero delle
opzioni, presenze/duplicati, poi Precision, Resolution, Representative, Datum.
L'ordine della slice non influisce sulla categoria d'errore. Interpretazione
GOP-022/024: gli zeri iniziali sono ammessi su tutte le componenti angolari,
quindi minuti e secondi con tre cifre sono accettati quando il valore e' in
[0,60). La scrittura usa sempre due cifre intere per minuti e secondi.

`encode_coordinates_with_limits` e `decode_coordinates_with_limits` accettano
`CoordinateLimits` e il flag cooperativo. I checkpoint sono all'ingresso,
prima di ogni elemento e prima della pubblicazione; intervallo massimo un
codec scalare con testo limitato a 256 byte. Nessun prefisso viene restituito.
La deadline prevale sulla cancellazione; errori di validazione gia' osservati
mantengono la propria categoria. Le API senza controlli sono wrapper con
limiti locali predefiniti e nessun flag. La contabilita' totale dell'heap
GOP-027 non e' qualificata da questa API.
