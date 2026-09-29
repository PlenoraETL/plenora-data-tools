//! Riproiezione fra CRS della tabella integrata, in Rust puro.
//!
//! Catena per ogni punto, in coordinate GIS normalizzate
//! (longitudine/easting prima):
//!
//! 1. dominio di validita' del CRS sorgente (rettangolo proiettato o mondo
//!    lon/lat), proiezione inversa, regione lon/lat del dominio (Transverse
//!    Mercator e Mercator);
//! 2. cambio di datum lungo un **percorso** di trasformazioni EPSG
//!    ([`PercorsoDatum`]): Helmert senza griglia o griglie `NTv2` fornite
//!    dall'utente, ciascuna con la propria area d'uso;
//! 3. regione lon/lat e proiezione diretta del CRS d'arrivo, poi il suo
//!    dominio di validita'.
//!
//! **Regola dell'accuratezza.** L'accuratezza di un percorso e' la somma
//! delle accuratezze EPSG dei suoi passi (per eccesso: gli errori non si
//! compensano per ipotesi). Un percorso entro la precisione
//! ([`GROUND_PRECISION_METRES`], 1 cm) e' sempre ammesso, quindi anche lo
//! stesso datum; oltre, solo se [`OpzioniRiproiezione::accuratezza_accettata_m`]
//! lo copre. Nessun ripiego silenzioso: se nessun percorso ammesso copre
//! un punto, e' un errore.
//!
//! [`PianoRiproiezione`] decide i percorsi senza leggere file (analisi a
//! secco); [`Riproiettore`] aggiunge le griglie lette e trasforma.

mod datum;
mod epsg;
mod latitudine;
mod ntv2;
mod percorsi;
mod proiezioni;
mod tabella;

use std::collections::{BTreeMap, BTreeSet};

pub use ntv2::{GrigliaNtv2, MAX_BYTE_GRIGLIA};

use super::{
    validate_geometry_domain, CoordinateDomainViolation, CrsError, ResolvedCrs,
    GROUND_PRECISION_METRES,
};
use datum::{Geocentrico, Helmert};
use proiezioni::{ErroreProiezione, Proiezione};
use tabella::{Definizione, MetodoTrasformazione, Trasformazione};

/// Versione del registro EPSG dei parametri di riproiezione.
pub const VERSIONE_EPSG_RIPROIEZIONE: &str = epsg::VERSIONE_EPSG;
/// Versione di PROJ che distribuiva quel registro.
pub const VERSIONE_PROJ_RIPROIEZIONE: &str = epsg::VERSIONE_PROJ;

/// Margine del confronto fra accuratezza accettata e somma delle
/// accuratezze dei passi: un nanometro, per l'arrotondamento della somma.
const TOLLERANZA_SOMMA_M: f64 = 1e-9;

/// Numero massimo di trasformazioni in un percorso fra datum.
pub const MAX_PASSI_PERCORSO: usize = 3;

/// Opzioni di una riproiezione (la config di `geo.reproject`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OpzioniRiproiezione {
    /// Accuratezza accettata, in metri, per un percorso fra datum oltre la
    /// precisione di 1 cm.
    pub accuratezza_accettata_m: Option<f64>,
    /// Codici EPSG delle trasformazioni `NTv2` di cui l'utente fornisce il
    /// file: solo queste griglie entrano nei percorsi.
    pub griglie: Vec<u32>,
    /// Codici EPSG delle trasformazioni da usare, nell'ordine: il percorso
    /// e' esattamente questo (verso di ogni passo dedotto dai datum).
    pub trasformazioni: Option<Vec<u32>>,
    /// Convenzione WGS 84 = famiglia ETRS89 (ETRS89, RDN2008): il passo
    /// ETRS89 to WGS 84 (1) conta accuratezza 0. Assente vale `true`;
    /// `false` usa l'accuratezza EPSG (1 m).
    pub convenzione_wgs84_etrs89: Option<bool>,
}

/// Un passo di un percorso: una trasformazione EPSG in un verso.
#[derive(Clone, Copy, Debug)]
pub struct PassoPercorso {
    trasformazione: &'static Trasformazione,
    inversa: bool,
    /// Il passo e' ETRS89 to WGS 84 (1) con la convenzione di equivalenza
    /// attiva: la sua accuratezza conta 0.
    convenzione: bool,
}

/// Codice EPSG di ETRS89 to WGS 84 (1), il solo passo fra WGS 84 e la
/// famiglia ETRS89 della tabella (RDN2008 vi arriva con EPSG:6710,
/// accuratezza 0).
pub const CODICE_ETRS89_WGS84: u32 = 1149;

impl PassoPercorso {
    /// Codice EPSG della trasformazione.
    #[must_use]
    pub const fn codice(&self) -> u32 {
        self.trasformazione.codice
    }

    /// Nome EPSG della trasformazione.
    #[must_use]
    pub const fn nome(&self) -> &'static str {
        self.trasformazione.nome
    }

    /// Il passo usa la trasformazione nel verso opposto a quello del
    /// registro.
    #[must_use]
    pub const fn inversa(&self) -> bool {
        self.inversa
    }

    /// Accuratezza del passo nel percorso, in metri: quella EPSG, o 0 per
    /// ETRS89 to WGS 84 (1) con la convenzione di equivalenza.
    #[must_use]
    pub const fn accuratezza_m(&self) -> f64 {
        if self.convenzione {
            0.0
        } else {
            self.trasformazione.accuratezza_m
        }
    }

    /// Accuratezza EPSG del registro, in metri, anche per convenzione.
    #[must_use]
    pub const fn accuratezza_registro_m(&self) -> f64 {
        self.trasformazione.accuratezza_m
    }

    /// Il passo conta 0 per la convenzione WGS 84 = ETRS89.
    #[must_use]
    pub const fn per_convenzione(&self) -> bool {
        self.convenzione
    }

    /// Il passo richiede una griglia `NTv2`.
    #[must_use]
    pub const fn a_griglia(&self) -> bool {
        self.trasformazione.a_griglia()
    }

    /// Nome del file di griglia nel registro EPSG (solo informativo: il
    /// file lo fornisce l'utente); `None` senza griglia.
    #[must_use]
    pub const fn file_registro(&self) -> Option<&'static str> {
        match self.trasformazione.metodo {
            MetodoTrasformazione::GrigliaNtv2 { file_registro } => Some(file_registro),
            _ => None,
        }
    }
}

/// Un percorso fra il datum sorgente e quello d'arrivo.
#[derive(Clone, Debug)]
pub struct PercorsoDatum {
    passi: Vec<PassoPercorso>,
    accuratezza_m: f64,
}

impl PercorsoDatum {
    /// I passi del percorso, dal datum sorgente a quello d'arrivo; vuoto
    /// per lo stesso datum.
    #[must_use]
    pub fn passi(&self) -> &[PassoPercorso] {
        &self.passi
    }

    /// Somma delle accuratezze EPSG dei passi, in metri; 0 per lo stesso
    /// datum.
    #[must_use]
    pub const fn accuratezza_m(&self) -> f64 {
        self.accuratezza_m
    }

    /// I codici EPSG dei passi, nell'ordine.
    #[must_use]
    pub fn codici(&self) -> Vec<u32> {
        self.passi.iter().map(PassoPercorso::codice).collect()
    }
}

/// Un lato della riproiezione: il CRS con la sua proiezione.
#[derive(Clone, Debug)]
struct Lato {
    definizione: &'static Definizione,
    proiezione: Proiezione,
    /// Nome del datum, per la diagnostica ([`PianoRiproiezione::datum`]).
    datum: &'static str,
    crs: ResolvedCrs,
}

impl Lato {
    fn nuovo(crs: &ResolvedCrs) -> Result<Self, CrsError> {
        let identificativo = crs.integrato.ok_or(CrsError::NotBuiltin)?;
        let definizione = tabella::definizione(identificativo).ok_or(CrsError::NotBuiltin)?;
        let datum = tabella::datum(definizione.datum).ok_or(CrsError::NotBuiltin)?;
        let proiezione = Proiezione::nuova(&definizione.metodo, datum.ellissoide)
            .map_err(|_| CrsError::ReprojectionConfig("parametri di proiezione non validi"))?;
        Ok(Self {
            definizione,
            proiezione,
            datum: datum.nome,
            crs: crs.clone(),
        })
    }

    /// La regione lon/lat del dominio contiene il punto. La regione dei
    /// fusi vicini all'antimeridiano esce da `[-180, 180]` (UTM 1N: da -192
    /// a -162): la longitudine si prova anche spostata di un giro.
    fn regione(&self, lon: f64, lat: f64) -> Result<(), CrsError> {
        let Some(regione) = self.definizione.regione else {
            return Ok(());
        };
        let dentro = (regione.south_latitude..=regione.north_latitude).contains(&lat)
            && [lon, lon - 360.0, lon + 360.0]
                .iter()
                .any(|l| (regione.west_longitude..=regione.east_longitude).contains(l));
        if dentro {
            Ok(())
        } else {
            Err(CrsError::CoordinateOutOfDomain {
                violation: CoordinateDomainViolation::OutsideProjectionRegion,
            })
        }
    }
}

const fn errore_proiezione(errore: ErroreProiezione) -> CrsError {
    match errore {
        ErroreProiezione::FuoriDominio | ErroreProiezione::Parametri => {
            CrsError::CoordinateOutOfDomain {
                violation: CoordinateDomainViolation::OutsideProjectionRegion,
            }
        }
        ErroreProiezione::NonConvergente => CrsError::ReprojectionNotConverged,
    }
}

/// I percorsi fra due CRS della tabella, decisi senza leggere file.
#[derive(Clone, Debug)]
pub struct PianoRiproiezione {
    sorgente: Lato,
    destinazione: Lato,
    /// Percorsi ammessi dalla regola dell'accuratezza, nell'ordine di
    /// preferenza (accuratezza, numero di passi, codici).
    percorsi: Vec<PercorsoDatum>,
    /// Stessa proiezione e stesso datum: le coordinate non cambiano.
    identita: bool,
}

impl PianoRiproiezione {
    /// Il piano fra `sorgente` e `destinazione`.
    ///
    /// # Errors
    ///
    /// - [`CrsError::NotBuiltin`] se uno dei due CRS non viene dalla
    ///   tabella integrata;
    /// - [`CrsError::ReprojectionConfig`] per opzioni non valide o senza
    ///   effetto (accuratezza non finita o negativa, o accettata senza
    ///   percorsi oltre la precisione; griglia che non e' una trasformazione
    ///   `NTv2` della tabella, ripetuta o fuori da ogni percorso ammesso;
    ///   `trasformazioni` che non formano un percorso fra i due datum);
    /// - [`CrsError::ReprojectionPathUnavailable`] se nessun percorso EPSG
    ///   collega i datum;
    /// - [`CrsError::ReprojectionAccuracyNotAccepted`] se nessun percorso
    ///   rispetta la regola dell'accuratezza.
    pub fn nuovo(
        sorgente: &ResolvedCrs,
        destinazione: &ResolvedCrs,
        opzioni: &OpzioniRiproiezione,
    ) -> Result<Self, CrsError> {
        let sorgente = Lato::nuovo(sorgente)?;
        let destinazione = Lato::nuovo(destinazione)?;
        if let Some(accettata) = opzioni.accuratezza_accettata_m {
            if !(accettata.is_finite() && accettata >= 0.0) {
                return Err(CrsError::ReprojectionConfig(
                    "accuratezza_accettata_m non finita o negativa",
                ));
            }
        }
        let mut griglie = BTreeSet::new();
        for codice in &opzioni.griglie {
            if !tabella::trasformazione(*codice).is_some_and(Trasformazione::a_griglia) {
                return Err(CrsError::ReprojectionConfig(
                    "griglia: il codice non e' una trasformazione NTv2 della tabella",
                ));
            }
            if !griglie.insert(*codice) {
                return Err(CrsError::ReprojectionConfig("griglia ripetuta"));
            }
        }
        let da = sorgente.definizione.datum;
        let a = destinazione.definizione.datum;
        let convenzione = opzioni.convenzione_wgs84_etrs89.unwrap_or(true);
        let mut candidati = percorsi::enumera(da, a, &griglie, convenzione);
        if opzioni.convenzione_wgs84_etrs89.is_some()
            && !candidati
                .iter()
                .any(|percorso| percorso.codici().contains(&CODICE_ETRS89_WGS84))
        {
            return Err(CrsError::ReprojectionConfig(
                "convenzione_wgs84_etrs89 senza effetto: nessun percorso passa da ETRS89 to \
                 WGS 84 (1)",
            ));
        }
        if let Some(codici) = &opzioni.trasformazioni {
            candidati.retain(|percorso| percorso.codici() == *codici);
            if candidati.is_empty() {
                return Err(CrsError::ReprojectionConfig(
                    "trasformazioni: i codici non formano un percorso fra i due datum \
                     (griglie comprese solo se fornite)",
                ));
            }
        }
        if candidati.is_empty() {
            return Err(CrsError::ReprojectionPathUnavailable);
        }
        let ammesso = |percorso: &PercorsoDatum| {
            percorso.accuratezza_m <= GROUND_PRECISION_METRES
                || opzioni
                    .accuratezza_accettata_m
                    // La somma delle accuratezze e' in virgola mobile (0,1 +
                    // 0,2 non e' 0,3): un nanometro di margine.
                    .is_some_and(|accettata| {
                        accettata >= percorso.accuratezza_m - TOLLERANZA_SOMMA_M
                    })
        };
        let migliore = candidati
            .iter()
            .map(PercorsoDatum::accuratezza_m)
            .fold(f64::INFINITY, f64::min);
        let percorsi: Vec<PercorsoDatum> = candidati.into_iter().filter(ammesso).collect();
        if percorsi.is_empty() {
            return Err(CrsError::ReprojectionAccuracyNotAccepted {
                accuracy_m: migliore,
            });
        }
        if opzioni.accuratezza_accettata_m.is_some()
            && percorsi
                .iter()
                .all(|percorso| percorso.accuratezza_m <= GROUND_PRECISION_METRES)
        {
            return Err(CrsError::ReprojectionConfig(
                "accuratezza_accettata_m senza effetto: ogni percorso ammesso sta entro la \
                 precisione di 1 cm",
            ));
        }
        for codice in &griglie {
            if !percorsi
                .iter()
                .any(|percorso| percorso.codici().contains(codice))
            {
                return Err(CrsError::ReprojectionConfig(
                    "griglia senza effetto: nessun percorso ammesso la usa",
                ));
            }
        }
        let identita = da == a && sorgente.definizione.metodo == destinazione.definizione.metodo;
        Ok(Self {
            sorgente,
            destinazione,
            percorsi,
            identita,
        })
    }

    /// I percorsi ammessi, nell'ordine in cui si provano.
    #[must_use]
    pub fn percorsi(&self) -> &[PercorsoDatum] {
        &self.percorsi
    }

    /// L'accuratezza peggiore fra i percorsi ammessi: quella che il
    /// risultato garantisce, in metri.
    #[must_use]
    pub fn accuratezza_garantita_m(&self) -> f64 {
        self.percorsi
            .iter()
            .map(PercorsoDatum::accuratezza_m)
            .fold(0.0, f64::max)
    }

    /// I codici delle griglie `NTv2` usate dai percorsi ammessi.
    #[must_use]
    pub fn griglie(&self) -> BTreeSet<u32> {
        self.percorsi
            .iter()
            .flat_map(|percorso| percorso.passi.iter())
            .filter(|passo| passo.a_griglia())
            .map(PassoPercorso::codice)
            .collect()
    }

    /// `crs` e' il CRS sorgente con cui il piano e' stato deciso.
    #[must_use]
    pub fn coincide_sorgente(&self, crs: &ResolvedCrs) -> bool {
        self.sorgente.crs.semantically_equals(crs)
    }

    /// I nomi dei datum sorgente e d'arrivo.
    #[must_use]
    pub const fn datum(&self) -> (&'static str, &'static str) {
        (self.sorgente.datum, self.destinazione.datum)
    }

    /// Il CRS d'arrivo.
    #[must_use]
    pub const fn destinazione(&self) -> &ResolvedCrs {
        &self.destinazione.crs
    }
}

/// Un passo pronto da applicare.
#[derive(Clone, Debug)]
enum Operazione {
    /// Traslazioni tutte nulle: latitudine e longitudine invariate anche
    /// fra ellissoidi diversi (NAD83 -> WGS 84, NZGD2000 -> WGS 84), come
    /// PROJ (`+proj=noop`) e come la trasformazione nulla del registro. Il
    /// passaggio geocentrico fra GRS 1980 e WGS 84 sposterebbe la latitudine
    /// di circa 0,1 mm.
    Nulla,
    Helmert {
        helmert: Helmert,
        da: Geocentrico,
        a: Geocentrico,
    },
    Griglia(usize),
}

/// Un piano con le griglie lette: trasforma i punti.
#[derive(Clone, Debug)]
pub struct Riproiettore {
    piano: PianoRiproiezione,
    /// Per percorso, i passi pronti con il loro verso.
    operazioni: Vec<Vec<(PassoPercorso, Operazione)>>,
    griglie: Vec<GrigliaNtv2>,
}

impl Riproiettore {
    /// # Errors
    ///
    /// [`CrsError::ReprojectionConfig`] se manca una griglia usata dal piano
    /// o ne arriva una che il piano non usa.
    pub fn nuovo(
        piano: PianoRiproiezione,
        griglie: BTreeMap<u32, GrigliaNtv2>,
    ) -> Result<Self, CrsError> {
        if piano.griglie() != griglie.keys().copied().collect::<BTreeSet<u32>>() {
            return Err(CrsError::ReprojectionConfig(
                "le griglie fornite non sono quelle del piano",
            ));
        }
        let indici: BTreeMap<u32, usize> = griglie
            .keys()
            .enumerate()
            .map(|(indice, codice)| (*codice, indice))
            .collect();
        let mut operazioni = Vec::with_capacity(piano.percorsi.len());
        for percorso in &piano.percorsi {
            let mut passi = Vec::with_capacity(percorso.passi.len());
            for passo in &percorso.passi {
                let trasformazione = passo.trasformazione;
                let pronta = match trasformazione.metodo {
                    MetodoTrasformazione::GrigliaNtv2 { .. } => {
                        Operazione::Griglia(*indici.get(&trasformazione.codice).ok_or(
                            CrsError::ReprojectionConfig("griglia del piano non fornita"),
                        )?)
                    }
                    MetodoTrasformazione::Traslazioni { tx, ty, tz }
                        if tx == 0.0 && ty == 0.0 && tz == 0.0 =>
                    {
                        Operazione::Nulla
                    }
                    metodo => {
                        let helmert = Helmert::da_metodo(&metodo).ok_or(
                            CrsError::ReprojectionConfig("trasformazione senza parametri"),
                        )?;
                        let ellissoide = |codice| {
                            tabella::datum(codice)
                                .map(|datum| Geocentrico::nuovo(datum.ellissoide))
                                .ok_or(CrsError::NotBuiltin)
                        };
                        Operazione::Helmert {
                            helmert,
                            da: ellissoide(trasformazione.da)?,
                            a: ellissoide(trasformazione.a)?,
                        }
                    }
                };
                passi.push((*passo, pronta));
            }
            operazioni.push(passi);
        }
        Ok(Self {
            piano,
            operazioni,
            griglie: griglie.into_values().collect(),
        })
    }

    /// Il piano deciso a secco da cui il riproiettore è costruito.
    #[must_use]
    pub const fn piano(&self) -> &PianoRiproiezione {
        &self.piano
    }

    /// Numero dei percorsi ammessi ([`PianoRiproiezione::percorsi`]).
    #[must_use]
    pub const fn numero_percorsi(&self) -> usize {
        self.operazioni.len()
    }

    /// Lon/lat nel datum sorgente di un punto del CRS sorgente, dopo i
    /// controlli di dominio e di regione.
    ///
    /// # Errors
    ///
    /// [`CrsError::CoordinateOutOfDomain`] per un punto non finito o fuori
    /// dal dominio; [`CrsError::ReprojectionNotConverged`] se l'inversa non
    /// converge.
    pub fn geografiche_sorgente(&self, x: f64, y: f64) -> Result<(f64, f64), CrsError> {
        let lato = &self.piano.sorgente;
        validate_geometry_domain(std::iter::once((x, y)), &lato.crs)?;
        let (lon, lat) = lato.proiezione.indietro(x, y).map_err(errore_proiezione)?;
        lato.regione(lon, lat)?;
        Ok((lon, lat))
    }

    /// I riquadri d'uso dei passi del percorso `percorso` (vuoto per lo
    /// stesso datum o per un indice fuori intervallo).
    #[must_use]
    pub fn aree_percorso(&self, percorso: usize) -> Vec<super::GeographicBounds> {
        self.piano
            .percorsi
            .get(percorso)
            .map(|p| {
                p.passi
                    .iter()
                    .map(|passo| passo.trasformazione.area)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Come [`Self::trasforma`], con le lon/lat sorgente del punto e la
    /// cella di griglia usata da ogni passo `NTv2`.
    ///
    /// # Errors
    ///
    /// Come [`Self::trasforma`].
    pub fn trasforma_dettagli(
        &self,
        percorso: usize,
        x: f64,
        y: f64,
    ) -> Result<Option<PuntoRiproiettato>, CrsError> {
        self.calcola(percorso, x, y)
    }

    /// Trasforma un punto lungo il percorso `percorso`.
    ///
    /// `Ok(None)` se il punto esce dall'area d'uso di un passo del percorso
    /// (o dalla sua griglia): il chiamante prova il percorso successivo.
    ///
    /// # Errors
    ///
    /// [`CrsError::CoordinateOutOfDomain`] per un punto fuori dal dominio
    /// di uno dei due CRS o dalla regione lon/lat della loro proiezione;
    /// [`CrsError::ReprojectionNotConverged`] se un'inversa non converge;
    /// [`CrsError::ReprojectionConfig`] per un indice di percorso fuori
    /// intervallo.
    pub fn trasforma(
        &self,
        percorso: usize,
        x: f64,
        y: f64,
    ) -> Result<Option<(f64, f64)>, CrsError> {
        Ok(self.calcola(percorso, x, y)?.map(|p| (p.x, p.y)))
    }

    fn calcola(
        &self,
        percorso: usize,
        x: f64,
        y: f64,
    ) -> Result<Option<PuntoRiproiettato>, CrsError> {
        let passi = self
            .operazioni
            .get(percorso)
            .ok_or(CrsError::ReprojectionConfig("percorso inesistente"))?;
        if self.piano.identita {
            // Stessa proiezione e stesso datum: coordinate invariate, dopo i
            // controlli di dominio del CRS (uguali sui due lati).
            let (lon, lat) = self.geografiche_sorgente(x, y)?;
            self.piano.destinazione.regione(lon, lat)?;
            validate_geometry_domain(std::iter::once((x, y)), &self.piano.destinazione.crs)?;
            return Ok(Some(PuntoRiproiettato {
                x,
                y,
                lon_sorgente: lon,
                lat_sorgente: lat,
                celle: [None; MAX_PASSI_PERCORSO],
            }));
        }
        let (lon_sorgente, lat_sorgente) = self.geografiche_sorgente(x, y)?;
        let (mut lon, mut lat) = (lon_sorgente, lat_sorgente);
        let mut celle = [None; MAX_PASSI_PERCORSO];
        for (posizione, (passo, operazione)) in passi.iter().enumerate() {
            let trasformazione = passo.trasformazione;
            // Un passo a griglia non riduce la longitudine: il riquadro
            // d'uso la vuole in [-180, 180].
            lon = proiezioni::riduci_gradi(lon);
            if !trasformazione.area.contains(lon, lat) {
                return Ok(None);
            }
            let nuovo = match operazione {
                Operazione::Nulla => Some((lon, lat)),
                Operazione::Helmert { helmert, da, a } => {
                    if passo.inversa {
                        Some(da.indietro(helmert.indietro(a.avanti(lon, lat))))
                    } else {
                        Some(a.indietro(helmert.avanti(da.avanti(lon, lat))))
                    }
                }
                Operazione::Griglia(indice) => {
                    let griglia = self
                        .griglie
                        .get(*indice)
                        .ok_or(CrsError::ReprojectionConfig(
                            "griglia del piano non fornita",
                        ))?;
                    // La cella dove si legge lo spostamento: nel verso del
                    // registro al punto d'ingresso, nell'inverso al punto
                    // trovato dall'iterazione.
                    let uscita = if passo.inversa {
                        griglia.indietro(lon, lat)?
                    } else {
                        griglia.avanti(lon, lat)
                    };
                    let (clon, clat) = if passo.inversa {
                        uscita.unwrap_or((lon, lat))
                    } else {
                        (lon, lat)
                    };
                    if let Some(cella) = celle.get_mut(posizione) {
                        *cella = griglia.cella(clon, clat);
                    }
                    uscita
                }
            };
            let Some((nuova_lon, nuova_lat)) = nuovo else {
                return Ok(None);
            };
            (lon, lat) = (nuova_lon, nuova_lat);
        }
        let lon = proiezioni::riduci_gradi(lon);
        let lato = &self.piano.destinazione;
        lato.regione(lon, lat)?;
        let (x, y) = lato
            .proiezione
            .avanti(lon, lat)
            .map_err(errore_proiezione)?;
        validate_geometry_domain(std::iter::once((x, y)), &lato.crs)?;
        Ok(Some(PuntoRiproiettato {
            x,
            y,
            lon_sorgente,
            lat_sorgente,
            celle,
        }))
    }
}

/// Un punto trasformato con i dettagli che servono al kernel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PuntoRiproiettato {
    /// Coordinata x (longitudine o easting) nel CRS d'arrivo.
    pub x: f64,
    /// Coordinata y (latitudine o northing) nel CRS d'arrivo.
    pub y: f64,
    /// Longitudine nel datum sorgente, in gradi.
    pub lon_sorgente: f64,
    /// Latitudine nel datum sorgente, in gradi.
    pub lat_sorgente: f64,
    /// Per ogni passo `NTv2`, la cella (sottogriglia, colonna, riga) il cui
    /// campo bilineare ha dato lo spostamento; `None` per gli altri passi.
    pub celle: [Option<(usize, usize, usize)>; MAX_PASSI_PERCORSO],
}

/// `true` se il CRS e' della tabella integrata e quindi riproiettabile.
#[must_use]
pub fn riproiettabile(crs: &ResolvedCrs) -> bool {
    crs.integrato.and_then(tabella::definizione).is_some()
}

#[cfg(test)]
mod oracolo;
#[cfg(test)]
mod tests;
