//! Il margine di memoria dei kernel geo sui profili avversari che il
//! modello di costo esclude (README, «Modelli di costo geo»): ogni kernel,
//! chiamato direttamente, si ferma con l'errore del margine quando il
//! margine e' piccolo, rende lo stesso risultato di prima quando e' grande,
//! e lascia vincere il proprio limite di conteggio quando scatta prima.

use geo::{Coord, Geometry, LineString, Point, Polygon};
use plenora_kernels_geo::analysis::{
    count_points_in_polygons_validated, count_points_in_polygons_validated_con_margine,
    nearest_matches_validated, nearest_matches_validated_con_margine, within_indexes_validated,
    within_indexes_validated_con_margine, AnalysisError,
};
use plenora_kernels_geo::extensions::ExtensionError;
use plenora_kernels_geo::extensions3::{
    coverage_validate_nullable, coverage_validate_nullable_con_margine,
};
use plenora_kernels_geo::margine::MargineMemoria;
use plenora_kernels_geo::operations::{
    buffer_with_cap, buffer_with_cap_con_margine, BufferCapStyle, OperationError,
};
use plenora_kernels_geo::rust_backend::precision::Precision;
use plenora_kernels_geo::spatial_join::{
    spatial_join_nullable_validated, spatial_join_nullable_validated_con_margine, JoinPredicate,
    SpatialJoinError,
};
use plenora_kernels_geo::topology::{
    polygon_overlay_validated, polygon_overlay_validated_con_margine, OverlayMode, TopologyError,
};

const GRANDE: MargineMemoria = MargineMemoria::byte(1 << 40);
const PICCOLO: MargineMemoria = MargineMemoria::byte(64 * 1024);

fn precisione() -> Precision {
    Precision::new(0.01).expect("1 cm")
}

/// Un disco regolare di `lati` lati.
#[allow(clippy::cast_precision_loss)]
fn disco(cx: f64, cy: f64, raggio: f64, lati: usize) -> Geometry<f64> {
    let mut punti: Vec<Coord<f64>> = (0..lati)
        .map(|k| {
            let a = std::f64::consts::TAU * k as f64 / lati as f64;
            Coord {
                x: raggio.mul_add(a.cos(), cx),
                y: raggio.mul_add(a.sin(), cy),
            }
        })
        .collect();
    punti.push(punti[0]);
    Geometry::Polygon(Polygon::new(LineString::new(punti), vec![]))
}

/// Una colonna di geometrie con null.
type Colonna = Vec<Option<Geometry<f64>>>;

/// Ogni punto in ogni disco: le coppie candidate sono `n * m`.
#[allow(clippy::cast_precision_loss)]
fn tutti_candidati(n: usize) -> (Colonna, Colonna) {
    let punti = (0..n)
        .map(|k| {
            let a = k as f64 * 2.399_963;
            let r = 10.0 * (k as f64 / n as f64).sqrt();
            Some(Geometry::Point(Point::new(r * a.cos(), r * a.sin())))
        })
        .collect();
    let dischi = (0..n)
        .map(|k| Some(disco(k as f64 * 0.01, 0.0, 100.0, 32)))
        .collect();
    (punti, dischi)
}

#[test]
fn join_within_e_conteggi_si_fermano_al_margine() {
    let (punti, dischi) = tutti_candidati(200);
    let senza =
        spatial_join_nullable_validated(&punti, &dischi, JoinPredicate::Intersects, u64::MAX)
            .expect("join");
    assert_eq!(senza.len(), 200 * 200);
    let con = spatial_join_nullable_validated_con_margine(
        &punti,
        &dischi,
        JoinPredicate::Intersects,
        u64::MAX,
        GRANDE,
    )
    .expect("join col margine grande");
    assert_eq!(con, senza);
    // 40.000 coppie da 40 byte non entrano in 64 KiB.
    assert!(matches!(
        spatial_join_nullable_validated_con_margine(
            &punti,
            &dischi,
            JoinPredicate::Intersects,
            u64::MAX,
            PICCOLO,
        ),
        Err(SpatialJoinError::MargineMemoria(superato))
            if superato.margine == 64 * 1024 && superato.previsti > superato.margine
    ));
    // L'uscita del chiamante conta: 1.000 coppie stanno in 64 KiB da sole,
    // non con 1 KiB di riga ciascuna.
    assert!(matches!(
        spatial_join_nullable_validated_con_margine(
            &punti[..5],
            &dischi,
            JoinPredicate::Intersects,
            u64::MAX,
            PICCOLO.con_uscita_per_risultato(1024),
        ),
        Err(SpatialJoinError::MargineMemoria(_))
    ));
    assert!(spatial_join_nullable_validated_con_margine(
        &punti[..5],
        &dischi,
        JoinPredicate::Intersects,
        u64::MAX,
        PICCOLO,
    )
    .is_ok());
    // Il limite di conteggio, se scatta prima, resta il suo errore.
    assert!(matches!(
        spatial_join_nullable_validated_con_margine(
            &punti,
            &dischi,
            JoinPredicate::Intersects,
            100,
            PICCOLO,
        ),
        Err(SpatialJoinError::PairLimitExceeded { limit: 100 })
    ));

    let dentro = within_indexes_validated(&punti, &dischi, u64::MAX).expect("within");
    assert_eq!(
        within_indexes_validated_con_margine(&punti, &dischi, u64::MAX, GRANDE).expect("within"),
        dentro
    );
    assert!(matches!(
        within_indexes_validated_con_margine(&punti, &dischi, u64::MAX, PICCOLO),
        Err(AnalysisError::SpatialJoin(
            SpatialJoinError::MargineMemoria(_)
        ))
    ));
    let conteggi = count_points_in_polygons_validated(&dischi, &punti, u64::MAX).expect("conta");
    assert_eq!(
        count_points_in_polygons_validated_con_margine(&dischi, &punti, u64::MAX, GRANDE)
            .expect("conta"),
        conteggi
    );
    assert!(matches!(
        count_points_in_polygons_validated_con_margine(&dischi, &punti, u64::MAX, PICCOLO),
        Err(AnalysisError::SpatialJoin(
            SpatialJoinError::MargineMemoria(_)
        ))
    ));
}

#[test]
#[allow(clippy::cast_precision_loss)]
fn vicini_con_pareggi_si_fermano_al_margine() {
    // Ogni punto al centro di un anello di punti: tutti alla stessa
    // distanza (pareggi). Distanze uguali in bit: anello su multipli di 90
    // gradi ripetuti.
    let centro: Vec<Option<Geometry<f64>>> = (0..300)
        .map(|_| Some(Geometry::Point(Point::new(0.0, 0.0))))
        .collect();
    let anello: Vec<Option<Geometry<f64>>> = (0..300)
        .map(|k| {
            let (x, y) = [(5.0, 0.0), (0.0, 5.0), (-5.0, 0.0), (0.0, -5.0)][k % 4];
            Some(Geometry::Point(Point::new(x, y)))
        })
        .collect();
    let senza =
        nearest_matches_validated(&centro, &anello, None, u64::MAX, u64::MAX).expect("vicini");
    assert_eq!(senza.len(), 300 * 300);
    assert_eq!(
        nearest_matches_validated_con_margine(&centro, &anello, None, (u64::MAX, u64::MAX), GRANDE)
            .expect("vicini"),
        senza
    );
    assert!(matches!(
        nearest_matches_validated_con_margine(
            &centro,
            &anello,
            None,
            (u64::MAX, u64::MAX),
            PICCOLO
        ),
        Err(AnalysisError::MargineMemoria(_))
    ));
    assert!(matches!(
        nearest_matches_validated_con_margine(&centro, &anello, None, (u64::MAX, 10), PICCOLO),
        Err(AnalysisError::ResultLimitExceeded { limit: 10 })
    ));
}

#[test]
#[allow(clippy::cast_precision_loss)]
fn overlay_e_copertura_su_sovrapposizioni_si_fermano_al_margine() {
    // Dischi di raggio 2,5 su un passo 1: ognuno sovrappone una ventina di
    // vicini.
    let dischi: Vec<Geometry<f64>> = (0..36)
        .map(|k| disco(f64::from(k % 6), f64::from(k / 6), 2.5, 64))
        .collect();
    let spostati: Vec<Geometry<f64>> = (0..36)
        .map(|k| disco(f64::from(k % 6) + 0.5, f64::from(k / 6) + 0.5, 2.5, 64))
        .collect();
    let senza = polygon_overlay_validated(
        &dischi,
        &spostati,
        OverlayMode::Intersection,
        u64::MAX,
        u64::MAX,
        precisione(),
    )
    .expect("overlay");
    assert!(senza.len() > 500);
    let con = polygon_overlay_validated_con_margine(
        &dischi,
        &spostati,
        OverlayMode::Intersection,
        (u64::MAX, u64::MAX),
        precisione(),
        GRANDE,
    )
    .expect("overlay col margine grande");
    assert_eq!(con, senza);
    assert!(matches!(
        polygon_overlay_validated_con_margine(
            &dischi,
            &spostati,
            OverlayMode::Intersection,
            (u64::MAX, u64::MAX),
            precisione(),
            PICCOLO,
        ),
        Err(TopologyError::ResourceLimit { name: "memoria", actual, limit })
            if limit == 64 * 1024 && actual > limit
    ));

    let celle: Vec<Option<Geometry<f64>>> = dischi.iter().cloned().map(Some).collect();
    let senza =
        coverage_validate_nullable(&celle, 0.0, usize::MAX, precisione()).expect("copertura");
    assert!(senza.len() > 100);
    assert_eq!(
        coverage_validate_nullable_con_margine(&celle, 0.0, usize::MAX, precisione(), GRANDE)
            .expect("copertura"),
        senza
    );
    assert!(matches!(
        coverage_validate_nullable_con_margine(&celle, 0.0, usize::MAX, precisione(), PICCOLO),
        Err(ExtensionError::MargineMemoria(_))
    ));
    // Il limite delle issue, se scatta prima, resta il suo errore.
    assert!(matches!(
        coverage_validate_nullable_con_margine(&celle, 0.0, 3, precisione(), PICCOLO),
        Err(ExtensionError::IssueLimit { limit: 3 })
    ));
}

#[test]
#[allow(clippy::cast_precision_loss)]
fn buffer_a_zig_zag_si_ferma_al_margine() {
    let zigzag = Geometry::LineString(LineString::new(
        (0..400)
            .map(|j| Coord {
                x: f64::from(j).mul_add(0.8, 1_000_000.0),
                y: 5_000_000.0 + if j % 2 == 0 { 0.0 } else { 600.0 },
            })
            .collect(),
    ));
    let senza =
        buffer_with_cap(&zigzag, 200.0, BufferCapStyle::Round, precisione()).expect("buffer");
    assert_eq!(
        buffer_with_cap_con_margine(&zigzag, 200.0, BufferCapStyle::Round, precisione(), GRANDE)
            .expect("buffer"),
        senza
    );
    assert!(matches!(
        buffer_with_cap_con_margine(&zigzag, 200.0, BufferCapStyle::Round, precisione(), PICCOLO),
        Err(OperationError::MargineMemoria(_))
    ));
    // Un poligono: il controllo dei punti dei contorni prima dell'overlay.
    let grande = disco(0.0, 0.0, 1_000.0, 4_000);
    assert!(matches!(
        buffer_with_cap_con_margine(&grande, 10.0, BufferCapStyle::Round, precisione(), PICCOLO),
        Err(OperationError::MargineMemoria(_))
    ));
}

/// Il conto per coppia e' un maggiorante esatto al byte del tetto: con un
/// margine di esattamente `coppie * BYTE_PER_COPPIA` il join riesce, con un
/// byte in meno no; il risultato ha capacita' esatta (nessuna crescita per
/// raddoppio oltre il conto), e cosi' i vicini.
#[test]
fn i_risultati_hanno_capacita_esatta_e_il_conto_e_al_byte() {
    use plenora_kernels_geo::analysis::BYTE_PER_VICINO;
    use plenora_kernels_geo::spatial_join::BYTE_PER_COPPIA;
    let (punti, dischi) = tutti_candidati(60);
    let coppie = 60 * 60;
    let esatto = MargineMemoria::byte(coppie * BYTE_PER_COPPIA);
    let trovate = spatial_join_nullable_validated_con_margine(
        &punti,
        &dischi,
        JoinPredicate::Intersects,
        u64::MAX,
        esatto,
    )
    .expect("margine esatto");
    assert_eq!(trovate.len() as u64, coppie);
    assert_eq!(trovate.capacity(), trovate.len());
    assert!(matches!(
        spatial_join_nullable_validated_con_margine(
            &punti,
            &dischi,
            JoinPredicate::Intersects,
            u64::MAX,
            MargineMemoria::byte(coppie * BYTE_PER_COPPIA - 1),
        ),
        Err(SpatialJoinError::MargineMemoria(_))
    ));
    let centro: Vec<Option<Geometry<f64>>> = (0..50)
        .map(|_| Some(Geometry::Point(Point::new(0.0, 0.0))))
        .collect();
    let anello: Vec<Option<Geometry<f64>>> = (0..40)
        .map(|k| {
            let (x, y) = [(5.0, 0.0), (0.0, 5.0), (-5.0, 0.0), (0.0, -5.0)][k % 4];
            Some(Geometry::Point(Point::new(x, y)))
        })
        .collect();
    let abbinamenti = 50 * 40;
    let vicini = nearest_matches_validated_con_margine(
        &centro,
        &anello,
        None,
        (u64::MAX, u64::MAX),
        MargineMemoria::byte(abbinamenti * BYTE_PER_VICINO),
    )
    .expect("margine esatto");
    assert_eq!(vicini.len() as u64, abbinamenti);
    assert_eq!(vicini.capacity(), vicini.len());
    assert!(matches!(
        nearest_matches_validated_con_margine(
            &centro,
            &anello,
            None,
            (u64::MAX, u64::MAX),
            MargineMemoria::byte(abbinamenti * BYTE_PER_VICINO - 1),
        ),
        Err(AnalysisError::MargineMemoria(_))
    ));
}
