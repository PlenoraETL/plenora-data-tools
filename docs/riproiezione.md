# Riproiezione

`geo.reproject` riproietta una colonna geometria fra due CRS della tabella
integrata ([«CRS integrati»](crs.md#crs-integrati)), tutti e 169, in Rust puro:
nessun PROJ, nessuna griglia scaricata. La matematica è in
`plenora_core::crs::riproiezione`, il kernel su `geo::Geometry` e su
`RecordBatch` in `plenora_kernels_geo::riproiezione` (`reproject_batches`),
l'analisi del contratto in `analyze_reproject`.

```json
{"out": "rdn", "op": "geo.reproject", "in": ["catasto"],
 "config": {"target_crs": "EPSG:7791",
            "accuratezza_accettata_m": 0.1,
            "griglie": [{"trasformazione": 9734,
                         "file": "C:/griglie/35160622_47161840_R40_F00.gsb"}]}}
```

- `target_crs` (obbligatorio): un identificatore della tabella integrata;
- `accuratezza_accettata_m` (facoltativo): l'accuratezza, in metri, che si
  accetta per il cambio di datum ([«La regola dell'accuratezza»](#la-regola-dellaccuratezza));
- `trasformazioni` (facoltativo): codici EPSG delle trasformazioni da usare,
  nell'ordine; il percorso fra i datum è esattamente quello;
- `griglie` (facoltativo): griglie NTv2 fornite dall'utente, ognuna con il
  codice EPSG della trasformazione a griglia e il percorso del file;
- `convenzione_wgs84_etrs89` (facoltativo, predefinito `true`): WGS 84 e la
  famiglia ETRS89 equivalenti per convenzione
  ([«WGS 84 = ETRS89 per convenzione»](#wgs-84--etrs89-per-convenzione)).

Contratto: schema, righe, tipi geometrici e `FieldId` invariati; il CRS del
contratto e il metadato `geo` del campo diventano il target, le chiavi
canoniche CRS della sorgente si sostituiscono, `axis_order` diventa quello
GIS normalizzato del target. Le coordinate si leggono e si scrivono
nell'ordine GIS normalizzato (longitudine o easting prima, anche per 4326,
6707–6709, 6875, 3035): una colonna che dichiara un altro ordine, anche
`unknown`, si rifiuta, come a 190c493. Un CRS risolto dal chiamante (non
della tabella) si rifiuta con `CRS_NOT_BUILTIN`.

## La catena

Per ogni punto: dominio di validità del CRS sorgente (rettangolo proiettato
o mondo lon/lat), proiezione inversa, **regione lon/lat** del dominio
(Transverse Mercator: ±15° dal meridiano centrale e le latitudini del fuso;
Mercator: ±85,06°), cambio di datum lungo un **percorso** di trasformazioni
EPSG, regione lon/lat e proiezione diretta del target, dominio di validità
del target. Ogni uscita da un dominio o da una regione è
`COORDINATE_OUT_OF_CRS_DOMAIN`, senza coordinate nel messaggio. Con la
regione lon/lat la riproiezione chiude, per sé, il limite «il rettangolo è
più largo della regione» dei [CRS integrati](crs.md#limiti-dei-crs-integrati):
un punto UTM nel rettangolo ma oltre i 15° dal meridiano si rifiuta.

Stesso CRS, o CRS che differiscono solo per l'ordine d'autorità degli assi
(6707 e 7791, 4326 e `OGC:CRS84`): coordinate invariate al bit, dopo i
controlli di dominio.

## Metodi di proiezione

I parametri li genera `scripts/genera_riproiezione.py` dal registro EPSG
v11.022 (PROJ 9.5.1, pyproj 3.7.2, stesso ambiente vincolato di
`genera_crs_integrati.py`) in `crates/plenora-core/src/crs/riproiezione/epsg.rs`,
che riporta la fonte e non si modifica a mano.

| metodo EPSG | CRS | formule | scarto massimo da PROJ (oracolo) |
| --- | --- | --- | --- |
| Transverse Mercator (9807) | 148: UTM WGS 84 ed ETRS89, IGM95, RDN2008, ED50, Gauss-Boaga 3003/3004, 6875/7794, 27700, 31467, 2193 | Krüger al sesto ordine nella forma di Karney (2011) | 7e-9 m |
| Mercator (variant A) (9804) | 3395 | isometrica esatta, inversa per Newton | 4e-9 m |
| Popular Visualisation Pseudo Mercator (1024) | 3857 | sferica con raggio `a` sulle coordinate geodetiche | 4e-9 m |
| Lambert Conic Conformal (2SP) (9802) | 2154 | EPSG 7-2, latitudine per Newton | 4e-9 m |
| Lambert Azimuthal Equal Area (9820) | 3035 | EPSG 7-2, latitudine autalica inversa per Newton | 1e-8 m |
| Oblique Stereographic (9809) | 28992 | sfera conforme di Gauss e stereografica | 7e-9 m |
| Hotine Oblique Mercator (variant B) (9815) | 2056 | Swiss Oblique Mercator, come PROJ (`somerc`) | 9e-9 m |

Scarti su punti che coprono la regione del dominio (per i TM fino a 14,5°
dal meridiano centrale), avanti e indietro; andata e ritorno entro 9e-9 m.

## Cambi di datum

Il datum di un CRS è il suo CRS geografico di base (18 datum). Le
trasformazioni sono quelle EPSG fra due datum della tabella, 156 in tutto:

- **senza griglia** (123): traslazioni geocentriche (9603), Position Vector
  (9606) e Coordinate Frame (9607), con le formule EPSG linearizzate, via
  coordinate geocentriche con altezza nulla (come PROJ per i CRS 2D);
- **a griglia NTv2** (33): entrano solo se l'utente fornisce il file
  ([«Griglie NTv2»](#griglie-ntv2)).

Restano fuori, e il generatore lo scrive nell'intestazione del file: le
operazioni concatenate del registro (il percorso lo compone il codice), le
griglie in altri formati (NADCON, NADCON5, …), le trasformazioni sostituite
(`supersession`) da un'altra inclusa, come fa PROJ. Molodensky-Badekas
(9636) fra questi datum compare solo in trasformazioni sostituite; nessuna
dipende dal tempo. Una trasformazione nuova con un metodo non supportato,
dipendente dal tempo o senza accuratezza fa rifiutare il generatore.

Un **percorso** è una catena di al più tre trasformazioni, ognuna in un
verso, che non ripassa per lo stesso datum. L'accuratezza del percorso è la
**somma** delle accuratezze EPSG dei passi. I percorsi si provano in un
ordine fisso: accuratezza, numero di passi, area d'uso più piccola (a parità
di accuratezza vince la trasformazione più specifica), codici EPSG, verso.
Per ogni geometria si usa il **primo percorso la cui area d'uso contiene
tutti i suoi punti** (vertici e punti aggiunti dalla densificazione): una
geometria non mescola mai due percorsi, e una che nessun percorso ammesso
copre è un errore (`REPROJECTION_OUTSIDE_TRANSFORMATION_AREA`), mai un
ripiego. Se poi un punto trasformato (vertice, campione o punto di
densificazione) sarebbe coperto da solo da un percorso che viene prima, o un
lato attraversa il riquadro d'uso di un percorso precedente (per esempio il
tratto sardo di una linea lungo il parallelo 40 da 7 a 11 E, che il riquadro
continentale contiene ma per cui vale la trasformazione della Sardegna: i
parametri continentali lo sposterebbero di circa 6 m), la geometria si
rifiuta (`REPROJECTION_MIXED_TRANSFORMATION_AREAS`), con o senza vertici
intermedi: l'esito non dipende da come l'ingresso è segmentato. Va divisa, o
il percorso fissato con `trasformazioni`, che vale per tutte le geometrie.

Esempi della scelta, per punti tipici e senza griglie:

| coppia | punto | percorso scelto | accuratezza |
| --- | --- | --- | --- |
| RDN2008 ↔ ETRS89 (7791 ↔ 25832) | Italia | RDN2008 to ETRS89 (1), EPSG:6710 | 0 m (equivalenti) |
| GDA94 ↔ GDA2020 (4283 ↔ 7844) | Canberra | GDA94 to GDA2020 (1), EPSG:8048 | 0,01 m (entro la precisione) |
| Monte Mario ↔ RDN2008 (Gauss-Boaga 3003 ↔ 7791) | Roma | EPSG:1659 + 6710 inversa | 4 m |
| | Cagliari | EPSG:1661 (Sardegna) + 6710 inversa | 4 m |
| | con la griglia IGM EPSG:9734 | EPSG:9734 | 0,1 m |
| IGM95 ↔ RDN2008 (3064 ↔ 7791) | Italia | EPSG:1098 + 6710 inversa | 0,5 m |
| ED50 ↔ ETRS89 (23032 ↔ 25832) | Roma | ED50 to WGS 84 (1) EPSG:1133 + 1149 inversa | 10 m (1149 conta 0) |
| | Copenaghen | ED50 to ETRS89 (4), EPSG:1626 | 1 m |
| OSGB36 ↔ WGS 84 (27700 ↔ 4326) | Londra | OSGB36 to WGS 84 (6), EPSG:1314 | 2 m |
| Amersfoort ↔ ETRS89 (28992 ↔ 4258) | Utrecht | Amersfoort to ETRS89 (8), EPSG:9281 | 0,25 m |
| CH1903+ ↔ ETRS89 (2056 ↔ 4258) | Berna | CH1903+ to ETRS89 (1), EPSG:1647 | 0,1 m |
| NAD27 ↔ NAD83 (4267 ↔ 4269) | Kansas | NAD27 to WGS 84 (6) EPSG:1175 + 1188 inversa | 11 m |
| DHDN ↔ ETRS89 (31467 ↔ 25832) | Stoccarda | DHDN to ETRS89 (3), EPSG:1778 | 1 m |
| RGF93 v1 ↔ ETRS89 (2154 ↔ 4258) | Parigi | EPSG:1591 | 0,1 m |
| ETRS89, RDN2008 ↔ WGS 84 (25832, 3035, 7791 ↔ 4326, 32632) | Europa | ETRS89 to WGS 84 (1), EPSG:1149 (con 6710 per RDN2008) | 0 per convenzione (1 m con `convenzione_wgs84_etrs89: false`) |
| NZGD2000, SIRGAS 2000 ↔ WGS 84 | | EPSG:1565, EPSG:15894 | 1 m |
| CGCS2000 ↔ altri datum | | nessuno (`REPROJECTION_PATH_UNAVAILABLE`) | — |

Stesso datum (per esempio 4326 ↔ 3857 ↔ 32632, 4258 ↔ 3035 ↔ 25832):
nessuna trasformazione, accuratezza 0.

## WGS 84 = ETRS89 per convenzione

**Regola** (decisione dell'utente). Come la maggior parte dei GIS, e come
PROJ quando applica EPSG:1149 (traslazioni nulle), WGS 84 e la famiglia
ETRS89 (ETRS89 e i CRS su di esso: 4258, 25828–25837, 3035; RDN2008 e i
suoi, 6706–6709, 7791–7794, 6875, equivalente a ETRS89 per il registro con
accuratezza 0) sono **equivalenti per convenzione**: il passo ETRS89 to WGS
84 (1), EPSG:1149, conta accuratezza 0 invece di 1 m. WGS 84 → RDN2008 /
UTM 32N non chiede `accuratezza_accettata_m`.

**Ambito.** Il solo passo EPSG:1149, anche dentro una catena: ED50 → WGS 84
→ ETRS89 conta l'accuratezza del passo ED50 e 0 per il resto. Ogni altro
cambio di datum resta sotto la regola dell'accuratezza.

**Hazard.** La differenza reale fra WGS 84 (G2139) ed ETRS89 in Europa è
oggi di circa 50–80 cm e cresce di circa 2,5 cm all'anno (deriva della
placca euroasiatica): il risultato si scosta dal vero di tanto, senza errore.

**Condizione di rientro.** `convenzione_wgs84_etrs89: false` nella config:
EPSG:1149 torna a 1 m e la regola dell'accuratezza lo chiede esplicitamente.
Scritta su una coppia che non passa da EPSG:1149 si rifiuta (senza effetto).

## La regola dell'accuratezza

**Regola.** Un percorso la cui accuratezza sta entro la precisione di 1 cm
è sempre ammesso: lo stesso datum, i datum equivalenti per il registro
(RDN2008 ed ETRS89, EPSG:6710 con accuratezza 0), WGS 84 ed ETRS89 per
convenzione, GDA94 → GDA2020 (1 cm).
Oltre, solo se `accuratezza_accettata_m` è almeno pari all'accuratezza del
percorso; altrimenti l'analisi rifiuta con
`REPROJECTION_ACCURACY_NOT_ACCEPTED`, che riporta l'accuratezza del percorso
migliore. Dichiarata l'accuratezza, **il risultato vale solo entro quella
accuratezza**: la matematica aggiunge al più mezzo centimetro (sotto),
l'errore del cambio di datum è quello che il registro dichiara. Sono ammessi
tutti i percorsi entro l'accuratezza accettata, e ogni geometria prende il
primo che la copre: il risultato non è mai peggiore di quanto dichiarato.

`accuratezza_accettata_m` senza effetto (ogni percorso ammesso è già entro 1
cm), non finita o negativa si rifiuta, come ogni parametro scritto e senza
effetto del runner; così una griglia che nessun percorso ammesso usa.

## Griglie NTv2

Le trasformazioni EPSG a griglia NTv2 fra i datum della tabella (33, fra cui
le griglie IGM 9732–9737 per Monte Mario, ED50, IGM95 e RDN2008, OSTN15
7709/7710, rdtrans2018 9282, BeTA2007 15948/15949, le GDA 8444–8447) entrano
nei percorsi solo se l'utente fornisce il file in `griglie`, con il codice
EPSG della trasformazione: da lì vengono datum, verso e **accuratezza** (quella
del registro, per esempio 0,1 m per EPSG:9734). Il file si legge con la sola
libreria standard, al più 256 MiB, record per record con ogni conteggio
verificato (intestazioni NTv2, `SECONDS`, estensioni multiple del passo,
`GS_COUNT`, valori finiti, gerarchia delle sottogriglie ad albero,
endianness da `NUM_OREC`); interpolazione bilineare sulla sottogriglia più
fine che contiene il punto, inversa iterativa come PROJ (1e-12 radianti, al
più 20 passi, altrimenti `REPROJECTION_NOT_CONVERGED`). Un punto fuori dalla
griglia rende il percorso non applicabile alla geometria. Nessun
download: l'analisi verifica la forma della config, il file si legge
all'esecuzione (`NTV2_GRID_UNREADABLE`, `NTV2_GRID_INVALID`).

## Densificazione dei lati

Un lato dritto nel CRS sorgente non è dritto nel target (6° lungo il
parallelo 45 in UTM 32N: la corda si scosta di metri dalla curva). Ogni
lato si prova nei punti a 1/4, 1/2 e 3/4: l'immagine esatta deve stare entro
**metà della precisione del target** (5 mm a terra; per un target
geografico metà di 1 cm in gradi all'equatore, più severo altrove) dal lato
d'uscita, i punti del lato d'uscita entro la stessa distanza dalla spezzata
delle immagini, e nessuna metà del lato può avere un'immagine più lunga di
3/4 dell'intero (continuità). Con una griglia NTv2 nel percorso, in più,
estremi e campioni devono stare nella **stessa cella** di ogni griglia (il
campo di spostamenti è bilineare a pezzi: dentro una cella è quadratico
lungo il lato e i campioni ne misurano lo scarto; un rilievo di un nodo fra
due campioni non si perde), oppure l'immagine del lato deve essere più
corta della tolleranza (il pezzo che attraversa un bordo di cella, ridotto
per bisezione). Altrimenti il lato si divide a metà nel CRS sorgente, fino a
48 livelli e a `MAX_CELL_COORDINATES` coordinate per cella:
oltre, `REPROJECTION_EDGE_NOT_CONVERGED` (il caso tipico: un lato che nel
target attraversa l'antimeridiano) o `ResourceLimit`. Le rette del target
(meridiani in Mercator, paralleli in lon/lat) non ricevono punti. L'uscita
ha lo stesso tipo e la stessa struttura dell'ingresso, anelli chiusi, e deve
essere valida OGC: una riproiezione che la rendesse non valida è un errore.
Il calcolo gira dietro la barriera `calcolo_protetto`.

## Oracolo

`scripts/genera_oracolo_riproiezione.py` (stesso ambiente vincolato) scrive
in `crates/plenora-core/tests/fixtures/riproiezione/` i risultati di PROJ
9.5.1 obbligato alla **stessa operazione EPSG**, senza rete e senza scelta
automatica: ogni CRS proiettato della tabella (3.780 punti), ogni
trasformazione senza griglia avanti e inversa (2.214), ogni CRS della tabella
da e verso WGS 84 e le coppie rappresentative (Gauss-Boaga ↔ RDN2008 UTM
32N, ED50 UTM ↔ ETRS89 UTM, OSGB36 ↔ WGS 84 ed ETRS89, RD New ↔ ETRS89,
LV95 ↔ ETRS89, NAD27 ↔ NAD83, GDA94 ↔ GDA2020, DHDN, Lambert-93, LAEA; 1.660
catene), e una griglia NTv2 sintetica a due livelli con il suo
`hgridshift` (148 punti, e in catena come EPSG:9734). La prova
(`crs::riproiezione::oracolo`) chiede **1 mm** ovunque e stampa gli scarti
massimi (`cargo test -p plenora-core oracolo -- --nocapture`):

| famiglia | scarto massimo da PROJ |
| --- | --- |
| proiezioni, avanti e indietro | 1,1e-8 m (tabella sopra) |
| traslazioni geocentriche, avanti / inversa | 2,2e-9 m / 3,1e-9 m |
| Position Vector, avanti / inversa | 1,6e-9 m / 3,2e-9 m |
| Coordinate Frame, avanti / inversa | 1,6e-9 m / 3,1e-9 m |
| griglia NTv2, avanti / inversa | 4,8e-6 m / 4,8e-6 m |
| catene con cambio di datum | 6,7e-4 m (3035 → 4326: l'inversa LAEA di PROJ usa una serie troncata per la latitudine autalica; la nostra, per Newton, torna al punto entro 1e-8 m) |
| catene nello stesso datum / con la griglia | 6,7e-9 m / 4,1e-6 m |

**Vicino alle singolarità** (poli, bordi dei domini, limiti di Mercator)
`scripts/genera_riferimenti_singolari.py` scrive `singolari.csv` con le
formule chiuse EPSG a 60 cifre (mpmath 1.3.0, strumento di sviluppo, non
dipendenza): LAEA a 5 cm e a 1 mm dal polo, Mercator e Pseudo Mercator a
±85,05°, LCC, stereografica e svizzera ai bordi dei domini. Scarto massimo
7,5e-9 m avanti e indietro. L'inversa e la diretta di LAEA calcolano `1 -
sin(beta)` dalla colatitudine: prima `sin(phi)` arrotondava a 1 a pochi
millimetri dal polo e il punto finiva sul polo (5,6 cm a terra). Transverse
Mercator resta fuori (la forma di Karney è stabile fino al polo).

Rigenerare: `PYTHONPATH=<dir> python -B scripts/genera_riproiezione.py`,
poi `PYTHONPATH=<dir> python -B scripts/genera_oracolo_riproiezione.py`,
`cargo fmt` e i test. I generatori controllano la terna
pyproj/PROJ/EPSG come `genera_crs_integrati.py`.

## Limiti dichiarati della riproiezione

- **Aree d'uso come riquadri.** *Regola:* un passo si applica a un punto se
  il punto sta nel riquadro lon/lat dell'area d'uso EPSG (nel datum
  d'ingresso del passo, anche nel verso inverso). *Ambito:* scelta del
  percorso per geometria. *Hazard:* il riquadro è più largo dell'area vera,
  come in PROJ: il riquadro di «Italy - mainland» contiene la Sardegna, e un
  punto sardo isolato prende la trasformazione sarda solo perché il suo
  riquadro, più piccolo, viene prima; una trasformazione di buona
  accuratezza su un'area offshore può coprire terraferma nel suo riquadro.
  Le geometrie con punti che preferiscono percorsi diversi si rifiutano; fra
  i punti trasformati il controllo dei lati usa la corda lon/lat sorgente
  contro il riquadro comune dei passi di ogni percorso precedente (per
  eccesso: può rifiutare un lato che sfiora un riquadro senza che quel
  percorso lo copra davvero, per esempio fuori dalla sua griglia). L'accuratezza EPSG vale
  nell'area vera, non nel riquadro. *Rientro:* poligoni delle aree d'uso
  (non nel `proj.db` distribuito), o `trasformazioni` per fissare il
  percorso.
- **Accuratezza sommata.** La somma delle accuratezze EPSG dei passi è per
  eccesso rispetto alla somma quadratica; l'accuratezza di WGS 84 ed ETRS89
  come insiemi di realizzazioni (2 m e 0,1 m nel registro) non si aggiunge:
  vale quella delle trasformazioni, come in PROJ.
- **EPSG:2056 non segue alla lettera Hotine B.** *Regola:* con azimut e
  angolo del reticolo di 90° si usa la Swiss Oblique Mercator (formule
  swisstopo, `somerc` di PROJ). *Hazard:* le formule letterali di Hotine
  variant B (aposfera) differiscono fino a 9 cm ai bordi dell'area d'uso;
  qui si segue il riferimento nazionale e PROJ. Ogni altra Hotine B si
  rifiuta (nessuna nella tabella). *Rientro:* nessuno, è la scelta del
  riferimento.
- **Inversa di Helmert algebrica.** Il verso inverso di Position Vector e
  Coordinate Frame è l'inversa della formula (`+inv +proj=helmert` di PROJ),
  non il cambio di segno dei parametri che il registro indica come
  approssimazione: differenza di pochi millimetri, molto sotto
  l'accuratezza di ogni trasformazione fuori dalla precisione.
- **CRS 2D: l'altezza si scarta.** Come PROJ, il cambio di datum parte da
  altezza ellissoidica nulla e scarta quella d'arrivo: andata e ritorno
  attraverso un cambio di datum torna al punto entro 3,1 mm (oracolo), non
  al nanometro.
- **Traslazioni nulle come identità.** Con tre traslazioni nulle
  (NAD83, NZGD2000, RGF93, IGM95, SIRGAS, RDN2008 verso WGS 84 o ETRS89)
  lon/lat restano invariate anche fra GRS 1980 e WGS 84, come `+proj=noop`
  di PROJ; il passaggio geocentrico sposterebbe la latitudine di circa 0,1
  mm.
- **Densificazione a campioni.** Lo scarto di un lato si misura in tre punti
  e nei due versi, con la continuità delle metà: è una verifica, non una
  dimostrazione. Una curva che oscilli fra i campioni di un lato lungo
  potrebbe scostarsene di più (per uno scarto a S circa il 3% oltre il
  valore campionato, dentro il margine di mezza precisione); sulle
  proiezioni della tabella (lisce nei loro domini) il controllo a 2.000
  punti della prova resta entro la precisione. Con una griglia NTv2 la
  verifica è per cella (sopra). La copertura di una griglia si prova sui
  punti trasformati: un lato può uscire da una griglia e rientrarvi fra due
  punti di celle diverse solo se la sua immagine è sotto la tolleranza.
- **Griglie non verificate contro il registro.** Il file di `griglie` si
  lega al codice EPSG dichiarato dall'utente: che sia davvero la griglia di
  quella trasformazione (e quindi che valga la sua accuratezza) non si
  controlla (il file non porta il codice). Un file sbagliato ma ben formato
  dà spostamenti sbagliati senza errore.
- **Nel runner, un passo per volta.** Il runner esegue `geo.reproject`
  con `reproject_batches` e analizza il passo seguente sul contratto con
  il CRS d'arrivo ([«Operazioni geo»](runner.md#operazioni-geo)). Un esecutore
  futuro che fonda le trasformazioni in place (`TransformInPlace`) deve
  rileggere il CRS dopo `geo.reproject`, che lo cambia a metà del gruppo.
- **Fuori ambito.** CRS fuori tabella, operazioni concatenate del registro,
  griglie non NTv2, percorsi di più di tre passi, CGCS2000 verso altri
  datum (nessuna trasformazione nel registro), coordinate Z/M.
