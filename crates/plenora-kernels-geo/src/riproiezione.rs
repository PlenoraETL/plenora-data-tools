//! `geo.reproject`: riproiezione delle geometrie fra CRS della tabella
//! integrata, in Rust puro ([`plenora_core::crs::riproiezione`]).
//!
//! Per ogni geometria:
//!
//! 1. si prova ogni percorso fra i datum ammesso dalla regola
//!    dell'accuratezza, nell'ordine di preferenza: il primo la cui area
//!    d'uso (e griglia) contiene **tutti** i punti trasformati, vertici e
//!    punti di densificazione, e' quello della geometria. Una geometria non
//!    mescola mai due percorsi; se nessuno la copre e' un errore
//!    (`REPROJECTION_OUTSIDE_TRANSFORMATION_AREA`);
//! 2. ogni lato si **densifica**: un lato dritto nel CRS sorgente non e'
//!    dritto nel CRS d'arrivo. Nei punti a 1/4, 1/2 e 3/4 del lato sorgente
//!    l'immagine esatta deve stare entro meta' della precisione del CRS
//!    d'arrivo (1 cm a terra, [`ResolvedCrs::precisione_coordinate`]) dal
//!    lato d'uscita, e i punti a 1/4, 1/2 e 3/4 del lato d'uscita entro la
//!    stessa distanza dalla spezzata delle immagini, e nessuna delle due
//!    meta' ha un'immagine piu' lunga di 3/4 dell'intero (continuita');
//!    altrimenti il lato si divide a meta' nel CRS sorgente,
//!    fino a [`MAX_PROFONDITA`] livelli (oltre: errore esplicito, per
//!    esempio un lato che attraversa l'antimeridiano d'uscita);
//! 3. la geometria d'uscita, stesso tipo e stessa struttura, deve essere
//!    valida OGC come quella d'ingresso: una riproiezione che la rende non
//!    valida e' un errore, mai un risultato.
//!
//! Il calcolo gira dietro la barriera [`crate::calcolo_protetto`]. Gli
//! errori non riportano coordinate.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use geo::{
    Coord, CoordsIter, Geometry, GeometryCollection, LineString, MultiLineString, MultiPoint,
    MultiPolygon, Point, Polygon,
};
use plenora_core::arrow::array::BinaryArray;
use plenora_core::arrow::{Field, RecordBatch, Schema, SchemaRef};
use plenora_core::contract::arrow_metadata::{
    canonical_geometry_axis_order, geo_metadata_json_with_encoding, geometry_column_index,
    read_geometry_contract_keys, strip_rewritten_crs_keys, GEO_METADATA_KEY, MAX_CELL_COORDINATES,
    PLENORA_GEOMETRY_AXIS_ORDER_KEY,
};
use plenora_core::contract::{GeometryDimensions, GeometryEncoding};
use plenora_core::crs::riproiezione::{
    GrigliaNtv2, OpzioniRiproiezione, PianoRiproiezione, Riproiettore,
};
use plenora_core::crs::{resolve_crs, CrsError, ResolvedCrs};
use plenora_core::PlenoraError;
use serde::Deserialize;
use serde_json::Value;

use crate::arrow_adapter::{
    batch_geometry_cells, decode_geometry_cell, encode_geometry, map_nullable,
};

/// Livelli massimi di divisione di un lato: un lato si divide al piu' in
/// `2^24` pezzi (e prima vale [`MAX_CELL_COORDINATES`]).
pub const MAX_PROFONDITA: u32 = 24;

/// Lunghezza massima del percorso di un file di griglia nella config.
pub const MAX_BYTE_PERCORSO_GRIGLIA: usize = 4096;

/// Una griglia `NTv2` della config: la trasformazione EPSG e il file.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GrigliaConfig {
    /// Codice EPSG della trasformazione a griglia (per esempio 9734,
    /// Monte Mario to RDN2008 (5)).
    pub trasformazione: u32,
    /// Percorso del file `NTv2` fornito dall'utente.
    pub file: String,
}

/// La config di `geo.reproject`, come arriva dal piano.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ReprojectConfig {
    /// CRS d'arrivo: un identificatore della tabella integrata.
    pub target_crs: String,
    /// Accuratezza accettata in metri per un cambio di datum oltre 1 cm.
    #[serde(default)]
    pub accuratezza_accettata_m: Option<f64>,
    /// Trasformazioni EPSG imposte, nell'ordine.
    #[serde(default)]
    pub trasformazioni: Option<Vec<u32>>,
    /// Griglie `NTv2` fornite.
    #[serde(default)]
    pub griglie: Vec<GrigliaConfig>,
}

/// I parametri di `geo.reproject` gia' verificati: il piano deciso
/// dall'analisi e i file delle griglie da leggere.
#[derive(Clone, Debug)]
pub struct ReprojectParams {
    piano: PianoRiproiezione,
    griglie: BTreeMap<u32, PathBuf>,
}

fn errore_config(op: &str, motivo: impl std::fmt::Display) -> PlenoraError {
    PlenoraError::InvalidConfiguration(format!("{op}: {motivo}"))
}

impl ReprojectParams {
    /// Legge e verifica la config contro il CRS sorgente: target risolto
    /// dalla tabella integrata (sempre: un CRS di piano risolto altrove non
    /// si riproietta), piano dei percorsi con la regola dell'accuratezza,
    /// griglie ben formate. Non legge file.
    ///
    /// # Errors
    ///
    /// `PlenoraError::InvalidConfiguration` per una config non leggibile o
    /// un percorso di griglia vuoto, troppo lungo o con NUL;
    /// `PlenoraError::Crs` per il target non risolvibile e per i rifiuti di
    /// [`PianoRiproiezione::nuovo`].
    pub fn da_config(
        op: &str,
        config: &Value,
        sorgente: &ResolvedCrs,
    ) -> Result<Self, PlenoraError> {
        let parsed = ReprojectConfig::deserialize(config)
            .map_err(|_| errore_config(op, "config non valida per geo.reproject"))?;
        let target = resolve_crs(&parsed.target_crs, "target_crs")
            .map_err(|error| PlenoraError::Crs(format!("{op}: parametro `target_crs`: {error}")))?;
        let mut griglie = BTreeMap::new();
        for griglia in &parsed.griglie {
            if griglia.file.trim().is_empty()
                || griglia.file.len() > MAX_BYTE_PERCORSO_GRIGLIA
                || griglia.file.contains('\0')
            {
                return Err(errore_config(
                    op,
                    "griglie: `file` vuoto, oltre 4096 byte o con NUL",
                ));
            }
            if griglie
                .insert(griglia.trasformazione, PathBuf::from(&griglia.file))
                .is_some()
            {
                return Err(PlenoraError::Crs(format!(
                    "{op}: {}",
                    CrsError::ReprojectionConfig("griglia ripetuta")
                )));
            }
        }
        let opzioni = OpzioniRiproiezione {
            accuratezza_accettata_m: parsed.accuratezza_accettata_m,
            griglie: parsed.griglie.iter().map(|g| g.trasformazione).collect(),
            trasformazioni: parsed.trasformazioni,
        };
        let piano = PianoRiproiezione::nuovo(sorgente, &target, &opzioni)
            .map_err(|error| PlenoraError::Crs(format!("{op}: {error}")))?;
        Ok(Self { piano, griglie })
    }

    /// Il piano dei percorsi.
    #[must_use]
    pub const fn piano(&self) -> &PianoRiproiezione {
        &self.piano
    }

    /// Il CRS d'arrivo.
    #[must_use]
    pub const fn target(&self) -> &ResolvedCrs {
        self.piano.destinazione()
    }

    /// Legge le griglie e prepara il riproiettore.
    ///
    /// # Errors
    ///
    /// `PlenoraError::Crs` per un file di griglia illeggibile o difettoso
    /// ([`GrigliaNtv2::leggi`]).
    pub fn riproiettore(&self) -> Result<Riproiettore, PlenoraError> {
        let mut griglie = BTreeMap::new();
        for (codice, file) in &self.griglie {
            griglie.insert(*codice, leggi_griglia(file)?);
        }
        Riproiettore::nuovo(self.piano.clone(), griglie).map_err(PlenoraError::from)
    }
}

fn leggi_griglia(file: &Path) -> Result<GrigliaNtv2, PlenoraError> {
    GrigliaNtv2::leggi(file).map_err(PlenoraError::from)
}

/// Il campo geometria dell'uscita, sul CRS d'arrivo.
///
/// Quello dell'ingresso con il metadato `geo` riscritto sul CRS d'arrivo
/// (dimensioni ed encoding invariati), senza le chiavi canoniche CRS della
/// sorgente e con l'ordine degli assi GIS normalizzato del CRS d'arrivo.
/// E' anche il campo che l'analisi
/// dichiara (`analyze::producers::analyze_reproject`, oracolo
/// `kernel_crosscheck`).
///
/// # Errors
///
/// Come [`geo_metadata_json_with_encoding`].
pub fn campo_riproiettato(
    ingresso: &Field,
    target: &ResolvedCrs,
    dimensions: GeometryDimensions,
    encoding: Option<GeometryEncoding>,
) -> Result<Field, PlenoraError> {
    let mut metadata = ingresso.metadata().clone();
    metadata.insert(
        GEO_METADATA_KEY.to_owned(),
        geo_metadata_json_with_encoding(target.definition(), dimensions, encoding)?,
    );
    strip_rewritten_crs_keys(&mut metadata);
    metadata.insert(
        PLENORA_GEOMETRY_AXIS_ORDER_KEY.to_owned(),
        target.normalized_gis_axis_order().as_str().to_owned(),
    );
    Ok(ingresso.clone().with_metadata(metadata))
}

/// L'ordine degli assi dichiarato dalla colonna deve essere quello GIS
/// normalizzato del CRS sorgente (o assente): con un altro ordine la
/// riproiezione leggerebbe coordinate scambiate senza accorgersene.
///
/// # Errors
///
/// `PlenoraError::Crs` se la colonna dichiara un altro ordine (anche
/// `unknown`); l'errore di lettura della chiave.
pub fn richiedi_assi_normalizzati(
    op: &str,
    campo: &Field,
    sorgente: &ResolvedCrs,
) -> Result<(), PlenoraError> {
    let Some(dichiarato) = canonical_geometry_axis_order(campo)? else {
        return Ok(());
    };
    let normalizzato = sorgente.normalized_gis_axis_order();
    if dichiarato == normalizzato {
        return Ok(());
    }
    Err(PlenoraError::Crs(format!(
        "{op}: colonna geometria `{}`: `axis_order` dichiarato `{dichiarato}`, ma il kernel \
         legge e riemette le coordinate nell'ordine GIS normalizzato `{normalizzato}` del CRS \
         sorgente: l'ordine va normalizzato a monte, mai in silenzio qui",
        campo.name()
    )))
}

// ---------------------------------------------------------------------------
// Geometrie.
// ---------------------------------------------------------------------------

/// Esito di un tentativo con un percorso.
enum Tentativo<T> {
    Fatto(T),
    /// Un punto e' uscito dall'area d'uso di un passo: si prova il
    /// percorso successivo.
    FuoriArea,
}

struct Contesto<'a> {
    riproiettore: &'a Riproiettore,
    percorso: usize,
    /// Tolleranza della densificazione, nelle unita' del CRS d'arrivo.
    tolleranza: f64,
    /// Coordinate prodotte finora (limite [`MAX_CELL_COORDINATES`]).
    prodotte: u64,
}

impl Contesto<'_> {
    fn punto(&self, c: Coord<f64>) -> Result<Option<Coord<f64>>, CrsError> {
        Ok(self
            .riproiettore
            .trasforma(self.percorso, c.x, c.y)?
            .map(|(x, y)| Coord { x, y }))
    }

    fn conta(&mut self, n: usize) -> Result<(), PlenoraError> {
        self.prodotte = self.prodotte.saturating_add(n as u64);
        if self.prodotte > MAX_CELL_COORDINATES {
            return Err(PlenoraError::ResourceLimit(format!(
                "geo.reproject: la densificazione supera {MAX_CELL_COORDINATES} coordinate per \
                 cella"
            )));
        }
        Ok(())
    }
}

/// Distanza di `p` dal segmento `a`-`b`.
fn distanza_dal_segmento(punto: Coord<f64>, inizio: Coord<f64>, fine: Coord<f64>) -> f64 {
    let direzione = fine - inizio;
    let lunghezza2 = direzione.x.mul_add(direzione.x, direzione.y * direzione.y);
    if lunghezza2 == 0.0 {
        return (punto.x - inizio.x).hypot(punto.y - inizio.y);
    }
    let parametro = ((punto.x - inizio.x).mul_add(direzione.x, (punto.y - inizio.y) * direzione.y)
        / lunghezza2)
        .clamp(0.0, 1.0);
    let piede = Coord {
        x: parametro.mul_add(direzione.x, inizio.x),
        y: parametro.mul_add(direzione.y, inizio.y),
    };
    (punto.x - piede.x).hypot(punto.y - piede.y)
}

fn interpola(a: Coord<f64>, b: Coord<f64>, t: f64) -> Coord<f64> {
    Coord {
        x: t.mul_add(b.x - a.x, a.x),
        y: t.mul_add(b.y - a.y, a.y),
    }
}

/// Aggiunge a `uscita` i punti del lato `a`-`b` (immagini `fa`, `fb`) dopo
/// `fa`: i punti di densificazione e `fb`.
fn lato(
    contesto: &mut Contesto<'_>,
    (a, b): (Coord<f64>, Coord<f64>),
    (fa, fb): (Coord<f64>, Coord<f64>),
    profondita: u32,
    uscita: &mut Vec<Coord<f64>>,
) -> Result<Tentativo<()>, PlenoraError> {
    if a == b {
        contesto.conta(1)?;
        uscita.push(fb);
        return Ok(Tentativo::Fatto(()));
    }
    let mut immagini = [Coord { x: 0.0, y: 0.0 }; 3];
    for (indice, t) in [0.25, 0.5, 0.75].into_iter().enumerate() {
        let Some(campione) = contesto.punto(interpola(a, b, t))? else {
            return Ok(Tentativo::FuoriArea);
        };
        immagini[indice] = campione;
    }
    // Scarto nei due versi: le immagini dei punti del lato sorgente dal lato
    // d'uscita, e i punti del lato d'uscita dalla spezzata delle immagini.
    // Il secondo verso vede una corda che attraversa il mondo fra due
    // immagini ai due lati dell'antimeridiano, che il primo non vede (le
    // immagini stanno sulla corda).
    let spezzata = [fa, immagini[0], immagini[1], immagini[2], fb];
    let mut scarto: f64 = 0.0;
    for (indice, t) in [0.25, 0.5, 0.75].into_iter().enumerate() {
        scarto = scarto.max(distanza_dal_segmento(immagini[indice], fa, fb));
        let sulla_corda = interpola(fa, fb, t);
        let dalla_spezzata = spezzata
            .windows(2)
            .map(|coppia| distanza_dal_segmento(sulla_corda, coppia[0], coppia[1]))
            .fold(f64::INFINITY, f64::min);
        scarto = scarto.max(dalla_spezzata);
    }
    if scarto.is_nan() {
        return Err(CrsError::ReprojectionEdgeNotConverged.into());
    }
    // Continuita': su una funzione continua le due meta' del lato hanno
    // immagini piu' corte dell'intero (per un lato abbastanza corto, circa
    // la meta' ciascuna). Un salto (la longitudine che passa da +180 a -180
    // in un CRS d'arrivo geografico o di Mercator) resta intero in una
    // delle due meta' a ogni divisione: il lato non converge ed e' un
    // errore. Un lato regolare che non passa la prova si divide ancora, e
    // basta.
    let fm = immagini[1];
    let intero = (fb.x - fa.x).hypot(fb.y - fa.y);
    let meta = (fm.x - fa.x)
        .hypot(fm.y - fa.y)
        .max((fb.x - fm.x).hypot(fb.y - fm.y));
    let continuo = meta <= 0.75f64.mul_add(intero, contesto.tolleranza);
    if scarto <= contesto.tolleranza && continuo {
        contesto.conta(1)?;
        uscita.push(fb);
        return Ok(Tentativo::Fatto(()));
    }
    if profondita >= MAX_PROFONDITA {
        return Err(CrsError::ReprojectionEdgeNotConverged.into());
    }
    let m = interpola(a, b, 0.5);
    if matches!(
        lato(contesto, (a, m), (fa, fm), profondita + 1, uscita)?,
        Tentativo::FuoriArea
    ) {
        return Ok(Tentativo::FuoriArea);
    }
    lato(contesto, (m, b), (fm, fb), profondita + 1, uscita)
}

/// Una sequenza di coordinate (linea o anello), densificata.
fn catena(
    contesto: &mut Contesto<'_>,
    coordinate: &[Coord<f64>],
) -> Result<Tentativo<LineString<f64>>, PlenoraError> {
    let Some((&primo, resto)) = coordinate.split_first() else {
        return Ok(Tentativo::Fatto(LineString::new(Vec::new())));
    };
    let Some(immagine_primo) = contesto.punto(primo)? else {
        return Ok(Tentativo::FuoriArea);
    };
    let mut uscita = Vec::with_capacity(coordinate.len());
    contesto.conta(1)?;
    uscita.push(immagine_primo);
    let (mut a, mut fa) = (primo, immagine_primo);
    for &b in resto {
        // Un anello chiuso: l'ultimo punto e' il primo, e la sua immagine e'
        // la stessa al bit (stessa funzione sullo stesso ingresso).
        let Some(fb) = contesto.punto(b)? else {
            return Ok(Tentativo::FuoriArea);
        };
        if matches!(
            lato(contesto, (a, b), (fa, fb), 0, &mut uscita)?,
            Tentativo::FuoriArea
        ) {
            return Ok(Tentativo::FuoriArea);
        }
        (a, fa) = (b, fb);
    }
    Ok(Tentativo::Fatto(LineString::new(uscita)))
}

fn poligono(
    contesto: &mut Contesto<'_>,
    poligono: &Polygon<f64>,
) -> Result<Tentativo<Polygon<f64>>, PlenoraError> {
    let Tentativo::Fatto(esterno) = catena(contesto, &poligono.exterior().0)? else {
        return Ok(Tentativo::FuoriArea);
    };
    let mut interni = Vec::with_capacity(poligono.interiors().len());
    for anello in poligono.interiors() {
        let Tentativo::Fatto(riproiettato) = catena(contesto, &anello.0)? else {
            return Ok(Tentativo::FuoriArea);
        };
        interni.push(riproiettato);
    }
    Ok(Tentativo::Fatto(Polygon::new(esterno, interni)))
}

fn geometria(
    contesto: &mut Contesto<'_>,
    ingresso: &Geometry<f64>,
) -> Result<Tentativo<Geometry<f64>>, PlenoraError> {
    macro_rules! prova {
        ($espressione:expr) => {
            match $espressione? {
                Tentativo::Fatto(valore) => valore,
                Tentativo::FuoriArea => return Ok(Tentativo::FuoriArea),
            }
        };
    }
    let uscita = match ingresso {
        Geometry::Point(punto) => {
            let Some(c) = contesto.punto(punto.0)? else {
                return Ok(Tentativo::FuoriArea);
            };
            contesto.conta(1)?;
            Geometry::Point(Point(c))
        }
        Geometry::MultiPoint(punti) => {
            let mut uscita = Vec::with_capacity(punti.0.len());
            for punto in &punti.0 {
                let Some(c) = contesto.punto(punto.0)? else {
                    return Ok(Tentativo::FuoriArea);
                };
                contesto.conta(1)?;
                uscita.push(Point(c));
            }
            Geometry::MultiPoint(MultiPoint(uscita))
        }
        Geometry::LineString(linea) => Geometry::LineString(prova!(catena(contesto, &linea.0))),
        Geometry::MultiLineString(linee) => {
            let mut uscita = Vec::with_capacity(linee.0.len());
            for linea in &linee.0 {
                uscita.push(prova!(catena(contesto, &linea.0)));
            }
            Geometry::MultiLineString(MultiLineString(uscita))
        }
        Geometry::Polygon(p) => Geometry::Polygon(prova!(poligono(contesto, p))),
        Geometry::MultiPolygon(poligoni) => {
            let mut uscita = Vec::with_capacity(poligoni.0.len());
            for p in &poligoni.0 {
                uscita.push(prova!(poligono(contesto, p)));
            }
            Geometry::MultiPolygon(MultiPolygon(uscita))
        }
        Geometry::GeometryCollection(collezione) => {
            let mut uscita = Vec::with_capacity(collezione.0.len());
            for parte in &collezione.0 {
                uscita.push(prova!(geometria(contesto, parte)));
            }
            Geometry::GeometryCollection(GeometryCollection(uscita))
        }
        Geometry::Line(_) | Geometry::Rect(_) | Geometry::Triangle(_) => {
            // Il decoder WKB non produce questi tipi: riproiettarli
            // cambierebbe il tipo (un rettangolo non resta un rettangolo).
            return Err(PlenoraError::Unsupported(
                "geo.reproject: tipo geometrico non WKB (Line, Rect, Triangle)".to_owned(),
            ));
        }
    };
    Ok(Tentativo::Fatto(uscita))
}

/// Ogni vertice deve preferire il percorso della geometria: se un percorso
/// precedente (piu' accurato o piu' specifico) copre da solo un vertice,
/// quel vertice riceverebbe i parametri di un'altra area (il riquadro
/// «Italy - mainland» contiene la Sardegna, ma per la Sardegna vale
/// un'altra trasformazione). Errore esplicito, mai un risultato misto.
fn richiedi_percorso_uniforme(
    ingresso: &Geometry<f64>,
    riproiettore: &Riproiettore,
    percorso: usize,
) -> Result<(), PlenoraError> {
    if percorso == 0 {
        return Ok(());
    }
    for vertice in ingresso.coords_iter() {
        for precedente in 0..percorso {
            if riproiettore
                .trasforma(precedente, vertice.x, vertice.y)?
                .is_some()
            {
                return Err(CrsError::ReprojectionMixedTransformationAreas.into());
            }
        }
    }
    Ok(())
}

/// Riproietta una geometria gia' valida: primo percorso che la copre tutta,
/// lati densificati, uscita valida OGC dello stesso tipo.
///
/// `tolleranza` e' lo scarto ammesso fra un lato d'uscita e l'immagine
/// esatta del lato sorgente, nelle unita' del CRS d'arrivo (meta' della sua
/// precisione nel kernel Arrow).
///
/// # Errors
///
/// `PlenoraError::Crs` per un punto fuori dominio o regione, per una
/// geometria che nessun percorso copre, per un lato che non converge;
/// `PlenoraError::ResourceLimit` oltre [`MAX_CELL_COORDINATES`] coordinate;
/// `PlenoraError::InvalidPlan` se l'uscita non e' valida OGC;
/// `PlenoraError::Internal` se il calcolo va in panico (barriera).
pub fn riproietta_geometria(
    ingresso: &Geometry<f64>,
    riproiettore: &Riproiettore,
    tolleranza: f64,
) -> Result<Geometry<f64>, PlenoraError> {
    if !(tolleranza.is_finite() && tolleranza > 0.0) {
        return Err(PlenoraError::Internal(
            "geo.reproject: tolleranza di densificazione non valida".to_owned(),
        ));
    }
    let calcolo = || -> Result<Geometry<f64>, PlenoraError> {
        for percorso in 0..riproiettore.numero_percorsi() {
            let mut contesto = Contesto {
                riproiettore,
                percorso,
                tolleranza,
                prodotte: 0,
            };
            if let Tentativo::Fatto(uscita) = geometria(&mut contesto, ingresso)? {
                richiedi_percorso_uniforme(ingresso, riproiettore, percorso)?;
                return Ok(uscita);
            }
        }
        Err(CrsError::ReprojectionOutsideTransformationArea.into())
    };
    let uscita = crate::calcolo_protetto(calcolo).map_err(|forma| {
        PlenoraError::Internal(format!("geo.reproject: calcolo non concluso: {forma}"))
    })??;
    crate::valida_ogc(&uscita).map_err(|error| match error {
        PlenoraError::InvalidPlan(motivo) => PlenoraError::InvalidPlan(format!(
            "geo.reproject: la geometria riproiettata non e' valida ({motivo})"
        )),
        altro => altro,
    })?;
    Ok(uscita)
}

/// `geo.reproject` su una tabella.
///
/// Stesse righe e colonne; ogni cella non-null si decodifica (validazione
/// WKB e OGC), si riproietta con [`riproietta_geometria`] (tolleranza:
/// meta' della precisione del CRS d'arrivo) e si ricodifica; i null restano
/// null. Il campo geometria diventa [`campo_riproiettato`]. Il primo errore
/// in ordine di riga vince.
///
/// # Errors
///
/// `PlenoraError::Schema` se la colonna manca o non e' WKB, o dichiara
/// dimensioni diverse da XY; `PlenoraError::Crs` per l'ordine degli assi
/// dichiarato non normalizzato, per le griglie illeggibili e per i rifiuti
/// di [`riproietta_geometria`]; gli errori di decode e di codifica.
pub fn reproject_batches(
    schema: &SchemaRef,
    batches: &[RecordBatch],
    geometry_column: &str,
    sorgente: &ResolvedCrs,
    params: &ReprojectParams,
) -> Result<(SchemaRef, Vec<RecordBatch>), PlenoraError> {
    const OP: &str = "geo.reproject";
    let geometry_index = geometry_column_index(schema, geometry_column)?;
    let campo = schema.field(geometry_index);
    richiedi_assi_normalizzati(OP, campo, sorgente)?;
    // Dimensioni ed encoding come li legge la scoperta del contratto
    // (chiavi canoniche e `geo`, R2.6/R2.7): il campo d'uscita e' quello
    // che l'analisi dichiara dal contratto.
    let chiavi = read_geometry_contract_keys(campo)?;
    let dimensioni = chiavi.dimensions.unwrap_or(GeometryDimensions::Unknown);
    if dimensioni != GeometryDimensions::Xy {
        return Err(PlenoraError::Unsupported(format!(
            "{OP}: dimensionalita' geometria `{dimensioni}` non supportata: il kernel accetta \
             solo `xy`"
        )));
    }
    if !params.piano.coincide_sorgente(sorgente) {
        return Err(PlenoraError::Crs(format!(
            "{OP}: il CRS sorgente non e' quello con cui il piano e' stato deciso"
        )));
    }
    let target = params.target();
    let tolleranza = target
        .precisione_coordinate()
        .map(|p| p / 2.0)
        .ok_or_else(|| {
            PlenoraError::Crs(format!("{OP}: precisione del CRS d'arrivo non definita"))
        })?;
    let riproiettore = params.riproiettore()?;
    let uscita_campo = campo_riproiettato(campo, target, dimensioni, chiavi.encoding)?;
    let mut fields: Vec<Field> = schema
        .fields()
        .iter()
        .map(|field| field.as_ref().clone())
        .collect();
    fields[geometry_index] = uscita_campo;
    let output_schema = Arc::new(Schema::new_with_metadata(fields, schema.metadata().clone()));
    let mut output = Vec::with_capacity(batches.len());
    for batch in batches {
        let cells = batch_geometry_cells(batch, geometry_index, geometry_column)?;
        let riproiettate = map_nullable(cells, |payload| {
            let geometria = decode_geometry_cell(payload)?;
            let uscita = riproietta_geometria(&geometria, &riproiettore, tolleranza)?;
            encode_geometry(&uscita).map(Some)
        })?;
        let mut columns = batch.columns().to_vec();
        columns[geometry_index] = Arc::new(
            riproiettate
                .iter()
                .map(|cell| cell.as_deref())
                .collect::<BinaryArray>(),
        );
        output.push(plenora_core::batch_with_rows(
            output_schema.clone(),
            columns,
            batch.num_rows(),
        )?);
    }
    Ok((output_schema, output))
}

#[cfg(test)]
mod tests;
