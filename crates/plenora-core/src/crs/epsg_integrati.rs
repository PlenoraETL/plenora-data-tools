//! Tabella dei CRS integrati.
//!
//! GENERATA da `scripts/genera_crs_integrati.py`: non si modifica a mano.
//! Fonte: registro EPSG v11.022 (2024-11-05), come distribuito con PROJ
//! 9.5.1 e letto con pyproj 3.7.2.
//!
//! Per ogni CRS: nome, tipo, assi dell'autorita', ellissoide, area d'uso
//! EPSG (riquadro lon/lat del registro e, per i proiettati, il suo
//! inviluppo proiettato arrotondato al millimetro verso l'esterno) e
//! dominio di validita' dei proiettati, con la regola che lo ha prodotto.

// Dati generati: i letterali restano nella forma piu' corta che rilegge
// lo stesso f64, senza separatori.
#![allow(clippy::unreadable_literal)]

use super::integrati::{geografico, proiettato, Asse, CrsIntegrato, Direzione, Identificativo};
use super::{Ellipsoid, GeographicBounds, ProjectedBounds};

pub(super) const VERSIONE_EPSG: &str = "v11.022";
pub(super) const DATA_EPSG: &str = "2024-11-05";
pub(super) const VERSIONE_PROJ: &str = "9.5.1";

/// WGS 84
const ELLISSOIDE_WGS_84: Ellipsoid = Ellipsoid {
    semi_major_axis_metre: 6378137.0,
    inverse_flattening: 298.257223563,
};
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
/// CGCS2000
const ELLISSOIDE_CGCS2000: Ellipsoid = Ellipsoid {
    semi_major_axis_metre: 6378137.0,
    inverse_flattening: 298.257222101,
};
const ASSI_LON_EAST_LAT_NORTH: [Asse; 2] = [
    Asse {
        nome: "Geodetic longitude",
        abbreviazione: "Lon",
        direzione: Direzione::East,
    },
    Asse {
        nome: "Geodetic latitude",
        abbreviazione: "Lat",
        direzione: Direzione::North,
    },
];
const ASSI_E_EAST_N_NORTH: [Asse; 2] = [
    Asse {
        nome: "Easting",
        abbreviazione: "E",
        direzione: Direzione::East,
    },
    Asse {
        nome: "Northing",
        abbreviazione: "N",
        direzione: Direzione::North,
    },
];
const ASSI_X_EAST_Y_NORTH: [Asse; 2] = [
    Asse {
        nome: "Easting",
        abbreviazione: "X",
        direzione: Direzione::East,
    },
    Asse {
        nome: "Northing",
        abbreviazione: "Y",
        direzione: Direzione::North,
    },
];
const ASSI_N_NORTH_E_EAST: [Asse; 2] = [
    Asse {
        nome: "Northing",
        abbreviazione: "N",
        direzione: Direzione::North,
    },
    Asse {
        nome: "Easting",
        abbreviazione: "E",
        direzione: Direzione::East,
    },
];
const ASSI_Y_NORTH_X_EAST: [Asse; 2] = [
    Asse {
        nome: "Northing",
        abbreviazione: "Y",
        direzione: Direzione::North,
    },
    Asse {
        nome: "Easting",
        abbreviazione: "X",
        direzione: Direzione::East,
    },
];
const ASSI_LAT_NORTH_LON_EAST: [Asse; 2] = [
    Asse {
        nome: "Geodetic latitude",
        abbreviazione: "Lat",
        direzione: Direzione::North,
    },
    Asse {
        nome: "Geodetic longitude",
        abbreviazione: "Lon",
        direzione: Direzione::East,
    },
];
const ASSI_X_NORTH_Y_EAST: [Asse; 2] = [
    Asse {
        nome: "Northing",
        abbreviazione: "X",
        direzione: Direzione::North,
    },
    Asse {
        nome: "Easting",
        abbreviazione: "Y",
        direzione: Direzione::East,
    },
];

/// OGC:CRS84: WGS 84 con longitudine prima della latitudine.
// OGC:CRS84. Dominio: geografico: longitudine [-180, 180], latitudine [-90, 90].
pub(super) const CRS84: CrsIntegrato = geografico(
    Identificativo::OgcCrs84,
    "WGS 84 (CRS84)",
    ASSI_LON_EAST_LAT_NORTH,
    ELLISSOIDE_WGS_84,
    GeographicBounds::new(-180.0, -90.0, 180.0, 90.0),
);

/// CRS EPSG integrati, in ordine crescente di codice.
pub(super) const EPSG: &[CrsIntegrato] = &[
    // EPSG:2056. Dominio: Hotine Oblique Mercator (variant B): inviluppo dell'area d'uso EPSG allargato del 50% dell'estensione per lato.
    proiettato(
        Identificativo::Epsg(2056),
        "CH1903+ / LV95",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_BESSEL_1841,
        GeographicBounds::new(5.96, 45.82, 10.49, 47.81),
        ProjectedBounds::new(2485014.011, 1074128.973, 2837016.893, 1299782.768),
        ProjectedBounds::new(2309012.571, 961302.077, 3013018.333, 1412609.664),
    ),
    // EPSG:2154. Dominio: Lambert Conic Conformal (2SP): inviluppo dell'area d'uso EPSG allargato del 50% dell'estensione per lato.
    proiettato(
        Identificativo::Epsg(2154),
        "RGF93 v1 / Lambert-93",
        ASSI_X_EAST_Y_NORTH,
        ELLISSOIDE_GRS_1980,
        GeographicBounds::new(-9.86, 41.15, 10.38, 51.56),
        ProjectedBounds::new(-378305.81, 6005280.99, 1320649.572, 7235612.725),
        ProjectedBounds::new(-1227783.501, 5390115.122, 2170127.262, 7850778.593),
    ),
    // EPSG:2193. Dominio: TM nazionale: longitudine 173 +/- 15, latitudine [-53.945, -27.485].
    proiettato(
        Identificativo::Epsg(2193),
        "NZGD2000 / New Zealand Transverse Mercator 2000",
        ASSI_N_NORTH_E_EAST,
        ELLISSOIDE_GRS_1980,
        GeographicBounds::new(166.37, -47.33, 178.63, -34.1),
        ProjectedBounds::new(987942.452, 4736810.692, 2119620.662, 6226756.131),
        ProjectedBounds::new(108419.17, 3917769.208, 3091580.83, 6959844.689),
    ),
    // EPSG:3003. Dominio: TM nazionale: longitudine 9 +/- 15, latitudine [31.275, 52.295].
    proiettato(
        Identificativo::Epsg(3003),
        "Monte Mario / Italy zone 1",
        ASSI_X_EAST_Y_NORTH,
        ELLISSOIDE_INTERNATIONAL_1924,
        GeographicBounds::new(5.93, 36.53, 12.0, 47.04),
        ProjectedBounds::new(1225120.587, 4042801.237, 1768610.103, 5214284.215),
        ProjectedBounds::new(64544.714, 3460130.87, 2935455.286, 5900670.603),
    ),
    // EPSG:3004. Dominio: TM nazionale: longitudine 15 +/- 15, latitudine [28.59, 53.27].
    proiettato(
        Identificativo::Epsg(3004),
        "Monte Mario / Italy zone 2",
        ASSI_X_EAST_Y_NORTH,
        ELLISSOIDE_INTERNATIONAL_1924,
        GeographicBounds::new(12.0, 34.76, 18.99, 47.1),
        ProjectedBounds::new(2245391.807, 3846488.962, 2885274.102, 5224106.004),
        ProjectedBounds::new(1044074.124, 3162607.506, 3995925.876, 6008070.422),
    ),
    // EPSG:3035. Dominio: Lambert Azimuthal Equal Area: inviluppo dell'area d'uso EPSG allargato del 50% dell'estensione per lato.
    proiettato(
        Identificativo::Epsg(3035),
        "ETRS89-extended / LAEA Europe",
        ASSI_Y_NORTH_X_EAST,
        ELLISSOIDE_GRS_1980,
        GeographicBounds::new(-35.58, 24.6, 44.83, 84.73),
        ProjectedBounds::new(-146473.114, 196462.568, 7824928.565, 6971477.485),
        ProjectedBounds::new(-4132173.953, -3191044.891, 11810629.404, 10358984.943),
    ),
    // EPSG:3064. Dominio: UTM: longitudine 9 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(3064),
        "IGM95 / UTM zone 32N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_GRS_1980,
        GeographicBounds::new(5.93, 36.53, 12.0, 47.04),
        ProjectedBounds::new(225132.787, 4042735.808, 768598.182, 5214183.816),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:3065. Dominio: UTM: longitudine 15 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(3065),
        "IGM95 / UTM zone 33N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_GRS_1980,
        GeographicBounds::new(12.0, 34.79, 18.0, 47.1),
        ProjectedBounds::new(225503.37, 3849755.266, 774496.63, 5220644.505),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:3395. Dominio: Mercator: longitudine [-180, 180], latitudine [-85.06, 85.06].
    proiettato(
        Identificativo::Epsg(3395),
        "WGS 84 / World Mercator",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-180.0, -80.0, 180.0, 84.0),
        ProjectedBounds::new(-20037508.343, -15496570.74, 20037508.343, 18764656.232),
        ProjectedBounds::new(-20037508.343, -20006332.438, 20037508.343, 20006332.438),
    ),
    // EPSG:3857. Dominio: Mercator: longitudine [-180, 180], latitudine [-85.06, 85.06].
    proiettato(
        Identificativo::Epsg(3857),
        "WGS 84 / Pseudo-Mercator",
        ASSI_X_EAST_Y_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-180.0, -85.06, 180.0, 85.06),
        ProjectedBounds::new(-20037508.343, -20048966.105, 20037508.343, 20048966.105),
        ProjectedBounds::new(-20037508.343, -20048966.105, 20037508.343, 20048966.105),
    ),
    // EPSG:4171. Dominio: geografico: longitudine [-180, 180], latitudine [-90, 90].
    geografico(
        Identificativo::Epsg(4171),
        "RGF93 v1",
        ASSI_LAT_NORTH_LON_EAST,
        ELLISSOIDE_GRS_1980,
        GeographicBounds::new(-9.86, 41.15, 10.38, 51.56),
    ),
    // EPSG:4230. Dominio: geografico: longitudine [-180, 180], latitudine [-90, 90].
    geografico(
        Identificativo::Epsg(4230),
        "ED50",
        ASSI_LAT_NORTH_LON_EAST,
        ELLISSOIDE_INTERNATIONAL_1924,
        GeographicBounds::new(-16.1, 25.71, 48.61, 84.73),
    ),
    // EPSG:4258. Dominio: geografico: longitudine [-180, 180], latitudine [-90, 90].
    geografico(
        Identificativo::Epsg(4258),
        "ETRS89",
        ASSI_LAT_NORTH_LON_EAST,
        ELLISSOIDE_GRS_1980,
        GeographicBounds::new(-16.1, 33.26, 38.01, 84.73),
    ),
    // EPSG:4265. Dominio: geografico: longitudine [-180, 180], latitudine [-90, 90].
    geografico(
        Identificativo::Epsg(4265),
        "Monte Mario",
        ASSI_LAT_NORTH_LON_EAST,
        ELLISSOIDE_INTERNATIONAL_1924,
        GeographicBounds::new(5.93, 34.76, 18.99, 47.1),
    ),
    // EPSG:4267. Dominio: geografico: longitudine [-180, 180], latitudine [-90, 90].
    geografico(
        Identificativo::Epsg(4267),
        "NAD27",
        ASSI_LAT_NORTH_LON_EAST,
        ELLISSOIDE_CLARKE_1866,
        GeographicBounds::new(167.65, 7.15, -47.74, 83.17),
    ),
    // EPSG:4269. Dominio: geografico: longitudine [-180, 180], latitudine [-90, 90].
    geografico(
        Identificativo::Epsg(4269),
        "NAD83",
        ASSI_LAT_NORTH_LON_EAST,
        ELLISSOIDE_GRS_1980,
        GeographicBounds::new(167.65, 14.92, -40.73, 86.45),
    ),
    // EPSG:4277. Dominio: geografico: longitudine [-180, 180], latitudine [-90, 90].
    geografico(
        Identificativo::Epsg(4277),
        "OSGB36",
        ASSI_LAT_NORTH_LON_EAST,
        ELLISSOIDE_AIRY_1830,
        GeographicBounds::new(-9.01, 49.75, 2.01, 61.01),
    ),
    // EPSG:4283. Dominio: geografico: longitudine [-180, 180], latitudine [-90, 90].
    geografico(
        Identificativo::Epsg(4283),
        "GDA94",
        ASSI_LAT_NORTH_LON_EAST,
        ELLISSOIDE_GRS_1980,
        GeographicBounds::new(93.41, -60.55, 173.34, -8.47),
    ),
    // EPSG:4326. Dominio: geografico: longitudine [-180, 180], latitudine [-90, 90].
    geografico(
        Identificativo::Epsg(4326),
        "WGS 84",
        ASSI_LAT_NORTH_LON_EAST,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-180.0, -90.0, 180.0, 90.0),
    ),
    // EPSG:4490. Dominio: geografico: longitudine [-180, 180], latitudine [-90, 90].
    geografico(
        Identificativo::Epsg(4490),
        "China Geodetic Coordinate System 2000",
        ASSI_LAT_NORTH_LON_EAST,
        ELLISSOIDE_CGCS2000,
        GeographicBounds::new(73.62, 16.7, 134.77, 53.56),
    ),
    // EPSG:4670. Dominio: geografico: longitudine [-180, 180], latitudine [-90, 90].
    geografico(
        Identificativo::Epsg(4670),
        "IGM95",
        ASSI_LAT_NORTH_LON_EAST,
        ELLISSOIDE_GRS_1980,
        GeographicBounds::new(5.93, 34.76, 18.99, 47.1),
    ),
    // EPSG:4674. Dominio: geografico: longitudine [-180, 180], latitudine [-90, 90].
    geografico(
        Identificativo::Epsg(4674),
        "SIRGAS 2000",
        ASSI_LAT_NORTH_LON_EAST,
        ELLISSOIDE_GRS_1980,
        GeographicBounds::new(-122.19, -59.87, -25.28, 32.72),
    ),
    // EPSG:6706. Dominio: geografico: longitudine [-180, 180], latitudine [-90, 90].
    geografico(
        Identificativo::Epsg(6706),
        "RDN2008",
        ASSI_LAT_NORTH_LON_EAST,
        ELLISSOIDE_GRS_1980,
        GeographicBounds::new(5.93, 34.76, 18.99, 47.1),
    ),
    // EPSG:6707. Dominio: UTM: longitudine 9 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(6707),
        "RDN2008 / UTM zone 32N (N-E)",
        ASSI_N_NORTH_E_EAST,
        ELLISSOIDE_GRS_1980,
        GeographicBounds::new(5.93, 36.53, 12.0, 47.04),
        ProjectedBounds::new(225132.787, 4042735.808, 768598.182, 5214183.816),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:6708. Dominio: UTM: longitudine 15 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(6708),
        "RDN2008 / UTM zone 33N (N-E)",
        ASSI_N_NORTH_E_EAST,
        ELLISSOIDE_GRS_1980,
        GeographicBounds::new(12.0, 34.79, 18.0, 47.1),
        ProjectedBounds::new(225503.37, 3849755.266, 774496.63, 5220644.505),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:6709. Dominio: UTM: longitudine 21 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(6709),
        "RDN2008 / UTM zone 34N (N-E)",
        ASSI_N_NORTH_E_EAST,
        ELLISSOIDE_GRS_1980,
        GeographicBounds::new(17.99, 34.76, 18.99, 41.64),
        ProjectedBounds::new(224488.264, 3848268.687, 332597.548, 4614184.667),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:6875. Dominio: TM nazionale: longitudine 12 +/- 15, latitudine [28.59, 53.27].
    proiettato(
        Identificativo::Epsg(6875),
        "RDN2008 / Italy zone (N-E)",
        ASSI_N_NORTH_E_EAST,
        ELLISSOIDE_GRS_1980,
        GeographicBounds::new(5.93, 34.76, 18.99, 47.1),
        ProjectedBounds::new(6444735.932, 3842195.738, 7639559.465, 5234265.086),
        ProjectedBounds::new(5525761.373, 3159082.3, 8474238.627, 6001328.797),
    ),
    // EPSG:7791. Dominio: UTM: longitudine 9 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(7791),
        "RDN2008 / UTM zone 32N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_GRS_1980,
        GeographicBounds::new(5.93, 36.53, 12.0, 47.04),
        ProjectedBounds::new(225132.787, 4042735.808, 768598.182, 5214183.816),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:7792. Dominio: UTM: longitudine 15 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(7792),
        "RDN2008 / UTM zone 33N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_GRS_1980,
        GeographicBounds::new(12.0, 34.79, 18.0, 47.1),
        ProjectedBounds::new(225503.37, 3849755.266, 774496.63, 5220644.505),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:7793. Dominio: UTM: longitudine 21 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(7793),
        "RDN2008 / UTM zone 34N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_GRS_1980,
        GeographicBounds::new(17.99, 34.76, 18.99, 41.64),
        ProjectedBounds::new(224488.264, 3848268.687, 332597.548, 4614184.667),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:7794. Dominio: TM nazionale: longitudine 12 +/- 15, latitudine [28.59, 53.27].
    proiettato(
        Identificativo::Epsg(7794),
        "RDN2008 / Italy zone (E-N)",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_GRS_1980,
        GeographicBounds::new(5.93, 34.76, 18.99, 47.1),
        ProjectedBounds::new(6444735.932, 3842195.738, 7639559.465, 5234265.086),
        ProjectedBounds::new(5525761.373, 3159082.3, 8474238.627, 6001328.797),
    ),
    // EPSG:7844. Dominio: geografico: longitudine [-180, 180], latitudine [-90, 90].
    geografico(
        Identificativo::Epsg(7844),
        "GDA2020",
        ASSI_LAT_NORTH_LON_EAST,
        ELLISSOIDE_GRS_1980,
        GeographicBounds::new(93.41, -60.55, 173.34, -8.47),
    ),
    // EPSG:23032. Dominio: UTM: longitudine 9 +/- 15, latitudine [0, 84.33].
    proiettato(
        Identificativo::Epsg(23032),
        "ED50 / UTM zone 32N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_INTERNATIONAL_1924,
        GeographicBounds::new(6.0, 36.53, 12.0, 84.33),
        ProjectedBounds::new(231389.897, 4042801.237, 768610.103, 9366084.804),
        ProjectedBounds::new(-1188726.442, 0.0, 2188726.442, 9386660.498),
    ),
    // EPSG:23033. Dominio: UTM: longitudine 15 +/- 15, latitudine [0, 84.42].
    proiettato(
        Identificativo::Epsg(23033),
        "ED50 / UTM zone 33N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_INTERNATIONAL_1924,
        GeographicBounds::new(12.0, 34.49, 18.01, 84.42),
        ProjectedBounds::new(224499.778, 3816548.1, 776418.861, 9376124.958),
        ProjectedBounds::new(-1188726.442, 0.0, 2188726.442, 9396372.367),
    ),
    // EPSG:23034. Dominio: UTM: longitudine 21 +/- 15, latitudine [0, 84.54].
    proiettato(
        Identificativo::Epsg(23034),
        "ED50 / UTM zone 34N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_INTERNATIONAL_1924,
        GeographicBounds::new(17.99, 33.59, 24.01, 84.54),
        ProjectedBounds::new(220642.267, 3716754.812, 779357.733, 9389504.192),
        ProjectedBounds::new(-1188726.442, 0.0, 2188726.442, 9409321.265),
    ),
    // EPSG:25828. Dominio: UTM: longitudine -15 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(25828),
        "ETRS89 / UTM zone 28N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_GRS_1980,
        GeographicBounds::new(-16.1, 34.93, -11.99, 72.44),
        ProjectedBounds::new(399535.629, 3865280.375, 774945.08, 8040550.061),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:25829. Dominio: UTM: longitudine -9 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(25829),
        "ETRS89 / UTM zone 29N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_GRS_1980,
        GeographicBounds::new(-12.0, 34.91, -6.0, 74.13),
        ProjectedBounds::new(225902.08, 3863062.48, 774097.92, 8228847.091),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:25830. Dominio: UTM: longitudine -3 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(25830),
        "ETRS89 / UTM zone 30N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_GRS_1980,
        GeographicBounds::new(-6.0, 35.26, 0.01, 80.49),
        ProjectedBounds::new(227071.857, 3901876.695, 773838.184, 8937716.117),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:25831. Dominio: UTM: longitudine 3 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(25831),
        "ETRS89 / UTM zone 31N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_GRS_1980,
        GeographicBounds::new(0.0, 37.0, 6.01, 82.45),
        ProjectedBounds::new(233037.879, 4094872.37, 767852.221, 9156211.356),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:25832. Dominio: UTM: longitudine 9 +/- 15, latitudine [0, 84.01].
    proiettato(
        Identificativo::Epsg(25832),
        "ETRS89 / UTM zone 32N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_GRS_1980,
        GeographicBounds::new(6.0, 36.53, 12.01, 84.01),
        ProjectedBounds::new(231401.818, 4042735.808, 769493.75, 9330126.131),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9351840.095),
    ),
    // EPSG:25833. Dominio: UTM: longitudine 15 +/- 15, latitudine [0, 84.01].
    proiettato(
        Identificativo::Epsg(25833),
        "ETRS89 / UTM zone 33N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_GRS_1980,
        GeographicBounds::new(12.0, 34.79, 18.01, 84.01),
        ProjectedBounds::new(225503.37, 3849755.266, 775411.914, 9330126.131),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9351840.095),
    ),
    // EPSG:25834. Dominio: UTM: longitudine 21 +/- 15, latitudine [0, 84.01].
    proiettato(
        Identificativo::Epsg(25834),
        "ETRS89 / UTM zone 34N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_GRS_1980,
        GeographicBounds::new(18.0, 34.76, 24.01, 84.01),
        ProjectedBounds::new(225403.88, 3846428.503, 775511.736, 9330126.131),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9351840.095),
    ),
    // EPSG:25835. Dominio: UTM: longitudine 27 +/- 15, latitudine [0, 84.01].
    proiettato(
        Identificativo::Epsg(25835),
        "ETRS89 / UTM zone 35N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_GRS_1980,
        GeographicBounds::new(24.0, 41.24, 30.01, 84.01),
        ProjectedBounds::new(248597.583, 4565399.895, 752240.527, 9330126.131),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9351840.095),
    ),
    // EPSG:25836. Dominio: UTM: longitudine 33 +/- 15, latitudine [0, 84.01].
    proiettato(
        Identificativo::Epsg(25836),
        "ETRS89 / UTM zone 36N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_GRS_1980,
        GeographicBounds::new(30.0, 42.56, 36.01, 84.01),
        ProjectedBounds::new(253727.658, 4711955.428, 747093.315, 9330126.131),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9351840.095),
    ),
    // EPSG:25837. Dominio: UTM: longitudine 39 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(25837),
        "ETRS89 / UTM zone 37N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_GRS_1980,
        GeographicBounds::new(36.0, 72.99, 38.01, 79.07),
        ProjectedBounds::new(402080.15, 8099631.324, 479045.429, 8779418.626),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:27700. Dominio: TM nazionale: longitudine -2 +/- 15, latitudine [44.12, 66.64].
    proiettato(
        Identificativo::Epsg(27700),
        "OSGB36 / British National Grid",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_AIRY_1830,
        GeographicBounds::new(-9.01, 49.75, 2.01, 61.01),
        ProjectedBounds::new(-104728.765, -16627.735, 688806.008, 1256616.321),
        ProjectedBounds::new(-800375.227, -642209.343, 1600375.227, 1943333.626),
    ),
    // EPSG:28992. Dominio: Oblique Stereographic: inviluppo dell'area d'uso EPSG allargato del 50% dell'estensione per lato.
    proiettato(
        Identificativo::Epsg(28992),
        "Amersfoort / RD New",
        ASSI_X_EAST_Y_NORTH,
        ELLISSOIDE_BESSEL_1841,
        GeographicBounds::new(3.2, 50.75, 7.22, 53.7),
        ProjectedBounds::new(634.573, 306579.354, 284300.026, 636981.77),
        ProjectedBounds::new(-141198.153, 141378.146, 426132.752, 802182.978),
    ),
    // EPSG:31467. Dominio: TM nazionale: longitudine 9 +/- 15, latitudine [43.36, 59].
    proiettato(
        Identificativo::Epsg(31467),
        "DHDN / 3-degree Gauss-Kruger zone 3",
        ASSI_X_NORTH_Y_EAST,
        ELLISSOIDE_BESSEL_1841,
        GeographicBounds::new(7.5, 47.27, 10.51, 55.09),
        ProjectedBounds::new(3386506.78, 5236730.598, 3614249.828, 6107658.749),
        ProjectedBounds::new(2283517.332, 4802229.552, 4716482.668, 6639027.89),
    ),
    // EPSG:32601. Dominio: UTM: longitudine -177 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32601),
        "WGS 84 / UTM zone 1N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-180.0, 0.0, -174.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32602. Dominio: UTM: longitudine -171 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32602),
        "WGS 84 / UTM zone 2N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-174.0, 0.0, -168.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32603. Dominio: UTM: longitudine -165 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32603),
        "WGS 84 / UTM zone 3N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-168.0, 0.0, -162.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32604. Dominio: UTM: longitudine -159 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32604),
        "WGS 84 / UTM zone 4N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-162.0, 0.0, -156.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32605. Dominio: UTM: longitudine -153 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32605),
        "WGS 84 / UTM zone 5N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-156.0, 0.0, -150.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32606. Dominio: UTM: longitudine -147 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32606),
        "WGS 84 / UTM zone 6N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-150.0, 0.0, -144.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32607. Dominio: UTM: longitudine -141 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32607),
        "WGS 84 / UTM zone 7N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-144.0, 0.0, -138.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32608. Dominio: UTM: longitudine -135 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32608),
        "WGS 84 / UTM zone 8N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-138.0, 0.0, -132.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32609. Dominio: UTM: longitudine -129 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32609),
        "WGS 84 / UTM zone 9N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-132.0, 0.0, -126.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32610. Dominio: UTM: longitudine -123 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32610),
        "WGS 84 / UTM zone 10N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-126.0, 0.0, -120.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32611. Dominio: UTM: longitudine -117 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32611),
        "WGS 84 / UTM zone 11N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-120.0, 0.0, -114.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32612. Dominio: UTM: longitudine -111 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32612),
        "WGS 84 / UTM zone 12N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-114.0, 0.0, -108.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32613. Dominio: UTM: longitudine -105 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32613),
        "WGS 84 / UTM zone 13N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-108.0, 0.0, -102.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32614. Dominio: UTM: longitudine -99 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32614),
        "WGS 84 / UTM zone 14N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-102.0, 0.0, -96.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32615. Dominio: UTM: longitudine -93 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32615),
        "WGS 84 / UTM zone 15N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-96.0, 0.0, -90.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32616. Dominio: UTM: longitudine -87 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32616),
        "WGS 84 / UTM zone 16N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-90.0, 0.0, -84.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32617. Dominio: UTM: longitudine -81 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32617),
        "WGS 84 / UTM zone 17N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-84.0, 0.0, -78.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32618. Dominio: UTM: longitudine -75 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32618),
        "WGS 84 / UTM zone 18N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-78.0, 0.0, -72.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32619. Dominio: UTM: longitudine -69 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32619),
        "WGS 84 / UTM zone 19N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-72.0, 0.0, -66.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32620. Dominio: UTM: longitudine -63 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32620),
        "WGS 84 / UTM zone 20N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-66.0, 0.0, -60.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32621. Dominio: UTM: longitudine -57 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32621),
        "WGS 84 / UTM zone 21N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-60.0, 0.0, -54.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32622. Dominio: UTM: longitudine -51 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32622),
        "WGS 84 / UTM zone 22N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-54.0, 0.0, -48.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32623. Dominio: UTM: longitudine -45 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32623),
        "WGS 84 / UTM zone 23N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-48.0, 0.0, -42.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32624. Dominio: UTM: longitudine -39 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32624),
        "WGS 84 / UTM zone 24N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-42.0, 0.0, -36.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32625. Dominio: UTM: longitudine -33 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32625),
        "WGS 84 / UTM zone 25N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-36.0, 0.0, -30.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32626. Dominio: UTM: longitudine -27 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32626),
        "WGS 84 / UTM zone 26N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-30.0, 0.0, -24.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32627. Dominio: UTM: longitudine -21 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32627),
        "WGS 84 / UTM zone 27N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-24.0, 0.0, -18.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32628. Dominio: UTM: longitudine -15 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32628),
        "WGS 84 / UTM zone 28N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-18.0, 0.0, -12.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32629. Dominio: UTM: longitudine -9 +/- 15, latitudine [0, 84.01].
    proiettato(
        Identificativo::Epsg(32629),
        "WGS 84 / UTM zone 29N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-12.01, 0.0, -6.0, 84.01),
        ProjectedBounds::new(164907.15, 0.0, 833978.557, 9330126.131),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9351840.095),
    ),
    // EPSG:32630. Dominio: UTM: longitudine -3 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32630),
        "WGS 84 / UTM zone 30N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-6.0, 0.0, 0.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32631. Dominio: UTM: longitudine 3 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32631),
        "WGS 84 / UTM zone 31N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(0.0, 0.0, 6.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32632. Dominio: UTM: longitudine 9 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32632),
        "WGS 84 / UTM zone 32N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(6.0, 0.0, 12.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32633. Dominio: UTM: longitudine 15 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32633),
        "WGS 84 / UTM zone 33N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(12.0, 0.0, 18.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32634. Dominio: UTM: longitudine 21 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32634),
        "WGS 84 / UTM zone 34N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(18.0, 0.0, 24.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32635. Dominio: UTM: longitudine 27 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32635),
        "WGS 84 / UTM zone 35N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(24.0, 0.0, 30.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32636. Dominio: UTM: longitudine 33 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32636),
        "WGS 84 / UTM zone 36N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(30.0, 0.0, 36.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32637. Dominio: UTM: longitudine 39 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32637),
        "WGS 84 / UTM zone 37N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(36.0, 0.0, 42.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32638. Dominio: UTM: longitudine 45 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32638),
        "WGS 84 / UTM zone 38N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(42.0, 0.0, 48.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32639. Dominio: UTM: longitudine 51 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32639),
        "WGS 84 / UTM zone 39N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(48.0, 0.0, 54.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32640. Dominio: UTM: longitudine 57 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32640),
        "WGS 84 / UTM zone 40N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(54.0, 0.0, 60.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32641. Dominio: UTM: longitudine 63 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32641),
        "WGS 84 / UTM zone 41N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(60.0, 0.0, 66.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32642. Dominio: UTM: longitudine 69 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32642),
        "WGS 84 / UTM zone 42N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(66.0, 0.0, 72.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32643. Dominio: UTM: longitudine 75 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32643),
        "WGS 84 / UTM zone 43N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(72.0, 0.0, 78.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32644. Dominio: UTM: longitudine 81 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32644),
        "WGS 84 / UTM zone 44N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(78.0, 0.0, 84.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32645. Dominio: UTM: longitudine 87 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32645),
        "WGS 84 / UTM zone 45N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(84.0, 0.0, 90.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32646. Dominio: UTM: longitudine 93 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32646),
        "WGS 84 / UTM zone 46N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(90.0, 0.0, 96.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32647. Dominio: UTM: longitudine 99 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32647),
        "WGS 84 / UTM zone 47N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(96.0, 0.0, 102.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32648. Dominio: UTM: longitudine 105 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32648),
        "WGS 84 / UTM zone 48N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(102.0, 0.0, 108.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32649. Dominio: UTM: longitudine 111 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32649),
        "WGS 84 / UTM zone 49N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(108.0, 0.0, 114.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32650. Dominio: UTM: longitudine 117 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32650),
        "WGS 84 / UTM zone 50N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(114.0, 0.0, 120.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32651. Dominio: UTM: longitudine 123 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32651),
        "WGS 84 / UTM zone 51N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(120.0, 0.0, 126.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32652. Dominio: UTM: longitudine 129 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32652),
        "WGS 84 / UTM zone 52N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(126.0, 0.0, 132.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32653. Dominio: UTM: longitudine 135 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32653),
        "WGS 84 / UTM zone 53N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(132.0, 0.0, 138.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32654. Dominio: UTM: longitudine 141 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32654),
        "WGS 84 / UTM zone 54N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(138.0, 0.0, 144.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32655. Dominio: UTM: longitudine 147 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32655),
        "WGS 84 / UTM zone 55N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(144.0, 0.0, 150.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32656. Dominio: UTM: longitudine 153 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32656),
        "WGS 84 / UTM zone 56N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(150.0, 0.0, 156.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32657. Dominio: UTM: longitudine 159 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32657),
        "WGS 84 / UTM zone 57N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(156.0, 0.0, 162.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32658. Dominio: UTM: longitudine 165 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32658),
        "WGS 84 / UTM zone 58N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(162.0, 0.0, 168.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32659. Dominio: UTM: longitudine 171 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32659),
        "WGS 84 / UTM zone 59N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(168.0, 0.0, 174.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32660. Dominio: UTM: longitudine 177 +/- 15, latitudine [0, 84].
    proiettato(
        Identificativo::Epsg(32660),
        "WGS 84 / UTM zone 60N",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(174.0, 0.0, 180.0, 84.0),
        ProjectedBounds::new(166021.443, 0.0, 833978.557, 9329005.183),
        ProjectedBounds::new(-1188659.414, 0.0, 2188659.414, 9350760.976),
    ),
    // EPSG:32701. Dominio: UTM: longitudine -177 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32701),
        "WGS 84 / UTM zone 1S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-180.0, -80.0, -174.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32702. Dominio: UTM: longitudine -171 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32702),
        "WGS 84 / UTM zone 2S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-174.0, -80.0, -168.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32703. Dominio: UTM: longitudine -165 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32703),
        "WGS 84 / UTM zone 3S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-168.0, -80.0, -162.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32704. Dominio: UTM: longitudine -159 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32704),
        "WGS 84 / UTM zone 4S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-162.0, -80.0, -156.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32705. Dominio: UTM: longitudine -153 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32705),
        "WGS 84 / UTM zone 5S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-156.0, -80.0, -150.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32706. Dominio: UTM: longitudine -147 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32706),
        "WGS 84 / UTM zone 6S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-150.0, -80.0, -144.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32707. Dominio: UTM: longitudine -141 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32707),
        "WGS 84 / UTM zone 7S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-144.0, -80.0, -138.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32708. Dominio: UTM: longitudine -135 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32708),
        "WGS 84 / UTM zone 8S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-138.0, -80.0, -132.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32709. Dominio: UTM: longitudine -129 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32709),
        "WGS 84 / UTM zone 9S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-132.0, -80.0, -126.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32710. Dominio: UTM: longitudine -123 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32710),
        "WGS 84 / UTM zone 10S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-126.0, -80.0, -120.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32711. Dominio: UTM: longitudine -117 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32711),
        "WGS 84 / UTM zone 11S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-120.0, -80.0, -114.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32712. Dominio: UTM: longitudine -111 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32712),
        "WGS 84 / UTM zone 12S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-114.0, -80.0, -108.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32713. Dominio: UTM: longitudine -105 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32713),
        "WGS 84 / UTM zone 13S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-108.0, -80.0, -102.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32714. Dominio: UTM: longitudine -99 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32714),
        "WGS 84 / UTM zone 14S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-102.0, -80.0, -96.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32715. Dominio: UTM: longitudine -93 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32715),
        "WGS 84 / UTM zone 15S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-96.0, -80.0, -90.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32716. Dominio: UTM: longitudine -87 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32716),
        "WGS 84 / UTM zone 16S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-90.0, -80.0, -84.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32717. Dominio: UTM: longitudine -81 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32717),
        "WGS 84 / UTM zone 17S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-84.0, -80.0, -78.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32718. Dominio: UTM: longitudine -75 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32718),
        "WGS 84 / UTM zone 18S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-78.0, -80.0, -72.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32719. Dominio: UTM: longitudine -69 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32719),
        "WGS 84 / UTM zone 19S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-72.0, -80.0, -66.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32720. Dominio: UTM: longitudine -63 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32720),
        "WGS 84 / UTM zone 20S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-66.0, -80.0, -60.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32721. Dominio: UTM: longitudine -57 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32721),
        "WGS 84 / UTM zone 21S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-60.0, -80.0, -54.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32722. Dominio: UTM: longitudine -51 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32722),
        "WGS 84 / UTM zone 22S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-54.0, -80.0, -48.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32723. Dominio: UTM: longitudine -45 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32723),
        "WGS 84 / UTM zone 23S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-48.0, -80.0, -42.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32724. Dominio: UTM: longitudine -39 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32724),
        "WGS 84 / UTM zone 24S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-42.0, -80.0, -36.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32725. Dominio: UTM: longitudine -33 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32725),
        "WGS 84 / UTM zone 25S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-36.0, -80.0, -30.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32726. Dominio: UTM: longitudine -27 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32726),
        "WGS 84 / UTM zone 26S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-30.0, -80.0, -24.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32727. Dominio: UTM: longitudine -21 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32727),
        "WGS 84 / UTM zone 27S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-24.0, -80.0, -18.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32728. Dominio: UTM: longitudine -15 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32728),
        "WGS 84 / UTM zone 28S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-18.0, -80.0, -12.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32729. Dominio: UTM: longitudine -9 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32729),
        "WGS 84 / UTM zone 29S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-12.0, -80.0, -6.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32730. Dominio: UTM: longitudine -3 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32730),
        "WGS 84 / UTM zone 30S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(-6.0, -80.0, 0.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32731. Dominio: UTM: longitudine 3 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32731),
        "WGS 84 / UTM zone 31S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(0.0, -80.0, 6.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32732. Dominio: UTM: longitudine 9 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32732),
        "WGS 84 / UTM zone 32S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(6.0, -80.0, 12.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32733. Dominio: UTM: longitudine 15 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32733),
        "WGS 84 / UTM zone 33S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(12.0, -80.0, 18.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32734. Dominio: UTM: longitudine 21 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32734),
        "WGS 84 / UTM zone 34S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(18.0, -80.0, 24.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32735. Dominio: UTM: longitudine 27 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32735),
        "WGS 84 / UTM zone 35S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(24.0, -80.0, 30.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32736. Dominio: UTM: longitudine 33 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32736),
        "WGS 84 / UTM zone 36S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(30.0, -80.0, 36.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32737. Dominio: UTM: longitudine 39 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32737),
        "WGS 84 / UTM zone 37S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(36.0, -80.0, 42.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32738. Dominio: UTM: longitudine 45 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32738),
        "WGS 84 / UTM zone 38S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(42.0, -80.0, 48.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32739. Dominio: UTM: longitudine 51 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32739),
        "WGS 84 / UTM zone 39S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(48.0, -80.0, 54.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32740. Dominio: UTM: longitudine 57 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32740),
        "WGS 84 / UTM zone 40S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(54.0, -80.0, 60.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32741. Dominio: UTM: longitudine 63 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32741),
        "WGS 84 / UTM zone 41S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(60.0, -80.0, 66.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32742. Dominio: UTM: longitudine 69 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32742),
        "WGS 84 / UTM zone 42S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(66.0, -80.0, 72.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32743. Dominio: UTM: longitudine 75 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32743),
        "WGS 84 / UTM zone 43S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(72.0, -80.0, 78.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32744. Dominio: UTM: longitudine 81 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32744),
        "WGS 84 / UTM zone 44S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(78.0, -80.0, 84.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32745. Dominio: UTM: longitudine 87 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32745),
        "WGS 84 / UTM zone 45S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(84.0, -80.0, 90.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32746. Dominio: UTM: longitudine 93 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32746),
        "WGS 84 / UTM zone 46S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(90.0, -80.0, 96.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32747. Dominio: UTM: longitudine 99 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32747),
        "WGS 84 / UTM zone 47S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(96.0, -80.0, 102.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32748. Dominio: UTM: longitudine 105 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32748),
        "WGS 84 / UTM zone 48S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(102.0, -80.0, 108.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32749. Dominio: UTM: longitudine 111 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32749),
        "WGS 84 / UTM zone 49S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(108.0, -80.0, 114.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32750. Dominio: UTM: longitudine 117 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32750),
        "WGS 84 / UTM zone 50S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(114.0, -80.0, 120.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32751. Dominio: UTM: longitudine 123 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32751),
        "WGS 84 / UTM zone 51S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(120.0, -80.0, 126.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32752. Dominio: UTM: longitudine 129 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32752),
        "WGS 84 / UTM zone 52S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(126.0, -80.0, 132.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32753. Dominio: UTM: longitudine 135 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32753),
        "WGS 84 / UTM zone 53S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(132.0, -80.0, 138.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32754. Dominio: UTM: longitudine 141 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32754),
        "WGS 84 / UTM zone 54S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(138.0, -80.0, 144.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32755. Dominio: UTM: longitudine 147 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32755),
        "WGS 84 / UTM zone 55S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(144.0, -80.0, 150.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32756. Dominio: UTM: longitudine 153 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32756),
        "WGS 84 / UTM zone 56S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(150.0, -80.0, 156.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32757. Dominio: UTM: longitudine 159 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32757),
        "WGS 84 / UTM zone 57S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(156.0, -80.0, 162.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32758. Dominio: UTM: longitudine 165 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32758),
        "WGS 84 / UTM zone 58S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(162.0, -80.0, 168.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32759. Dominio: UTM: longitudine 171 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32759),
        "WGS 84 / UTM zone 59S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(168.0, -80.0, 174.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
    // EPSG:32760. Dominio: UTM: longitudine 177 +/- 15, latitudine [-80, 0].
    proiettato(
        Identificativo::Epsg(32760),
        "WGS 84 / UTM zone 60S",
        ASSI_E_EAST_N_NORTH,
        ELLISSOIDE_WGS_84,
        GeographicBounds::new(174.0, -80.0, 180.0, 0.0),
        ProjectedBounds::new(166021.443, 1116915.044, 833978.557, 10000000.0),
        ProjectedBounds::new(-1188659.414, 1081104.36, 2188659.414, 10000000.0),
    ),
];
