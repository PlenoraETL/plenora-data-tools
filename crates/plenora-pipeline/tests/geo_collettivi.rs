//! Operazioni geo bloccanti nel runner: espansioni, aggregazioni,
//! collettive allineate, coperture e griglia uguali alla chiamata diretta
//! dei kernel; CRS lungo `reproject` → `buffer` → `dissolve`; budget.

// Un caso per operazione in ogni test; singolare e plurale sono nomi voluti.
#![allow(clippy::too_many_lines, clippy::similar_names)]

mod comune_geo;

use std::sync::Arc;

use geo::{Geometry, LineString, MultiLineString, MultiPoint, MultiPolygon, Point, Polygon};
use plenora_core::arrow::array::{
    Array, ArrayRef, BinaryArray, Int64Array, RecordBatch, StringArray, UInt64Array,
};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_core::crs::resolve_crs;
use plenora_core::{ErrorCategory, PlenoraError};
use plenora_kernels_geo::extensions2::{GridExtent, GridShape};
use plenora_kernels_geo::operations::BufferCapStyle;
use plenora_kernels_geo::rust_backend::arrow::{
    make_valid_batches, polygonize_batches, split_batches, PolygonizeParams,
};
use plenora_kernels_geo::rust_backend::precision::Precision;
use plenora_kernels_geo::{
    advanced, cluster, construction, extended_algorithms, extensions, extensions2, extensions3,
    operations, topology,
};
use serde_json::json;

use comune_geo::{
    con_budget, esadecimale_di, esegui, geometrie, linee, passo, piano, poligoni, punti,
    punti_lonlat, quadrato, tabella, un_passo, un_passo_con_crs, wkb, LONLAT, UTM, X0, Y0,
};

/// Il tetto di righe che il runner passa ai kernel per un output del piano.
fn righe_massime() -> u64 {
    plenora_core::limits::Limits::default().rows.max_output_rows
}

fn centimetro() -> Precision {
    Precision::from_crs(&resolve_crs(UTM, "crs").expect("crs")).expect("precisione")
}

fn colonna<'a>(tabella: &'a RecordBatch, nome: &str) -> &'a ArrayRef {
    tabella
        .column_by_name(nome)
        .unwrap_or_else(|| panic!("colonna `{nome}` assente"))
}

fn celle(tabella: &RecordBatch) -> BinaryArray {
    colonna(tabella, "geometry")
        .as_any()
        .downcast_ref::<BinaryArray>()
        .expect("binaria")
        .clone()
}

fn binaria(celle: &[Option<Vec<u8>>]) -> BinaryArray {
    celle.iter().map(Option::as_deref).collect()
}

fn nomi(tabella: &RecordBatch) -> Vec<String> {
    tabella
        .schema()
        .fields()
        .iter()
        .map(|campo| campo.name().clone())
        .collect()
}

fn multi() -> RecordBatch {
    tabella(
        UTM,
        &[
            Some(Geometry::MultiPolygon(MultiPolygon::new(vec![
                quadrato(X0, Y0, 10.0),
                quadrato(X0 + 20.0, Y0, 10.0),
            ]))),
            None,
            Some(Geometry::MultiLineString(MultiLineString::new(vec![
                LineString::from(vec![(X0, Y0), (X0 + 5.0, Y0 + 5.0)]),
                LineString::from(vec![(X0 + 9.0, Y0), (X0 + 9.0, Y0 + 5.0)]),
                LineString::from(vec![(X0 + 12.0, Y0), (X0 + 19.0, Y0 + 5.0)]),
            ]))),
        ],
    )
}

/// Le espansioni: una riga per parte, attributi della madre, indice.
#[test]
fn le_espansioni_sono_i_kernel() {
    let ingresso = multi();
    let uscita = un_passo("geo.explode", json!({}), std::slice::from_ref(&ingresso)).unwrap();
    let mut madri = Vec::new();
    let mut parti = Vec::new();
    for (riga, g) in geometrie(&ingresso, "geometry").iter().enumerate() {
        if let Some(g) = g {
            for parte in operations::explode(g).unwrap() {
                madri.push(u64::try_from(riga).unwrap());
                parti.push(Some(wkb(&parte)));
            }
        }
    }
    assert_eq!(uscita.num_rows(), 5);
    assert_eq!(
        colonna(&uscita, "geometry").as_ref(),
        &binaria(&parti) as &dyn Array
    );
    let indici = UInt64Array::from(madri.clone());
    assert_eq!(
        colonna(&uscita, "__parent_index").as_ref(),
        &indici as &dyn Array
    );
    let id: Int64Array = madri
        .iter()
        .map(|m| Some(i64::try_from(*m).unwrap()))
        .collect();
    assert_eq!(colonna(&uscita, "id").as_ref(), &id as &dyn Array);

    // Delaunay di un multipunto.
    let nuvola = tabella(
        UTM,
        &[Some(Geometry::MultiPoint(MultiPoint::from(vec![
            (X0, Y0),
            (X0 + 10.0, Y0),
            (X0 + 5.0, Y0 + 8.0),
            (X0 + 12.0, Y0 + 12.0),
        ])))],
    );
    let uscita = un_passo("geo.delaunay", json!({}), std::slice::from_ref(&nuvola)).unwrap();
    let triangoli: Vec<Option<Vec<u8>>> = extended_algorithms::delaunay(
        geometrie(&nuvola, "geometry")[0].as_ref().unwrap(),
        u64::MAX,
        u64::MAX,
    )
    .unwrap()
    .into_iter()
    .map(|t| Some(wkb(&Geometry::Polygon(t))))
    .collect();
    assert_eq!(
        colonna(&uscita, "geometry").as_ref(),
        &binaria(&triangoli) as &dyn Array
    );

    // Subdivide: la riga null resta, con geometria null.
    let denso = Geometry::Polygon(Polygon::new(
        LineString::from(
            (0..40_u8)
                .map(|i| {
                    let angolo = f64::from(i) * std::f64::consts::TAU / 40.0;
                    (
                        100.0_f64.mul_add(angolo.cos(), X0),
                        100.0_f64.mul_add(angolo.sin(), Y0),
                    )
                })
                .chain(std::iter::once((X0 + 100.0, Y0)))
                .collect::<Vec<_>>(),
        ),
        vec![],
    ));
    let ingresso = tabella(UTM, &[Some(denso.clone()), None]);
    let uscita = un_passo(
        "geo.subdivide",
        json!({"max_vertices": 8}),
        std::slice::from_ref(&ingresso),
    )
    .unwrap();
    let mut attese: Vec<Option<Vec<u8>>> =
        extensions2::subdivide_wkb(&wkb(&denso), 8, centimetro())
            .unwrap()
            .into_iter()
            .map(Some)
            .collect();
    let pezzi = attese.len();
    attese.push(None);
    assert!(pezzi > 1);
    assert_eq!(
        colonna(&uscita, "geometry").as_ref(),
        &binaria(&attese) as &dyn Array
    );

    // Split: l'adapter dei kernel, con la lama su ogni riga.
    let lama = Geometry::LineString(LineString::from(vec![
        (X0 + 25.0, Y0 - 10.0),
        (X0 + 25.0, Y0 + 200.0),
    ]));
    let ingresso = poligoni();
    let uscita = un_passo(
        "geo.split",
        json!({"other_wkb": esadecimale_di(&lama)}),
        std::slice::from_ref(&ingresso),
    )
    .unwrap();
    let lame: BinaryArray = (0..ingresso.num_rows())
        .map(|_| Some(wkb(&lama)))
        .collect::<Vec<_>>()
        .iter()
        .map(Option::as_deref)
        .collect();
    let (_, attese) = split_batches(
        &ingresso.schema(),
        std::slice::from_ref(&ingresso),
        "geometry",
        &lame,
        None,
        righe_massime(),
        centimetro(),
    )
    .unwrap();
    assert_eq!(colonna(&uscita, "geometry"), attese[0].column(2));
    assert_eq!(colonna(&uscita, "__parent_index"), attese[0].column(3));
}

/// Aggregazioni a sole geometrie e raccolta per gruppo.
#[test]
fn le_aggregazioni_sono_i_kernel() {
    let ingresso = poligoni();
    let uscita = un_passo("geo.dissolve", json!({}), std::slice::from_ref(&ingresso)).unwrap();
    let presenti: Vec<Geometry<f64>> = geometrie(&ingresso, "geometry")
        .into_iter()
        .flatten()
        .collect();
    let unione = topology::dissolve(&presenti, centimetro()).unwrap();
    assert_eq!(nomi(&uscita), ["geometry"]);
    assert_eq!(
        colonna(&uscita, "geometry").as_ref(),
        &binaria(&[Some(wkb(&unione))]) as &dyn Array
    );

    let ingresso = punti();
    let uscita = un_passo(
        "geo.line_builder",
        json!({}),
        std::slice::from_ref(&ingresso),
    )
    .unwrap();
    let linea = construction::line_from_ordered_points(&geometrie(&ingresso, "geometry"))
        .unwrap()
        .unwrap();
    assert_eq!(
        colonna(&uscita, "geometry").as_ref(),
        &binaria(&[Some(wkb(&linea))]) as &dyn Array
    );
    let uscita = un_passo(
        "geo.polygon_builder",
        json!({}),
        std::slice::from_ref(&ingresso),
    )
    .unwrap();
    let poligono =
        construction::polygon_from_ordered_points(&geometrie(&ingresso, "geometry")).unwrap();
    assert_eq!(
        colonna(&uscita, "geometry").as_ref(),
        &binaria(&[poligono.as_ref().map(wkb)]) as &dyn Array
    );

    let ingresso = linee();
    let uscita = un_passo("geo.line_merge", json!({}), std::slice::from_ref(&ingresso)).unwrap();
    let raccolte: geo::GeometryCollection<f64> = geometrie(&ingresso, "geometry")
        .into_iter()
        .flatten()
        .collect();
    let fuse: Vec<Option<Vec<u8>>> = extended_algorithms::line_merge(
        &Geometry::GeometryCollection(raccolte),
        u64::MAX,
        u64::MAX,
    )
    .unwrap()
    .into_iter()
    .map(|l| Some(wkb(&Geometry::LineString(l))))
    .collect();
    assert_eq!(fuse.len(), 1);
    assert_eq!(
        colonna(&uscita, "geometry").as_ref(),
        &binaria(&fuse) as &dyn Array
    );

    // Polygonize: l'adapter dei kernel.
    let anelli = tabella(
        UTM,
        &[
            Some(Geometry::LineString(LineString::from(vec![
                (X0, Y0),
                (X0 + 4.0, Y0),
                (X0 + 4.0, Y0 + 4.0),
                (X0, Y0 + 4.0),
                (X0, Y0),
            ]))),
            Some(Geometry::LineString(LineString::from(vec![
                (X0 + 4.0, Y0 + 4.0),
                (X0 + 6.0, Y0 + 6.0),
            ]))),
        ],
    );
    let uscita = un_passo("geo.polygonize", json!({}), std::slice::from_ref(&anelli)).unwrap();
    let (_, attese) = polygonize_batches(
        &anelli.schema(),
        std::slice::from_ref(&anelli),
        "geometry",
        PolygonizeParams::default(),
        righe_massime(),
        centimetro(),
    )
    .unwrap();
    assert_eq!(uscita.num_rows(), 2);
    assert_eq!(colonna(&uscita, "geometry"), attese[0].column(0));
    assert_eq!(colonna(&uscita, "__class"), attese[0].column(1));

    // Collect: un gruppo per valore di `label`, le geometrie in ordine
    // d'ingresso; l'ordine dei gruppi e' deterministico.
    let ingresso = punti();
    let uscita = un_passo(
        "geo.collect",
        json!({"group_by": ["label"]}),
        std::slice::from_ref(&ingresso),
    )
    .unwrap();
    assert_eq!(nomi(&uscita), ["geometry", "label"]);
    let etichette = colonna(&uscita, "label")
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap()
        .clone();
    let tutte = geometrie(&ingresso, "geometry");
    let etichette_in = colonna(&ingresso, "label")
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap()
        .clone();
    let raccolte = geometrie(&uscita, "geometry");
    for (gruppo, raccolta) in raccolte.iter().enumerate() {
        let membri: Vec<Option<Geometry<f64>>> = (0..tutte.len())
            .filter(|riga| etichette_in.value(*riga) == etichette.value(gruppo))
            .map(|riga| tutte[riga].clone())
            .collect();
        assert_eq!(raccolta, &extensions::collect_geometries(&membri).unwrap());
    }
    let di_nuovo = un_passo(
        "geo.collect",
        json!({"group_by": ["label"]}),
        std::slice::from_ref(&ingresso),
    )
    .unwrap();
    assert_eq!(di_nuovo, uscita);
}

/// `collect`: i gruppi escono nell'ordine naturale dei valori tipizzati
/// delle chiavi (il comparatore di `table.sort`), null in coda. A `190c493`
/// la chiave testuale metteva la lunghezza in testa come testo: un valore di
/// 10 caratteri prima di uno di 9, `3` prima di `-5`, `-1` prima di `-10`.
#[test]
fn collect_ordina_i_gruppi_per_valore() {
    let base = punti();
    let righe = 7;
    let testi = [
        Some("bbbbbbbbbb"),
        Some("aaaaaaaaa"),
        None,
        Some("bbbbbbbbbb"),
        Some("b"),
        Some("aaaaaaaaa"),
        Some("ab"),
    ];
    let numeri = [
        Some(3_i64),
        Some(-5),
        Some(-1),
        None,
        Some(-10),
        Some(3),
        Some(100),
    ];
    let geometrie_in: Vec<Option<Geometry<f64>>> = (0..righe)
        .map(|i| {
            Some(Geometry::Point(geo::Point::new(
                X0 + f64::from(u8::try_from(i).unwrap()),
                Y0,
            )))
        })
        .collect();
    let schema = Schema::new_with_metadata(
        vec![
            Field::new("testo", DataType::Utf8, true),
            Field::new("numero", DataType::Int64, true),
            base.schema().field_with_name("geometry").unwrap().clone(),
        ],
        base.schema().metadata().clone(),
    );
    let tabella = RecordBatch::try_new(
        std::sync::Arc::new(schema),
        vec![
            std::sync::Arc::new(StringArray::from(testi.to_vec())),
            std::sync::Arc::new(Int64Array::from(numeri.to_vec())),
            std::sync::Arc::new(binaria(
                &geometrie_in
                    .iter()
                    .map(|g| g.as_ref().map(wkb))
                    .collect::<Vec<_>>(),
            )),
        ],
    )
    .unwrap();

    let uscita = un_passo(
        "geo.collect",
        json!({"group_by": ["testo"]}),
        std::slice::from_ref(&tabella),
    )
    .unwrap();
    let chiavi: Vec<Option<&str>> = colonna(&uscita, "testo")
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap()
        .iter()
        .collect();
    assert_eq!(
        chiavi,
        [
            Some("aaaaaaaaa"),
            Some("ab"),
            Some("b"),
            Some("bbbbbbbbbb"),
            None
        ]
    );
    // Il gruppo raccoglie le righe in ordine d'ingresso (1 e 5).
    assert_eq!(
        geometrie(&uscita, "geometry")[0],
        extensions::collect_geometries(&[geometrie_in[1].clone(), geometrie_in[5].clone()])
            .unwrap()
    );

    let uscita = un_passo(
        "geo.collect",
        json!({"group_by": ["numero"]}),
        std::slice::from_ref(&tabella),
    )
    .unwrap();
    let chiavi: Vec<Option<i64>> = colonna(&uscita, "numero")
        .as_any()
        .downcast_ref::<Int64Array>()
        .unwrap()
        .iter()
        .collect();
    assert_eq!(
        chiavi,
        [Some(-10), Some(-5), Some(-1), Some(3), Some(100), None]
    );

    // Due chiavi: la prima decide, la seconda a parita' della prima.
    let uscita = un_passo(
        "geo.collect",
        json!({"group_by": ["numero", "testo"]}),
        std::slice::from_ref(&tabella),
    )
    .unwrap();
    let testi_usciti: Vec<Option<&str>> = colonna(&uscita, "testo")
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap()
        .iter()
        .collect();
    // numero 3: "aaaaaaaaa" (riga 5) prima di "bbbbbbbbbb" (riga 0).
    assert_eq!(testi_usciti[3], Some("aaaaaaaaa"));
    assert_eq!(testi_usciti[4], Some("bbbbbbbbbb"));
    assert_eq!(uscita.num_rows(), 7);
}

/// Collettive allineate alle righe, coperture, griglia.
#[test]
fn le_collettive_e_le_coperture_sono_i_kernel() {
    let ingresso = punti();
    let presenti: Vec<Geometry<f64>> = geometrie(&ingresso, "geometry")
        .into_iter()
        .flatten()
        .collect();

    let uscita = un_passo("geo.voronoi", json!({}), std::slice::from_ref(&ingresso)).unwrap();
    let mut celle_attese = advanced::voronoi_cells(&presenti, 100_000, centimetro())
        .unwrap()
        .into_iter()
        .map(|c| Some(wkb(&c)));
    let attese: Vec<Option<Vec<u8>>> = geometrie(&ingresso, "geometry")
        .iter()
        .map(|g| g.as_ref().and_then(|_| celle_attese.next().flatten()))
        .collect();
    assert_eq!(
        colonna(&uscita, "geometry").as_ref(),
        &binaria(&attese) as &dyn Array
    );

    let uscita = un_passo(
        "geo.cluster_dbscan",
        json!({"eps": 15.0, "min_points": 2}),
        std::slice::from_ref(&ingresso),
    )
    .unwrap();
    let etichette: UInt64Array = cluster::dbscan_column(&celle(&ingresso), 15.0, 2)
        .unwrap()
        .into_iter()
        .collect();
    assert_eq!(
        colonna(&uscita, "cluster_id").as_ref(),
        &etichette as &dyn Array
    );

    // Pulizia: la seconda riga sovrapposta perde la parte comune; la
    // geometria dell'uscita e' nullable (righe assorbite).
    let ingresso = poligoni();
    let uscita = un_passo(
        "geo.clean_topology",
        json!({"snap_tolerance": 0.0, "remove_overlaps": true, "fill_gaps": false}),
        std::slice::from_ref(&ingresso),
    )
    .unwrap();
    let presenti: Vec<Geometry<f64>> = geometrie(&ingresso, "geometry")
        .into_iter()
        .flatten()
        .collect();
    let mut pulite = topology::clean_valid_polygon_topology(
        &presenti,
        0.0,
        true,
        false,
        u64::MAX,
        u64::MAX,
        centimetro(),
    )
    .unwrap()
    .into_iter();
    let attese: Vec<Option<Vec<u8>>> = geometrie(&ingresso, "geometry")
        .iter()
        .map(|g| {
            g.as_ref()
                .and_then(|_| pulite.next().flatten().map(|p| wkb(&p)))
        })
        .collect();
    assert_eq!(
        colonna(&uscita, "geometry").as_ref(),
        &binaria(&attese) as &dyn Array
    );
    assert!(uscita
        .schema()
        .field_with_name("geometry")
        .unwrap()
        .is_nullable());

    let uscita = un_passo(
        "geo.coverage_validate",
        json!({}),
        std::slice::from_ref(&ingresso),
    )
    .unwrap();
    let problemi =
        extensions3::coverage_validate_rows(&celle(&ingresso), 0.0, 1000, centimetro()).unwrap();
    assert_eq!(uscita.num_rows(), problemi.len());
    assert!(!problemi.is_empty());
    let geometrie_problemi: Vec<Option<Vec<u8>>> =
        problemi.iter().map(|p| Some(p.wkb.clone())).collect();
    assert_eq!(
        colonna(&uscita, "geometry").as_ref(),
        &binaria(&geometrie_problemi) as &dyn Array
    );

    let adiacenti = tabella(
        UTM,
        &[
            Some(Geometry::Polygon(quadrato(X0, Y0, 10.0))),
            Some(Geometry::Polygon(quadrato(X0 + 10.0, Y0, 10.0))),
        ],
    );
    let uscita = un_passo(
        "geo.shared_paths",
        json!({}),
        std::slice::from_ref(&adiacenti),
    )
    .unwrap();
    let tratti = extensions3::shared_paths_rows(&celle(&adiacenti), 0.0, 0.0).unwrap();
    assert_eq!(tratti.len(), 1);
    let geometrie_tratti: Vec<Option<Vec<u8>>> =
        tratti.iter().map(|t| Some(t.wkb.clone())).collect();
    assert_eq!(
        colonna(&uscita, "geometry").as_ref(),
        &binaria(&geometrie_tratti) as &dyn Array
    );

    // Griglia, dal CRS di piano; l'ingresso fa da innesco.
    let innesco = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new("n", DataType::Int64, false)])),
        vec![Arc::new(Int64Array::from(vec![1]))],
    )
    .unwrap();
    let config = json!({"extent": {"xmin": X0, "ymin": Y0, "xmax": X0 + 30.0, "ymax": Y0 + 20.0},
                        "cell_size": 10.0, "include_centroid": true});
    let uscita = un_passo_con_crs("geo.generate_grid", config, innesco, UTM).unwrap();
    let estensione = GridExtent::new(X0, Y0, X0 + 30.0, Y0 + 20.0).unwrap();
    let righe = extensions2::generate_grid_rows(&estensione, 10.0, GridShape::Square).unwrap();
    assert_eq!(uscita.num_rows(), righe.len());
    let attese: Vec<Option<Vec<u8>>> = righe.iter().map(|r| Some(r.wkb.clone())).collect();
    assert_eq!(
        colonna(&uscita, "geometry").as_ref(),
        &binaria(&attese) as &dyn Array
    );
    assert_eq!(
        nomi(&uscita),
        ["geometry", "cell_i", "cell_j", "centroid_x", "centroid_y"]
    );
}

/// `make_valid` ripara con l'adapter dei kernel, anche la geometria che
/// ogni altra operazione rifiuterebbe.
#[test]
fn make_valid_e_l_adapter_dei_kernel() {
    let farfalla = Geometry::Polygon(Polygon::new(
        LineString::from(vec![
            (X0, Y0),
            (X0 + 4.0, Y0 + 4.0),
            (X0 + 4.0, Y0),
            (X0, Y0 + 4.0),
            (X0, Y0),
        ]),
        vec![],
    ));
    let ingresso = tabella(UTM, &[Some(farfalla), None]);
    let uscita = un_passo("geo.make_valid", json!({}), std::slice::from_ref(&ingresso)).unwrap();
    let attese = make_valid_batches(
        &ingresso.schema(),
        std::slice::from_ref(&ingresso),
        "geometry",
        centimetro(),
    )
    .unwrap();
    assert_eq!(colonna(&uscita, "geometry"), attese[0].column(2));
    // La stessa geometria su un'operazione che la valida: rifiutata.
    let errore = un_passo("geo.area", json!({}), &[ingresso]).unwrap_err();
    assert_eq!(errore.category(), ErrorCategory::InvalidPlan, "{errore}");
}

/// Il CRS attraversa `reproject` → `buffer` → `dissolve`.
#[test]
fn il_crs_attraversa_reproject_buffer_dissolve() {
    let pipeline = piano(
        &["t"],
        vec![
            passo("utm", "geo.reproject", &["t"], json!({"target_crs": UTM})),
            passo(
                "fasce",
                "geo.buffer",
                &["utm"],
                json!({"distance": 100_000.0}),
            ),
            passo("unione", "geo.dissolve", &["fasce"], json!({})),
        ],
        &["unione"],
    );
    let ingresso = punti_lonlat();
    let esito = esegui(&pipeline, &[("t", ingresso.clone())]).unwrap();
    let uscita = &esito.outputs[0].1;
    let contratto = comune_geo::contratto(uscita);
    assert_eq!(
        contratto.geometries[0]
            .crs
            .as_resolved()
            .unwrap()
            .definition(),
        UTM
    );
    // Stesso risultato dei kernel in sequenza.
    let sorgente = resolve_crs("OGC:CRS84", "crs").unwrap();
    let parametri = plenora_kernels_geo::riproiezione::ReprojectParams::da_config(
        "geo.reproject",
        &json!({"target_crs": UTM}),
        &sorgente,
    )
    .unwrap();
    let (_, riproiettate) = plenora_kernels_geo::riproiezione::reproject_batches(
        &ingresso.schema(),
        std::slice::from_ref(&ingresso),
        "geometry",
        &sorgente,
        &parametri,
    )
    .unwrap();
    let fasce: Vec<Geometry<f64>> = geometrie(&riproiettate[0], "geometry")
        .into_iter()
        .flatten()
        .map(|g| {
            operations::buffer_with_cap(&g, 100_000.0, BufferCapStyle::Round, centimetro()).unwrap()
        })
        .collect();
    let unione = topology::dissolve(&fasce, centimetro()).unwrap();
    assert_eq!(
        colonna(uscita, "geometry").as_ref(),
        &binaria(&[Some(wkb(&unione))]) as &dyn Array
    );
}

/// La griglia prevede il costo dalle celle note a secco: oltre il budget
/// si rifiuta prima di generarle.
#[test]
fn una_griglia_oltre_il_budget_si_rifiuta_prima_di_eseguire() {
    let innesco = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new("n", DataType::Int64, false)])),
        vec![Arc::new(Int64Array::from(vec![1]))],
    )
    .unwrap();
    let mut pipeline = con_budget(
        piano(
            &["t"],
            vec![passo(
                "x",
                "geo.generate_grid",
                &["t"],
                json!({"extent": {"xmin": X0, "ymin": Y0, "xmax": X0 + 1000.0, "ymax": Y0 + 1000.0},
                       "cell_size": 1.0}),
            )],
            &["x"],
        ),
        64 * 1024 * 1024,
    );
    pipeline.crs = Some(UTM.to_owned());
    let errore = esegui(&pipeline, &[("t", innesco)]).unwrap_err();
    let PlenoraError::ResourceLimit(messaggio) = &errore else {
        panic!("atteso ResourceLimit: {errore}");
    };
    assert!(messaggio.contains("geo.generate_grid"), "{messaggio}");
}

#[test]
fn le_espansioni_rispettano_il_limite_dell_arco() {
    let ingresso = multi();
    let mut pipeline = piano(
        &["t"],
        vec![
            passo("parti", "geo.explode", &["t"], json!({})),
            passo("aree", "geo.area", &["parti"], json!({})),
        ],
        &["aree"],
    );
    pipeline.limits = Some(plenora_pipeline::LimitiParziali {
        max_rows_per_edge: Some(3),
        max_expansion_factor: Some(100.0),
        ..plenora_pipeline::LimitiParziali::default()
    });
    let errore = esegui(&pipeline, &[("t", ingresso)]).unwrap_err();
    assert_eq!(errore.category(), ErrorCategory::ResourceLimit, "{errore}");
}

#[allow(dead_code)]
const fn _punto(_: Point<f64>) {}

#[test]
fn la_validazione_rifiuta_esattamente_cio_che_l_analisi_rifiuta() {
    let innesco = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new("n", DataType::Int64, false)])),
        vec![Arc::new(Int64Array::from(vec![1]))],
    )
    .unwrap();
    let griglia = json!({"extent": {"xmin": X0, "ymin": Y0, "xmax": X0 + 10.0, "ymax": Y0 + 10.0},
                         "cell_size": 5.0});
    let casi = vec![
        ("geo.dissolve", json!({}), vec![poligoni()], None),
        ("geo.dissolve", json!({"extra": 1}), vec![poligoni()], None),
        (
            "geo.subdivide",
            json!({"max_vertices": 2}),
            vec![poligoni()],
            None,
        ),
        (
            "geo.subdivide",
            json!({"max_vertices": 8, "output_column": "id"}),
            vec![poligoni()],
            None,
        ),
        ("geo.voronoi", json!({"max_points": 1}), vec![punti()], None),
        (
            "geo.cluster_dbscan",
            json!({"eps": 0.0, "min_points": 2}),
            vec![punti()],
            None,
        ),
        (
            "geo.coverage_validate",
            json!({"max_issues": 0}),
            vec![poligoni()],
            None,
        ),
        (
            "geo.shared_paths",
            json!({"min_length": -1.0}),
            vec![poligoni()],
            None,
        ),
        ("geo.collect", json!({"group_by": []}), vec![punti()], None),
        (
            "geo.collect",
            json!({"group_by": ["geometry"]}),
            vec![punti()],
            None,
        ),
        (
            "geo.clean_topology",
            json!({"snap_tolerance": -1.0, "remove_overlaps": true, "fill_gaps": true}),
            vec![poligoni()],
            None,
        ),
        (
            "geo.clean_topology",
            json!({"snap_tolerance": 1.0}),
            vec![poligoni()],
            None,
        ),
        (
            "geo.clean_topology",
            json!({"snap_tolerance": 1.0, "remove_overlaps": true}),
            vec![poligoni()],
            None,
        ),
        (
            "geo.polygonize",
            json!({"node_input": false}),
            vec![linee()],
            None,
        ),
        (
            "geo.split",
            json!({"other_wkb": "00"}),
            vec![poligoni()],
            None,
        ),
        (
            "geo.generate_grid",
            griglia.clone(),
            vec![innesco.clone()],
            None,
        ),
        (
            "geo.generate_grid",
            griglia,
            vec![innesco.clone()],
            Some(UTM),
        ),
        (
            "geo.generate_grid",
            json!({"extent": {"xmin": 0.0, "ymin": 0.0, "xmax": 1.0, "ymax": 1.0},
                                     "cell_size": 0.5}),
            vec![innesco],
            Some(UTM),
        ),
    ];
    comune_geo::stessa_validazione(&casi);
}

/// Le chiavi di `collect` devono leggersi come testo: il tipo si decide
/// dallo schema e si rifiuta in validazione, anche su una tabella vuota.
#[test]
fn i_tipi_delle_chiavi_di_collect_si_verificano_in_validazione() {
    use plenora_core::arrow::array::{
        Int32Array, StringViewArray, TimestampMillisecondArray, TimestampNanosecondArray,
    };
    use plenora_core::arrow::schema::TimeUnit;

    let geometrie_in = punti();
    let righe = geometrie_in.num_rows();
    let con_chiave = |campo: Field, colonna: ArrayRef| {
        let mut campi: Vec<Field> = geometrie_in
            .schema()
            .fields()
            .iter()
            .map(|c| c.as_ref().clone())
            .collect();
        campi.push(campo);
        let mut colonne = geometrie_in.columns().to_vec();
        colonne.push(colonna);
        RecordBatch::try_new(Arc::new(Schema::new(campi)), colonne).unwrap()
    };
    let millisecondi = |fuso: &str| {
        con_chiave(
            Field::new(
                "k",
                DataType::Timestamp(TimeUnit::Millisecond, Some(fuso.into())),
                true,
            ),
            Arc::new(TimestampMillisecondArray::from(vec![Some(0_i64); righe]).with_timezone(fuso)),
        )
    };
    let rifiutate = [
        con_chiave(
            Field::new("k", DataType::Int32, true),
            Arc::new(Int32Array::from(vec![Some(1); righe])),
        ),
        con_chiave(
            Field::new("k", DataType::Utf8View, true),
            Arc::new(StringViewArray::from(vec![Some("a"); righe])),
        ),
        con_chiave(
            Field::new("k", DataType::Timestamp(TimeUnit::Nanosecond, None), true),
            Arc::new(TimestampNanosecondArray::from(vec![Some(0_i64); righe])),
        ),
    ];
    for tabella_chiave in rifiutate {
        let tipo = tabella_chiave
            .schema()
            .field_with_name("k")
            .unwrap()
            .data_type()
            .clone();
        let config = json!({"group_by": ["k"]});
        let analizzata = comune_geo::analisi(
            "geo.collect",
            std::slice::from_ref(&tabella_chiave),
            &config,
            None,
        )
        .expect_err("l'analisi rifiuta");
        // Anche vuota: la validazione non guarda i dati.
        let vuota = tabella_chiave.slice(0, 0);
        let errore = un_passo("geo.collect", config, &[vuota]).expect_err("rifiutata");
        assert!(
            comune_geo::stesso_errore(&errore, &analizzata),
            "{tipo}: {errore} / {analizzata}"
        );
        assert_eq!(
            errore.category(),
            ErrorCategory::InvalidPlan,
            "{tipo}: {errore}"
        );
    }
    // Tipi con un ordine naturale (quelli di `table.sort`): accettati, vuota
    // o no. Un fuso orario inesistente non conta: l'ordine e' per istante,
    // e le chiavi escono invariate (prima si rifiutava perche' la chiave si
    // scriveva come testo nel fuso).
    let accettate = [
        millisecondi("Europe/Rome"),
        millisecondi("Fuso/Inesistente"),
        con_chiave(
            Field::new("k", DataType::Int64, true),
            Arc::new(Int64Array::from(vec![Some(7); righe])),
        ),
    ];
    for tabella_chiave in accettate {
        let piena = un_passo(
            "geo.collect",
            json!({"group_by": ["k"]}),
            std::slice::from_ref(&tabella_chiave),
        )
        .unwrap();
        assert_eq!(piena.num_rows(), 1);
        let vuota = un_passo(
            "geo.collect",
            json!({"group_by": ["k"]}),
            &[tabella_chiave.slice(0, 0)],
        )
        .unwrap();
        assert_eq!(vuota.num_rows(), 0);
    }
}

/// `make_valid` su un CRS geografico usa il passo di 1 cm sul raggio polare
/// dell'ellissoide (per WGS 84 lo 0,335% piu' fine del vecchio passo
/// all'equatore). Una farfalla lon/lat con un vertice di un'altra parte
/// sopra l'incrocio arrotondato: `LINEWORK` rifiuta
/// (`PrecisionInsufficient`) sotto una soglia proporzionale alla
/// precisione. Fra la soglia del passo nuovo e quella del vecchio, il runner
/// accetta e il kernel con il vecchio passo rifiuta.
#[test]
fn make_valid_geografico_usa_il_passo_sul_raggio_polare() {
    let (x0, y0) = (11.123_456_789_f64, 44.987_654_321_f64);
    let s = 1e-4 / 3.0;
    let incrocio = (1.5f64.mul_add(s, x0), 0.5f64.mul_add(s, y0));
    let tabella_con = |d: f64| {
        let farfalla = Polygon::new(
            LineString::from(vec![
                (x0, y0),
                (3.0f64.mul_add(s, x0), y0 + s),
                (3.0f64.mul_add(s, x0), y0),
                (x0, y0 + s),
                (x0, y0),
            ]),
            vec![],
        );
        let (vx, vy) = (incrocio.0, incrocio.1 + d);
        let triangolo = Polygon::new(
            LineString::from(vec![
                (vx, vy),
                (vx + s, 0.1f64.mul_add(s, vy)),
                ((-0.3f64).mul_add(s, vx), 0.2f64.mul_add(s, vy)),
                (vx, vy),
            ]),
            vec![],
        );
        let parti = Geometry::MultiPolygon(MultiPolygon::new(vec![farfalla, triangolo]));
        tabella(LONLAT, &[Some(parti)])
    };
    let accetta = |d: f64, precisione: Precision| {
        let t = tabella_con(d);
        make_valid_batches(
            &t.schema(),
            std::slice::from_ref(&t),
            "geometry",
            precisione,
        )
        .is_ok()
    };
    let soglia = |precisione: Precision| {
        let (mut sotto, mut sopra) = (1e-9, 9e-8);
        assert!(!accetta(sotto, precisione) && accetta(sopra, precisione));
        for _ in 0..60 {
            let mezzo = f64::midpoint(sotto, sopra);
            if accetta(mezzo, precisione) {
                sopra = mezzo;
            } else {
                sotto = mezzo;
            }
        }
        sopra
    };
    let nuova = Precision::from_crs(&resolve_crs(LONLAT, "crs").expect("crs")).expect("precisione");
    let polare = 6_378_137.0 / (1.0 - 1.0 / 298.257_223_563);
    assert!((nuova.value() - 0.01 / f64::to_radians(polare)).abs() < 1e-18);
    let vecchia = Precision::new(0.01 / 111_319.49).expect("precisione");
    let (soglia_nuova, soglia_vecchia) = (soglia(nuova), soglia(vecchia));
    assert!(
        soglia_nuova < soglia_vecchia,
        "{soglia_nuova} {soglia_vecchia}"
    );
    let d = f64::midpoint(soglia_nuova, soglia_vecchia);
    assert!(!accetta(d, vecchia));
    let uscita = un_passo("geo.make_valid", json!({}), &[tabella_con(d)]);
    assert!(uscita.is_ok(), "{uscita:?}");
}
