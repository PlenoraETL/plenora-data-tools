//! Prove del kernel `geo.reproject`: tipi e struttura preservati,
//! densificazione entro la precisione, percorso unico per geometria, area
//! d'uso, domini, identita', griglie, tabelle con null.

// Nomi brevi delle formule (punti, lati) e confronti esatti voluti
// (accuratezze del registro).
#![allow(clippy::many_single_char_names, clippy::float_cmp)]

use std::collections::{BTreeMap, HashMap};

use geo::{coord, line_string, point, polygon, Coord, CoordsIter, Geometry, LineString};
use geozero::{CoordDimensions, ToWkb};
use plenora_core::arrow::array::{Array, BinaryArray, Int64Array};
use plenora_core::contract::arrow_metadata::{
    geo_metadata_json_with_dimensions, GEOARROW_EXTENSION_KEY, GEOARROW_WKB_EXTENSION,
};
use plenora_core::crs::riproiezione::PercorsoDatum;
use serde_json::json;

use super::*;

fn crs(definizione: &str) -> ResolvedCrs {
    resolve_crs(definizione, "crs").expect("CRS integrato")
}

fn params(sorgente: &str, config: &Value) -> ReprojectParams {
    ReprojectParams::da_config("geo.reproject", config, &crs(sorgente)).expect("config")
}

fn riproiettore(sorgente: &str, config: &Value) -> Riproiettore {
    params(sorgente, config)
        .riproiettore()
        .expect("riproiettore")
}

fn tolleranza(target: &str) -> f64 {
    crs(target).precisione_coordinate().expect("precisione") / 2.0
}

fn griglia_sintetica() -> String {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../plenora-core/tests/fixtures/riproiezione/griglia_sintetica.gsb")
        .to_string_lossy()
        .into_owned()
}

/// Distanza massima dell'immagine esatta di `n` punti del lato sorgente
/// dalla polilinea d'uscita.
fn scarto_massimo(
    r: &Riproiettore,
    sorgente: &LineString<f64>,
    uscita: &LineString<f64>,
    n: u32,
) -> f64 {
    let mut massimo: f64 = 0.0;
    for lato in sorgente.lines() {
        for i in 0..=n {
            let t = f64::from(i) / f64::from(n);
            let p = interpola(lato.start, lato.end, t);
            let (x, y) = r.trasforma(0, p.x, p.y).expect("punto").expect("area");
            let immagine = Coord { x, y };
            let distanza = uscita
                .lines()
                .map(|l| distanza_dal_segmento(immagine, l.start, l.end))
                .fold(f64::INFINITY, f64::min);
            massimo = massimo.max(distanza);
        }
    }
    massimo
}

#[test]
fn tipi_e_struttura_si_conservano() {
    let r = riproiettore("EPSG:4326", &json!({"target_crs": "EPSG:32632"}));
    let t = tolleranza("EPSG:32632");
    let poligono = polygon![
        exterior: [(x: 8.0, y: 44.0), (x: 10.0, y: 44.0), (x: 10.0, y: 46.0), (x: 8.0, y: 46.0), (x: 8.0, y: 44.0)],
        interiors: [[(x: 8.5, y: 44.5), (x: 8.5, y: 45.5), (x: 9.5, y: 45.5), (x: 9.5, y: 44.5), (x: 8.5, y: 44.5)]],
    ];
    let ingressi = vec![
        Geometry::Point(point!(x: 9.0, y: 45.0)),
        Geometry::MultiPoint(vec![point!(x: 9.0, y: 45.0), point!(x: 10.0, y: 46.0)].into()),
        Geometry::LineString(line_string![(x: 8.0, y: 44.0), (x: 11.0, y: 47.0)]),
        Geometry::MultiLineString(geo::MultiLineString(vec![
            line_string![(x: 8.0, y: 44.0), (x: 9.0, y: 45.0)],
        ])),
        Geometry::Polygon(poligono.clone()),
        Geometry::MultiPolygon(vec![poligono.clone()].into()),
        Geometry::GeometryCollection(
            vec![
                Geometry::Point(point!(x: 9.0, y: 45.0)),
                Geometry::Polygon(poligono),
            ]
            .into(),
        ),
        Geometry::GeometryCollection(GeometryCollection(Vec::new())),
    ];
    for ingresso in &ingressi {
        let uscita = riproietta_geometria(ingresso, &r, t).expect("riproiezione");
        assert_eq!(
            crate::geometry_type_name(ingresso),
            crate::geometry_type_name(&uscita)
        );
        match (ingresso, &uscita) {
            (Geometry::Polygon(a), Geometry::Polygon(b)) => {
                assert_eq!(a.interiors().len(), b.interiors().len());
                assert_eq!(
                    b.exterior().0.first(),
                    b.exterior().0.last(),
                    "anello chiuso"
                );
            }
            (Geometry::MultiPoint(a), Geometry::MultiPoint(b)) => assert_eq!(a.0.len(), b.0.len()),
            (Geometry::GeometryCollection(a), Geometry::GeometryCollection(b)) => {
                assert_eq!(a.0.len(), b.0.len());
            }
            _ => {}
        }
        // Il punto: easting e northing plausibili per UTM 32N.
        if let Geometry::Point(p) = uscita {
            assert!((p.x() - 500_000.0).abs() < 1.0, "meridiano centrale 9 E");
            assert!((p.y() - 4_982_950.4).abs() < 1.0);
        }
    }
    // Le coordinate d'uscita sono quelle del riproiettore, vertice per
    // vertice (i punti aggiunti stanno fra i vertici).
    let linea = line_string![(x: 8.0, y: 44.0), (x: 11.0, y: 47.0)];
    let Geometry::LineString(uscita) =
        riproietta_geometria(&Geometry::LineString(linea.clone()), &r, t).expect("linea")
    else {
        panic!("tipo");
    };
    for estremo in [linea.0[0], linea.0[1]] {
        let (x, y) = r
            .trasforma(0, estremo.x, estremo.y)
            .expect("punto")
            .expect("area");
        assert!(uscita.0.contains(&coord! {x: x, y: y}));
    }
}

#[test]
fn i_lati_lunghi_si_densificano_entro_la_precisione() {
    // Un lato di 6 gradi lungo il parallelo 45 in lon/lat diventa una curva
    // in UTM 32N: senza densificazione la corda se ne scosta di metri.
    let r = riproiettore("EPSG:4326", &json!({"target_crs": "EPSG:32632"}));
    let t = tolleranza("EPSG:32632");
    let linea = line_string![(x: 6.0, y: 45.0), (x: 12.0, y: 45.0)];
    let Geometry::LineString(uscita) =
        riproietta_geometria(&Geometry::LineString(linea.clone()), &r, t).expect("linea")
    else {
        panic!("tipo");
    };
    assert!(uscita.0.len() > 2, "nessuna densificazione");
    // Controllo indipendente, molto piu' fitto dei tre campioni per lato del
    // kernel: resta entro la precisione (1 cm).
    let scarto = scarto_massimo(&r, &linea, &uscita, 2000);
    assert!(scarto <= 2.0 * t, "scarto {scarto} m");
    // Senza densificazione (la sola corda fra gli estremi) lo scarto e' di
    // metri: la prova vede davvero la curvatura.
    let corda = LineString::new(vec![uscita.0[0], *uscita.0.last().expect("ultimo")]);
    assert!(scarto_massimo(&r, &linea, &corda, 200) > 1.0);
}

#[test]
fn un_meridiano_in_mercator_resta_dritto_e_converge() {
    // In Pseudo Mercator un meridiano e' una retta, percorsa a velocita'
    // molto diversa (fattore di scala da 1 a 11 fra 0 e 85 gradi): la
    // densificazione converge e i punti stanno sulla retta.
    let r = riproiettore("EPSG:4326", &json!({"target_crs": "EPSG:3857"}));
    let t = tolleranza("EPSG:3857");
    let meridiano = line_string![(x: 10.0, y: 0.0), (x: 10.0, y: 85.0)];
    let Geometry::LineString(uscita) =
        riproietta_geometria(&Geometry::LineString(meridiano.clone()), &r, t).expect("meridiano")
    else {
        panic!("tipo");
    };
    let x0 = uscita.0[0].x;
    assert!(uscita.0.iter().all(|c| (c.x - x0).abs() < 1e-6));
    assert!(scarto_massimo(&r, &meridiano, &uscita, 2000) <= 2.0 * t);
    // Un parallelo in lon/lat verso Mercator: anche questo una retta.
    let parallelo = line_string![(x: -170.0, y: 60.0), (x: 170.0, y: 60.0)];
    let Geometry::LineString(uscita) =
        riproietta_geometria(&Geometry::LineString(parallelo), &r, t).expect("parallelo")
    else {
        panic!("tipo");
    };
    assert_eq!(uscita.0.len(), 2, "nessun punto in piu' su una retta");
}

#[test]
fn lo_stesso_crs_non_cambia_le_coordinate() {
    let r = riproiettore("EPSG:32632", &json!({"target_crs": "EPSG:32632"}));
    let poligono = Geometry::Polygon(polygon![
        (x: 500_000.123, y: 4_900_000.456), (x: 510_000.0, y: 4_900_000.0),
        (x: 510_000.0, y: 4_910_000.0), (x: 500_000.123, y: 4_900_000.456),
    ]);
    let uscita = riproietta_geometria(&poligono, &r, tolleranza("EPSG:32632")).expect("identita'");
    assert_eq!(uscita, poligono);
}

#[test]
fn l_antimeridiano_d_uscita_e_un_errore_esplicito() {
    // Una linea nel fuso UTM 1N che attraversa i 180 gradi: in lon/lat il
    // lato salta da +180 a -180 e non si approssima con una corda.
    let r = riproiettore("EPSG:32601", &json!({"target_crs": "EPSG:4326"}));
    let (x1, y1) = da_lonlat("EPSG:32601", 179.5, 50.0);
    let (x2, y2) = da_lonlat("EPSG:32601", -179.5, 50.0);
    let linea = Geometry::LineString(line_string![(x: x1, y: y1), (x: x2, y: y2)]);
    let errore = riproietta_geometria(&linea, &r, tolleranza("EPSG:4326")).expect_err("salto");
    assert!(
        errore
            .to_string()
            .contains("REPROJECTION_EDGE_NOT_CONVERGED"),
        "{errore}"
    );
}

fn da_lonlat(target: &str, lon: f64, lat: f64) -> (f64, f64) {
    riproiettore("EPSG:4326", &json!({"target_crs": target}))
        .trasforma(0, lon, lat)
        .expect("punto")
        .expect("area")
}

#[test]
fn una_geometria_non_mescola_i_percorsi_e_fuori_da_tutti_e_un_errore() {
    // Monte Mario -> WGS 84 con 4 m accettati: ammessi Monte Mario to WGS 84
    // (4), (2) e (3), per Italia continentale, Sardegna e Sicilia, in
    // ordine di area d'uso crescente (la Sardegna prima: il riquadro
    // continentale contiene anche la Sardegna).
    let config = json!({"target_crs": "EPSG:4326", "accuratezza_accettata_m": 4.0});
    let p = params("EPSG:4265", &config);
    let codici: Vec<Vec<u32>> = p
        .piano()
        .percorsi()
        .iter()
        .map(PercorsoDatum::codici)
        .collect();
    assert_eq!(
        codici,
        vec![
            vec![1662],
            vec![1664],
            vec![1660],
            vec![1661, 1149],
            vec![1663, 1149],
            vec![1659, 1149]
        ]
    );
    let r = p.riproiettore().expect("riproiettore");
    let t = tolleranza("EPSG:4326");
    let (roma, sardegna) = (coord! {x: 12.5, y: 41.9}, coord! {x: 9.0, y: 40.0});
    let per_percorso = |percorso: usize, c: Coord<f64>| {
        let (x, y) = r
            .trasforma(percorso, c.x, c.y)
            .expect("punto")
            .expect("area");
        coord! {x: x, y: y}
    };
    let uscita = |g: Geometry<f64>| riproietta_geometria(&g, &r, t).expect("riproiezione");
    // Da solo, ogni punto prende il suo percorso.
    assert_eq!(
        uscita(Geometry::Point(sardegna.into())),
        Geometry::Point(per_percorso(0, sardegna).into())
    );
    assert_eq!(
        uscita(Geometry::Point(roma.into())),
        Geometry::Point(per_percorso(2, roma).into())
    );
    // Una linea da Roma alla Sardegna sta solo nel riquadro continentale,
    // ma il vertice sardo preferisce la trasformazione della Sardegna (i
    // parametri continentali lo sposterebbero di metri): errore esplicito,
    // mai un risultato misto.
    assert_ne!(per_percorso(2, sardegna), per_percorso(0, sardegna));
    let mista = Geometry::LineString(LineString::new(vec![roma, sardegna]));
    let errore = riproietta_geometria(&mista, &r, t).expect_err("percorsi misti");
    assert!(
        errore
            .to_string()
            .contains("REPROJECTION_MIXED_TRANSFORMATION_AREAS"),
        "{errore}"
    );
    // Con il percorso fissato la stessa linea passa, tutta continentale.
    let fissato = riproiettore(
        "EPSG:4265",
        &json!({"target_crs": "EPSG:4326", "accuratezza_accettata_m": 4.0,
                "trasformazioni": [1660]}),
    );
    let Geometry::LineString(linea) = riproietta_geometria(&mista, &fissato, t).expect("fissato")
    else {
        panic!("tipo");
    };
    assert_eq!(linea.0.last(), Some(&per_percorso(2, sardegna)));
    // Fuori da ogni area d'uso: errore esplicito, mai un ripiego.
    let lontano = Geometry::LineString(line_string![(x: 12.5, y: 41.9), (x: 20.5, y: 41.9)]);
    let errore = riproietta_geometria(&lontano, &r, t).expect_err("fuori area");
    assert!(
        errore
            .to_string()
            .contains("REPROJECTION_OUTSIDE_TRANSFORMATION_AREA"),
        "{errore}"
    );
}

#[test]
fn fuori_dominio_si_rifiuta_senza_riportare_coordinate() {
    let r = riproiettore("EPSG:4326", &json!({"target_crs": "EPSG:3857"}));
    let polo = Geometry::Point(point!(x: 12.345_678, y: 88.765_432));
    let errore = riproietta_geometria(&polo, &r, tolleranza("EPSG:3857")).expect_err("polo");
    let testo = errore.to_string();
    assert!(testo.contains("COORDINATE_OUT_OF_CRS_DOMAIN"), "{testo}");
    assert!(
        !testo.contains("12.34") && !testo.contains("88.76"),
        "{testo}"
    );
}

#[test]
fn la_griglia_ntv2_fornita_si_usa_e_ha_la_sua_accuratezza() {
    let config = json!({
        "target_crs": "EPSG:7791",
        "accuratezza_accettata_m": 0.1,
        "griglie": [{"trasformazione": 9734, "file": griglia_sintetica()}],
    });
    let p = params("EPSG:3003", &config);
    assert_eq!(p.piano().percorsi()[0].codici(), vec![9734]);
    assert_eq!(p.piano().accuratezza_garantita_m(), 0.1);
    let r = p.riproiettore().expect("griglia letta");
    let (x, y) = r
        .geografiche_sorgente(1_600_000.0, 4_850_000.0)
        .expect("dominio");
    assert!((9.0..13.0).contains(&x) && (43.0..45.0).contains(&y));
    let punto = Geometry::Point(point!(x: 1_600_000.0, y: 4_850_000.0));
    riproietta_geometria(&punto, &r, tolleranza("EPSG:7791")).expect("dentro la griglia");
    // Un file che non c'e': errore alla lettura, non all'analisi.
    let assente = json!({
        "target_crs": "EPSG:7791",
        "accuratezza_accettata_m": 0.1,
        "griglie": [{"trasformazione": 9734, "file": "non-esiste.gsb"}],
    });
    let errore = params("EPSG:3003", &assente)
        .riproiettore()
        .expect_err("file assente");
    assert!(
        errore.to_string().contains("NTV2_GRID_UNREADABLE"),
        "{errore}"
    );
}

#[test]
fn config_non_valide_si_rifiutano() {
    let sorgente = crs("EPSG:3003");
    for config in [
        json!({}),
        json!({"target_crs": "EPSG:7791", "sconosciuto": 1}),
        json!({"target_crs": "EPSG:999999"}),
        json!({"target_crs": "EPSG:7791"}),
        json!({"target_crs": "EPSG:7791", "accuratezza_accettata_m": 1.0}),
        json!({"target_crs": "EPSG:7791", "accuratezza_accettata_m": 4.0,
               "griglie": [{"trasformazione": 9734, "file": ""}]}),
        json!({"target_crs": "EPSG:7791", "accuratezza_accettata_m": 4.0,
               "griglie": [{"trasformazione": 9734, "file": "a.gsb"},
                           {"trasformazione": 9734, "file": "b.gsb"}]}),
        json!({"target_crs": "EPSG:7791", "accuratezza_accettata_m": 4.0,
               "trasformazioni": [1149]}),
    ] {
        assert!(
            ReprojectParams::da_config("geo.reproject", &config, &sorgente).is_err(),
            "{config}"
        );
    }
}

/// Una config illeggibile e' un piano non valido, come per ogni altra
/// operazione geo: fino alla versione 1 dell'analisi era
/// `InvalidConfiguration`.
#[test]
fn una_config_illeggibile_e_un_piano_non_valido() {
    let sorgente = crs("EPSG:4326");
    for config in [
        json!({}),
        json!({"target_crs": "EPSG:7791", "sconosciuto": 1}),
        json!({"target_crs": 7791}),
        json!({"target_crs": "EPSG:7791", "accuratezza_accettata_m": 4.0,
               "griglie": [{"trasformazione": 9734, "file": ""}]}),
    ] {
        let errore = ReprojectParams::da_config("geo.reproject", &config, &sorgente)
            .expect_err("config illeggibile");
        assert_eq!(
            errore.category(),
            plenora_core::ErrorCategory::InvalidPlan,
            "{config}: {errore}"
        );
    }
}

fn campo_geometria(definizione: &str) -> Field {
    let mut metadata = HashMap::new();
    metadata.insert(
        GEOARROW_EXTENSION_KEY.to_owned(),
        GEOARROW_WKB_EXTENSION.to_owned(),
    );
    metadata.insert(
        GEO_METADATA_KEY.to_owned(),
        geo_metadata_json_with_dimensions(definizione, GeometryDimensions::Xy).expect("geo"),
    );
    Field::new("geometry", plenora_core::arrow::DataType::Binary, true).with_metadata(metadata)
}

#[test]
fn la_tabella_conserva_righe_null_e_attributi() {
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", plenora_core::arrow::DataType::Int64, false),
        campo_geometria("EPSG:4326"),
    ]));
    let celle: Vec<Option<Vec<u8>>> = vec![
        Some(
            Geometry::Point(point!(x: 9.0, y: 45.0))
                .to_wkb(CoordDimensions::xy())
                .expect("wkb"),
        ),
        None,
        Some(
            Geometry::LineString(line_string![(x: 8.0, y: 44.0), (x: 10.0, y: 46.0)])
                .to_wkb(CoordDimensions::xy())
                .expect("wkb"),
        ),
    ];
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(Int64Array::from(vec![1, 2, 3])),
            Arc::new(celle.iter().map(|c| c.as_deref()).collect::<BinaryArray>()),
        ],
    )
    .expect("batch");
    let p = params("EPSG:4326", &json!({"target_crs": "EPSG:32632"}));
    let (uscita_schema, uscita) =
        reproject_batches(&schema, &[batch], "geometry", &crs("EPSG:4326"), &p).expect("kernel");
    assert_eq!(uscita.len(), 1);
    let geometrie = uscita[0]
        .column(1)
        .as_any()
        .downcast_ref::<BinaryArray>()
        .expect("binario");
    assert!(geometrie.is_valid(0) && geometrie.is_null(1) && geometrie.is_valid(2));
    let punto = crate::geometry_from_wkb(geometrie.value(0)).expect("wkb");
    assert!(punto.coords_iter().all(|c| c.x > 100_000.0));
    let metadata = uscita_schema.field(1).metadata();
    let geo: Value = serde_json::from_str(&metadata[GEO_METADATA_KEY]).expect("geo");
    assert_eq!(geo["crs"], "EPSG:32632");
    assert_eq!(
        metadata[PLENORA_GEOMETRY_AXIS_ORDER_KEY],
        "easting_northing"
    );
    // Il piano deciso per un'altra sorgente non si applica.
    assert!(reproject_batches(&schema, &[], "geometry", &crs("EPSG:4258"), &p).is_err());
}

#[test]
fn l_esito_non_dipende_dalla_segmentazione() {
    // Una linea da 7 a 11 E lungo il parallelo 40 sta nel solo riquadro
    // continentale, ma il suo punto a 9 E sta anche in quello sardo: con o
    // senza un vertice collineare in mezzo, errore di percorsi misti.
    let config = json!({"target_crs": "EPSG:4326", "accuratezza_accettata_m": 4.0});
    let r = riproiettore("EPSG:4265", &config);
    let t = tolleranza("EPSG:4326");
    for linea in [
        line_string![(x: 7.0, y: 40.0), (x: 11.0, y: 40.0)],
        line_string![(x: 7.0, y: 40.0), (x: 9.0, y: 40.0), (x: 11.0, y: 40.0)],
        line_string![(x: 7.0, y: 40.0), (x: 10.0, y: 40.0), (x: 11.0, y: 40.0)],
    ] {
        let errore =
            riproietta_geometria(&Geometry::LineString(linea), &r, t).expect_err("percorsi misti");
        assert!(
            errore
                .to_string()
                .contains("REPROJECTION_MIXED_TRANSFORMATION_AREAS"),
            "{errore}"
        );
    }
}

fn record_ntv2(chiave: [u8; 8], valore: [u8; 8]) -> Vec<u8> {
    let mut r = chiave.to_vec();
    r.extend_from_slice(&valore);
    r
}

fn intero_ntv2(v: i32) -> [u8; 8] {
    let mut b = [0_u8; 8];
    b[..4].copy_from_slice(&v.to_le_bytes());
    b
}

/// Byte di una griglia `NTv2` di una sola sottogriglia, little-endian:
/// limiti e passo in secondi (longitudini positive a ovest), spostamenti
/// `(dlat, dlon)` in secondi da `f(lon, lat)` in gradi.
#[allow(clippy::cast_possible_truncation)]
fn griglia_ntv2(
    (sud, nord, est, ovest, passo): (f64, f64, f64, f64, f64),
    f: impl Fn(f64, f64) -> (f32, f32),
) -> Vec<u8> {
    let righe = ((nord - sud) / passo).round() as i32 + 1;
    let colonne = ((ovest - est) / passo).round() as i32 + 1;
    let mut byte = Vec::new();
    for (k, v) in [
        (b"NUM_OREC", intero_ntv2(11)),
        (b"NUM_SREC", intero_ntv2(11)),
        (b"NUM_FILE", intero_ntv2(1)),
        (b"GS_TYPE ", *b"SECONDS "),
        (b"VERSION ", *b"NTv2.0  "),
        (b"SYSTEM_F", *b"PROVA   "),
        (b"SYSTEM_T", *b"PROVA   "),
        (b"MAJOR_F ", 6_378_388.0_f64.to_le_bytes()),
        (b"MINOR_F ", 6_356_911.946_f64.to_le_bytes()),
        (b"MAJOR_T ", 6_378_137.0_f64.to_le_bytes()),
        (b"MINOR_T ", 6_356_752.314_f64.to_le_bytes()),
        (b"SUB_NAME", *b"UNICA   "),
        (b"PARENT  ", *b"NONE    "),
        (b"CREATED ", *b"20260929"),
        (b"UPDATED ", *b"20260929"),
        (b"S_LAT   ", sud.to_le_bytes()),
        (b"N_LAT   ", nord.to_le_bytes()),
        (b"E_LONG  ", est.to_le_bytes()),
        (b"W_LONG  ", ovest.to_le_bytes()),
        (b"LAT_INC ", passo.to_le_bytes()),
        (b"LONG_INC", passo.to_le_bytes()),
        (b"GS_COUNT", intero_ntv2(righe * colonne)),
    ] {
        byte.extend(record_ntv2(*k, v));
    }
    for j in 0..righe {
        for i in 0..colonne {
            let lat = f64::from(j).mul_add(passo, sud) / 3600.0;
            let lon = -f64::from(i).mul_add(passo, est) / 3600.0;
            let (dlat, dlon) = f(lon, lat);
            for v in [dlat, dlon, 0.0, 0.0] {
                byte.extend_from_slice(&v.to_le_bytes());
            }
        }
    }
    byte
}

#[test]
fn un_rilievo_della_griglia_fra_i_campioni_non_si_perde() {
    // Griglia nulla salvo un rilievo bilineare di 1 secondo in latitudine
    // nel nodo a 8,5 E (passo 0,125 gradi): estremi e campioni del lato
    // (8, 40.5)-(12, 40.5) non lo vedono, e l'immagine esatta se ne scosta
    // di circa 31 m. La densificazione deve seguirlo cella per cella.
    let byte = griglia_ntv2(
        (
            40.0 * 3600.0,
            41.0 * 3600.0,
            -13.0 * 3600.0,
            -7.0 * 3600.0,
            450.0,
        ),
        |lon, _| {
            if (lon - 8.5).abs() < 1e-9 {
                (1.0, 0.0)
            } else {
                (0.0, 0.0)
            }
        },
    );
    let griglia = GrigliaNtv2::da_byte(&byte).expect("griglia");
    let opzioni = OpzioniRiproiezione {
        accuratezza_accettata_m: Some(0.1),
        griglie: vec![9734],
        trasformazioni: Some(vec![9734]),
        convenzione_wgs84_etrs89: None,
    };
    let piano =
        PianoRiproiezione::nuovo(&crs("EPSG:4265"), &crs("EPSG:6706"), &opzioni).expect("piano");
    let r = Riproiettore::nuovo(piano, BTreeMap::from([(9734, griglia)])).expect("riproiettore");
    let t = tolleranza("EPSG:6706");
    let linea = line_string![(x: 8.0, y: 40.5), (x: 12.0, y: 40.5)];
    let Geometry::LineString(uscita) =
        riproietta_geometria(&Geometry::LineString(linea.clone()), &r, t).expect("riproiezione")
    else {
        panic!("tipo");
    };
    // Il rilievo e' nell'uscita: la latitudine massima e' 40.5 + 1".
    let massima = uscita
        .0
        .iter()
        .map(|c| c.y)
        .fold(f64::NEG_INFINITY, f64::max);
    assert!((massima - (40.5 + 1.0 / 3600.0)).abs() < 1e-9, "{massima}");
    assert!(scarto_massimo(&r, &linea, &uscita, 20_000) <= 2.0 * t);
}
