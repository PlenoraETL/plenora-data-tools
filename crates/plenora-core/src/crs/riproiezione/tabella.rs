//! Tipi della tabella generata [`super::epsg`] e ricerche su di essa.
//!
//! I valori sono quelli del registro EPSG nelle unita' del registro: gradi
//! per gli angoli dei metodi di proiezione, metri, secondi d'arco per le
//! rotazioni delle trasformazioni, parti per milione per la scala. La
//! conversione in radianti avviene una volta, alla costruzione di
//! [`super::proiezioni::Proiezione`] e di [`super::helmert::Helmert`].

use super::super::integrati::Identificativo;
use super::super::{Ellipsoid, GeographicBounds};
use super::epsg::{DATUM, DEFINIZIONI, TRASFORMAZIONI};

/// Un datum della tabella, identificato dal codice EPSG del suo CRS
/// geografico 2D (per esempio 4326 per WGS 84, 4265 per Monte Mario).
#[derive(Clone, Copy, Debug)]
pub(in crate::crs) struct Datum {
    pub(in crate::crs) codice: u32,
    pub(in crate::crs) nome: &'static str,
    pub(in crate::crs) ellissoide: Ellipsoid,
}

/// Metodo di proiezione di un CRS della tabella, con i parametri EPSG.
///
/// I nomi dei campi seguono i parametri EPSG (codici nel generatore):
/// angoli in gradi, distanze in metri, fattori di scala adimensionali.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::crs) enum MetodoProiezione {
    /// CRS geografico: nessuna proiezione.
    Geografico,
    /// Transverse Mercator (EPSG 9807).
    TrasversaDiMercatore {
        lat0: f64,
        lon0: f64,
        k0: f64,
        falsa_est: f64,
        falsa_nord: f64,
    },
    /// Mercator (variant A) (EPSG 9804); `lat0` e' sempre 0 (generatore).
    MercatoreA {
        lat0: f64,
        lon0: f64,
        k0: f64,
        falsa_est: f64,
        falsa_nord: f64,
    },
    /// Popular Visualisation Pseudo Mercator (EPSG 1024); `lat0` e' 0.
    PseudoMercatore {
        lat0: f64,
        lon0: f64,
        falsa_est: f64,
        falsa_nord: f64,
    },
    /// Lambert Conic Conformal (2SP) (EPSG 9802).
    LambertConicaConforme2Sp {
        lat_origine: f64,
        lon_origine: f64,
        lat1: f64,
        lat2: f64,
        est_origine: f64,
        nord_origine: f64,
    },
    /// Lambert Azimuthal Equal Area (EPSG 9820).
    LambertAzimutaleEquivalente {
        lat0: f64,
        lon0: f64,
        falsa_est: f64,
        falsa_nord: f64,
    },
    /// Oblique Stereographic (EPSG 9809).
    StereograficaObliqua {
        lat0: f64,
        lon0: f64,
        k0: f64,
        falsa_est: f64,
        falsa_nord: f64,
    },
    /// Hotine Oblique Mercator (variant B) (EPSG 9815).
    HotineObliquaB {
        lat_centro: f64,
        lon_centro: f64,
        azimut: f64,
        angolo_reticolo: f64,
        k_centro: f64,
        est_centro: f64,
        nord_centro: f64,
    },
}

/// La definizione di riproiezione di un CRS della tabella.
#[derive(Clone, Copy, Debug)]
pub(in crate::crs) struct Definizione {
    pub(in crate::crs) crs: Identificativo,
    /// Codice del datum ([`Datum::codice`]).
    pub(in crate::crs) datum: u32,
    pub(in crate::crs) metodo: MetodoProiezione,
    /// Regione lon/lat del dominio di validita' (Transverse Mercator e
    /// Mercator); `None` per i geografici e per i metodi il cui dominio e'
    /// solo il rettangolo proiettato.
    pub(in crate::crs) regione: Option<GeographicBounds>,
}

/// Metodo di una trasformazione fra datum.
#[derive(Clone, Copy, Debug)]
pub(in crate::crs) enum MetodoTrasformazione {
    /// Geocentric translations (EPSG 9603), metri.
    Traslazioni { tx: f64, ty: f64, tz: f64 },
    /// Position Vector transformation (EPSG 9606): traslazioni in metri,
    /// rotazioni in secondi d'arco, scala in parti per milione.
    VettorePosizione {
        tx: f64,
        ty: f64,
        tz: f64,
        rx: f64,
        ry: f64,
        rz: f64,
        ds: f64,
    },
    /// Coordinate Frame rotation (EPSG 9607), stesse unita'.
    TelaioCoordinate {
        tx: f64,
        ty: f64,
        tz: f64,
        rx: f64,
        ry: f64,
        rz: f64,
        ds: f64,
    },
    /// `NTv2` (EPSG 9615): il file lo fornisce l'utente; `file_registro` e'
    /// il nome del file nel registro, solo informativo.
    GrigliaNtv2 { file_registro: &'static str },
}

/// Una trasformazione EPSG fra due datum della tabella.
#[derive(Clone, Copy, Debug)]
pub(in crate::crs) struct Trasformazione {
    pub(in crate::crs) codice: u32,
    pub(in crate::crs) nome: &'static str,
    /// Datum sorgente e destinazione nel verso del registro.
    pub(in crate::crs) da: u32,
    pub(in crate::crs) a: u32,
    /// Accuratezza EPSG, in metri.
    pub(in crate::crs) accuratezza_m: f64,
    pub(in crate::crs) metodo: MetodoTrasformazione,
    /// Riquadro dell'area d'uso EPSG (lon/lat, nel datum sorgente).
    pub(in crate::crs) area: GeographicBounds,
}

impl Trasformazione {
    pub(in crate::crs) const fn a_griglia(&self) -> bool {
        matches!(self.metodo, MetodoTrasformazione::GrigliaNtv2 { .. })
    }
}

/// La definizione di un CRS della tabella.
pub(in crate::crs) fn definizione(identificativo: Identificativo) -> Option<&'static Definizione> {
    DEFINIZIONI.iter().find(|voce| voce.crs == identificativo)
}

pub(in crate::crs) fn datum(codice: u32) -> Option<&'static Datum> {
    DATUM.iter().find(|voce| voce.codice == codice)
}

pub(in crate::crs) fn trasformazione(codice: u32) -> Option<&'static Trasformazione> {
    TRASFORMAZIONI.iter().find(|voce| voce.codice == codice)
}

pub(in crate::crs) const fn trasformazioni() -> &'static [Trasformazione] {
    TRASFORMAZIONI
}
