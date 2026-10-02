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
d'autorità fuori tabella fallisce con `CRS_NOT_BUILTIN`; WKT, WKT2, PROJJSON
e proj-string con `CRS_BACKEND_UNAVAILABLE`.

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

## Aggiungere un codice

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
   formattare con `cargo fmt --all`;
4. aggiornare l'elenco atteso in
   `crates/plenora-core/src/crs/integrati/tests.rs`;
5. rigenerare i parametri di riproiezione e l'oracolo
   (`scripts/genera_riproiezione.py`, poi
   `scripts/genera_oracolo_riproiezione.py`, stesso ambiente) e rieseguire i
   test: l'oracolo pretende ogni CRS proiettato e ogni trasformazione senza
   griglia della tabella.

Il generatore rifiuta con un errore esplicito ciò che non sa descrivere: CRS
deprecati, non bidimensionali, con meridiano diverso da Greenwich, unità
diverse da gradi o metri, assi non nord/est, metodi di proiezione senza una
regola di dominio. Una nuova regola di dominio è una decisione da prendere in
PR, non un default.
