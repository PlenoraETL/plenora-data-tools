//! Costanti di fixture condivise anche con i test unitari del binario.
//!
//! Il file non usa nulla oltre al linguaggio: i test in `src/main.rs` lo
//! includono con `#[path]`, e cosi' il testo delle fixture resta uno solo fra
//! test unitari e test di integrazione.

/// POINT (2 3), little-endian OGC WKB (come `write_self_test` del sorgente).
pub const POINT_WKB: [u8; 21] = [
    1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 64, 0, 0, 0, 0, 0, 0, 8, 64,
];

/// WKT1 realistico di Monte Mario / Italy zone 1 con `AUTHORITY` e
/// `TOWGS84` (EPSG:3003): la forma dello shapefile catastale owner.
pub const MONTE_MARIO_WKT: &str = concat!(
    r#"PROJCS["Monte Mario / Italy zone 1",GEOGCS["Monte Mario","#,
    r#"DATUM["Monte_Mario",SPHEROID["International 1924",6378388,297],"#,
    r#"TOWGS84[-104.1,-49.1,-9.9,0.971,-2.917,0.714,-11.68]],"#,
    r#"PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433]],"#,
    r#"PROJECTION["Transverse_Mercator"],PARAMETER["latitude_of_origin",0],"#,
    r#"PARAMETER["central_meridian",9],PARAMETER["scale_factor",0.9996],"#,
    r#"PARAMETER["false_easting",1500000],PARAMETER["false_northing",0],"#,
    r#"UNIT["metre",1],AXIS["Easting",EAST],AXIS["Northing",NORTH],"#,
    r#"AUTHORITY["EPSG","3003"]]"#
);
