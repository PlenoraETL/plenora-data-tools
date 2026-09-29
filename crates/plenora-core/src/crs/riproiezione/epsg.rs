//! Parametri di riproiezione dei CRS integrati.
//!
//! GENERATA da `scripts/genera_riproiezione.py`: non si modifica a mano.
//! Fonte: registro EPSG v11.022 (2024-11-05), come distribuito con PROJ
//! 9.5.1 e letto con pyproj 3.7.2 (conversioni dei
//! CRS) e dal suo `proj.db` (trasformazioni fra datum).
//!
//! Per ogni CRS: datum (codice del CRS geografico di base), metodo di
//! proiezione con i parametri EPSG (gradi, metri, fattore di scala) e,
//! per Transverse Mercator e Mercator, la regione lon/lat del dominio di
//! validita'. Per ogni datum: ellissoide. Trasformazioni EPSG fra datum
//! della tabella: metodo, parametri (metri, secondi d'arco, parti per
//! milione), accuratezza EPSG in metri, riquadro dell'area d'uso.
//!
//! Operazioni del registro fra questi datum lasciate fuori (concatenate o
//! di altri metodi; il percorso fra datum lo compone `percorsi`):
//! - EPSG:4837 Amersfoort to ED50 (1) (`concatenated_operation`)
//! - EPSG:8047 ED50 to WGS 84 (15) (`concatenated_operation`)
//! - EPSG:8569 ED50 to WGS 84 (21) (`concatenated_operation`)
//! - EPSG:8585 NAD27 to WGS 84 (36) (`concatenated_operation`)
//! - EPSG:8590 NAD27 to WGS 84 (37) (`concatenated_operation`)
//! - EPSG:8591 NAD27 to WGS 84 (38) (`concatenated_operation`)
//! - EPSG:8592 NAD27 to WGS 84 (39) (`concatenated_operation`)
//! - EPSG:8593 NAD27 to WGS 84 (40) (`concatenated_operation`)
//! - EPSG:8594 NAD27 to WGS 84 (41) (`concatenated_operation`)
//! - EPSG:8595 NAD27 to WGS 84 (42) (`concatenated_operation`)
//! - EPSG:8596 NAD27 to WGS 84 (43) (`concatenated_operation`)
//! - EPSG:8597 NAD27 to WGS 84 (44) (`concatenated_operation`)
//! - EPSG:8598 NAD27 to WGS 84 (45) (`concatenated_operation`)
//! - EPSG:8599 NAD27 to WGS 84 (46) (`concatenated_operation`)
//! - EPSG:8600 NAD27 to WGS 84 (47) (`concatenated_operation`)
//! - EPSG:8601 NAD27 to WGS 84 (48) (`concatenated_operation`)
//! - EPSG:8602 NAD27 to WGS 84 (49) (`concatenated_operation`)
//! - EPSG:8603 NAD27 to WGS 84 (50) (`concatenated_operation`)
//! - EPSG:8604 NAD27 to WGS 84 (51) (`concatenated_operation`)
//! - EPSG:8605 NAD27 to WGS 84 (52) (`concatenated_operation`)
//! - EPSG:8606 NAD27 to WGS 84 (53) (`concatenated_operation`)
//! - EPSG:8607 NAD27 to WGS 84 (54) (`concatenated_operation`)
//! - EPSG:8608 NAD27 to WGS 84 (55) (`concatenated_operation`)
//! - EPSG:8609 NAD27 to WGS 84 (56) (`concatenated_operation`)
//! - EPSG:8610 NAD27 to WGS 84 (57) (`concatenated_operation`)
//! - EPSG:8611 NAD27 to WGS 84 (58) (`concatenated_operation`)
//! - EPSG:8612 NAD27 to WGS 84 (59) (`concatenated_operation`)
//! - EPSG:8613 NAD27 to WGS 84 (60) (`concatenated_operation`)
//! - EPSG:8614 NAD27 to WGS 84 (61) (`concatenated_operation`)
//! - EPSG:8615 NAD27 to WGS 84 (62) (`concatenated_operation`)
//! - EPSG:8616 NAD27 to WGS 84 (63) (`concatenated_operation`)
//! - EPSG:8617 NAD27 to WGS 84 (64) (`concatenated_operation`)
//! - EPSG:8618 NAD27 to WGS 84 (65) (`concatenated_operation`)
//! - EPSG:8619 NAD27 to WGS 84 (66) (`concatenated_operation`)
//! - EPSG:8620 NAD27 to WGS 84 (67) (`concatenated_operation`)
//! - EPSG:8621 NAD27 to WGS 84 (68) (`concatenated_operation`)
//! - EPSG:8622 NAD27 to WGS 84 (69) (`concatenated_operation`)
//! - EPSG:8623 NAD27 to WGS 84 (70) (`concatenated_operation`)
//! - EPSG:8624 NAD27 to WGS 84 (71) (`concatenated_operation`)
//! - EPSG:8625 NAD27 to WGS 84 (72) (`concatenated_operation`)
//! - EPSG:8626 NAD27 to WGS 84 (73) (`concatenated_operation`)
//! - EPSG:8627 NAD27 to WGS 84 (74) (`concatenated_operation`)
//! - EPSG:8628 NAD27 to WGS 84 (75) (`concatenated_operation`)
//! - EPSG:8629 NAD27 to WGS 84 (76) (`concatenated_operation`)
//! - EPSG:8630 NAD27 to WGS 84 (77) (`concatenated_operation`)
//! - EPSG:8647 NAD27 to WGS 84 (78) (`concatenated_operation`)

// Dati generati: i letterali restano nella forma piu' corta che rilegge
// lo stesso f64, senza separatori.
#![allow(clippy::unreadable_literal)]

use super::super::integrati::Identificativo;
use super::super::{Ellipsoid, GeographicBounds};
use super::tabella::{Datum, Definizione, MetodoProiezione, MetodoTrasformazione, Trasformazione};

pub(in crate::crs) const VERSIONE_EPSG: &str = "v11.022";
pub(in crate::crs) const VERSIONE_PROJ: &str = "9.5.1";

/// Bessel 1841
const ELLISSOIDE_BESSEL_1841: Ellipsoid = Ellipsoid {
    semi_major_axis_metre: 6377397.155,
    inverse_flattening: 299.1528128,
};
/// GRS 1980
const ELLISSOIDE_GRS_1980: Ellipsoid = Ellipsoid {
    semi_major_axis_metre: 6378137.0,
    inverse_flattening: 298.257222101,
};
/// International 1924
const ELLISSOIDE_INTERNATIONAL_1924: Ellipsoid = Ellipsoid {
    semi_major_axis_metre: 6378388.0,
    inverse_flattening: 297.0,
};
/// Clarke 1866
const ELLISSOIDE_CLARKE_1866: Ellipsoid = Ellipsoid {
    semi_major_axis_metre: 6378206.4,
    inverse_flattening: 294.9786982138982,
};
/// Airy 1830
const ELLISSOIDE_AIRY_1830: Ellipsoid = Ellipsoid {
    semi_major_axis_metre: 6377563.396,
    inverse_flattening: 299.3249646,
};
/// WGS 84
const ELLISSOIDE_WGS_84: Ellipsoid = Ellipsoid {
    semi_major_axis_metre: 6378137.0,
    inverse_flattening: 298.257223563,
};
/// CGCS2000
const ELLISSOIDE_CGCS2000: Ellipsoid = Ellipsoid {
    semi_major_axis_metre: 6378137.0,
    inverse_flattening: 298.257222101,
};

/// Datum della tabella, in ordine di codice del CRS geografico di base.
pub(in crate::crs) const DATUM: &[Datum] = &[
    Datum {
        codice: 4150,
        nome: "CH1903+",
        ellissoide: ELLISSOIDE_BESSEL_1841,
    },
    Datum {
        codice: 4167,
        nome: "New Zealand Geodetic Datum 2000",
        ellissoide: ELLISSOIDE_GRS_1980,
    },
    Datum {
        codice: 4171,
        nome: "Reseau Geodesique Francais 1993 v1",
        ellissoide: ELLISSOIDE_GRS_1980,
    },
    Datum {
        codice: 4230,
        nome: "European Datum 1950",
        ellissoide: ELLISSOIDE_INTERNATIONAL_1924,
    },
    Datum {
        codice: 4258,
        nome: "European Terrestrial Reference System 1989 ensemble",
        ellissoide: ELLISSOIDE_GRS_1980,
    },
    Datum {
        codice: 4265,
        nome: "Monte Mario",
        ellissoide: ELLISSOIDE_INTERNATIONAL_1924,
    },
    Datum {
        codice: 4267,
        nome: "North American Datum 1927",
        ellissoide: ELLISSOIDE_CLARKE_1866,
    },
    Datum {
        codice: 4269,
        nome: "North American Datum 1983",
        ellissoide: ELLISSOIDE_GRS_1980,
    },
    Datum {
        codice: 4277,
        nome: "Ordnance Survey of Great Britain 1936",
        ellissoide: ELLISSOIDE_AIRY_1830,
    },
    Datum {
        codice: 4283,
        nome: "Geocentric Datum of Australia 1994",
        ellissoide: ELLISSOIDE_GRS_1980,
    },
    Datum {
        codice: 4289,
        nome: "Amersfoort",
        ellissoide: ELLISSOIDE_BESSEL_1841,
    },
    Datum {
        codice: 4314,
        nome: "Deutsches Hauptdreiecksnetz",
        ellissoide: ELLISSOIDE_BESSEL_1841,
    },
    Datum {
        codice: 4326,
        nome: "World Geodetic System 1984 ensemble",
        ellissoide: ELLISSOIDE_WGS_84,
    },
    Datum {
        codice: 4490,
        nome: "China 2000",
        ellissoide: ELLISSOIDE_CGCS2000,
    },
    Datum {
        codice: 4670,
        nome: "Istituto Geografico Militaire 1995",
        ellissoide: ELLISSOIDE_GRS_1980,
    },
    Datum {
        codice: 4674,
        nome: "Sistema de Referencia Geocentrico para las AmericaS 2000",
        ellissoide: ELLISSOIDE_GRS_1980,
    },
    Datum {
        codice: 6706,
        nome: "Rete Dinamica Nazionale 2008",
        ellissoide: ELLISSOIDE_GRS_1980,
    },
    Datum {
        codice: 7844,
        nome: "Geocentric Datum of Australia 2020",
        ellissoide: ELLISSOIDE_GRS_1980,
    },
];

/// Definizioni dei CRS: `OGC:CRS84` per primo, poi i codici EPSG in ordine.
pub(in crate::crs) const DEFINIZIONI: &[Definizione] = &[
    Definizione {
        crs: Identificativo::OgcCrs84,
        datum: 4326,
        metodo: MetodoProiezione::Geografico,
        regione: None,
    },
    Definizione {
        crs: Identificativo::Epsg(2056),
        datum: 4150,
        metodo: MetodoProiezione::HotineObliquaB {
            lat_centro: 46.95240555555556,
            lon_centro: 7.439583333333333,
            azimut: 90.0,
            angolo_reticolo: 90.0,
            k_centro: 1.0,
            est_centro: 2600000.0,
            nord_centro: 1200000.0,
        },
        regione: None,
    },
    Definizione {
        crs: Identificativo::Epsg(2154),
        datum: 4171,
        metodo: MetodoProiezione::LambertConicaConforme2Sp {
            lat_origine: 46.5,
            lon_origine: 3.0,
            lat1: 49.0,
            lat2: 44.0,
            est_origine: 700000.0,
            nord_origine: 6600000.0,
        },
        regione: None,
    },
    Definizione {
        crs: Identificativo::Epsg(2193),
        datum: 4167,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 173.0,
            k0: 0.9996,
            falsa_est: 1600000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(
            158.0,
            -53.94499999999999,
            188.0,
            -27.485000000000003,
        )),
    },
    Definizione {
        crs: Identificativo::Epsg(3003),
        datum: 4265,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 9.0,
            k0: 0.9996,
            falsa_est: 1500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(
            -6.0,
            31.275000000000002,
            24.0,
            52.295,
        )),
    },
    Definizione {
        crs: Identificativo::Epsg(3004),
        datum: 4265,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 15.0,
            k0: 0.9996,
            falsa_est: 2520000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(0.0, 28.589999999999996, 30.0, 53.27)),
    },
    Definizione {
        crs: Identificativo::Epsg(3035),
        datum: 4258,
        metodo: MetodoProiezione::LambertAzimutaleEquivalente {
            lat0: 52.0,
            lon0: 10.0,
            falsa_est: 4321000.0,
            falsa_nord: 3210000.0,
        },
        regione: None,
    },
    Definizione {
        crs: Identificativo::Epsg(3064),
        datum: 4670,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 9.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(-6.0, 0.0, 24.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(3065),
        datum: 4670,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 15.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(0.0, 0.0, 30.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(3395),
        datum: 4326,
        metodo: MetodoProiezione::MercatoreA {
            lat0: 0.0,
            lon0: 0.0,
            k0: 1.0,
            falsa_est: 0.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(-180.0, -85.06, 180.0, 85.06)),
    },
    Definizione {
        crs: Identificativo::Epsg(3857),
        datum: 4326,
        metodo: MetodoProiezione::PseudoMercatore {
            lat0: 0.0,
            lon0: 0.0,
            falsa_est: 0.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(-180.0, -85.06, 180.0, 85.06)),
    },
    Definizione {
        crs: Identificativo::Epsg(4171),
        datum: 4171,
        metodo: MetodoProiezione::Geografico,
        regione: None,
    },
    Definizione {
        crs: Identificativo::Epsg(4230),
        datum: 4230,
        metodo: MetodoProiezione::Geografico,
        regione: None,
    },
    Definizione {
        crs: Identificativo::Epsg(4258),
        datum: 4258,
        metodo: MetodoProiezione::Geografico,
        regione: None,
    },
    Definizione {
        crs: Identificativo::Epsg(4265),
        datum: 4265,
        metodo: MetodoProiezione::Geografico,
        regione: None,
    },
    Definizione {
        crs: Identificativo::Epsg(4267),
        datum: 4267,
        metodo: MetodoProiezione::Geografico,
        regione: None,
    },
    Definizione {
        crs: Identificativo::Epsg(4269),
        datum: 4269,
        metodo: MetodoProiezione::Geografico,
        regione: None,
    },
    Definizione {
        crs: Identificativo::Epsg(4277),
        datum: 4277,
        metodo: MetodoProiezione::Geografico,
        regione: None,
    },
    Definizione {
        crs: Identificativo::Epsg(4283),
        datum: 4283,
        metodo: MetodoProiezione::Geografico,
        regione: None,
    },
    Definizione {
        crs: Identificativo::Epsg(4326),
        datum: 4326,
        metodo: MetodoProiezione::Geografico,
        regione: None,
    },
    Definizione {
        crs: Identificativo::Epsg(4490),
        datum: 4490,
        metodo: MetodoProiezione::Geografico,
        regione: None,
    },
    Definizione {
        crs: Identificativo::Epsg(4670),
        datum: 4670,
        metodo: MetodoProiezione::Geografico,
        regione: None,
    },
    Definizione {
        crs: Identificativo::Epsg(4674),
        datum: 4674,
        metodo: MetodoProiezione::Geografico,
        regione: None,
    },
    Definizione {
        crs: Identificativo::Epsg(6706),
        datum: 6706,
        metodo: MetodoProiezione::Geografico,
        regione: None,
    },
    Definizione {
        crs: Identificativo::Epsg(6707),
        datum: 6706,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 9.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(-6.0, 0.0, 24.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(6708),
        datum: 6706,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 15.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(0.0, 0.0, 30.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(6709),
        datum: 6706,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 21.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(6.0, 0.0, 36.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(6875),
        datum: 6706,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 12.0,
            k0: 0.9985,
            falsa_est: 7000000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(-3.0, 28.589999999999996, 27.0, 53.27)),
    },
    Definizione {
        crs: Identificativo::Epsg(7791),
        datum: 6706,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 9.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(-6.0, 0.0, 24.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(7792),
        datum: 6706,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 15.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(0.0, 0.0, 30.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(7793),
        datum: 6706,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 21.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(6.0, 0.0, 36.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(7794),
        datum: 6706,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 12.0,
            k0: 0.9985,
            falsa_est: 7000000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(-3.0, 28.589999999999996, 27.0, 53.27)),
    },
    Definizione {
        crs: Identificativo::Epsg(7844),
        datum: 7844,
        metodo: MetodoProiezione::Geografico,
        regione: None,
    },
    Definizione {
        crs: Identificativo::Epsg(23032),
        datum: 4230,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 9.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(-6.0, 0.0, 24.0, 84.33)),
    },
    Definizione {
        crs: Identificativo::Epsg(23033),
        datum: 4230,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 15.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(0.0, 0.0, 30.0, 84.42)),
    },
    Definizione {
        crs: Identificativo::Epsg(23034),
        datum: 4230,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 21.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(6.0, 0.0, 36.0, 84.54)),
    },
    Definizione {
        crs: Identificativo::Epsg(25828),
        datum: 4258,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -15.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(-30.0, 0.0, 0.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(25829),
        datum: 4258,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -9.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(-24.0, 0.0, 6.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(25830),
        datum: 4258,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -3.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(-18.0, 0.0, 12.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(25831),
        datum: 4258,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 3.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(-12.0, 0.0, 18.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(25832),
        datum: 4258,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 9.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(-6.0, 0.0, 24.0, 84.01)),
    },
    Definizione {
        crs: Identificativo::Epsg(25833),
        datum: 4258,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 15.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(0.0, 0.0, 30.0, 84.01)),
    },
    Definizione {
        crs: Identificativo::Epsg(25834),
        datum: 4258,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 21.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(6.0, 0.0, 36.0, 84.01)),
    },
    Definizione {
        crs: Identificativo::Epsg(25835),
        datum: 4258,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 27.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(12.0, 0.0, 42.0, 84.01)),
    },
    Definizione {
        crs: Identificativo::Epsg(25836),
        datum: 4258,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 33.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(18.0, 0.0, 48.0, 84.01)),
    },
    Definizione {
        crs: Identificativo::Epsg(25837),
        datum: 4258,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 39.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(24.0, 0.0, 54.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(27700),
        datum: 4277,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 49.0,
            lon0: -2.0,
            k0: 0.9996012717,
            falsa_est: 400000.0,
            falsa_nord: -100000.0,
        },
        regione: Some(GeographicBounds::new(
            -17.0,
            44.120000000000005,
            13.0,
            66.64,
        )),
    },
    Definizione {
        crs: Identificativo::Epsg(28992),
        datum: 4289,
        metodo: MetodoProiezione::StereograficaObliqua {
            lat0: 52.15616055555555,
            lon0: 5.3876388888888895,
            k0: 0.9999079,
            falsa_est: 155000.0,
            falsa_nord: 463000.0,
        },
        regione: None,
    },
    Definizione {
        crs: Identificativo::Epsg(31467),
        datum: 4314,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 9.0,
            k0: 1.0,
            falsa_est: 3500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(-6.0, 43.36, 24.0, 59.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32601),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -177.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(-192.0, 0.0, -162.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32602),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -171.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(-186.0, 0.0, -156.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32603),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -165.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(-180.0, 0.0, -150.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32604),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -159.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(-174.0, 0.0, -144.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32605),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -153.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(-168.0, 0.0, -138.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32606),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -147.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(-162.0, 0.0, -132.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32607),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -141.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(-156.0, 0.0, -126.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32608),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -135.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(-150.0, 0.0, -120.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32609),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -129.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(-144.0, 0.0, -114.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32610),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -123.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(-138.0, 0.0, -108.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32611),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -117.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(-132.0, 0.0, -102.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32612),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -111.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(-126.0, 0.0, -96.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32613),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -105.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(-120.0, 0.0, -90.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32614),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -99.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(-114.0, 0.0, -84.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32615),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -93.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(-108.0, 0.0, -78.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32616),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -87.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(-102.0, 0.0, -72.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32617),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -81.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(-96.0, 0.0, -66.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32618),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -75.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(-90.0, 0.0, -60.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32619),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -69.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(-84.0, 0.0, -54.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32620),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -63.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(-78.0, 0.0, -48.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32621),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -57.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(-72.0, 0.0, -42.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32622),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -51.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(-66.0, 0.0, -36.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32623),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -45.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(-60.0, 0.0, -30.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32624),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -39.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(-54.0, 0.0, -24.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32625),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -33.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(-48.0, 0.0, -18.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32626),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -27.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(-42.0, 0.0, -12.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32627),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -21.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(-36.0, 0.0, -6.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32628),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -15.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(-30.0, 0.0, 0.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32629),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -9.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(-24.0, 0.0, 6.0, 84.01)),
    },
    Definizione {
        crs: Identificativo::Epsg(32630),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -3.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(-18.0, 0.0, 12.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32631),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 3.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(-12.0, 0.0, 18.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32632),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 9.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(-6.0, 0.0, 24.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32633),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 15.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(0.0, 0.0, 30.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32634),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 21.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(6.0, 0.0, 36.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32635),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 27.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(12.0, 0.0, 42.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32636),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 33.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(18.0, 0.0, 48.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32637),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 39.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(24.0, 0.0, 54.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32638),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 45.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(30.0, 0.0, 60.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32639),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 51.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(36.0, 0.0, 66.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32640),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 57.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(42.0, 0.0, 72.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32641),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 63.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(48.0, 0.0, 78.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32642),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 69.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(54.0, 0.0, 84.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32643),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 75.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(60.0, 0.0, 90.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32644),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 81.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(66.0, 0.0, 96.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32645),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 87.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(72.0, 0.0, 102.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32646),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 93.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(78.0, 0.0, 108.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32647),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 99.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(84.0, 0.0, 114.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32648),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 105.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(90.0, 0.0, 120.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32649),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 111.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(96.0, 0.0, 126.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32650),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 117.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(102.0, 0.0, 132.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32651),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 123.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(108.0, 0.0, 138.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32652),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 129.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(114.0, 0.0, 144.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32653),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 135.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(120.0, 0.0, 150.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32654),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 141.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(126.0, 0.0, 156.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32655),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 147.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(132.0, 0.0, 162.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32656),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 153.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(138.0, 0.0, 168.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32657),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 159.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(144.0, 0.0, 174.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32658),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 165.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(150.0, 0.0, 180.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32659),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 171.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(156.0, 0.0, 186.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32660),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 177.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 0.0,
        },
        regione: Some(GeographicBounds::new(162.0, 0.0, 192.0, 84.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32701),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -177.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(-192.0, -80.0, -162.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32702),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -171.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(-186.0, -80.0, -156.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32703),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -165.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(-180.0, -80.0, -150.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32704),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -159.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(-174.0, -80.0, -144.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32705),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -153.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(-168.0, -80.0, -138.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32706),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -147.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(-162.0, -80.0, -132.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32707),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -141.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(-156.0, -80.0, -126.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32708),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -135.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(-150.0, -80.0, -120.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32709),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -129.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(-144.0, -80.0, -114.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32710),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -123.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(-138.0, -80.0, -108.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32711),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -117.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(-132.0, -80.0, -102.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32712),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -111.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(-126.0, -80.0, -96.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32713),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -105.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(-120.0, -80.0, -90.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32714),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -99.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(-114.0, -80.0, -84.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32715),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -93.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(-108.0, -80.0, -78.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32716),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -87.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(-102.0, -80.0, -72.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32717),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -81.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(-96.0, -80.0, -66.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32718),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -75.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(-90.0, -80.0, -60.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32719),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -69.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(-84.0, -80.0, -54.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32720),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -63.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(-78.0, -80.0, -48.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32721),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -57.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(-72.0, -80.0, -42.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32722),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -51.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(-66.0, -80.0, -36.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32723),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -45.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(-60.0, -80.0, -30.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32724),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -39.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(-54.0, -80.0, -24.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32725),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -33.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(-48.0, -80.0, -18.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32726),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -27.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(-42.0, -80.0, -12.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32727),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -21.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(-36.0, -80.0, -6.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32728),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -15.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(-30.0, -80.0, 0.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32729),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -9.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(-24.0, -80.0, 6.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32730),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: -3.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(-18.0, -80.0, 12.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32731),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 3.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(-12.0, -80.0, 18.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32732),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 9.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(-6.0, -80.0, 24.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32733),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 15.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(0.0, -80.0, 30.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32734),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 21.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(6.0, -80.0, 36.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32735),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 27.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(12.0, -80.0, 42.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32736),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 33.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(18.0, -80.0, 48.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32737),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 39.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(24.0, -80.0, 54.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32738),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 45.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(30.0, -80.0, 60.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32739),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 51.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(36.0, -80.0, 66.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32740),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 57.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(42.0, -80.0, 72.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32741),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 63.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(48.0, -80.0, 78.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32742),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 69.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(54.0, -80.0, 84.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32743),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 75.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(60.0, -80.0, 90.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32744),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 81.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(66.0, -80.0, 96.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32745),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 87.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(72.0, -80.0, 102.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32746),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 93.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(78.0, -80.0, 108.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32747),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 99.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(84.0, -80.0, 114.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32748),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 105.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(90.0, -80.0, 120.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32749),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 111.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(96.0, -80.0, 126.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32750),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 117.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(102.0, -80.0, 132.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32751),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 123.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(108.0, -80.0, 138.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32752),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 129.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(114.0, -80.0, 144.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32753),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 135.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(120.0, -80.0, 150.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32754),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 141.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(126.0, -80.0, 156.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32755),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 147.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(132.0, -80.0, 162.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32756),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 153.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(138.0, -80.0, 168.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32757),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 159.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(144.0, -80.0, 174.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32758),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 165.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(150.0, -80.0, 180.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32759),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 171.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(156.0, -80.0, 186.0, 0.0)),
    },
    Definizione {
        crs: Identificativo::Epsg(32760),
        datum: 4326,
        metodo: MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: 177.0,
            k0: 0.9996,
            falsa_est: 500000.0,
            falsa_nord: 10000000.0,
        },
        regione: Some(GeographicBounds::new(162.0, -80.0, 192.0, 0.0)),
    },
];

/// Trasformazioni fra datum della tabella, in ordine di codice.
pub(in crate::crs) const TRASFORMAZIONI: &[Trasformazione] = &[
    // Asia - Middle East - Israel, Palestine Territory, Turkey - offshore
    Trasformazione {
        codice: 1075,
        nome: "ED50 to WGS 84 (38)",
        da: 4230,
        a: 4326,
        accuratezza_m: 10.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -89.05,
            ty: -87.03,
            tz: -124.56,
        },
        area: GeographicBounds::new(28.03, 31.35, 41.47, 43.45),
    },
    // Jordan
    Trasformazione {
        codice: 1087,
        nome: "ED50 to WGS 84 (37)",
        da: 4230,
        a: 4326,
        accuratezza_m: 2.5,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -112.0,
            ty: -110.3,
            tz: -140.2,
        },
        area: GeographicBounds::new(34.88, 29.18, 39.31, 33.38),
    },
    // Italy - Adriatic - North Ancona
    Trasformazione {
        codice: 1088,
        nome: "Monte Mario to WGS 84 (5)",
        da: 4265,
        a: 4326,
        accuratezza_m: 10.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -223.7,
            ty: -67.38,
            tz: 1.34,
        },
        area: GeographicBounds::new(12.22, 43.62, 13.96, 45.73),
    },
    // Italy - Adriatic - South Ancona / North Gargano
    Trasformazione {
        codice: 1089,
        nome: "Monte Mario to WGS 84 (6)",
        da: 4265,
        a: 4326,
        accuratezza_m: 10.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -225.4,
            ty: -67.7,
            tz: 7.85,
        },
        area: GeographicBounds::new(13.61, 41.95, 16.14, 44.04),
    },
    // Italy - Adriatic - South Gargano
    Trasformazione {
        codice: 1090,
        nome: "Monte Mario to WGS 84 (7)",
        da: 4265,
        a: 4326,
        accuratezza_m: 10.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -227.1,
            ty: -68.1,
            tz: 14.4,
        },
        area: GeographicBounds::new(15.95, 40.72, 18.63, 42.28),
    },
    // Italy - Otranto channel
    Trasformazione {
        codice: 1091,
        nome: "Monte Mario to WGS 84 (8)",
        da: 4265,
        a: 4326,
        accuratezza_m: 10.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -231.61,
            ty: -68.21,
            tz: 13.93,
        },
        area: GeographicBounds::new(17.95, 39.77, 18.99, 41.03),
    },
    // Italy - north Ionian Sea
    Trasformazione {
        codice: 1092,
        nome: "Monte Mario to WGS 84 (9)",
        da: 4265,
        a: 4326,
        accuratezza_m: 10.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -225.06,
            ty: -67.37,
            tz: 14.61,
        },
        area: GeographicBounds::new(16.55, 37.67, 18.93, 40.47),
    },
    // Italy - Sicily Strait east of 13°E
    Trasformazione {
        codice: 1093,
        nome: "Monte Mario to WGS 84 (10)",
        da: 4265,
        a: 4326,
        accuratezza_m: 10.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -229.08,
            ty: -65.73,
            tz: 20.21,
        },
        area: GeographicBounds::new(13.0, 35.22, 15.16, 37.48),
    },
    // Italy - Sicily Strait west of 13°E
    Trasformazione {
        codice: 1094,
        nome: "Monte Mario to WGS 84 (11)",
        da: 4265,
        a: 4326,
        accuratezza_m: 10.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -230.47,
            ty: -56.08,
            tz: 22.43,
        },
        area: GeographicBounds::new(10.68, 35.28, 13.01, 38.45),
    },
    // Italy - including San Marino and Vatican
    Trasformazione {
        codice: 1098,
        nome: "IGM95 to ETRS89 (1)",
        da: 4670,
        a: 4258,
        accuratezza_m: 0.5,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: 0.0,
            ty: 0.0,
            tz: 0.0,
        },
        area: GeographicBounds::new(5.93, 34.76, 18.99, 47.1),
    },
    // Italy - including San Marino and Vatican
    Trasformazione {
        codice: 1099,
        nome: "IGM95 to WGS 84 (1)",
        da: 4670,
        a: 4326,
        accuratezza_m: 1.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: 0.0,
            ty: 0.0,
            tz: 0.0,
        },
        area: GeographicBounds::new(5.93, 34.76, 18.99, 47.1),
    },
    // Europe - west (DMA ED50 mean)
    Trasformazione {
        codice: 1133,
        nome: "ED50 to WGS 84 (1)",
        da: 4230,
        a: 4326,
        accuratezza_m: 10.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -87.0,
            ty: -98.0,
            tz: -121.0,
        },
        area: GeographicBounds::new(-9.56, 34.88, 31.59, 71.24),
    },
    // Europe - west central (by country)
    Trasformazione {
        codice: 1134,
        nome: "ED50 to WGS 84 (2)",
        da: 4230,
        a: 4326,
        accuratezza_m: 6.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -87.0,
            ty: -96.0,
            tz: -120.0,
        },
        area: GeographicBounds::new(-4.87, 42.33, 17.17, 57.8),
    },
    // Asia - Middle East - Iraq; Israel; Jordan; Lebanon; Kuwait; Saudi Arabia; Syria
    Trasformazione {
        codice: 1135,
        nome: "ED50 to WGS 84 (3)",
        da: 4230,
        a: 4326,
        accuratezza_m: 999.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -103.0,
            ty: -106.0,
            tz: -141.0,
        },
        area: GeographicBounds::new(34.17, 16.37, 55.67, 37.39),
    },
    // Cyprus
    Trasformazione {
        codice: 1136,
        nome: "ED50 to WGS 84 (4)",
        da: 4230,
        a: 4326,
        accuratezza_m: 26.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -104.0,
            ty: -101.0,
            tz: -140.0,
        },
        area: GeographicBounds::new(29.95, 32.88, 35.2, 36.21),
    },
    // Egypt - Western Desert
    Trasformazione {
        codice: 1137,
        nome: "ED50 to WGS 84 (5)",
        da: 4230,
        a: 4326,
        accuratezza_m: 13.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -130.0,
            ty: -117.0,
            tz: -151.0,
        },
        area: GeographicBounds::new(24.7, 25.71, 30.0, 31.68),
    },
    // Europe - British Isles and Channel Islands onshore
    Trasformazione {
        codice: 1138,
        nome: "ED50 to WGS 84 (6)",
        da: 4230,
        a: 4326,
        accuratezza_m: 6.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -86.0,
            ty: -96.0,
            tz: -120.0,
        },
        area: GeographicBounds::new(-10.56, 49.11, 1.84, 60.9),
    },
    // Europe - Finland and Norway - onshore
    Trasformazione {
        codice: 1139,
        nome: "ED50 to WGS 84 (7)",
        da: 4230,
        a: 4326,
        accuratezza_m: 7.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -87.0,
            ty: -95.0,
            tz: -120.0,
        },
        area: GeographicBounds::new(4.39, 57.9, 31.59, 71.24),
    },
    // Greece - onshore
    Trasformazione {
        codice: 1140,
        nome: "ED50 to WGS 84 (8)",
        da: 4230,
        a: 4326,
        accuratezza_m: 44.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -84.0,
            ty: -95.0,
            tz: -130.0,
        },
        area: GeographicBounds::new(19.57, 34.88, 28.3, 41.75),
    },
    // Italy - Sardinia onshore
    Trasformazione {
        codice: 1142,
        nome: "ED50 to WGS 84 (10)",
        da: 4230,
        a: 4326,
        accuratezza_m: 44.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -97.0,
            ty: -103.0,
            tz: -120.0,
        },
        area: GeographicBounds::new(8.08, 38.82, 9.89, 41.31),
    },
    // Italy - Sicily onshore
    Trasformazione {
        codice: 1143,
        nome: "ED50 to WGS 84 (11)",
        da: 4230,
        a: 4326,
        accuratezza_m: 35.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -97.0,
            ty: -88.0,
            tz: -135.0,
        },
        area: GeographicBounds::new(12.36, 36.59, 15.71, 38.35),
    },
    // Malta - onshore
    Trasformazione {
        codice: 1144,
        nome: "ED50 to WGS 84 (12)",
        da: 4230,
        a: 4326,
        accuratezza_m: 44.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -107.0,
            ty: -88.0,
            tz: -149.0,
        },
        area: GeographicBounds::new(14.27, 35.74, 14.63, 36.05),
    },
    // Europe - Portugal and Spain
    Trasformazione {
        codice: 1145,
        nome: "ED50 to WGS 84 (13)",
        da: 4230,
        a: 4326,
        accuratezza_m: 9.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -84.0,
            ty: -107.0,
            tz: -120.0,
        },
        area: GeographicBounds::new(-9.56, 35.26, 3.39, 43.82),
    },
    // Europe - ETRF by country
    Trasformazione {
        codice: 1149,
        nome: "ETRS89 to WGS 84 (1)",
        da: 4258,
        a: 4326,
        accuratezza_m: 1.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: 0.0,
            ty: 0.0,
            tz: 0.0,
        },
        area: GeographicBounds::new(-16.1, 33.26, 38.01, 84.73),
    },
    // Australia - GDA
    Trasformazione {
        codice: 1150,
        nome: "GDA94 to WGS 84 (1)",
        da: 4283,
        a: 4326,
        accuratezza_m: 3.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: 0.0,
            ty: 0.0,
            tz: 0.0,
        },
        area: GeographicBounds::new(93.41, -60.55, 173.34, -8.47),
    },
    // Italy - Sardinia onshore
    Trasformazione {
        codice: 1169,
        nome: "Monte Mario to WGS 84 (1)",
        da: 4265,
        a: 4326,
        accuratezza_m: 44.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -225.0,
            ty: -65.0,
            tz: 9.0,
        },
        area: GeographicBounds::new(8.08, 38.82, 9.89, 41.31),
    },
    // Caribbean - central (DMA tfm)
    Trasformazione {
        codice: 1170,
        nome: "NAD27 to WGS 84 (1)",
        da: 4267,
        a: 4326,
        accuratezza_m: 16.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -3.0,
            ty: 142.0,
            tz: 183.0,
        },
        area: GeographicBounds::new(-85.01, 13.0, -59.37, 23.25),
    },
    // Central America - Belize to Costa Rica
    Trasformazione {
        codice: 1171,
        nome: "NAD27 to WGS 84 (2)",
        da: 4267,
        a: 4326,
        accuratezza_m: 10.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: 0.0,
            ty: 125.0,
            tz: 194.0,
        },
        area: GeographicBounds::new(-92.29, 7.98, -82.53, 18.49),
    },
    // Canada - NAD27
    Trasformazione {
        codice: 1172,
        nome: "NAD27 to WGS 84 (3)",
        da: 4267,
        a: 4326,
        accuratezza_m: 20.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -10.0,
            ty: 158.0,
            tz: 187.0,
        },
        area: GeographicBounds::new(-141.01, 40.0, -44.0, 83.17),
    },
    // USA - CONUS - onshore
    Trasformazione {
        codice: 1173,
        nome: "NAD27 to WGS 84 (4)",
        da: 4267,
        a: 4326,
        accuratezza_m: 10.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -8.0,
            ty: 160.0,
            tz: 176.0,
        },
        area: GeographicBounds::new(-124.79, 24.41, -66.91, 49.38),
    },
    // USA - CONUS east of Mississippi River - onshore
    Trasformazione {
        codice: 1174,
        nome: "NAD27 to WGS 84 (5)",
        da: 4267,
        a: 4326,
        accuratezza_m: 11.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -9.0,
            ty: 161.0,
            tz: 179.0,
        },
        area: GeographicBounds::new(-97.22, 24.41, -66.91, 49.38),
    },
    // USA - CONUS west of Mississippi River - onshore
    Trasformazione {
        codice: 1175,
        nome: "NAD27 to WGS 84 (6)",
        da: 4267,
        a: 4326,
        accuratezza_m: 7.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -8.0,
            ty: 159.0,
            tz: 175.0,
        },
        area: GeographicBounds::new(-124.79, 25.83, -89.64, 49.05),
    },
    // USA - Alaska mainland
    Trasformazione {
        codice: 1176,
        nome: "NAD27 to WGS 84 (7)",
        da: 4267,
        a: 4326,
        accuratezza_m: 12.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -5.0,
            ty: 135.0,
            tz: 172.0,
        },
        area: GeographicBounds::new(-168.26, 54.34, -129.99, 71.4),
    },
    // Bahamas - main islands onshore
    Trasformazione {
        codice: 1177,
        nome: "NAD27 to WGS 84 (8)",
        da: 4267,
        a: 4326,
        accuratezza_m: 8.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -4.0,
            ty: 154.0,
            tz: 178.0,
        },
        area: GeographicBounds::new(-79.04, 20.86, -72.68, 27.29),
    },
    // Bahamas (San Salvador Island) - onshore
    Trasformazione {
        codice: 1178,
        nome: "NAD27 to WGS 84 (9)",
        da: 4267,
        a: 4326,
        accuratezza_m: 44.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: 1.0,
            ty: 140.0,
            tz: 165.0,
        },
        area: GeographicBounds::new(-74.6, 23.9, -74.37, 24.19),
    },
    // Canada - Alberta and British Columbia
    Trasformazione {
        codice: 1179,
        nome: "NAD27 to WGS 84 (10)",
        da: 4267,
        a: 4326,
        accuratezza_m: 13.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -7.0,
            ty: 162.0,
            tz: 188.0,
        },
        area: GeographicBounds::new(-139.04, 48.25, -109.98, 60.01),
    },
    // Canada - Manitoba and Ontario
    Trasformazione {
        codice: 1180,
        nome: "NAD27 to WGS 84 (11)",
        da: 4267,
        a: 4326,
        accuratezza_m: 12.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -9.0,
            ty: 157.0,
            tz: 184.0,
        },
        area: GeographicBounds::new(-102.0, 41.67, -74.35, 60.01),
    },
    // Canada - eastern provinces
    Trasformazione {
        codice: 1181,
        nome: "NAD27 to WGS 84 (12)",
        da: 4267,
        a: 4326,
        accuratezza_m: 9.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -22.0,
            ty: 160.0,
            tz: 190.0,
        },
        area: GeographicBounds::new(-79.85, 43.41, -52.54, 62.62),
    },
    // Canada - NWT; Nunavut; Saskatchewan
    Trasformazione {
        codice: 1182,
        nome: "NAD27 to WGS 84 (13)",
        da: 4267,
        a: 4326,
        accuratezza_m: 8.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: 4.0,
            ty: 159.0,
            tz: 188.0,
        },
        area: GeographicBounds::new(-136.46, 49.0, -60.72, 83.17),
    },
    // Canada - Yukon
    Trasformazione {
        codice: 1183,
        nome: "NAD27 to WGS 84 (14)",
        da: 4267,
        a: 4326,
        accuratezza_m: 10.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -7.0,
            ty: 139.0,
            tz: 181.0,
        },
        area: GeographicBounds::new(-141.01, 59.99, -123.91, 69.7),
    },
    // Panama - Canal Zone
    Trasformazione {
        codice: 1184,
        nome: "NAD27 to WGS 84 (15)",
        da: 4267,
        a: 4326,
        accuratezza_m: 35.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: 0.0,
            ty: 125.0,
            tz: 201.0,
        },
        area: GeographicBounds::new(-80.07, 8.82, -79.46, 9.45),
    },
    // Cuba - onshore
    Trasformazione {
        codice: 1185,
        nome: "NAD27 to WGS 84 (16)",
        da: 4267,
        a: 4326,
        accuratezza_m: 44.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -9.0,
            ty: 152.0,
            tz: 178.0,
        },
        area: GeographicBounds::new(-85.01, 19.77, -74.07, 23.25),
    },
    // Greenland - Hayes Peninsula
    Trasformazione {
        codice: 1186,
        nome: "NAD27 to WGS 84 (17)",
        da: 4267,
        a: 4326,
        accuratezza_m: 44.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: 11.0,
            ty: 114.0,
            tz: 195.0,
        },
        area: GeographicBounds::new(-73.29, 75.86, -60.98, 79.2),
    },
    // Mexico - onshore
    Trasformazione {
        codice: 1187,
        nome: "NAD27 to WGS 84 (18)",
        da: 4267,
        a: 4326,
        accuratezza_m: 12.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -12.0,
            ty: 130.0,
            tz: 190.0,
        },
        area: GeographicBounds::new(-118.47, 14.51, -86.68, 32.72),
    },
    // North America - Canada and USA (CONUS, Alaska mainland)
    Trasformazione {
        codice: 1188,
        nome: "NAD83 to WGS 84 (1)",
        da: 4269,
        a: 4326,
        accuratezza_m: 4.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: 0.0,
            ty: 0.0,
            tz: 0.0,
        },
        area: GeographicBounds::new(-172.54, 23.81, -47.74, 86.46),
    },
    // UK - Great Britain onshore and nearshore; Isle of Man
    Trasformazione {
        codice: 1195,
        nome: "OSGB36 to WGS 84 (1)",
        da: 4277,
        a: 4326,
        accuratezza_m: 21.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: 375.0,
            ty: -111.0,
            tz: 431.0,
        },
        area: GeographicBounds::new(-8.82, 49.79, 1.92, 60.94),
    },
    // UK - England
    Trasformazione {
        codice: 1196,
        nome: "OSGB36 to WGS 84 (2)",
        da: 4277,
        a: 4326,
        accuratezza_m: 10.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: 371.0,
            ty: -112.0,
            tz: 434.0,
        },
        area: GeographicBounds::new(-6.5, 49.81, 1.84, 55.85),
    },
    // UK - England and Wales, Isle of Man
    Trasformazione {
        codice: 1197,
        nome: "OSGB36 to WGS 84 (3)",
        da: 4277,
        a: 4326,
        accuratezza_m: 21.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: 371.0,
            ty: -111.0,
            tz: 434.0,
        },
        area: GeographicBounds::new(-6.5, 49.81, 1.84, 55.85),
    },
    // UK - Scotland
    Trasformazione {
        codice: 1198,
        nome: "OSGB36 to WGS 84 (4)",
        da: 4277,
        a: 4326,
        accuratezza_m: 18.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: 384.0,
            ty: -111.0,
            tz: 425.0,
        },
        area: GeographicBounds::new(-8.74, 54.57, -0.65, 60.9),
    },
    // UK - Wales
    Trasformazione {
        codice: 1199,
        nome: "OSGB36 to WGS 84 (5)",
        da: 4277,
        a: 4326,
        accuratezza_m: 35.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: 370.0,
            ty: -108.0,
            tz: 434.0,
        },
        area: GeographicBounds::new(-5.34, 51.28, -2.65, 53.48),
    },
    // Tunisia
    Trasformazione {
        codice: 1245,
        nome: "ED50 to WGS 84 (16)",
        da: 4230,
        a: 4326,
        accuratezza_m: 44.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -112.0,
            ty: -77.0,
            tz: -145.0,
        },
        area: GeographicBounds::new(7.49, 30.23, 13.67, 38.41),
    },
    // USA - Alaska - Aleutian Islands east of 180°E
    Trasformazione {
        codice: 1249,
        nome: "NAD27 to WGS 84 (21)",
        da: 4267,
        a: 4326,
        accuratezza_m: 15.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -2.0,
            ty: 152.0,
            tz: 149.0,
        },
        area: GeographicBounds::new(-178.3, 51.54, -164.84, 54.34),
    },
    // USA - Alaska - Aleutian Islands west of 180°W
    Trasformazione {
        codice: 1250,
        nome: "NAD27 to WGS 84 (22)",
        da: 4267,
        a: 4326,
        accuratezza_m: 18.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: 2.0,
            ty: 204.0,
            tz: 105.0,
        },
        area: GeographicBounds::new(172.42, 51.3, 179.86, 53.07),
    },
    // USA - Alaska - Aleutian Islands
    Trasformazione {
        codice: 1251,
        nome: "NAD83 to WGS 84 (2)",
        da: 4269,
        a: 4326,
        accuratezza_m: 8.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -2.0,
            ty: 0.0,
            tz: 4.0,
        },
        area: GeographicBounds::new(172.42, 51.3, -164.84, 54.34),
    },
    // USA - Hawaii - main islands
    Trasformazione {
        codice: 1252,
        nome: "NAD83 to WGS 84 (3)",
        da: 4269,
        a: 4326,
        accuratezza_m: 4.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: 1.0,
            ty: 1.0,
            tz: -1.0,
        },
        area: GeographicBounds::new(-163.74, 15.56, -151.27, 25.58),
    },
    // France
    Trasformazione {
        codice: 1275,
        nome: "ED50 to WGS 84 (17)",
        da: 4230,
        a: 4326,
        accuratezza_m: 2.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -84.0,
            ty: -97.0,
            tz: -117.0,
        },
        area: GeographicBounds::new(-9.86, 41.15, 10.38, 51.56),
    },
    // Europe - common offshore
    Trasformazione {
        codice: 1311,
        nome: "ED50 to WGS 84 (18)",
        da: 4230,
        a: 4326,
        accuratezza_m: 1.0,
        metodo: MetodoTrasformazione::VettorePosizione {
            tx: -89.5,
            ty: -93.8,
            tz: -123.1,
            rx: 0.0,
            ry: 0.0,
            rz: -0.15599999999999983,
            ds: 1.2,
        },
        area: GeographicBounds::new(-16.1, 47.42, 10.86, 63.89),
    },
    // Canada - NAD27
    Trasformazione {
        codice: 1313,
        nome: "NAD27 to NAD83 (4)",
        da: 4267,
        a: 4269,
        accuratezza_m: 1.5,
        metodo: MetodoTrasformazione::GrigliaNtv2 {
            file_registro: "NTv2_0.gsb",
        },
        area: GeographicBounds::new(-141.01, 40.0, -44.0, 83.17),
    },
    // UK - Great Britain onshore and nearshore; Isle of Man
    Trasformazione {
        codice: 1314,
        nome: "OSGB36 to WGS 84 (6)",
        da: 4277,
        a: 4326,
        accuratezza_m: 2.0,
        metodo: MetodoTrasformazione::VettorePosizione {
            tx: 446.448,
            ty: -125.157,
            tz: 542.06,
            rx: 0.14999999999999983,
            ry: 0.24699999999999972,
            rz: 0.8419999999999991,
            ds: -20.489,
        },
        area: GeographicBounds::new(-8.82, 49.79, 1.92, 60.94),
    },
    // UK - Great Britain onshore and nearshore; Isle of Man
    Trasformazione {
        codice: 1315,
        nome: "OSGB36 to ED50 (1)",
        da: 4277,
        a: 4230,
        accuratezza_m: 2.0,
        metodo: MetodoTrasformazione::VettorePosizione {
            tx: 535.948,
            ty: -31.357,
            tz: 665.16,
            rx: 0.14999999999999983,
            ry: 0.24699999999999972,
            rz: 0.9979999999999989,
            ds: -21.689,
        },
        area: GeographicBounds::new(-8.82, 49.79, 1.92, 60.94),
    },
    // Greece - onshore
    Trasformazione {
        codice: 1440,
        nome: "ED50 to WGS 84 (19)",
        da: 4230,
        a: 4326,
        accuratezza_m: 999.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -86.0,
            ty: -92.2,
            tz: -127.5,
        },
        area: GeographicBounds::new(19.57, 34.88, 28.3, 41.75),
    },
    // Cuba
    Trasformazione {
        codice: 1530,
        nome: "NAD27 to WGS 84 (30)",
        da: 4267,
        a: 4326,
        accuratezza_m: 3.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -4.2,
            ty: 135.4,
            tz: 181.9,
        },
        area: GeographicBounds::new(-87.01, 18.83, -73.57, 25.51),
    },
    // New Zealand
    Trasformazione {
        codice: 1565,
        nome: "NZGD2000 to WGS 84 (1)",
        da: 4167,
        a: 4326,
        accuratezza_m: 1.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: 0.0,
            ty: 0.0,
            tz: 0.0,
        },
        area: GeographicBounds::new(160.6, -55.95, -171.2, -25.88),
    },
    // Canada - Quebec
    Trasformazione {
        codice: 1573,
        nome: "NAD27 to NAD83 (6)",
        da: 4267,
        a: 4269,
        accuratezza_m: 1.5,
        metodo: MetodoTrasformazione::GrigliaNtv2 {
            file_registro: "NA27NA83.GSB",
        },
        area: GeographicBounds::new(-79.85, 44.99, -57.1, 62.62),
    },
    // Norway - offshore north of 65°N; Svalbard
    Trasformazione {
        codice: 1588,
        nome: "ED50 to ETRS89 (1)",
        da: 4230,
        a: 4258,
        accuratezza_m: 1.0,
        metodo: MetodoTrasformazione::VettorePosizione {
            tx: -116.641,
            ty: -56.931,
            tz: -110.559,
            rx: 0.8925078166311858,
            ry: 0.9207660950870381,
            rz: -0.9166407989620963,
            ds: -3.5199999999999996,
        },
        area: GeographicBounds::new(-3.35, 65.0, 38.01, 84.73),
    },
    // France
    Trasformazione {
        codice: 1591,
        nome: "RGF93 v1 to ETRS89 (1)",
        da: 4171,
        a: 4258,
        accuratezza_m: 0.1,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: 0.0,
            ty: 0.0,
            tz: 0.0,
        },
        area: GeographicBounds::new(-9.86, 41.15, 10.38, 51.56),
    },
    // Norway - offshore north of 62°N; Svalbard
    Trasformazione {
        codice: 1612,
        nome: "ED50 to WGS 84 (23)",
        da: 4230,
        a: 4326,
        accuratezza_m: 1.0,
        metodo: MetodoTrasformazione::VettorePosizione {
            tx: -116.641,
            ty: -56.931,
            tz: -110.559,
            rx: 0.8929999999999991,
            ry: 0.920999999999999,
            rz: -0.916999999999999,
            ds: -3.5199999999999996,
        },
        area: GeographicBounds::new(-3.35, 62.0, 38.01, 84.73),
    },
    // Norway - North Sea - offshore south of 62°N
    Trasformazione {
        codice: 1613,
        nome: "ED50 to WGS 84 (24)",
        da: 4230,
        a: 4326,
        accuratezza_m: 1.0,
        metodo: MetodoTrasformazione::VettorePosizione {
            tx: -90.365,
            ty: -101.13,
            tz: -123.384,
            rx: 0.33299999999999963,
            ry: 0.07699999999999992,
            rz: 0.8939999999999991,
            ds: 1.9939999999999998,
        },
        area: GeographicBounds::new(1.37, 56.08, 10.81, 62.01),
    },
    // Denmark - onshore
    Trasformazione {
        codice: 1626,
        nome: "ED50 to ETRS89 (4)",
        da: 4230,
        a: 4258,
        accuratezza_m: 1.0,
        metodo: MetodoTrasformazione::VettorePosizione {
            tx: -81.1,
            ty: -89.4,
            tz: -115.8,
            rx: 0.4849999999999995,
            ry: 0.023999999999999976,
            rz: 0.41299999999999953,
            ds: -0.54,
        },
        area: GeographicBounds::new(7.98, 54.5, 15.28, 57.81),
    },
    // Denmark - onshore
    Trasformazione {
        codice: 1627,
        nome: "ED50 to WGS 84 (25)",
        da: 4230,
        a: 4326,
        accuratezza_m: 1.0,
        metodo: MetodoTrasformazione::VettorePosizione {
            tx: -81.1,
            ty: -89.4,
            tz: -115.8,
            rx: 0.4849999999999995,
            ry: 0.023999999999999976,
            rz: 0.41299999999999953,
            ds: -0.54,
        },
        area: GeographicBounds::new(7.98, 54.5, 15.28, 57.81),
    },
    // Gibraltar
    Trasformazione {
        codice: 1628,
        nome: "ED50 to ETRS89 (5)",
        da: 4230,
        a: 4258,
        accuratezza_m: 1.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -116.8,
            ty: -106.4,
            tz: -154.4,
        },
        area: GeographicBounds::new(-5.42, 36.0, -4.89, 36.16),
    },
    // Gibraltar
    Trasformazione {
        codice: 1629,
        nome: "ED50 to WGS 84 (26)",
        da: 4230,
        a: 4326,
        accuratezza_m: 1.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -116.8,
            ty: -106.4,
            tz: -154.4,
        },
        area: GeographicBounds::new(-5.42, 36.0, -4.89, 36.16),
    },
    // Spain - Balearic Islands
    Trasformazione {
        codice: 1630,
        nome: "ED50 to ETRS89 (6)",
        da: 4230,
        a: 4258,
        accuratezza_m: 1.5,
        metodo: MetodoTrasformazione::VettorePosizione {
            tx: -181.5,
            ty: -90.3,
            tz: -187.2,
            rx: 0.14399999999999982,
            ry: 0.4919999999999995,
            rz: -0.3939999999999996,
            ds: 17.57,
        },
        area: GeographicBounds::new(1.12, 38.59, 4.39, 40.15),
    },
    // Spain - Balearic Islands
    Trasformazione {
        codice: 1631,
        nome: "ED50 to WGS 84 (27)",
        da: 4230,
        a: 4326,
        accuratezza_m: 1.5,
        metodo: MetodoTrasformazione::VettorePosizione {
            tx: -181.5,
            ty: -90.3,
            tz: -187.2,
            rx: 0.14399999999999982,
            ry: 0.4919999999999995,
            rz: -0.3939999999999996,
            ds: 17.57,
        },
        area: GeographicBounds::new(1.12, 38.59, 4.39, 40.15),
    },
    // Spain - mainland except northwest
    Trasformazione {
        codice: 1632,
        nome: "ED50 to ETRS89 (7)",
        da: 4230,
        a: 4258,
        accuratezza_m: 1.5,
        metodo: MetodoTrasformazione::VettorePosizione {
            tx: -131.0,
            ty: -100.3,
            tz: -163.4,
            rx: -1.2439999999999987,
            ry: -0.01999999999999998,
            rz: -1.1439999999999988,
            ds: 9.39,
        },
        area: GeographicBounds::new(-7.54, 35.26, 3.39, 43.56),
    },
    // Spain - mainland except northwest
    Trasformazione {
        codice: 1633,
        nome: "ED50 to WGS 84 (28)",
        da: 4230,
        a: 4326,
        accuratezza_m: 1.5,
        metodo: MetodoTrasformazione::VettorePosizione {
            tx: -131.0,
            ty: -100.3,
            tz: -163.4,
            rx: -1.2439999999999987,
            ry: -0.01999999999999998,
            rz: -1.1439999999999988,
            ds: 9.39,
        },
        area: GeographicBounds::new(-7.54, 35.26, 3.39, 43.56),
    },
    // Spain - mainland northwest
    Trasformazione {
        codice: 1634,
        nome: "ED50 to ETRS89 (8)",
        da: 4230,
        a: 4258,
        accuratezza_m: 1.5,
        metodo: MetodoTrasformazione::VettorePosizione {
            tx: -178.4,
            ty: -83.2,
            tz: -221.3,
            rx: 0.5399999999999995,
            ry: -0.5319999999999995,
            rz: -0.12599999999999986,
            ds: 21.199999999999996,
        },
        area: GeographicBounds::new(-9.37, 41.5, -4.5, 43.82),
    },
    // Spain - mainland northwest
    Trasformazione {
        codice: 1635,
        nome: "ED50 to WGS 84 (29)",
        da: 4230,
        a: 4326,
        accuratezza_m: 1.5,
        metodo: MetodoTrasformazione::VettorePosizione {
            tx: -178.4,
            ty: -83.2,
            tz: -221.3,
            rx: 0.5399999999999995,
            ry: -0.5319999999999995,
            rz: -0.12599999999999986,
            ds: 21.199999999999996,
        },
        area: GeographicBounds::new(-9.37, 41.5, -4.5, 43.82),
    },
    // Europe - Liechtenstein and Switzerland
    Trasformazione {
        codice: 1647,
        nome: "CH1903+ to ETRS89 (1)",
        da: 4150,
        a: 4258,
        accuratezza_m: 0.1,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: 674.374,
            ty: 15.056,
            tz: 405.346,
        },
        area: GeographicBounds::new(5.96, 45.82, 10.49, 47.81),
    },
    // France
    Trasformazione {
        codice: 1650,
        nome: "ED50 to ETRS89 (10)",
        da: 4230,
        a: 4258,
        accuratezza_m: 2.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -84.0,
            ty: -97.0,
            tz: -117.0,
        },
        area: GeographicBounds::new(-9.86, 41.15, 10.38, 51.56),
    },
    // Italy - mainland
    Trasformazione {
        codice: 1659,
        nome: "Monte Mario to ETRS89 (1)",
        da: 4265,
        a: 4258,
        accuratezza_m: 4.0,
        metodo: MetodoTrasformazione::VettorePosizione {
            tx: -104.1,
            ty: -49.1,
            tz: -9.9,
            rx: 0.9709999999999989,
            ry: -2.9169999999999967,
            rz: 0.7139999999999992,
            ds: -11.68,
        },
        area: GeographicBounds::new(6.62, 37.86, 18.58, 47.1),
    },
    // Italy - mainland
    Trasformazione {
        codice: 1660,
        nome: "Monte Mario to WGS 84 (4)",
        da: 4265,
        a: 4326,
        accuratezza_m: 4.0,
        metodo: MetodoTrasformazione::VettorePosizione {
            tx: -104.1,
            ty: -49.1,
            tz: -9.9,
            rx: 0.9709999999999989,
            ry: -2.9169999999999967,
            rz: 0.7139999999999992,
            ds: -11.68,
        },
        area: GeographicBounds::new(6.62, 37.86, 18.58, 47.1),
    },
    // Italy - Sardinia onshore
    Trasformazione {
        codice: 1661,
        nome: "Monte Mario to ETRS89 (2)",
        da: 4265,
        a: 4258,
        accuratezza_m: 4.0,
        metodo: MetodoTrasformazione::VettorePosizione {
            tx: -168.6,
            ty: -34.0,
            tz: 38.6,
            rx: -0.3739999999999996,
            ry: -0.6789999999999993,
            rz: -1.3789999999999987,
            ds: -9.48,
        },
        area: GeographicBounds::new(8.08, 38.82, 9.89, 41.31),
    },
    // Italy - Sardinia onshore
    Trasformazione {
        codice: 1662,
        nome: "Monte Mario to WGS 84 (2)",
        da: 4265,
        a: 4326,
        accuratezza_m: 4.0,
        metodo: MetodoTrasformazione::VettorePosizione {
            tx: -168.6,
            ty: -34.0,
            tz: 38.6,
            rx: -0.3739999999999996,
            ry: -0.6789999999999993,
            rz: -1.3789999999999987,
            ds: -9.48,
        },
        area: GeographicBounds::new(8.08, 38.82, 9.89, 41.31),
    },
    // Italy - Sicily onshore
    Trasformazione {
        codice: 1663,
        nome: "Monte Mario to ETRS89 (3)",
        da: 4265,
        a: 4258,
        accuratezza_m: 4.0,
        metodo: MetodoTrasformazione::VettorePosizione {
            tx: -50.2,
            ty: -50.4,
            tz: 84.8,
            rx: -0.6899999999999992,
            ry: -2.011999999999998,
            rz: 0.4589999999999995,
            ds: -28.079999999999995,
        },
        area: GeographicBounds::new(12.36, 36.59, 15.71, 38.35),
    },
    // Italy - Sicily onshore
    Trasformazione {
        codice: 1664,
        nome: "Monte Mario to WGS 84 (3)",
        da: 4265,
        a: 4326,
        accuratezza_m: 4.0,
        metodo: MetodoTrasformazione::VettorePosizione {
            tx: -50.2,
            ty: -50.4,
            tz: 84.8,
            rx: -0.6899999999999992,
            ry: -2.011999999999998,
            rz: 0.4589999999999995,
            ds: -28.079999999999995,
        },
        area: GeographicBounds::new(12.36, 36.59, 15.71, 38.35),
    },
    // France
    Trasformazione {
        codice: 1671,
        nome: "RGF93 v1 to WGS 84 (1)",
        da: 4171,
        a: 4326,
        accuratezza_m: 1.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: 0.0,
            ty: 0.0,
            tz: 0.0,
        },
        area: GeographicBounds::new(-9.86, 41.15, 10.38, 51.56),
    },
    // Europe - Liechtenstein and Switzerland
    Trasformazione {
        codice: 1676,
        nome: "CH1903+ to WGS 84 (1)",
        da: 4150,
        a: 4326,
        accuratezza_m: 1.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: 674.374,
            ty: 15.056,
            tz: 405.346,
        },
        area: GeographicBounds::new(5.96, 45.82, 10.49, 47.81),
    },
    // Canada - Quebec
    Trasformazione {
        codice: 1692,
        nome: "NAD27 to WGS 84 (34)",
        da: 4267,
        a: 4326,
        accuratezza_m: 1.5,
        metodo: MetodoTrasformazione::GrigliaNtv2 {
            file_registro: "NA27SCRS.GSB",
        },
        area: GeographicBounds::new(-79.85, 44.99, -57.1, 62.62),
    },
    // Canada - NAD27
    Trasformazione {
        codice: 1693,
        nome: "NAD27 to WGS 84 (33)",
        da: 4267,
        a: 4326,
        accuratezza_m: 2.0,
        metodo: MetodoTrasformazione::GrigliaNtv2 {
            file_registro: "NTv2_0.gsb",
        },
        area: GeographicBounds::new(-141.01, 40.0, -44.0, 83.17),
    },
    // Canada - Quebec
    Trasformazione {
        codice: 1696,
        nome: "NAD83 to WGS 84 (6)",
        da: 4269,
        a: 4326,
        accuratezza_m: 1.5,
        metodo: MetodoTrasformazione::GrigliaNtv2 {
            file_registro: "NA83SCRS.GSB",
        },
        area: GeographicBounds::new(-79.85, 44.99, -57.1, 62.62),
    },
    // Canada - Saskatchewan
    Trasformazione {
        codice: 1697,
        nome: "NAD83 to WGS 84 (7)",
        da: 4269,
        a: 4326,
        accuratezza_m: 1.5,
        metodo: MetodoTrasformazione::GrigliaNtv2 {
            file_registro: "SK83-98.gsb",
        },
        area: GeographicBounds::new(-110.0, 49.0, -101.34, 60.01),
    },
    // Canada - Alberta
    Trasformazione {
        codice: 1702,
        nome: "NAD83 to WGS 84 (8)",
        da: 4269,
        a: 4326,
        accuratezza_m: 1.5,
        metodo: MetodoTrasformazione::GrigliaNtv2 {
            file_registro: "AB_CSRS.DAC",
        },
        area: GeographicBounds::new(-120.0, 48.99, -109.98, 60.0),
    },
    // Canada - Saskatchewan
    Trasformazione {
        codice: 1703,
        nome: "NAD27 to WGS 84 (32)",
        da: 4267,
        a: 4326,
        accuratezza_m: 1.5,
        metodo: MetodoTrasformazione::GrigliaNtv2 {
            file_registro: "SK27-98.gsb",
        },
        area: GeographicBounds::new(-110.0, 49.0, -101.34, 60.01),
    },
    // Germany - West Germany all states
    Trasformazione {
        codice: 1776,
        nome: "DHDN to ETRS89 (2)",
        da: 4314,
        a: 4258,
        accuratezza_m: 3.0,
        metodo: MetodoTrasformazione::VettorePosizione {
            tx: 598.1,
            ty: 73.7,
            tz: 418.2,
            rx: 0.2019999999999998,
            ry: 0.04499999999999995,
            rz: -2.4549999999999974,
            ds: 6.7,
        },
        area: GeographicBounds::new(5.86, 47.27, 13.84, 55.09),
    },
    // Germany - West Germany all states
    Trasformazione {
        codice: 1777,
        nome: "DHDN to WGS 84 (2)",
        da: 4314,
        a: 4326,
        accuratezza_m: 3.0,
        metodo: MetodoTrasformazione::VettorePosizione {
            tx: 598.1,
            ty: 73.7,
            tz: 418.2,
            rx: 0.2019999999999998,
            ry: 0.04499999999999995,
            rz: -2.4549999999999974,
            ds: 6.7,
        },
        area: GeographicBounds::new(5.86, 47.27, 13.84, 55.09),
    },
    // Germany - West Germany S
    Trasformazione {
        codice: 1778,
        nome: "DHDN to ETRS89 (3)",
        da: 4314,
        a: 4258,
        accuratezza_m: 1.0,
        metodo: MetodoTrasformazione::VettorePosizione {
            tx: 597.1,
            ty: 71.4,
            tz: 412.1,
            rx: 0.8939999999999991,
            ry: 0.06799999999999992,
            rz: -1.5629999999999984,
            ds: 7.579999999999999,
        },
        area: GeographicBounds::new(6.11, 47.27, 13.84, 50.34),
    },
    // Germany - West Germany C
    Trasformazione {
        codice: 1779,
        nome: "DHDN to ETRS89 (4)",
        da: 4314,
        a: 4258,
        accuratezza_m: 1.0,
        metodo: MetodoTrasformazione::VettorePosizione {
            tx: 584.8,
            ty: 67.0,
            tz: 400.3,
            rx: 0.1049999999999999,
            ry: 0.012999999999999986,
            rz: -2.377999999999998,
            ds: 10.289999999999997,
        },
        area: GeographicBounds::new(5.86, 50.33, 12.03, 52.34),
    },
    // Germany - West Germany N
    Trasformazione {
        codice: 1780,
        nome: "DHDN to ETRS89 (5)",
        da: 4314,
        a: 4258,
        accuratezza_m: 1.0,
        metodo: MetodoTrasformazione::VettorePosizione {
            tx: 590.5,
            ty: 69.5,
            tz: 411.6,
            rx: -0.7959999999999993,
            ry: -0.05199999999999994,
            rz: -3.6009999999999964,
            ds: 8.3,
        },
        area: GeographicBounds::new(6.56, 52.33, 11.59, 55.09),
    },
    // Turkey
    Trasformazione {
        codice: 1783,
        nome: "ED50 to ETRS89 (9)",
        da: 4230,
        a: 4258,
        accuratezza_m: 2.0,
        metodo: MetodoTrasformazione::VettorePosizione {
            tx: -84.1,
            ty: -101.8,
            tz: -129.7,
            rx: 0.0,
            ry: 0.0,
            rz: 0.4679999999999996,
            ds: 1.0499999999999998,
        },
        area: GeographicBounds::new(25.62, 34.42, 44.83, 43.45),
    },
    // Turkey
    Trasformazione {
        codice: 1784,
        nome: "ED50 to WGS 84 (30)",
        da: 4230,
        a: 4326,
        accuratezza_m: 2.0,
        metodo: MetodoTrasformazione::VettorePosizione {
            tx: -84.1,
            ty: -101.8,
            tz: -129.7,
            rx: 0.0,
            ry: 0.0,
            rz: 0.4679999999999996,
            ds: 1.0499999999999998,
        },
        area: GeographicBounds::new(25.62, 34.42, 44.83, 43.45),
    },
    // Egypt - Western Desert
    Trasformazione {
        codice: 1810,
        nome: "ED50 to WGS 84 (31)",
        da: 4230,
        a: 4326,
        accuratezza_m: 15.0,
        metodo: MetodoTrasformazione::VettorePosizione {
            tx: -84.0,
            ty: -103.0,
            tz: -122.5,
            rx: 0.0,
            ry: 0.0,
            rz: 0.5539999999999995,
            ds: 0.22629999999999997,
        },
        area: GeographicBounds::new(24.7, 25.71, 30.0, 31.68),
    },
    // Ireland - Corrib and Errigal
    Trasformazione {
        codice: 1853,
        nome: "ED50 to WGS 84 (39)",
        da: 4230,
        a: 4326,
        accuratezza_m: 5.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -82.31,
            ty: -95.23,
            tz: -114.96,
        },
        area: GeographicBounds::new(-12.5, 53.75, -9.49, 55.76),
    },
    // Portugal - mainland - onshore
    Trasformazione {
        codice: 1985,
        nome: "ED50 to WGS 84 (33)",
        da: 4230,
        a: 4326,
        accuratezza_m: 5.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -87.987,
            ty: -108.639,
            tz: -121.593,
        },
        area: GeographicBounds::new(-9.56, 36.95, -6.19, 42.16),
    },
    // Portugal - mainland - onshore
    Trasformazione {
        codice: 1989,
        nome: "ED50 to WGS 84 (34)",
        da: 4230,
        a: 4326,
        accuratezza_m: 1.0,
        metodo: MetodoTrasformazione::TelaioCoordinate {
            tx: -74.292,
            ty: -135.889,
            tz: -104.967,
            rx: 0.5239999999999995,
            ry: 0.13599999999999984,
            rz: -0.6099999999999993,
            ds: -3.761,
        },
        area: GeographicBounds::new(-9.56, 36.95, -6.19, 42.16),
    },
    // Germany - offshore North Sea
    Trasformazione {
        codice: 1998,
        nome: "ED50 to WGS 84 (36)",
        da: 4230,
        a: 4326,
        accuratezza_m: 1.0,
        metodo: MetodoTrasformazione::VettorePosizione {
            tx: -157.89,
            ty: -17.16,
            tz: -78.41,
            rx: 2.1179999999999977,
            ry: 2.696999999999997,
            rz: -1.4339999999999984,
            ds: -5.379999999999999,
        },
        area: GeographicBounds::new(3.34, 53.58, 8.88, 55.92),
    },
    // Netherlands - offshore
    Trasformazione {
        codice: 3904,
        nome: "ED50 to WGS 84 (32)",
        da: 4230,
        a: 4326,
        accuratezza_m: 5.0,
        metodo: MetodoTrasformazione::VettorePosizione {
            tx: -83.11,
            ty: -97.38,
            tz: -117.22,
            rx: 0.0056929086524198595,
            ry: -0.04469758351374578,
            rz: 0.044285053901251585,
            ds: 0.12179999999999999,
        },
        area: GeographicBounds::new(2.53, 51.44, 6.37, 55.77),
    },
    // Netherlands - onshore
    Trasformazione {
        codice: 4833,
        nome: "Amersfoort to WGS 84 (4)",
        da: 4289,
        a: 4326,
        accuratezza_m: 1.0,
        metodo: MetodoTrasformazione::TelaioCoordinate {
            tx: 565.4171,
            ty: 50.3319,
            tz: 465.5524,
            rx: 0.3989573882431337,
            ry: -0.3439878173782826,
            rz: 1.8774016399804463,
            ds: 4.0725,
        },
        area: GeographicBounds::new(3.2, 50.75, 7.22, 53.7),
    },
    // Portugal - mainland - onshore
    Trasformazione {
        codice: 5040,
        nome: "ED50 to ETRS89 (13)",
        da: 4230,
        a: 4258,
        accuratezza_m: 5.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -87.987,
            ty: -108.639,
            tz: -121.593,
        },
        area: GeographicBounds::new(-9.56, 36.95, -6.19, 42.16),
    },
    // UK - Wytch Farm
    Trasformazione {
        codice: 5622,
        nome: "OSGB36 to WGS 84 (8)",
        da: 4277,
        a: 4326,
        accuratezza_m: 3.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: 370.936,
            ty: -108.938,
            tz: 435.682,
        },
        area: GeographicBounds::new(-2.2, 50.53, -1.68, 50.8),
    },
    // Spain - Catalonia onshore
    Trasformazione {
        codice: 5661,
        nome: "ED50 to ETRS89 (14)",
        da: 4230,
        a: 4258,
        accuratezza_m: 0.05,
        metodo: MetodoTrasformazione::GrigliaNtv2 {
            file_registro: "100800401.gsb",
        },
        area: GeographicBounds::new(0.16, 40.49, 3.39, 42.86),
    },
    // Italy - including San Marino and Vatican
    Trasformazione {
        codice: 6710,
        nome: "RDN2008 to ETRS89 (1)",
        da: 6706,
        a: 4258,
        accuratezza_m: 0.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: 0.0,
            ty: 0.0,
            tz: 0.0,
        },
        area: GeographicBounds::new(5.93, 34.76, 18.99, 47.1),
    },
    // Italy
    Trasformazione {
        codice: 6711,
        nome: "RDN2008 to WGS 84 (1)",
        da: 6706,
        a: 4326,
        accuratezza_m: 1.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: 0.0,
            ty: 0.0,
            tz: 0.0,
        },
        area: GeographicBounds::new(5.93, 34.76, 18.99, 47.1),
    },
    // Panama - onshore
    Trasformazione {
        codice: 7698,
        nome: "NAD27 to WGS 84 (89)",
        da: 4267,
        a: 4326,
        accuratezza_m: 1.0,
        metodo: MetodoTrasformazione::TelaioCoordinate {
            tx: -32.3841359,
            ty: 180.4090461,
            tz: 120.8442577,
            rx: 2.154585399999998,
            ry: 0.14987819999999982,
            rz: -0.5742914999999994,
            ds: 8.104916399999999,
        },
        area: GeographicBounds::new(-83.04, 7.15, -77.19, 9.68),
    },
    // UK - Britain and UKCS 49°45'N to 61°N, 9°W to 2°E
    Trasformazione {
        codice: 7709,
        nome: "OSGB36 to ETRS89 (2)",
        da: 4277,
        a: 4258,
        accuratezza_m: 0.03,
        metodo: MetodoTrasformazione::GrigliaNtv2 {
            file_registro: "OSTN15_NTv2_OSGBtoETRS.gsb",
        },
        area: GeographicBounds::new(-9.01, 49.75, 2.01, 61.01),
    },
    // UK - Britain and UKCS 49°45'N to 61°N, 9°W to 2°E
    Trasformazione {
        codice: 7710,
        nome: "OSGB36 to WGS 84 (9)",
        da: 4277,
        a: 4326,
        accuratezza_m: 1.0,
        metodo: MetodoTrasformazione::GrigliaNtv2 {
            file_registro: "OSTN15_NTv2_OSGBtoETRS.gsb",
        },
        area: GeographicBounds::new(-9.01, 49.75, 2.01, 61.01),
    },
    // Australia - GDA
    Trasformazione {
        codice: 8048,
        nome: "GDA94 to GDA2020 (1)",
        da: 4283,
        a: 7844,
        accuratezza_m: 0.01,
        metodo: MetodoTrasformazione::TelaioCoordinate {
            tx: 0.06155,
            ty: -0.01087,
            tz: -0.04019,
            rx: -0.03949239999999997,
            ry: -0.03272209999999997,
            rz: -0.032897899999999966,
            ds: -0.009994,
        },
        area: GeographicBounds::new(93.41, -60.55, 173.34, -8.47),
    },
    // Christmas Island - onshore
    Trasformazione {
        codice: 8444,
        nome: "GDA94 to GDA2020 (4)",
        da: 4283,
        a: 7844,
        accuratezza_m: 0.05,
        metodo: MetodoTrasformazione::GrigliaNtv2 {
            file_registro: "GDA94_GDA2020_conformal_christmas_island.gsb",
        },
        area: GeographicBounds::new(105.48, -10.63, 105.77, -10.36),
    },
    // Cocos (Keeling) Islands - onshore
    Trasformazione {
        codice: 8445,
        nome: "GDA94 to GDA2020 (5)",
        da: 4283,
        a: 7844,
        accuratezza_m: 0.05,
        metodo: MetodoTrasformazione::GrigliaNtv2 {
            file_registro: "GDA94_GDA2020_conformal_cocos_island.gsb",
        },
        area: GeographicBounds::new(96.76, -12.27, 96.99, -11.76),
    },
    // Australia - onshore
    Trasformazione {
        codice: 8446,
        nome: "GDA94 to GDA2020 (3)",
        da: 4283,
        a: 7844,
        accuratezza_m: 0.05,
        metodo: MetodoTrasformazione::GrigliaNtv2 {
            file_registro: "GDA94_GDA2020_conformal.gsb",
        },
        area: GeographicBounds::new(112.85, -43.7, 153.69, -9.86),
    },
    // Australia - onshore
    Trasformazione {
        codice: 8447,
        nome: "GDA94 to GDA2020 (2)",
        da: 4283,
        a: 7844,
        accuratezza_m: 0.05,
        metodo: MetodoTrasformazione::GrigliaNtv2 {
            file_registro: "GDA94_GDA2020_conformal_and_distortion.gsb",
        },
        area: GeographicBounds::new(112.85, -43.7, 153.69, -9.86),
    },
    // Australia - GDA
    Trasformazione {
        codice: 8450,
        nome: "GDA2020 to WGS 84 (2)",
        da: 7844,
        a: 4326,
        accuratezza_m: 3.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: 0.0,
            ty: 0.0,
            tz: 0.0,
        },
        area: GeographicBounds::new(93.41, -60.55, 173.34, -8.47),
    },
    // Canada - Saskatchewan
    Trasformazione {
        codice: 9111,
        nome: "NAD27 to NAD83 (9)",
        da: 4267,
        a: 4269,
        accuratezza_m: 1.5,
        metodo: MetodoTrasformazione::GrigliaNtv2 {
            file_registro: "SK27-83.gsb",
        },
        area: GeographicBounds::new(-110.0, 49.0, -101.34, 60.01),
    },
    // Europe - offshore North Sea - Germany and Netherlands east of 5°E
    Trasformazione {
        codice: 9224,
        nome: "ED50 to ETRS89 (15)",
        da: 4230,
        a: 4258,
        accuratezza_m: 1.0,
        metodo: MetodoTrasformazione::VettorePosizione {
            tx: -157.89,
            ty: -17.16,
            tz: -78.41,
            rx: 2.1179999999999977,
            ry: 2.696999999999997,
            rz: -1.4339999999999984,
            ds: -5.379999999999999,
        },
        area: GeographicBounds::new(3.34, 53.49, 8.88, 55.92),
    },
    // Netherlands - onshore
    Trasformazione {
        codice: 9281,
        nome: "Amersfoort to ETRS89 (8)",
        da: 4289,
        a: 4258,
        accuratezza_m: 0.25,
        metodo: MetodoTrasformazione::TelaioCoordinate {
            tx: 565.7381,
            ty: 50.4018,
            tz: 465.2904,
            rx: 0.3950259810360641,
            ry: -0.33077243124203115,
            rz: 1.8760732946282148,
            ds: 4.07244,
        },
        area: GeographicBounds::new(3.2, 50.75, 7.22, 53.7),
    },
    // Netherlands - onshore
    Trasformazione {
        codice: 9282,
        nome: "Amersfoort to ETRS89 (9)",
        da: 4289,
        a: 4258,
        accuratezza_m: 0.0,
        metodo: MetodoTrasformazione::GrigliaNtv2 {
            file_registro: "rdtrans2018.gsb",
        },
        area: GeographicBounds::new(3.2, 50.75, 7.22, 53.7),
    },
    // Germany - Saarland
    Trasformazione {
        codice: 9310,
        nome: "DHDN to ETRS89 (10)",
        da: 4314,
        a: 4258,
        accuratezza_m: 0.01,
        metodo: MetodoTrasformazione::GrigliaNtv2 {
            file_registro: "SeTa2016.gsb",
        },
        area: GeographicBounds::new(6.35, 49.11, 7.41, 49.64),
    },
    // Germany - Baden-Wurttemberg
    Trasformazione {
        codice: 9338,
        nome: "DHDN to ETRS89 (9)",
        da: 4314,
        a: 4258,
        accuratezza_m: 0.1,
        metodo: MetodoTrasformazione::GrigliaNtv2 {
            file_registro: "BWTA2017.gsb",
        },
        area: GeographicBounds::new(7.51, 47.54, 10.5, 49.8),
    },
    // Spain - mainland onshore and Ceuta
    Trasformazione {
        codice: 9408,
        nome: "ED50 to ETRS89 (16)",
        da: 4230,
        a: 4258,
        accuratezza_m: 0.2,
        metodo: MetodoTrasformazione::GrigliaNtv2 {
            file_registro: "PENR2009.gsb",
        },
        area: GeographicBounds::new(-9.37, 35.82, 3.39, 43.82),
    },
    // Spain - Balearic Islands
    Trasformazione {
        codice: 9409,
        nome: "ED50 to ETRS89 (17)",
        da: 4230,
        a: 4258,
        accuratezza_m: 0.2,
        metodo: MetodoTrasformazione::GrigliaNtv2 {
            file_registro: "BALR2009.gsb",
        },
        area: GeographicBounds::new(1.12, 38.59, 4.39, 40.15),
    },
    // Australia - GDA
    Trasformazione {
        codice: 9688,
        nome: "GDA94 to WGS 84 (2)",
        da: 4283,
        a: 4326,
        accuratezza_m: 3.0,
        metodo: MetodoTrasformazione::TelaioCoordinate {
            tx: 0.06155,
            ty: -0.01087,
            tz: -0.04019,
            rx: -0.03949239999999997,
            ry: -0.03272209999999997,
            rz: -0.032897899999999966,
            ds: -0.009994,
        },
        area: GeographicBounds::new(93.41, -60.55, 173.34, -8.47),
    },
    // Australia - onshore
    Trasformazione {
        codice: 9689,
        nome: "GDA94 to WGS 84 (3)",
        da: 4283,
        a: 4326,
        accuratezza_m: 3.0,
        metodo: MetodoTrasformazione::GrigliaNtv2 {
            file_registro: "GDA94_GDA2020_conformal_and_distortion.gsb",
        },
        area: GeographicBounds::new(112.85, -43.7, 153.69, -9.86),
    },
    // Australia - GDA
    Trasformazione {
        codice: 9690,
        nome: "WGS 84 to GDA2020 (3)",
        da: 4326,
        a: 7844,
        accuratezza_m: 3.0,
        metodo: MetodoTrasformazione::TelaioCoordinate {
            tx: 0.06155,
            ty: -0.01087,
            tz: -0.04019,
            rx: -0.03949239999999997,
            ry: -0.03272209999999997,
            rz: -0.032897899999999966,
            ds: -0.009994,
        },
        area: GeographicBounds::new(93.41, -60.55, 173.34, -8.47),
    },
    // Australia - onshore
    Trasformazione {
        codice: 9691,
        nome: "WGS 84 to GDA2020 (4)",
        da: 4326,
        a: 7844,
        accuratezza_m: 3.0,
        metodo: MetodoTrasformazione::GrigliaNtv2 {
            file_registro: "GDA94_GDA2020_conformal_and_distortion.gsb",
        },
        area: GeographicBounds::new(112.85, -43.7, 153.69, -9.86),
    },
    // Italy - 6°22'E to 18°40'E and north of 35°16'N; San Marino, Vatican City State
    Trasformazione {
        codice: 9732,
        nome: "Monte Mario to ED50 (1)",
        da: 4265,
        a: 4230,
        accuratezza_m: 0.1,
        metodo: MetodoTrasformazione::GrigliaNtv2 {
            file_registro: "35160622_47161840_R40_E50.gsb",
        },
        area: GeographicBounds::new(6.36, 35.26, 18.67, 47.1),
    },
    // Italy - 6°22'E to 18°40'E and north of 35°16'N; San Marino, Vatican City State
    Trasformazione {
        codice: 9733,
        nome: "Monte Mario to IGM95 (4)",
        da: 4265,
        a: 4670,
        accuratezza_m: 0.1,
        metodo: MetodoTrasformazione::GrigliaNtv2 {
            file_registro: "35160622_47161840_R40_F89.gsb",
        },
        area: GeographicBounds::new(6.36, 35.26, 18.67, 47.1),
    },
    // Italy - 6°22'E to 18°40'E and north of 35°16'N; San Marino, Vatican City State
    Trasformazione {
        codice: 9734,
        nome: "Monte Mario to RDN2008 (5)",
        da: 4265,
        a: 6706,
        accuratezza_m: 0.1,
        metodo: MetodoTrasformazione::GrigliaNtv2 {
            file_registro: "35160622_47161840_R40_F00.gsb",
        },
        area: GeographicBounds::new(6.36, 35.26, 18.67, 47.1),
    },
    // Italy - 6°22'E to 18°40'E and north of 35°16'N; San Marino, Vatican City State
    Trasformazione {
        codice: 9735,
        nome: "ED50 to IGM95 (1)",
        da: 4230,
        a: 4670,
        accuratezza_m: 0.2,
        metodo: MetodoTrasformazione::GrigliaNtv2 {
            file_registro: "35160622_47161840_E50_F89.gsb",
        },
        area: GeographicBounds::new(6.36, 35.26, 18.67, 47.1),
    },
    // Italy - 6°22'E to 18°40'E and north of 35°16'N; San Marino, Vatican City State
    Trasformazione {
        codice: 9736,
        nome: "ED50 to RDN2008 (1)",
        da: 4230,
        a: 6706,
        accuratezza_m: 0.2,
        metodo: MetodoTrasformazione::GrigliaNtv2 {
            file_registro: "35160622_47161840_E50_F00.gsb",
        },
        area: GeographicBounds::new(6.36, 35.26, 18.67, 47.1),
    },
    // Italy - 6°22'E to 18°40'E and north of 35°16'N; San Marino, Vatican City State
    Trasformazione {
        codice: 9737,
        nome: "IGM95 to RDN2008 (1)",
        da: 4670,
        a: 6706,
        accuratezza_m: 0.01,
        metodo: MetodoTrasformazione::GrigliaNtv2 {
            file_registro: "35160622_47161840_F89_F00.gsb",
        },
        area: GeographicBounds::new(6.36, 35.26, 18.67, 47.1),
    },
    // Germany - Hessen
    Trasformazione {
        codice: 9940,
        nome: "DHDN to ETRS89 (11)",
        da: 4314,
        a: 4258,
        accuratezza_m: 0.1,
        metodo: MetodoTrasformazione::GrigliaNtv2 {
            file_registro: "HeTa2010.gsb",
        },
        area: GeographicBounds::new(7.77, 49.39, 10.24, 51.66),
    },
    // Mexico - offshore GoM - Campeche area S
    Trasformazione {
        codice: 15699,
        nome: "NAD27 to WGS 84 (87)",
        da: 4267,
        a: 4326,
        accuratezza_m: 5.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -2.0,
            ty: 124.7,
            tz: 196.0,
        },
        area: GeographicBounds::new(-94.79, 17.85, -89.75, 20.89),
    },
    // USA - GoM - east of 87.25°W
    Trasformazione {
        codice: 15852,
        nome: "NAD27 to WGS 84 (80)",
        da: 4267,
        a: 4326,
        accuratezza_m: 5.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -3.0,
            ty: 154.0,
            tz: 177.0,
        },
        area: GeographicBounds::new(-87.25, 23.82, -81.17, 30.25),
    },
    // USA - GoM - 95°W to 87.25°W
    Trasformazione {
        codice: 15853,
        nome: "NAD27 to WGS 84 (81)",
        da: 4267,
        a: 4326,
        accuratezza_m: 5.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -7.0,
            ty: 151.0,
            tz: 175.0,
        },
        area: GeographicBounds::new(-95.0, 25.61, -87.25, 30.23),
    },
    // USA - GoM - west of 95°W
    Trasformazione {
        codice: 15854,
        nome: "NAD27 to WGS 84 (82)",
        da: 4267,
        a: 4326,
        accuratezza_m: 5.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -7.0,
            ty: 151.0,
            tz: 178.0,
        },
        area: GeographicBounds::new(-97.22, 25.97, -95.0, 28.97),
    },
    // Mexico - offshore GoM - Tampico area
    Trasformazione {
        codice: 15855,
        nome: "NAD27 to WGS 84 (83)",
        da: 4267,
        a: 4326,
        accuratezza_m: 5.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -8.0,
            ty: 125.0,
            tz: 190.0,
        },
        area: GeographicBounds::new(-98.1, 21.51, -96.89, 22.75),
    },
    // USA - GoM OCS
    Trasformazione {
        codice: 15856,
        nome: "NAD27 to WGS 84 (84)",
        da: 4267,
        a: 4326,
        accuratezza_m: 8.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -7.0,
            ty: 158.0,
            tz: 172.0,
        },
        area: GeographicBounds::new(-97.22, 23.82, -81.17, 30.25),
    },
    // Germany - East Germany all states
    Trasformazione {
        codice: 15869,
        nome: "DHDN to WGS 84 (3)",
        da: 4314,
        a: 4326,
        accuratezza_m: 2.0,
        metodo: MetodoTrasformazione::VettorePosizione {
            tx: 612.4,
            ty: 77.0,
            tz: 440.2,
            rx: -0.053999999999999944,
            ry: 0.05699999999999994,
            rz: -2.796999999999997,
            ds: 2.55,
        },
        area: GeographicBounds::new(9.92, 50.2, 15.04, 54.74),
    },
    // Latin America - SIRGAS 2000 by country
    Trasformazione {
        codice: 15894,
        nome: "SIRGAS 2000 to WGS 84 (1)",
        da: 4674,
        a: 4326,
        accuratezza_m: 1.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: 0.0,
            ty: 0.0,
            tz: 0.0,
        },
        area: GeographicBounds::new(-122.19, -59.87, -25.28, 32.72),
    },
    // Mexico - offshore GoM - Campeche area N
    Trasformazione {
        codice: 15913,
        nome: "NAD27 to WGS 84 (86)",
        da: 4267,
        a: 4326,
        accuratezza_m: 5.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: 0.0,
            ty: 125.0,
            tz: 196.0,
        },
        area: GeographicBounds::new(-94.33, 20.87, -88.67, 23.01),
    },
    // Spain - mainland and Balearic Islands onshore
    Trasformazione {
        codice: 15933,
        nome: "ED50 to WGS 84 (41)",
        da: 4230,
        a: 4326,
        accuratezza_m: 1.0,
        metodo: MetodoTrasformazione::GrigliaNtv2 {
            file_registro: "SPED2ETV2.gsb",
        },
        area: GeographicBounds::new(-9.37, 35.26, 4.39, 43.82),
    },
    // Netherlands - onshore
    Trasformazione {
        codice: 15934,
        nome: "Amersfoort to WGS 84 (3)",
        da: 4289,
        a: 4326,
        accuratezza_m: 1.0,
        metodo: MetodoTrasformazione::TelaioCoordinate {
            tx: 565.2369,
            ty: 50.0087,
            tz: 465.658,
            rx: 0.4068573303223975,
            ry: -0.3507326765425626,
            rz: 1.8703473836067956,
            ds: 4.0812,
        },
        area: GeographicBounds::new(3.2, 50.75, 7.22, 53.7),
    },
    // Germany - onshore
    Trasformazione {
        codice: 15948,
        nome: "DHDN to ETRS89 (8)",
        da: 4314,
        a: 4258,
        accuratezza_m: 0.9,
        metodo: MetodoTrasformazione::GrigliaNtv2 {
            file_registro: "BETA2007.gsb",
        },
        area: GeographicBounds::new(5.86, 47.27, 15.04, 55.09),
    },
    // Germany - onshore
    Trasformazione {
        codice: 15949,
        nome: "DHDN to WGS 84 (4)",
        da: 4314,
        a: 4326,
        accuratezza_m: 1.0,
        metodo: MetodoTrasformazione::GrigliaNtv2 {
            file_registro: "BETA2007.gsb",
        },
        area: GeographicBounds::new(5.86, 47.27, 15.04, 55.09),
    },
    // Portugal - mainland - offshore
    Trasformazione {
        codice: 15964,
        nome: "ED50 to WGS 84 (42)",
        da: 4230,
        a: 4326,
        accuratezza_m: 5.0,
        metodo: MetodoTrasformazione::Traslazioni {
            tx: -86.277,
            ty: -108.879,
            tz: -120.181,
        },
        area: GeographicBounds::new(-13.87, 34.91, -7.24, 41.88),
    },
    // Cuba
    Trasformazione {
        codice: 15978,
        nome: "NAD27 to WGS 84 (88)",
        da: 4267,
        a: 4326,
        accuratezza_m: 1.0,
        metodo: MetodoTrasformazione::TelaioCoordinate {
            tx: 2.478,
            ty: 149.752,
            tz: 197.726,
            rx: -0.5259999999999995,
            ry: -0.4979999999999995,
            rz: 0.5009999999999996,
            ds: 0.685,
        },
        area: GeographicBounds::new(-87.01, 18.83, -73.57, 25.51),
    },
];
