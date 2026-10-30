# CRS integrati

Senza PROJ, `plenora_core::crs::resolve_crs` risolve gli identificatori
d'autorità di una tabella integrata, generata dal registro EPSG. Descrive i
CRS (tipo, unità, assi, area d'uso, dominio di validità, ellissoide); la
riproiezione fra questi CRS è in [«Riproiezione»](riproiezione.md#riproiezione). Per gli
identificatori in tabella la risoluzione è qui; per ogni altra definizione
vale la voce
«Risoluzione CRS fuori tabella» di
[«Che cosa non c'è ancora»](../README.md#che-cosa-non-cè-ancora).

**Fonte.** Registro EPSG v11.022 (2024-11-05) come distribuito con PROJ
9.5.1, letto con pyproj 3.7.2 da `scripts/genera_crs_integrati.py`, che
scrive `crates/plenora-core/src/crs/epsg_integrati.rs` (generato, non si
modifica a mano). Le costanti `BUILTIN_EPSG_VERSION`, `BUILTIN_EPSG_DATE` e
`BUILTIN_PROJ_VERSION` riportano la fonte nel codice.

**Che cosa c'è** (169 CRS, tutti bidimensionali, meridiano di Greenwich,
gradi o metri):

| gruppo | codici |
| --- | --- |
| Italia | 4265 Monte Mario; 3003, 3004 Monte Mario / Italy zone 1 e 2 (Gauss-Boaga); 4670 IGM95; 3064, 3065 IGM95 / UTM 32N, 33N; 6706 RDN2008; 6707, 6708, 6709 RDN2008 / UTM 32N, 33N, 34N **(N-E)**; 7791, 7792, 7793 RDN2008 / UTM 32N, 33N, 34N (E-N); 6875 RDN2008 / Italy zone (N-E); 7794 RDN2008 / Italy zone (E-N); 4230 ED50; 23032, 23033, 23034 ED50 / UTM 32N, 33N, 34N |
| mondo | 4326 WGS 84; `OGC:CRS84`; 3857 Pseudo-Mercator; 3395 World Mercator; 4258 ETRS89; 3035 ETRS89-extended / LAEA Europe; 4269 NAD83; 4267 NAD27; 4283 GDA94; 7844 GDA2020; 4171 RGF93 v1; 2154 RGF93 v1 / Lambert-93; 4277 OSGB36; 27700 British National Grid; 4674 SIRGAS 2000; 4490 CGCS2000; 2056 CH1903+ / LV95; 31467 DHDN / 3-degree Gauss-Kruger zone 3; 28992 Amersfoort / RD New; 2193 NZTM2000 |
| fusi UTM | WGS 84 32601–32660 e 32701–32760; ETRS89 25828–25837 |

Restano fuori, di proposito: 25838 (ETRS89 / UTM zone 38N, deprecato nel
registro), i CRS 3D (per esempio 4979) e i CRS con meridiano fondamentale
diverso da Greenwich (per esempio 4806, Monte Mario (Rome)). Attenzione ai
nomi: nel registro 6707–6709 sono le varianti **northing-first** e
7791–7793 quelle easting-first.

**Forme accettate.** `EPSG:<codice>` (autorità senza distinzione di
maiuscole, codice senza zeri iniziali),
`urn:ogc:def:crs:EPSG:<versione>:<codice>` (versione vuota o numerica),
`OGC:CRS84` e `urn:ogc:def:crs:OGC:<versione>:CRS84`. Due forme dello stesso
codice sono semanticamente uguali (il canonical dipende dal CRS, non dal
testo); `EPSG:4326` (lat/lon) e `OGC:CRS84` (lon/lat) no. Un identificatore
d'autorità fuori tabella fallisce con `CRS_NOT_BUILTIN`. Sono accettati anche
i PROJJSON, WKT1 con assi espliciti e WKT2 completi equivalenti a una voce
integrata; le proj-string restano `CRS_BACKEND_UNAVAILABLE`.

**Area d'uso e dominio di validità.** Sono due cose diverse:

- l'**area d'uso EPSG** (riquadro lon/lat del registro e, per i proiettati,
  il suo inviluppo proiettato) è un metadato e non rifiuta dati. I dati reali
  la superano di norma: ISTAT pubblica i confini di tutta l'Italia in UTM
  32N, fino a circa 9,5° dal meridiano centrale, fuori dall'area 6°–12° E del
  fuso;
- il **dominio di validità** è il controllo: `validate_geometry_domain`
  rifiuta con `COORDINATE_OUT_OF_CRS_DOMAIN` una coordinata che ne esce, senza
  riportarne il valore. È un controllo contro un CRS sbagliato o coordinate
  prive di senso, non una garanzia di precisione. La regola è fissa per
  famiglia di proiezione:
  - geografici: longitudine −180…180, latitudine −90…90;
  - Transverse Mercator (UTM, Gauss-Boaga, GK, British National Grid, NZTM,
    RDN2008 Italy zone): inviluppo della regione a ±15° di longitudine dal
    meridiano centrale (l'algoritmo di Krüger/Karney usato da PROJ resta
    accurato a pochi nanometri entro 3900 km dal meridiano centrale).
    Latitudini: 0…84 per gli UTM nord e −80…0 per i sud, estese quando l'area
    EPSG le supera (32629 e 25832 arrivano a 84,01); per i TM nazionali l'area
    EPSG allargata del 50% dell'estensione per lato;
  - Mercator (3857, 3395): longitudine ±180, latitudine ±85,06;
  - altri reticoli (Lambert-93, LAEA Europe, LV95, RD New): inviluppo
    proiettato dell'area EPSG allargato del 50% dell'estensione per lato.

  Gli inviluppi li calcola il generatore con PROJ (solo la conversione dal CRS
  geografico di base, bordi campionati e raffinati), spostati di un
  micrometro e arrotondati al millimetro verso l'esterno: un limite esatto
  come l'equatore degli UTM (northing 0) diventa −0,001. Il dominio contiene
  sempre l'area d'uso.

**Dove gira il controllo.** Nell'analisi dei contratti geo, sulle geometrie
che arrivano dalla config con il CRS dell'input: `other_wkb` di distanze e
predicati e la lama `other_wkb` di `geo.split`, `point_wkb` di
`line_locate_point`, `reference_wkb` di `snap`, e
l'`extent` di `generate_grid` con il CRS del produttore. I kernel non
ricevono un CRS: sulle colonne il controllo lo chiama il chiamante, con
`plenora_kernels_geo::crs::validate_geometry_domain`, dopo la decodifica;
nel runner, su ogni colonna geometria d'ingresso di un passo geo e sulle
geometrie prodotte ([«Operazioni geo»](runner.md#operazioni-geo)).

**Precisione.** `ResolvedCrs::precisione_coordinate()` esprime 1 cm a terra
nelle unità del CRS: `0.01 / horizontal_unit_to_metre` per i proiettati,
`0.01` metri in gradi sul raggio di curvatura massimo `a / (1 - f)`
dell'ellissoide per i geografici; `None` per un geografico senza
ellissoide o quando il quoziente non
è un `f64` normale e positivo. È la precisione dichiarata delle operazioni
geografiche ([«Limiti dichiarati»](limiti.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)):
`Precision::from_crs` dei kernel geo delega a questa funzione, e un `None` è
`InvalidPrecision`.

## Limiti dei CRS integrati

**Regola.** Il dominio di validità di un proiettato è un rettangolo in
easting/northing, non la regione esatta: senza una proiezione inversa il
punto non si riporta in lon/lat.

**Ambito.** `plenora_core::crs::validate_geometry_domain` sui CRS proiettati
della tabella integrata.

**Hazard.**

- il rettangolo è più largo della regione che descrive: vicino ai poli un
  punto UTM può stare nel rettangolo e oltre i 15° dal meridiano centrale;
- gradi dati per errore a un fuso UTM nord passano: sono un punto vicino
  all'equatore dentro il fuso, e le sole coordinate non lo distinguono (fusi
  sud, Gauss-Boaga e reticoli nazionali invece li rifiutano);
- un fuso UTM copre un solo emisfero: dati che attraversano l'equatore nello
  stesso fuso (northing negativi in un fuso nord, pratica comune in Kenya e
  Uganda) sono rifiutati, e vanno scritti nel fuso dell'altro emisfero;
- i reticoli nazionali non TM accettano solo il 50% dell'estensione oltre
  l'area EPSG per lato: dati lontani dal territorio (per esempio la
  piattaforma continentale olandese in RD New, verso 55,7° N) sono rifiutati;
- il canonical è un sottoinsieme del PROJJSON (tipo, nome, sistema di
  coordinate, `id`): è stabile qui, ma non è confrontabile con un canonical
  prodotto da PROJ in `plenora-data-tools`;
- le coordinate si leggono nell'ordine GIS normalizzato; un dato scritto
  nell'ordine d'autorità di un CRS northing-first (6707, 3035) va
  normalizzato prima del controllo.

**Condizione di rientro.** Una proiezione inversa verificata che riporti il
punto in lon/lat e lo confronti con la regione esatta: `geo.reproject` lo fa
già ([«La catena»](riproiezione.md#la-catena)); `validate_geometry_domain` resta sul
rettangolo.

## Definizioni PROJJSON

Il profilo [PROJJSON](https://proj.org/en/stable/specifications/projjson.html)
riconosce `GeographicCRS` ellissoidici 2D e `ProjectedCRS` 2D, con datum o
ensemble, assi espliciti e unità metro/grado; `id` o un solo elemento in
`ids` alla radice è obbligatorio. Ogni campo semantico deve coincidere con
il registro fissato: datum storici WGS84/ETRS89 non equivalgono agli ensemble.
Nomi descrittivi, `$schema`, scope, area, bbox, usages e remarks non decidono
l'identità, ma la loro forma viene verificata. Campi sconosciuti si rifiutano.
Greenwich e le unità implicite dell'ellissoide seguono il default PROJJSON.

Il limite è 64 KiB, profondità 32 e budget strutturale conservativo 8.192
(elementi, chiavi e separatori). Sintassi, duplicati, limiti e numeri non
esatti danno `CRS_INVALID`; documento incompleto o non equivalente,
ID mancante/estraneo, costrutti non supportati danno `CRS_NOT_BUILTIN`.
I messaggi non includono valori del documento. Fuzz e qualifica della
matrice interop restano alla PR 4.

Risolvere non modifica le coordinate. EPSG:4326 richiede assi lat/lon nella
definizione, CRS84 lon/lat; le ordinate dei dati possono essere `lon_lat`.
EPSG:6707 richiede northing/easting nella definizione, mentre i dati GIS
restano easting/northing. `axis_order=unknown` resta ignoto.

Le prove in `crates/plenora-io/tests/definizioni_projjson.rs` coprono tutti i
169 documenti, mutazioni, round-trip IPC/GeoParquet e una definizione Monte
Mario estratta senza riserializzazione dalla fixture Arrow canonica di
IO-tools. `scripts/estrai_projjson_io_tools.py` la rigenera dal checkout
IO-tools; la provenienza riporta gli hash del file e dei byte estratti.

## Aggiungere un codice

Il core contiene anche il modello semantico privato in
`crates/plenora-core/src/crs/definizioni/`: datum o ensemble con membri e
accuratezza, ellissoide, meridiano fondamentale, CRS geografico di base,
conversione, metodo e parametri identificati, unità e assi ordinati.
Il parser PROJJSON alimenta `resolve_crs` e tutti i percorsi pubblici
elencati sopra. Il riconoscimento richiede l'ID radice EPSG
(oppure l'identità distinta `OGC:CRS84`) e verifica tutta la semantica.
Non scambia ordinate e non adotta contratti opt-in.

`semantica.json` e `metodi_generati.rs` sono generati da
`scripts/genera_projjson_integrati.py`, tramite
`scripts/genera_semantica_crs.py`, nello stesso ambiente vincolato. Il primo
è la codifica del modello Rust, non un nuovo formato pubblico. Il secondo
collega nelle prove i codici dei parametri ai campi della riproiezione.
I soli alias senza ID sono i nomi canonici del registro: non si equiparano
datum storici ed ensemble, né WGS84 ed ETRS89. Gli ID annidati presenti
devono coincidere. Il modello richiede assi espliciti, anche nella base.

Il modello confronta numeri come decimali esatti:
`500000`, `500000.0` e `5e5` coincidono. Il token è limitato a 64 KiB;
l'esponente normalizzato deve rientrare in `i64`, altrimenti il rifiuto è
`CRS_INVALID`. Nel parser PROJJSON `ensure_numbers_exact` precede ogni
conversione a `Value`: con `float_roundtrip` impedisce la perdita di cifre.
Overflow, underflow e numeri che cambierebbero valore sono `CRS_INVALID`. Per gli ellissoidi
si conserva la forma del registro (inverso dello schiacciamento oppure
semiasse minore), senza conversioni approssimate tra forme. Le unità
ammesse sono metro, grado e unità adimensionale; il fattore angolare
serializzato ammesso è `0.0174532925199433`, senza epsilon.

L'oracolo legge il corpus PROJJSON già usato da GeoParquet, verifica tutti
gli ID e le proprietà contro le tabelle integrate e operative, poi muta
singolarmente campi e cardinalità. La coerenza dei parametri operativi
verifica la serializzazione PROJJSON a 15 cifre significative: non è una
tolleranza del riconoscimento. Per verificare la rigenerazione senza
scrivere: `PYTHONPATH=<dir> python scripts/genera_semantica_crs.py --verifica`.
La guardia `scripts/test_semantica_crs.py` confronta offline il modello
con il corpus usando decimali esatti e gira con le altre guardie Python.

1. aggiungerlo a una delle liste di `scripts/genera_crs_integrati.py`
   (`ITALIA`, `MONDO` o i fusi UTM);
2. preparare fuori dal workspace l'ambiente che ha prodotto la tabella:
   pyproj 3.7.2 come ruota binaria che include PROJ 9.5.1 e il registro EPSG
   v11.022. Quella usata è `cp311-cp311-win_amd64` (CPython 3.11, Windows
   x64):
   `python -m pip install --only-binary=:all: --target <dir> pyproj==3.7.2`.
   Altre ruote o una build contro un PROJ di sistema possono portare un altro
   PROJ (per esempio 9.8.1 con EPSG v12.029) e limiti diversi: il generatore
   controlla la terna pyproj/PROJ/EPSG e si rifiuta di girare se non coincide.
   Cambiare versione è una decisione da prendere in PR, con il diff dei dati;
3. rigenerare con `PYTHONPATH=<dir> python scripts/genera_crs_integrati.py` e
   formattare con `cargo fmt`;
4. aggiornare l'elenco atteso in
   `crates/plenora-core/src/crs/integrati/tests.rs`;
5. rigenerare i parametri di riproiezione e l'oracolo
   (`scripts/genera_riproiezione.py`, poi
   `scripts/genera_oracolo_riproiezione.py`, stesso ambiente) e rieseguire i
   test: l'oracolo pretende ogni CRS proiettato e ogni trasformazione senza
   griglia della tabella.
6. rigenerare PROJJSON e modello semantico con
   `PYTHONPATH=<dir> python scripts/genera_projjson_integrati.py`.

Il generatore rifiuta con un errore esplicito ciò che non sa descrivere: CRS
deprecati, non bidimensionali, con meridiano diverso da Greenwich, unità
diverse da gradi o metri, assi non nord/est, metodi di proiezione senza una
regola di dominio. Una nuova regola di dominio è una decisione da prendere in
PR, non un default.

## Definizioni WKT2

Il parser Rust legge un profilo chiuso di WKT2:2015 e WKT2:2019
([OGC 18-010r11](https://docs.ogc.org/is/18-010r11/18-010r11.pdf)).
Le radici ammesse sono `GEOGCRS`/`GEOGRAPHICCRS`,
`GEODCRS`/`GEODETICCRS` ellissoidici 2D e `PROJCRS`/`PROJECTEDCRS` 2D.
La sequenza è nome, datum/ensemble e meridiano oppure base e conversione,
sistema di coordinate, metadati d'uso opzionali, ID radice, remark opzionale.
`BASEGEOGCRS` e `BASEGEODCRS` non contengono CS: nel modello interno
la base è rappresentata in lat/lon e gradi. Questa convenzione non
attribuisce un ordine alle coordinate memorizzate e non scambia ordinate.

I nodi semantici sono `DATUM`/`GEODETICDATUM`, `ENSEMBLE`, `MEMBER`,
`ELLIPSOID`, `ENSEMBLEACCURACY`, `PRIMEM`/`PRIMEMERIDIAN`, `CONVERSION`,
`METHOD`, `PARAMETER`, `CS`, `AXIS`, `ORDER`, `LENGTHUNIT`, `ANGLEUNIT`,
`SCALEUNIT`, `ID`. Le unità sono esplicite su ellissoide, meridiano e
parametri; per gli assi compaiono su entrambi oppure una volta sul CS.
`ORDER` compare su entrambi gli assi oppure su nessuno; non si riordinano
assi scritti in ordine diverso. Sono ammessi scope, area, bbox e remark
con forma verificata; ogni `USAGE` richiede scope ed estensione e può
ripetersi. I metadati sciolti 2015 non si mescolano con `USAGE`.
Un solo `ID[autorità,codice]` per entità, senza versione/URI; quello radice
è obbligatorio. Ogni ID annidato presente deve coincidere con la tabella.

Le keyword e le direzioni ignorano il caso ASCII; i nomi senza ID sono
sensibili al caso. Gli alias di metodo ammessi senza ID sono soltanto
questi sette nomi canonici del registro:

- `Transverse Mercator`;
- `Popular Visualisation Pseudo Mercator`;
- `Mercator (variant A)`;
- `Lambert Azimuthal Equal Area`;
- `Lambert Conic Conformal (2SP)`;
- `Hotine Oblique Mercator (variant B)`;
- `Oblique Stereographic`.

I nomi dei parametri senza ID ammessi sono `Latitude of natural origin`,
`Longitude of natural origin`, `Scale factor at natural origin`,
`False easting`, `False northing`, `Latitude of false origin`,
`Longitude of false origin`, `Latitude of 1st standard parallel`,
`Latitude of 2nd standard parallel`, `Easting at false origin`,
`Northing at false origin`, `Latitude of projection centre`,
`Longitude of projection centre`, `Azimuth at projection centre`,
`Angle from Rectified to Skew Grid`, `Scale factor at projection centre`,
`Easting at projection centre`, `Northing at projection centre`.
Per unità, datum, membri, ellissoidi e meridiano valgono solo i nomi canonici
in `crates/plenora-core/src/crs/definizioni/semantica.json`: nessun alias
storico o euristico. Le unità sono `metre`, `degree`, `unity`, rispettivamente
con fattori esatti `1`, `0.0174532925199433`, `1`; un ID corretto non
rende accettabile un fattore diverso.

**Limiti.** Massimo 64 KiB, 32 livelli di nodi e 8.192 valori/nodi,
controllati prima di allocare o discendere. Delimitatori quadri o tondi
accoppiati; stringhe con virgolette doppie raddoppiate; spazi ASCII.
I decimali richiedono cifre su entrambi i lati del punto, se presente,
e ammettono segno ed esponente. Il confronto conserva tutte le cifre:
`500000`, `500000.0` e `5e5` coincidono; `500000.00000000001` è diverso.
Non si passa da `f64`, non si introducono epsilon. Un esponente non
rappresentabile dal normalizzatore è errore, non overflow/underflow silenzioso.
Il documento deve essere consumato integralmente; nodi sconosciuti,
duplicati semantici, dimensioni diverse da 2, CRS dinamici, composti,
verticali, geocentrici, bound e trasformazioni incorporate sono rifiutati.
La mancanza di un ID radice non attiva il riconoscimento per somiglianza.

**Errori.** Lessico/sintassi malformati, limiti e numeri non normalizzabili
producono `CRS_INVALID`. Struttura fuori dal profilo, cardinalità/ordine
semantici errati, ID assente o discordante e differenze esatte producono
`CRS_NOT_BUILTIN`. I messaggi non contengono dati della definizione.
WKT1 usa gli stessi errori e lo stesso confronto; il suo profilo è
descritto nella sezione seguente.

**Corpus e condizione di rientro.** `scripts/genera_wkt_integrati.py` usa
pyproj 3.7.2 / PROJ 9.5.1 / EPSG v11.022 e verifica ogni PROJJSON contro
la sorgente già integrata prima di esportare WKT2 senza ritocchi.
`crates/plenora-core/src/crs/definizioni/wkt_corpus.json` contiene entrambi
gli export per tutti i 169 CRS e il motivo di ogni esclusione.
WKT2:2019 ne risolve **168**; WKT2:2015 ne risolve **32**. Il 2015 perde
gli ensemble WGS84/ETRS89: i datum storici non sono equivalenti.
In entrambe le versioni EPSG:4267 (NAD27) è rifiutato: Clarke 1866 è
definito dai semiassi `6378206.4` e `6356583.8`, mentre l'export WKT usa
l'inverso `294.978698213898`, arrotondato. Accettarlo richiederebbe una
regola ulteriore di equivalenza delle rappresentazioni dell'ellissoide;
non rientra nella decisione di equivalenza decimale esatta. Non si rivendica
quindi l'accettazione di tutti i 169 export WKT2.

Le prove mutano separatamente numeri, metodo, parametri, ensemble, ID,
unità e assi. I round-trip IPC/GeoParquet e `target_crs` passano dai percorsi
pubblici in `crates/plenora-io/tests/definizioni_wkt.rs`, con coordinate
asimmetriche e le tre rappresentazioni Arrow. Risolvere conserva il testo
originale e usa solo i parametri operativi della tabella integrata.


## Definizioni WKT1

Il profilo accetta `GEOGCS` e `PROJCS` bidimensionali. La sequenza geografica
è nome, `DATUM` con `SPHEROID`, `PRIMEM`, `UNIT`, due `AXIS`, `AUTHORITY`.
Quella proiettata è nome, `GEOGCS` completo, `PROJECTION`, `PARAMETER`,
`UNIT`, due `AXIS`, `AUTHORITY`. Gli assi sono obbligatori anche nel
`GEOGCS` interno: non si ricavano dall'EPSG. L'ordine scritto deve coincidere
con quello della tabella; i nomi degli assi sono descrittivi. Nessuna
permutazione delle coordinate dei dati.

`AUTHORITY["autorità","codice"]` richiede due stringhe: obbligatoria alla
radice, opzionale su base, datum, ellissoide, meridiano, unità e metodo.
Le identità presenti si confrontano con il registro. I nomi senza autorità
sono soltanto quelli canonici, sensibili al caso: non si convertono
arbitrariamente underscore, spazi o prefissi ESRI. In particolare un nome
di datum WKT1 con underscore privo di autorità può essere rifiutato.

I quattro alias di metodo sono chiusi; l'eventuale autorità di `PROJECTION`
deve concordare anche con l'alias, perché questo determina i parametri:

| Alias WKT1 | Metodo EPSG |
| --- | --- |
| `Transverse_Mercator` | 9807, Transverse Mercator |
| `Lambert_Conformal_Conic_2SP` | 9802, Lambert Conic Conformal (2SP) |
| `Hotine_Oblique_Mercator_Azimuth_Center` | 9815, Hotine Oblique Mercator (variant B) |
| `Oblique_Stereographic` | 9809, Oblique Stereographic |

Per Transverse Mercator e Oblique Stereographic, `latitude_of_origin`,
`central_meridian`, `scale_factor`, `false_easting`, `false_northing`
indicano latitudine/longitudine dell'origine naturale, fattore di scala
all'origine naturale e falsi est/nord. Per Lambert 2SP, origine e falsi
est/nord sono riferiti alla falsa origine; `standard_parallel_1` e
`standard_parallel_2` indicano i due paralleli standard. Per Hotine B,
`latitude_of_center`, `longitude_of_center`, `azimuth`,
`rectified_grid_angle`, `scale_factor`, `false_easting`, `false_northing`
indicano rispettivamente centro, azimut, angolo di rettifica, scala al
centro e coordinate est/nord del centro. Nessun altro alias è ammesso;
parametri mancanti, duplicati o in più sono rifiutati.

Le unità sono `degree` e `metre` (oppure autorità corrispondente), con i
fattori decimali esatti già indicati per WKT2. Il semiasse dello `SPHEROID`
è in metri, il secondo numero è l'inverso dello schiacciamento;
meridiano e parametri angolari ereditano la `UNIT` geografica, i parametri
lineari quella proiettata, la scala è adimensionale (`unity`, fattore 1).
Non si convertono unità o numeri attraverso `f64`.

Si applicano limiti lessicali, consumo integrale ed errori del profilo WKT2.
`TOWGS84`, `EXTENSION`, nodi WKT2 mescolati, assi omessi (inclusi gli export
ESRI), CRS verticali, geocentrici e composti sono fuori profilo:
`CRS_NOT_BUILTIN`. I datum storici WGS84/ETRS89 non sostituiscono gli
ensemble, nemmeno con lo stesso codice EPSG. Le proj-string restano
`CRS_BACKEND_UNAVAILABLE`.

**Corpus.** Nello stesso `wkt_corpus.json`, per ciascuno dei 169 CRS:
`wkt1_gdal` è l'export PROJ intatto con `output_axis_rule=True`;
`wkt1_assi` sostituisce, nei proiettati, il solo `GEOGCS` interno con
l'export autonomo della base dal suo codice, sempre con assi espliciti.
Il generatore verifica che l'export autonomo senza assi coincida esattamente
con l'unica base da sostituire: non corregge numeri o datum.
Questa variante è necessaria perché PROJ omette gli assi della base anche
con `output_axis_rule=True`. Entrambe le forme hanno un motivo di esclusione
per ciascun caso rifiutato (`esclusione_wkt1_gdal`, `esclusione_wkt1`).
La variante esplicita risolve **32** CRS: **136** sono esclusi per perdita
dell'ensemble e **1** (NAD27) per l'inverso dello schiacciamento arrotondato.
Il corpus grezzo dei proiettati è inoltre rifiutato per assi della base assenti.

Le prove verificano tutte le voci e mutano ogni parametro, identità,
ellissoide, meridiano, unità e assi. IPC e GeoParquet conservano le coordinate
asimmetriche; `target_crs` WKT1 coincide con il percorso per codice.
Le chiavi Arrow canoniche usano formato `wkt`; è accettata anche la forma
legacy `geo`. I metadati GeoArrow standard ammettono WKT2, non WKT1.
