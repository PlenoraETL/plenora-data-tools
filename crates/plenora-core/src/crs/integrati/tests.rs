//! Invarianti della tabella dei CRS integrati e contratto del risolutore.
//!
//! I punti proiettati dei casi «ISTAT» sono calcolati con PROJ 9.5.1
//! (conversione dal CRS geografico di base, `always_xy`): servono come
//! coordinate realistiche, non come oracolo di una proiezione che qui non
//! c'e'.

// I confronti esatti fra f64 qui sono voluti: la tabella e' generata e i
// valori attesi sono letterali della stessa tabella o costruzioni esatte.
#![allow(clippy::float_cmp)]

use super::super::epsg_integrati::{CRS84, EPSG, VERSIONE_EPSG};
use super::super::{
    builtin_crs_identifiers, resolve_crs, validate_geometry_domain, validate_requirement,
    CoordinateDomainViolation, CrsError, CrsKind, ResolvedCrs, BUILTIN_EPSG_VERSION,
    METRES_PER_DEGREE_AT_EQUATOR,
};
use super::{cerca, identificativo, Identificativo};
use crate::catalog::CrsRequirement;
use crate::contract::AxisOrder;
use crate::error::PlenoraError;

fn risolto(definizione: &str) -> ResolvedCrs {
    resolve_crs(definizione, "crs").expect("CRS della tabella")
}

fn dominio_ok(crs: &ResolvedCrs, x: f64, y: f64) -> bool {
    validate_geometry_domain([(x, y)].into_iter(), crs).is_ok()
}

fn violazione(crs: &ResolvedCrs, x: f64, y: f64) -> Option<CoordinateDomainViolation> {
    match validate_geometry_domain([(x, y)].into_iter(), crs) {
        Ok(()) => None,
        Err(CrsError::CoordinateOutOfDomain { violation }) => Some(violation),
        Err(altro) => panic!("errore inatteso: {altro}"),
    }
}

// ---------------------------------------------------------------------------
// Tabella
// ---------------------------------------------------------------------------

#[test]
fn la_tabella_e_ordinata_e_senza_doppioni() {
    assert_eq!(CRS84.identificativo, Identificativo::OgcCrs84);
    let codici: Vec<u32> = EPSG
        .iter()
        .map(|voce| match voce.identificativo {
            Identificativo::Epsg(codice) => codice,
            Identificativo::OgcCrs84 => panic!("OGC:CRS84 fra le righe EPSG"),
        })
        .collect();
    assert!(
        codici.windows(2).all(|coppia| coppia[0] < coppia[1]),
        "codici non strettamente crescenti: la ricerca binaria non vale"
    );
    assert_eq!(BUILTIN_EPSG_VERSION, VERSIONE_EPSG);
}

#[test]
fn la_tabella_contiene_gli_insiemi_dichiarati() {
    let mut attesi: Vec<u32> = vec![
        // Italia.
        4265, 3003, 3004, 4670, 3064, 3065, 6706, 6707, 6708, 6709, 7791, 7792, 7793, 6875, 7794,
        4230, 23032, 23033, 23034, // Mondo.
        4326, 3857, 3395, 4258, 3035, 4269, 4267, 4283, 7844, 4171, 2154, 4277, 27700, 4674, 4490,
        2056, 31467, 28992, 2193,
    ];
    attesi.extend(32601..=32660);
    attesi.extend(32701..=32760);
    attesi.extend(25828..=25837);
    attesi.sort_unstable();
    let presenti: Vec<u32> = EPSG
        .iter()
        .filter_map(|voce| match voce.identificativo {
            Identificativo::Epsg(codice) => Some(codice),
            Identificativo::OgcCrs84 => None,
        })
        .collect();
    assert_eq!(presenti, attesi);
    // 25838 e' deprecato nel registro: resta fuori.
    assert!(cerca(Identificativo::Epsg(25838)).is_none());
    assert_eq!(builtin_crs_identifiers().count(), EPSG.len() + 1);
}

#[test]
fn nomi_e_assi_dei_crs_italiani_come_nel_registro() {
    let attesi = [
        (
            3003,
            "Monte Mario / Italy zone 1",
            AxisOrder::EastingNorthing,
        ),
        (
            3004,
            "Monte Mario / Italy zone 2",
            AxisOrder::EastingNorthing,
        ),
        (6706, "RDN2008", AxisOrder::LatLon),
        (
            6707,
            "RDN2008 / UTM zone 32N (N-E)",
            AxisOrder::NorthingEasting,
        ),
        (
            6708,
            "RDN2008 / UTM zone 33N (N-E)",
            AxisOrder::NorthingEasting,
        ),
        (
            6709,
            "RDN2008 / UTM zone 34N (N-E)",
            AxisOrder::NorthingEasting,
        ),
        (7791, "RDN2008 / UTM zone 32N", AxisOrder::EastingNorthing),
        (7792, "RDN2008 / UTM zone 33N", AxisOrder::EastingNorthing),
        (7793, "RDN2008 / UTM zone 34N", AxisOrder::EastingNorthing),
        (
            6875,
            "RDN2008 / Italy zone (N-E)",
            AxisOrder::NorthingEasting,
        ),
        (
            7794,
            "RDN2008 / Italy zone (E-N)",
            AxisOrder::EastingNorthing,
        ),
        (4265, "Monte Mario", AxisOrder::LatLon),
        (25832, "ETRS89 / UTM zone 32N", AxisOrder::EastingNorthing),
        (
            3035,
            "ETRS89-extended / LAEA Europe",
            AxisOrder::NorthingEasting,
        ),
        (
            31467,
            "DHDN / 3-degree Gauss-Kruger zone 3",
            AxisOrder::NorthingEasting,
        ),
        (
            2193,
            "NZGD2000 / New Zealand Transverse Mercator 2000",
            AxisOrder::NorthingEasting,
        ),
    ];
    for (codice, nome, assi) in attesi {
        let voce = cerca(Identificativo::Epsg(codice)).expect("codice in tabella");
        assert_eq!(voce.nome, nome, "{codice}");
        let crs = risolto(&format!("EPSG:{codice}"));
        assert_eq!(crs.authority_axis_order(), Some(assi), "{codice}");
    }
}

#[test]
fn ogni_voce_si_risolve_con_metadati_coerenti() {
    for testo in builtin_crs_identifiers() {
        let crs = risolto(&testo);
        assert_eq!(crs.definition(), testo);
        let area = crs.area_of_use().expect("area d'uso");
        let geo = area.geographic;
        assert!(
            (-90.0..=90.0).contains(&geo.south_latitude)
                && (-90.0..=90.0).contains(&geo.north_latitude)
                && geo.south_latitude < geo.north_latitude
                && (-180.0..=180.0).contains(&geo.west_longitude)
                && (-180.0..=180.0).contains(&geo.east_longitude),
            "{testo}"
        );
        let ellissoide = crs.ellipsoid().expect("ellissoide");
        assert!(
            (6_377_000.0..6_379_000.0).contains(&ellissoide.semi_major_axis_metre),
            "{testo}"
        );
        match crs.kind() {
            CrsKind::Geographic => {
                assert_eq!(crs.horizontal_unit_to_metre(), None, "{testo}");
                assert!(area.projected.is_none(), "{testo}");
                assert!(crs.validity_domain().is_none(), "{testo}");
                assert!(
                    matches!(
                        crs.authority_axis_order(),
                        Some(AxisOrder::LatLon | AxisOrder::LonLat)
                    ),
                    "{testo}"
                );
                validate_requirement(CrsRequirement::Geographic, &[&crs]).expect("geografico");
            }
            CrsKind::Projected => {
                assert_eq!(crs.horizontal_unit_to_metre(), Some(1.0), "{testo}");
                assert_eq!(crs.precisione_coordinate(), Some(0.01), "{testo}");
                validate_requirement(CrsRequirement::Projected, &[&crs]).expect("proiettato");
                let stretta = area.projected.expect("area proiettata");
                let dominio = crs.validity_domain().expect("dominio");
                // Il dominio contiene l'area d'uso: un dato dentro l'area
                // EPSG non e' mai rifiutato.
                assert!(
                    dominio.min_easting <= stretta.min_easting
                        && dominio.min_northing <= stretta.min_northing
                        && dominio.max_easting >= stretta.max_easting
                        && dominio.max_northing >= stretta.max_northing,
                    "{testo}: area d'uso fuori dal dominio"
                );
                assert!(
                    stretta.min_easting < stretta.max_easting
                        && stretta.min_northing < stretta.max_northing,
                    "{testo}"
                );
                for (x, y) in [
                    (dominio.min_easting, dominio.min_northing),
                    (dominio.max_easting, dominio.max_northing),
                    (stretta.min_easting, stretta.max_northing),
                ] {
                    assert!(dominio_ok(&crs, x, y), "{testo}: bordo rifiutato");
                }
                assert!(
                    matches!(
                        crs.authority_axis_order(),
                        Some(AxisOrder::EastingNorthing | AxisOrder::NorthingEasting)
                    ),
                    "{testo}"
                );
            }
        }
        match identificativo(&testo).expect("identificativo") {
            Identificativo::Epsg(codice) => {
                assert_eq!(
                    crs.authority_identifier(),
                    Some(("EPSG", codice)),
                    "{testo}"
                );
            }
            Identificativo::OgcCrs84 => assert_eq!(crs.authority_srid(), None),
        }
    }
}

/// Oracolo programmatico dei fusi UTM WGS 84: nome, area d'uso a 6 gradi
/// (il registro ha uno scarto di un centesimo su 32629) e dominio con la
/// stessa regola per tutti i fusi dello stesso emisfero, simmetrico attorno
/// al falso est.
#[test]
fn i_fusi_utm_wgs84_seguono_la_regola_dei_fusi() {
    let nord = cerca(Identificativo::Epsg(32632))
        .and_then(|voce| voce.dominio)
        .expect("dominio 32632");
    let sud = cerca(Identificativo::Epsg(32732))
        .and_then(|voce| voce.dominio)
        .expect("dominio 32732");
    for fuso in 1..=60_u32 {
        for (base, emisfero, dominio) in [(32600, 'N', nord), (32700, 'S', sud)] {
            let voce = cerca(Identificativo::Epsg(base + fuso)).expect("fuso in tabella");
            assert_eq!(voce.nome, format!("WGS 84 / UTM zone {fuso}{emisfero}"));
            let ovest = 6.0_f64.mul_add(f64::from(fuso - 1), -180.0);
            let (sud_atteso, nord_atteso) = if emisfero == 'N' {
                (0.0, 84.0)
            } else {
                (-80.0, 0.0)
            };
            let area = voce.area.geographic;
            for (valore, atteso) in [
                (area.west_longitude, ovest),
                (area.east_longitude, ovest + 6.0),
                (area.south_latitude, sud_atteso),
                (area.north_latitude, nord_atteso),
            ] {
                assert!((valore - atteso).abs() <= 0.01 + 1e-9, "{}", voce.nome);
            }
            // Stessa regola per tutti i fusi: eastings e bordo equatoriale
            // identici; il bordo polare si allarga solo per contenere
            // un'area d'uso del registro che supera 84 gradi (32629).
            let proprio = voce.dominio.expect("dominio");
            assert_eq!(proprio.min_easting, dominio.min_easting, "{}", voce.nome);
            assert_eq!(proprio.max_easting, dominio.max_easting, "{}", voce.nome);
            if emisfero == 'N' {
                assert_eq!(proprio.min_northing, dominio.min_northing, "{}", voce.nome);
                assert!(
                    proprio.max_northing >= dominio.max_northing,
                    "{}",
                    voce.nome
                );
                assert_eq!(
                    proprio.max_northing == dominio.max_northing,
                    area.north_latitude <= 84.0,
                    "{}",
                    voce.nome
                );
            } else {
                assert_eq!(proprio, dominio, "{}", voce.nome);
            }
        }
    }
    assert!((nord.min_easting + nord.max_easting - 1_000_000.0).abs() <= 0.002);
    // L'equatore e' esatto (northing 0 e 1e7): la guardia del generatore
    // lo sposta di un millimetro verso l'esterno.
    assert_eq!(nord.min_northing, -0.001);
    assert_eq!(sud.max_northing, 10_000_000.001);
    // +/- 15 gradi all'equatore: circa 1670 km dal meridiano centrale.
    assert!((1_688_000.0..1_689_000.0).contains(&(500_000.0 - nord.min_easting)));
}

// ---------------------------------------------------------------------------
// Risolutore
// ---------------------------------------------------------------------------

#[test]
fn forme_equivalenti_sono_semanticamente_uguali() {
    let base = risolto("EPSG:4326");
    for forma in [
        "epsg:4326",
        "urn:ogc:def:crs:EPSG::4326",
        "urn:ogc:def:crs:EPSG:9.9.1:4326",
        "URN:OGC:DEF:CRS:EPSG::4326",
    ] {
        let altro = risolto(forma);
        assert!(base.semantically_equals(&altro), "{forma}");
        assert_eq!(
            altro.definition(),
            forma,
            "la definizione resta quella data"
        );
        assert_eq!(altro.authority_srid(), Some(4326));
    }
    let crs84 = risolto("OGC:CRS84");
    for forma in [
        "urn:ogc:def:crs:OGC:1.3:CRS84",
        "urn:ogc:def:crs:OGC::CRS84",
        "ogc:crs84",
    ] {
        assert!(crs84.semantically_equals(&risolto(forma)), "{forma}");
    }
    // Codici diversi, anche con la stessa proiezione, non sono uguali.
    for (sinistra, destra) in [
        ("EPSG:4326", "OGC:CRS84"),
        ("EPSG:32632", "EPSG:32633"),
        ("EPSG:6707", "EPSG:7791"),
        ("EPSG:32632", "EPSG:25832"),
        ("EPSG:4258", "EPSG:6706"),
    ] {
        assert!(
            !risolto(sinistra).semantically_equals(&risolto(destra)),
            "{sinistra} {destra}"
        );
    }
    let utm = risolto("EPSG:32632");
    validate_requirement(
        CrsRequirement::SameProjected,
        &[&utm, &risolto("urn:ogc:def:crs:EPSG::32632")],
    )
    .expect("stesso CRS");
    assert!(matches!(
        validate_requirement(
            CrsRequirement::SameProjected,
            &[&utm, &risolto("EPSG:32633")]
        ),
        Err(CrsError::Mismatch)
    ));
}

#[test]
fn ordine_degli_assi_4326_contro_crs84() {
    let epsg = risolto("EPSG:4326");
    let crs84 = risolto("OGC:CRS84");
    assert_eq!(epsg.authority_axis_order(), Some(AxisOrder::LatLon));
    assert_eq!(crs84.authority_axis_order(), Some(AxisOrder::LonLat));
    // L'ordine delle coordinate in ingresso resta quello GIS normalizzato.
    assert_eq!(epsg.normalized_gis_axis_order(), AxisOrder::LonLat);
    assert_eq!(epsg.authority_srid(), Some(4326));
    assert_eq!(crs84.authority_srid(), None);
}

#[test]
fn unita_e_precisione() {
    let utm = risolto("EPSG:32632");
    assert_eq!(utm.kind(), CrsKind::Projected);
    assert_eq!(utm.horizontal_unit_to_metre(), Some(1.0));
    assert_eq!(utm.precisione_coordinate(), Some(0.01));
    let geografico = risolto("EPSG:4326");
    assert_eq!(geografico.kind(), CrsKind::Geographic);
    assert_eq!(geografico.horizontal_unit_to_metre(), None);
    assert_eq!(
        geografico.precisione_coordinate(),
        Some(0.01 / METRES_PER_DEGREE_AT_EQUATOR)
    );
    let piedi = ResolvedCrs::from_resolved_parts(
        "X:1".to_owned(),
        serde_json::json!({"type": "ProjectedCRS"}),
        CrsKind::Projected,
        Some(0.3048),
    );
    assert_eq!(piedi.precisione_coordinate(), Some(0.01 / 0.3048));
    let senza_unita = ResolvedCrs::from_resolved_parts(
        "X:1".to_owned(),
        serde_json::json!({"type": "ProjectedCRS"}),
        CrsKind::Projected,
        None,
    );
    assert_eq!(senza_unita.precisione_coordinate(), None);
}

#[test]
fn codici_sconosciuti_e_forme_non_riconosciute_falliscono_chiuse() {
    for definizione in [
        "EPSG:99999",
        "EPSG:25838",
        "EPSG:4979",
        "EPSG:04326",
        "EPSG:4326.0",
        "ESRI:102100",
        "IGNF:LAMB93",
        "OGC:CRS83",
        "urn:ogc:def:crs:EPSG:v1:4326",
        "urn:ogc:def:crs:EPSG:.:4326",
        "urn:ogc:def:crs:EPSG:9..1:4326",
        "urn:ogc:def:crs:EPSG:9.:4326",
        "urn:ogc:def:crs:ESRI::102100",
        "urn:ogc:def:crs:EPSG::4326:extra",
        "http://www.opengis.net/def/crs/EPSG/0/4326",
    ] {
        let errore = resolve_crs(definizione, "crs").expect_err(definizione);
        assert!(
            matches!(errore, CrsError::NotBuiltin),
            "{definizione}: {errore}"
        );
        let testo = PlenoraError::from(errore).to_string();
        assert!(testo.contains("CRS_NOT_BUILTIN"), "{testo}");
        assert!(
            !testo.contains(definizione),
            "la definizione nel messaggio: {testo}"
        );
    }
    // Con uno spazio la forma non e' un identificatore d'autorita'.
    assert!(matches!(
        resolve_crs("EPSG:4326 ", "crs"),
        Err(CrsError::BackendUnavailable)
    ));
}

// ---------------------------------------------------------------------------
// Dominio di validita'
// ---------------------------------------------------------------------------

#[test]
fn geografico_mondo_intero_e_area_epsg_solo_metadato() {
    let wgs84 = risolto("EPSG:4326");
    assert!(dominio_ok(&wgs84, -180.0, -90.0));
    assert!(dominio_ok(&wgs84, 180.0, 90.0));
    assert_eq!(
        violazione(&wgs84, 180.0_f64.next_up(), 0.0),
        Some(CoordinateDomainViolation::LongitudeOutOfRange)
    );
    assert_eq!(
        violazione(&wgs84, 0.0, (-90.0_f64).next_down()),
        Some(CoordinateDomainViolation::LatitudeOutOfRange)
    );
    // Metri UTM dati a un CRS geografico.
    assert_eq!(
        violazione(&wgs84, 1_312_068.675, 4_482_531.752),
        Some(CoordinateDomainViolation::LongitudeOutOfRange)
    );
    // Monte Mario ha l'area d'uso EPSG sull'Italia, ma il dominio resta il
    // mondo: l'area d'uso non rifiuta dati.
    let monte_mario = risolto("EPSG:4265");
    let area = monte_mario.area_of_use().expect("area").geographic;
    assert!(!area.contains(100.0, 10.0));
    assert!(dominio_ok(&monte_mario, 100.0, 10.0));
}

#[test]
fn area_d_uso_che_attraversa_l_antimeridiano() {
    let nad83 = risolto("EPSG:4269").area_of_use().expect("area").geographic;
    assert!(nad83.crosses_antimeridian());
    assert!(nad83.contains(170.0, 50.0));
    assert!(nad83.contains(-100.0, 40.0));
    assert!(!nad83.contains(0.0, 40.0));
    assert!(!nad83.contains(-100.0, 0.0));
}

/// Caso ISTAT: i confini amministrativi di tutta l'Italia pubblicati in UTM
/// 32N arrivano a circa 9.5 gradi dal meridiano centrale. Il dominio li
/// accetta, anche fuori dall'area d'uso EPSG del fuso (6-12 gradi est).
#[test]
fn caso_istat_tutta_italia_in_un_fuso_utm() {
    // (lon, lat) -> (easting, northing) con PROJ 9.5.1.
    let punti_utm = [
        (18.52, 40.10, 1_312_068.675, 4_482_531.752), // Otranto
        (18.5, 40.0, 1_311_558.030, 4_471_222.562),
        (12.6, 35.5, 826_565.661, 3_934_455.702), // Lampedusa
        (6.63, 45.1, 313_533.063, 4_996_791.752), // confine occidentale
    ];
    for codice in [
        "EPSG:32632",
        "EPSG:25832",
        "EPSG:6707",
        "EPSG:7791",
        "EPSG:23032",
    ] {
        let crs = risolto(codice);
        for (lon, lat, easting, northing) in punti_utm {
            assert!(dominio_ok(&crs, easting, northing), "{codice} {lon} {lat}");
        }
        let fuori_area = crs.area_of_use().expect("area").geographic;
        assert!(
            !fuori_area.contains(18.52, 40.10),
            "{codice}: area d'uso stretta"
        );
    }
    // Gauss-Boaga fuso Ovest (meridiano centrale 9, falso est 1500 km).
    let gauss_boaga = risolto("EPSG:3003");
    for (easting, northing) in [
        (2_312_105.447, 4_482_609.951),
        (2_311_594.760, 4_471_300.435),
        (1_826_580.076, 3_934_518.472),
        (1_313_524.397, 4_996_884.959),
    ] {
        assert!(dominio_ok(&gauss_boaga, easting, northing));
    }
}

#[test]
fn coordinate_sbagliate_per_il_crs_sono_rifiutate() {
    // Gradi dati a Gauss-Boaga: easting e northing di pochi metri.
    let gauss_boaga = risolto("EPSG:3003");
    assert!(violazione(&gauss_boaga, 12.5, 41.9).is_some());
    // Gradi dati a un fuso UTM sud: northing sotto il dominio.
    let utm_sud = risolto("EPSG:32733");
    assert_eq!(
        violazione(&utm_sud, 15.0, -4.3),
        Some(CoordinateDomainViolation::NorthingOutOfValidityDomain)
    );
    // Gradi dati a Lambert-93.
    let lambert = risolto("EPSG:2154");
    assert!(violazione(&lambert, 2.35, 48.85).is_some());
    assert!(dominio_ok(&lambert, 652_301.565, 6_861_302.726)); // Parigi
                                                               // Metri UTM di un altro emisfero.
    let utm_nord = risolto("EPSG:32632");
    assert_eq!(
        violazione(&utm_nord, 500_000.0, 10_000_000.0),
        Some(CoordinateDomainViolation::NorthingOutOfValidityDomain)
    );
    assert_eq!(
        violazione(&utm_nord, 5_000_000.0, 4_000_000.0),
        Some(CoordinateDomainViolation::EastingOutOfValidityDomain)
    );
    // Limite dichiarato: gradi dati a un fuso UTM nord sono un punto vicino
    // all'equatore dentro il dominio, e passano. Non e' decidibile dalle
    // sole coordinate.
    assert!(dominio_ok(&utm_nord, 12.5, 41.9));
}

#[test]
fn bordi_del_dominio_utm_e_gauss_boaga() {
    for codice in ["EPSG:32632", "EPSG:3003", "EPSG:32733"] {
        let crs = risolto(codice);
        let dominio = crs.validity_domain().expect("dominio");
        let centro_n = f64::midpoint(dominio.min_northing, dominio.max_northing);
        let centro_e = f64::midpoint(dominio.min_easting, dominio.max_easting);
        assert!(dominio_ok(&crs, dominio.min_easting, centro_n), "{codice}");
        assert!(dominio_ok(&crs, dominio.max_easting, centro_n), "{codice}");
        assert!(dominio_ok(&crs, centro_e, dominio.min_northing), "{codice}");
        assert!(dominio_ok(&crs, centro_e, dominio.max_northing), "{codice}");
        assert_eq!(
            violazione(&crs, dominio.min_easting.next_down(), centro_n),
            Some(CoordinateDomainViolation::EastingOutOfValidityDomain),
            "{codice}"
        );
        assert_eq!(
            violazione(&crs, dominio.max_easting.next_up(), centro_n),
            Some(CoordinateDomainViolation::EastingOutOfValidityDomain),
            "{codice}"
        );
        assert_eq!(
            violazione(&crs, centro_e, dominio.min_northing.next_down()),
            Some(CoordinateDomainViolation::NorthingOutOfValidityDomain),
            "{codice}"
        );
        assert_eq!(
            violazione(&crs, centro_e, dominio.max_northing.next_up()),
            Some(CoordinateDomainViolation::NorthingOutOfValidityDomain),
            "{codice}"
        );
        assert_eq!(
            violazione(&crs, f64::NAN, centro_n),
            Some(CoordinateDomainViolation::NonFinite)
        );
    }
}

/// Web Mercator: latitudine entro 85.06 gradi, x entro pi volte il semiasse.
#[test]
fn web_mercator_a_85_06_gradi() {
    let crs = risolto("EPSG:3857");
    let dominio = crs.validity_domain().expect("dominio");
    // y di 85.06 gradi sulla sfera di WGS 84, arrotondato al millimetro
    // verso l'esterno: 6378137 * ln(tan(pi/4 + phi/2)).
    let fi = 85.06_f64.to_radians();
    let y = 6_378_137.0 * (std::f64::consts::FRAC_PI_4 + fi / 2.0).tan().ln();
    assert!((dominio.max_northing - y).abs() < 0.002, "{y}");
    assert_eq!(dominio.min_northing, -dominio.max_northing);
    let x = std::f64::consts::PI * 6_378_137.0;
    assert!((dominio.max_easting - x).abs() < 0.002);
    assert!(dominio_ok(&crs, x, y));
    assert!(dominio_ok(&crs, -x, -y));
    assert_eq!(
        violazione(&crs, 0.0, dominio.max_northing + 1.0),
        Some(CoordinateDomainViolation::NorthingOutOfValidityDomain)
    );
    assert_eq!(
        violazione(&crs, -x - 1.0, 0.0),
        Some(CoordinateDomainViolation::EastingOutOfValidityDomain)
    );
    // L'area d'uso EPSG di 3857 e' gia' quella del dominio.
    assert_eq!(
        crs.area_of_use().and_then(|area| area.projected),
        Some(dominio)
    );
}

#[test]
fn i_messaggi_del_dominio_proiettato_non_riportano_la_coordinata() {
    let crs = risolto("EPSG:32632");
    for (x, y) in [(5_123_456.5, 4_000_000.0), (500_000.0, 12_345_678.25)] {
        let errore = validate_geometry_domain([(x, y)].into_iter(), &crs).expect_err("fuori");
        let testo = PlenoraError::from(errore).to_string();
        for numero in [x, y] {
            for forma in [format!("{numero}"), format!("{numero:?}")] {
                assert!(!testo.contains(&forma), "coordinata nel messaggio: {testo}");
            }
        }
        assert!(testo.contains("COORDINATE_OUT_OF_CRS_DOMAIN"), "{testo}");
    }
}
