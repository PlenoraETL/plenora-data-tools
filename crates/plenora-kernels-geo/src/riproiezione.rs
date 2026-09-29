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
//! Il calcolo gira dietro la barriera dei panici (`calcolo_protetto`). Gli
//! errori non riportano coordinate.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use geo::{
    Coord, Geometry, GeometryCollection, LineString, MultiLineString, MultiPoint, MultiPolygon,
    Point, Polygon,
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
    GrigliaNtv2, OpzioniRiproiezione, PassoPercorso, PianoRiproiezione, Riproiettore,
    MAX_PASSI_PERCORSO,
};
use plenora_core::crs::{resolve_crs, CrsError, GeographicBounds, ResolvedCrs};
use plenora_core::PlenoraError;
use serde::Deserialize;
use serde_json::Value;

use crate::arrow_adapter::{
    batch_geometry_cells, decode_geometry_cell, encode_geometry, map_nullable,
};

/// Livelli massimi di divisione di un lato: 48 bastano a ridurre un lato
/// di 20.000 km sotto il micrometro attorno a un bordo di cella `NTv2`
/// (prima vale comunque [`MAX_CELL_COORDINATES`]).
pub const MAX_PROFONDITA: u32 = 48;

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

/// La config di `geo.reproject`, come arriva dal piano. Campi sconosciuti
/// rifiutati; i domini si verificano in [`ReprojectParams::da_config`].
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ReprojectConfig {
    /// CRS d'arrivo, obbligatorio: un identificatore della tabella
    /// integrata (`EPSG:<codice>`, `OGC:CRS84` e le forme URN).
    pub target_crs: String,
    /// Accuratezza accettata in metri per un cambio di datum oltre 1 cm
    /// (assente: nessuna, e un percorso oltre 1 cm si rifiuta). Non finita,
    /// negativa o senza effetto (ogni percorso ammesso gia' entro 1 cm): si
    /// rifiuta.
    #[serde(default)]
    pub accuratezza_accettata_m: Option<f64>,
    /// Trasformazioni EPSG imposte, nell'ordine: il percorso fra i datum e'
    /// esattamente questo, per tutte le geometrie, e resta soggetto alla
    /// regola dell'accuratezza (assente: si provano tutti i percorsi
    /// ammessi).
    #[serde(default)]
    pub trasformazioni: Option<Vec<u32>>,
    /// Griglie `NTv2` fornite (assente: nessuna); una griglia che nessun
    /// percorso ammesso usa si rifiuta.
    #[serde(default)]
    pub griglie: Vec<GrigliaConfig>,
    /// Convenzione WGS 84 = famiglia ETRS89 (assente: attiva); `false`
    /// usa l'accuratezza EPSG di ETRS89 to WGS 84 (1), 1 m. Scritta, con
    /// l'uno o l'altro valore, su una coppia che non passa da quella
    /// trasformazione si rifiuta.
    #[serde(default)]
    pub convenzione_wgs84_etrs89: Option<bool>,
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
            convenzione_wgs84_etrs89: parsed.convenzione_wgs84_etrs89,
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
    /// Per ogni percorso che precede quello della geometria, i riquadri
    /// d'uso dei suoi passi: un lato che ne attraversa l'intersezione passa
    /// da punti che preferiscono quel percorso.
    precedenti: Vec<Vec<GeographicBounds>>,
    /// Il percorso ha un passo `NTv2`.
    con_griglia: bool,
    /// Un punto o un lato preferisce un percorso precedente: se il
    /// percorso copre tutta la geometria, e' l'errore dei percorsi misti.
    misto: bool,
}

/// Un punto trasformato: coordinate d'arrivo, lon/lat sorgente e celle
/// delle griglie usate.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Immagine {
    c: Coord<f64>,
    sorgente: Coord<f64>,
    celle: [Option<(usize, usize, usize)>; MAX_PASSI_PERCORSO],
}

impl Contesto<'_> {
    /// Il punto lungo il percorso della geometria; `None` se esce
    /// dall'area d'uso. Errore se un percorso precedente lo coprirebbe:
    /// quel punto preferisce un'altra trasformazione, e una sola per tutta
    /// la geometria gli darebbe parametri di un'altra area. Vale per ogni
    /// punto trasformato (vertici, campioni, punti di densificazione), cosi'
    /// l'esito non dipende da come l'ingresso e' segmentato.
    fn punto(&mut self, c: Coord<f64>) -> Result<Option<Immagine>, PlenoraError> {
        let Some(p) = self
            .riproiettore
            .trasforma_dettagli(self.percorso, c.x, c.y)?
        else {
            return Ok(None);
        };
        if !self.misto {
            for precedente in 0..self.percorso {
                if self.riproiettore.trasforma(precedente, c.x, c.y)?.is_some() {
                    self.misto = true;
                    break;
                }
            }
        }
        Ok(Some(Immagine {
            c: Coord { x: p.x, y: p.y },
            sorgente: Coord {
                x: p.lon_sorgente,
                y: p.lat_sorgente,
            },
            celle: p.celle,
        }))
    }

    /// Il lato sorgente (in lon/lat del datum sorgente, approssimato dalla
    /// corda fra gli estremi) attraversa l'area di un percorso precedente.
    fn attraversa_precedenti(&mut self, a: Coord<f64>, b: Coord<f64>) {
        if self
            .precedenti
            .iter()
            .any(|aree| segmento_nelle_aree(a, b, aree))
        {
            self.misto = true;
        }
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

/// Intervalli del parametro `t` in `[0, 1]` per cui il segmento `a`-`b`
/// sta nel riquadro (Liang-Barsky; un riquadro oltre l'antimeridiano e'
/// l'unione di due).
fn intervalli_nel_riquadro(a: Coord<f64>, b: Coord<f64>, r: &GeographicBounds) -> Vec<(f64, f64)> {
    let rettangoli = if r.crosses_antimeridian() {
        vec![
            (r.west_longitude, 180.0, r.south_latitude, r.north_latitude),
            (-180.0, r.east_longitude, r.south_latitude, r.north_latitude),
        ]
    } else {
        vec![(
            r.west_longitude,
            r.east_longitude,
            r.south_latitude,
            r.north_latitude,
        )]
    };
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let mut uscita = Vec::new();
    for (ovest, est, sud, nord) in rettangoli {
        let (mut t0, mut t1) = (0.0_f64, 1.0_f64);
        let mut vuoto = false;
        for (p, q) in [
            (-dx, a.x - ovest),
            (dx, est - a.x),
            (-dy, a.y - sud),
            (dy, nord - a.y),
        ] {
            if p == 0.0 {
                if q < 0.0 {
                    vuoto = true;
                }
            } else {
                let t = q / p;
                if p < 0.0 {
                    t0 = t0.max(t);
                } else {
                    t1 = t1.min(t);
                }
            }
        }
        if !vuoto && t0 <= t1 {
            uscita.push((t0, t1));
        }
    }
    uscita
}

/// Il segmento passa per un punto contenuto in tutti i riquadri.
fn segmento_nelle_aree(a: Coord<f64>, b: Coord<f64>, aree: &[GeographicBounds]) -> bool {
    let mut comuni = vec![(0.0_f64, 1.0_f64)];
    for area in aree {
        let nuovi = intervalli_nel_riquadro(a, b, area);
        comuni = comuni
            .iter()
            .flat_map(|(x0, x1)| nuovi.iter().map(move |(y0, y1)| (x0.max(*y0), x1.min(*y1))))
            .filter(|(t0, t1)| t0 <= t1)
            .collect();
        if comuni.is_empty() {
            return false;
        }
    }
    !comuni.is_empty()
}

/// Distanza di `punto` dal segmento `inizio`-`fine`.
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
    (fa, fb): (Immagine, Immagine),
    profondita: u32,
    uscita: &mut Vec<Coord<f64>>,
) -> Result<Tentativo<()>, PlenoraError> {
    if a == b {
        contesto.conta(1)?;
        uscita.push(fb.c);
        return Ok(Tentativo::Fatto(()));
    }
    let mut campioni = [fa; 3];
    for (indice, t) in [0.25, 0.5, 0.75].into_iter().enumerate() {
        let Some(trovato) = contesto.punto(interpola(a, b, t))? else {
            return Ok(Tentativo::FuoriArea);
        };
        campioni[indice] = trovato;
    }
    let immagini = campioni.map(|campione| campione.c);
    // Scarto nei due versi: le immagini dei punti del lato sorgente dal lato
    // d'uscita, e i punti del lato d'uscita dalla spezzata delle immagini.
    // Il secondo verso vede una corda che attraversa il mondo fra due
    // immagini ai due lati dell'antimeridiano, che il primo non vede (le
    // immagini stanno sulla corda).
    let spezzata = [fa.c, immagini[0], immagini[1], immagini[2], fb.c];
    let mut scarto: f64 = 0.0;
    for (indice, t) in [0.25, 0.5, 0.75].into_iter().enumerate() {
        scarto = scarto.max(distanza_dal_segmento(immagini[indice], fa.c, fb.c));
        let sulla_corda = interpola(fa.c, fb.c, t);
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
    let fm = campioni[1];
    let intero = (fb.c.x - fa.c.x).hypot(fb.c.y - fa.c.y);
    let meta = (fm.c.x - fa.c.x)
        .hypot(fm.c.y - fa.c.y)
        .max((fb.c.x - fm.c.x).hypot(fb.c.y - fm.c.y));
    let continuo = meta <= 0.75f64.mul_add(intero, contesto.tolleranza);
    // Griglie NTv2: il campo di spostamenti e' bilineare a pezzi, e i tre
    // campioni non vedono una cella attraversata fra due di loro. Un lato
    // si accetta solo se estremi e campioni stanno nella stessa cella di
    // ogni griglia (li' il campo lungo il lato e' quadratico, e i campioni
    // ne misurano lo scarto), oppure se la sua immagine e' piu' corta della
    // tolleranza (il pezzo che attraversa un bordo di cella, ridotto per
    // bisezione).
    let stessa_cella = !contesto.con_griglia
        || campioni
            .iter()
            .chain([&fb])
            .all(|campione| campione.celle == fa.celle)
        || intero <= contesto.tolleranza;
    if scarto <= contesto.tolleranza && continuo && stessa_cella {
        contesto.attraversa_precedenti(fa.sorgente, fb.sorgente);
        contesto.conta(1)?;
        uscita.push(fb.c);
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
    uscita.push(immagine_primo.c);
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
            let Some(immagine) = contesto.punto(punto.0)? else {
                return Ok(Tentativo::FuoriArea);
            };
            contesto.conta(1)?;
            Geometry::Point(Point(immagine.c))
        }
        Geometry::MultiPoint(punti) => {
            let mut uscita = Vec::with_capacity(punti.0.len());
            for punto in &punti.0 {
                let Some(immagine) = contesto.punto(punto.0)? else {
                    return Ok(Tentativo::FuoriArea);
                };
                contesto.conta(1)?;
                uscita.push(Point(immagine.c));
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

/// Riproietta una geometria gia' valida: primo percorso che la copre tutta,
/// lati densificati, uscita valida OGC dello stesso tipo.
///
/// `tolleranza` e' lo scarto ammesso fra un lato d'uscita e l'immagine
/// esatta del lato sorgente, nelle unita' del CRS d'arrivo (meta' della sua
/// precisione nel kernel Arrow).
///
/// # Errors
///
/// `PlenoraError::Crs` per un punto fuori dominio o regione
/// (`COORDINATE_OUT_OF_CRS_DOMAIN`), per un'inversa iterativa che non
/// converge (`REPROJECTION_NOT_CONVERGED`), per una geometria che nessun
/// percorso copre (`REPROJECTION_OUTSIDE_TRANSFORMATION_AREA`) o i cui
/// punti preferiscono percorsi diversi
/// (`REPROJECTION_MIXED_TRANSFORMATION_AREAS`), per un lato che non
/// converge (`REPROJECTION_EDGE_NOT_CONVERGED`);
/// `PlenoraError::ResourceLimit` oltre [`MAX_CELL_COORDINATES`] coordinate
/// prodotte;
/// `PlenoraError::Unsupported` per `Line`, `Rect` e `Triangle`, che il
/// decoder WKB non produce;
/// `PlenoraError::InvalidPlan` se l'uscita non e' valida OGC;
/// `PlenoraError::Internal` per una `tolleranza` non finita o non
/// positiva, se il calcolo va in panico (barriera) o se la validazione OGC
/// dell'uscita non conclude.
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
                precedenti: (0..percorso)
                    .map(|precedente| riproiettore.aree_percorso(precedente))
                    .collect(),
                con_griglia: riproiettore
                    .piano()
                    .percorsi()
                    .get(percorso)
                    .is_some_and(|p| p.passi().iter().any(PassoPercorso::a_griglia)),
                misto: false,
            };
            if let Tentativo::Fatto(uscita) = geometria(&mut contesto, ingresso)? {
                // Il percorso copre tutta la geometria, ma alcuni punti ne
                // preferiscono uno precedente: mai un risultato misto.
                if contesto.misto {
                    return Err(CrsError::ReprojectionMixedTransformationAreas.into());
                }
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
/// `PlenoraError::Schema` se la colonna manca, non e' `Binary` o non si
/// dichiara geometria WKB; `PlenoraError::Unsupported` se dichiara
/// dimensioni diverse da XY (o non le dichiara); `PlenoraError::InvalidPlan`
/// per chiavi canoniche del campo non valide; `PlenoraError::Crs` per
/// l'ordine degli assi dichiarato non normalizzato, per un CRS sorgente
/// diverso da quello del piano, per le griglie illeggibili o difettose e
/// per i rifiuti di [`riproietta_geometria`]; per cella, gli errori di
/// decode (`ResourceLimit` oltre [`MAX_CELL_BYTES`](plenora_core::contract::arrow_metadata::MAX_CELL_BYTES),
/// `InvalidPlan` per WKB o geometria non validi, `Unsupported` per Z/M), di
/// [`riproietta_geometria`] e di codifica.
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
    // (chiavi canoniche e metadato `geo`): il campo d'uscita e' quello
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
