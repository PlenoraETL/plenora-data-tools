//! Oracolo delle misure geodetiche su ogni ellissoide della tabella.
//!
//! I riferimenti sono di `GeographicLib` 2.0 per Python (Karney), controllati
//! nel generatore contro PROJ (`pyproj.Geod`), sull'ellissoide che la
//! tabella integrata da' a ogni CRS geografico
//! (`scripts/genera_riferimenti_geodetici.py`, fixture in
//! `tests/fixtures/geodetica/`). Prima della correzione ogni misura usava
//! WGS 84: su ED50 (Internazionale 1924) Bologna-Modena differiva di 1,6 m.

use std::collections::BTreeSet;

use geo::{Geometry, Point};
use plenora_core::crs::{builtin_crs_identifiers, resolve_crs, CrsKind};
use plenora_kernels_geo::extended::{
    geodesic_distance_m, geodesic_line_length_m, haversine_distance_m,
};
use plenora_kernels_geo::extended_algorithms::{
    geodesic_area_m2, geodesic_bearing_degrees, ExtendedAlgorithmError,
};
use plenora_kernels_geo::geodetica::EllissoideGeodetico;
use wkt::TryFromWkt;

const DISTANZE: &str = include_str!("fixtures/geodetica/distanze.csv");
const AREE: &str = include_str!("fixtures/geodetica/aree.csv");

/// Scarto ammesso sulle distanze e sulle lunghezze: un micrometro, quattro
/// ordini sotto la precisione di 1 cm (le due implementazioni di Karney
/// concordano ai nanometri).
const SCARTO_M: f64 = 1e-6;
/// Scarto ammesso sull'azimut, come spostamento laterale alla distanza
/// della destinazione: un micrometro. Fra punti a pochi millimetri l'azimut
/// e' mal condizionato (1e-9 m di posizione sono gradi di direzione), e lo
/// scarto in gradi non direbbe nulla.
const SCARTO_LATERALE_M: f64 = 1e-6;

fn ellissoide(crs: &str) -> EllissoideGeodetico {
    EllissoideGeodetico::da_crs(&resolve_crs(crs, "crs").expect("crs integrato"))
        .expect("ellissoide del datum")
}

fn numero(testo: &str) -> f64 {
    testo.parse().expect("numero")
}

/// Ogni CRS geografico della tabella ha i suoi riferimenti: un CRS nuovo
/// senza riferimenti ferma il test (rigenerare le fixture).
#[test]
fn ogni_crs_geografico_della_tabella_ha_riferimenti() {
    let geografici: BTreeSet<String> = builtin_crs_identifiers()
        .filter(|id| resolve_crs(id, "crs").is_ok_and(|crs| crs.kind() == CrsKind::Geographic))
        .collect();
    let con_distanze: BTreeSet<String> = DISTANZE
        .lines()
        .skip(1)
        .filter_map(|riga| riga.split(',').next().map(str::to_owned))
        .collect();
    let con_aree: BTreeSet<String> = AREE
        .lines()
        .skip(1)
        .filter_map(|riga| riga.split(',').next().map(str::to_owned))
        .collect();
    assert!(!geografici.is_empty());
    assert_eq!(geografici, con_distanze);
    assert_eq!(geografici, con_aree);
}

#[test]
fn distanze_azimut_e_sfera_contro_geographiclib() {
    let mut righe = 0;
    for riga in DISTANZE.lines().skip(1) {
        let campi: Vec<&str> = riga.split(',').collect();
        let [crs, lon1, lat1, lon2, lat2, geodetica, azimut, sfera] = campi.as_slice() else {
            panic!("riga malformata");
        };
        let e = ellissoide(crs);
        let da = Point::new(numero(lon1), numero(lat1));
        let a = Point::new(numero(lon2), numero(lat2));

        let calcolata = geodesic_distance_m(da, a, &e).expect("distanza");
        assert!(
            (calcolata - numero(geodetica)).abs() <= SCARTO_M,
            "{crs} {da:?} {a:?}: geodetica {calcolata} contro {geodetica}"
        );
        let calcolata = haversine_distance_m(da, a, &e).expect("sfera");
        assert!(
            (calcolata - numero(sfera)).abs() <= SCARTO_M,
            "{crs} {da:?} {a:?}: sfera {calcolata} contro {sfera}"
        );
        let esito_azimut = geodesic_bearing_degrees(da, a, &e);
        if *azimut == "indefinito" {
            assert!(
                matches!(
                    esito_azimut,
                    Err(ExtendedAlgorithmError::AzimutNonDefinito(_))
                ),
                "{crs} {da:?} {a:?}: azimut {esito_azimut:?} dove non e' definito"
            );
        } else {
            let gradi = esito_azimut.expect("azimut");
            let atteso = numero(azimut);
            let scarto = (gradi - atteso).rem_euclid(360.0);
            let laterale = scarto.min(360.0 - scarto).to_radians() * numero(geodetica);
            assert!(
                laterale <= SCARTO_LATERALE_M,
                "{crs} {da:?} {a:?}: azimut {gradi} contro {atteso}"
            );
            assert!((0.0..360.0).contains(&gradi));
        }
        righe += 1;
    }
    assert!(righe > 300);
}

/// Le righe di `aree.csv`: CRS, tipo, WKT (tra virgolette), valore.
fn righe_aree() -> impl Iterator<Item = (&'static str, &'static str, &'static str, f64)> {
    AREE.lines().skip(1).map(|riga| {
        let (crs, resto) = riga.split_once(',').expect("crs");
        let (tipo, resto) = resto.split_once(",\"").expect("tipo");
        let (wkt, valore) = resto.split_once("\",").expect("wkt");
        (crs, tipo, wkt, numero(valore))
    })
}

#[test]
fn aree_e_lunghezze_contro_geographiclib() {
    let mut aree = 0;
    for (crs, tipo, testo, atteso) in righe_aree() {
        let e = ellissoide(crs);
        let geometria = Geometry::<f64>::try_from_wkt_str(testo).expect("wkt");
        match tipo {
            "area" => {
                let calcolata = geodesic_area_m2(&geometria, &e).expect("area");
                // L'area somma senza la somma compensata di GeographicLib
                // (feature `accurate` spenta, come in `geo`): le aree
                // parziali dei lati sono dell'ordine di larghezza per
                // distanza dall'equatore, e la cancellazione lascia scarti
                // di 1e-5 m^2 (O2 di Londra, 78 596 m^2). Si ammette un
                // millesimo di metro quadrato, molto sotto l'effetto di 1 cm
                // sul perimetro (metri quadrati).
                let ammesso = (atteso.abs() * 1e-12).max(1e-3);
                assert!(
                    (calcolata - atteso).abs() <= ammesso,
                    "{crs} {testo}: area {calcolata} contro {atteso}"
                );
                aree += 1;
            }
            "lunghezza" => {
                let Geometry::LineString(linea) = geometria else {
                    panic!("linea attesa");
                };
                let calcolata = geodesic_line_length_m(&linea, &e).expect("lunghezza");
                assert!(
                    (calcolata - atteso).abs() <= SCARTO_M,
                    "{crs} {testo}: lunghezza {calcolata} contro {atteso}"
                );
            }
            altro => panic!("tipo sconosciuto {altro}"),
        }
    }
    assert!(aree > 50);
}

/// Il difetto: prima della correzione la distanza di ED50 era quella di
/// WGS 84. Ora le due differiscono di metri su 100 km, come gli
/// ellissoidi.
#[test]
fn ed50_e_wgs84_non_danno_la_stessa_distanza() {
    let da = Point::new(9.0, 45.0);
    let a = Point::new(10.0, 45.8);
    let wgs84 = geodesic_distance_m(da, a, &ellissoide("EPSG:4326")).expect("wgs84");
    let ed50 = geodesic_distance_m(da, a, &ellissoide("EPSG:4230")).expect("ed50");
    assert!((ed50 - wgs84) > 3.0, "{ed50} {wgs84}");
}
