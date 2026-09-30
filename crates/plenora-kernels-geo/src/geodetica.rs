//! L'ellissoide delle misure geodetiche: quello del datum del CRS della
//! colonna, mai un ellissoide di comodo.
//!
//! `geo.geodesic_distance`, `geo.geodesic_line_length`, `geo.geodesic_area`
//! e `geo.bearing` risolvono il problema geodetico (Karney 2013,
//! `geographiclib-rs`) sull'ellissoide del CRS geografico della colonna;
//! `geo.haversine_distance` sulla sfera del suo raggio medio `R1 = a (1 -
//! f / 3)` (IUGG). Il chiamante costruisce il valore dal CRS
//! ([`EllissoideGeodetico::da_crs`]) e lo passa ai kernel come argomento
//! esplicito, senza valore predefinito: nessun kernel sceglie WGS 84 da
//! solo (README, «Misure geodetiche: l'ellissoide del datum»).

use geo::{Coord, LineString};
use geographiclib_rs::Geodesic;
use plenora_core::crs::{CrsError, Ellipsoid, ResolvedCrs};

/// L'ellissoide del datum di un CRS geografico, pronto per il problema
/// geodetico, con la sfera del suo raggio medio.
#[derive(Clone, Copy, Debug)]
pub struct EllissoideGeodetico {
    parametri: Ellipsoid,
    ellissoide: Geodesic,
    sfera: Geodesic,
    raggio_medio_m: f64,
}

impl EllissoideGeodetico {
    /// L'ellissoide di parametri dati: semiasse maggiore finito e
    /// positivo, inverso dello schiacciamento finito e maggiore di 1 (uno
    /// schiacciamento in `(0, 1)`: l'ellissoide oblato dei datum reali).
    /// `None` fuori da questi domini: una sfera o parametri non finiti non
    /// si accettano per ripiego.
    #[must_use]
    pub fn nuovo(parametri: Ellipsoid) -> Option<Self> {
        let a = parametri.semi_major_axis_metre;
        let inverso = parametri.inverse_flattening;
        if !(a.is_finite() && a > 0.0 && inverso.is_finite() && inverso > 1.0) {
            return None;
        }
        let f = 1.0 / inverso;
        // Raggio medio IUGG R1 = (2a + b) / 3 = a (1 - f / 3).
        let raggio_medio_m = a * (1.0 - f / 3.0);
        Some(Self {
            parametri,
            ellissoide: Geodesic::new(a, f),
            sfera: Geodesic::new(raggio_medio_m, 0.0),
            raggio_medio_m,
        })
    }

    /// L'ellissoide del datum del CRS: quello della tabella dei CRS
    /// integrati.
    ///
    /// # Errors
    ///
    /// [`CrsError::EllipsoidRequired`] se il CRS non porta un ellissoide
    /// (un CRS risolto dal chiamante) o se i suoi parametri non sono un
    /// ellissoide oblato valido: le misure geodetiche non ripiegano su
    /// WGS 84.
    pub fn da_crs(crs: &ResolvedCrs) -> Result<Self, CrsError> {
        crs.ellipsoid()
            .and_then(Self::nuovo)
            .ok_or(CrsError::EllipsoidRequired)
    }

    /// I parametri dell'ellissoide.
    #[must_use]
    pub const fn parametri(&self) -> Ellipsoid {
        self.parametri
    }

    /// Il raggio medio `R1 = a (1 - f / 3)`, in metri: la sfera di
    /// `geo.haversine_distance`.
    #[must_use]
    pub const fn raggio_medio_m(&self) -> f64 {
        self.raggio_medio_m
    }

    /// Il problema geodetico sull'ellissoide.
    pub(crate) const fn geodetica(&self) -> &Geodesic {
        &self.ellissoide
    }

    /// Il problema geodetico sulla sfera di raggio [`Self::raggio_medio_m`]
    /// (schiacciamento nullo: cerchi massimi).
    pub(crate) const fn sfera(&self) -> &Geodesic {
        &self.sfera
    }
}

/// Margine assoluto sulle distanze nel piano lon/lat, in gradi (circa
/// 11 µm a terra): copre gli arrotondamenti delle distanze fra segmenti.
const MARGINE_GRADI: f64 = 1e-10;

/// Rotazione massima della tangente di un lato geodetico nel piano
/// lon/lat, in radianti (`K * L`): sotto `pi / 2` la geodetica e' un grafico
/// sulla corda del lato, e il suo scarto dalla corda si limita.
const ROTAZIONE_MASSIMA: f64 = 1.0;

/// Il massimo di `|u| v^2` con `u^2 + v^2 = 1` (`2 / (3 sqrt 3)`),
/// arrotondato per eccesso.
const MASSIMO_U_V2: f64 = 0.385;

/// Un lato di un anello nel piano lon/lat (gradi) con lo scarto massimo,
/// in gradi, fra la sua geodetica e la sua corda.
#[derive(Clone, Copy, Debug)]
struct LatoGeodetico {
    da: Coord<f64>,
    a: Coord<f64>,
    scarto: f64,
}

/// Lo scarto massimo, in gradi del piano lon/lat, fra la geodetica di un
/// lato e la sua corda (il segmento nel piano lon/lat).
///
/// Nel piano (lambda, phi) in radianti la geodetica ha curvatura euclidea
/// `kappa = |sin phi| |u| (A u^2 + B v^2) / (u^2 + v^2)^(3/2)`, con
/// `(u, v)` la velocita' nel piano, `A = (N / M) cos phi` e
/// `B = 3 e^2 cos phi / W^2 + 2 (M / N) / cos phi` (dalle equazioni delle
/// geodetiche della metrica `M^2 dphi^2 + (N cos phi)^2 dlambda^2`). Con
/// `N / M <= 1 / (1 - e^2)`, `M / N <= 1`, `W^2 >= 1 - e^2`, `|u|^3 <= 1`,
/// `|u| v^2 <= 0.385` e `|sin phi cos phi| <= min(1/2, sin phi_max)`:
///
/// `kappa <= K = s / (1 - e^2) + 0.385 (3 e^2 s / (1 - e^2) + 2 tan phi_max)`,
/// `s = min(1/2, sin phi_max)`,
///
/// con `phi_max` la latitudine massima in modulo lungo la geodetica (quella
/// degli estremi o del vertice, se il lato lo contiene). La lunghezza nel
/// piano e' al piu' `L = s12 / (a min(1 - e^2, cos phi_max))`. Se `K L <= 1`
/// la tangente ruota di al piu' `K L` rispetto alla corda (in un punto le e'
/// parallela), quindi la geodetica e' il grafico `h(t)` di una funzione
/// sulla corda di lunghezza `l`, con `|h''| <= K / cos^3(K L)` e
/// `h(0) = h(l) = 0`: `|h| <= l^2 / 8 * K / cos^3(K L)`.
///
/// `Err` per un lato su un polo o con `K L > 1` (troppo lungo per la
/// verifica).
fn scarto_del_lato(
    ellissoide: &EllissoideGeodetico,
    da: Coord<f64>,
    a: Coord<f64>,
) -> Result<f64, &'static str> {
    use geographiclib_rs::InverseGeodesic as _;
    const LATO_TROPPO_LUNGO: &str =
        "lato troppo lungo o troppo vicino a un polo per verificare che la \
         topologia delle geodetiche sia quella del piano lon/lat";
    let parametri = ellissoide.parametri();
    let semiasse = parametri.semi_major_axis_metre;
    let f = 1.0 / parametri.inverse_flattening;
    let e2 = f * (2.0 - f);
    let (s12, azimut1, azimut2, _arco): (f64, f64, f64, f64) =
        ellissoide.geodetica().inverse(da.y, da.x, a.y, a.x);
    if !(s12.is_finite() && azimut1.is_finite() && azimut2.is_finite()) {
        return Err(LATO_TROPPO_LUNGO);
    }
    let mut latitudine_massima = da.y.abs().max(a.y.abs());
    // Vertice dentro il lato: la latitudine cambia verso (verso il polo
    // alla partenza, verso l'equatore all'arrivo), o un azimut e' quasi
    // est-ovest (vertice vicino a un estremo).
    let (c1, c2) = (azimut1.to_radians().cos(), azimut2.to_radians().cos());
    let vertice_dentro = c1 * c2 < 0.0 || c1.abs() < 1e-9 || c2.abs() < 1e-9;
    if vertice_dentro {
        // Clairaut: cos(beta0) = |cos(beta1) sin(alpha1)|, beta ridotta.
        let beta1 = ((1.0 - f) * da.y.to_radians().tan()).atan();
        let coseno = (beta1.cos() * azimut1.to_radians().sin()).abs().min(1.0);
        let beta0 = coseno.acos();
        let vertice = (beta0.tan() / (1.0 - f)).atan().to_degrees().abs();
        latitudine_massima = latitudine_massima.max(vertice);
    }
    let phi = latitudine_massima.to_radians();
    let (seno, coseno) = phi.sin_cos();
    let tangente = phi.tan();
    if !(tangente.is_finite() && coseno > 0.0) {
        return Err(LATO_TROPPO_LUNGO);
    }
    let s = seno.min(0.5);
    let curvatura = MASSIMO_U_V2.mul_add(
        2.0f64.mul_add(tangente, 3.0 * e2 * s / (1.0 - e2)),
        s / (1.0 - e2),
    );
    let lunghezza = s12 / (semiasse * (1.0 - e2).min(coseno));
    let rotazione = curvatura * lunghezza;
    if !(rotazione.is_finite() && rotazione <= ROTAZIONE_MASSIMA) {
        return Err(LATO_TROPPO_LUNGO);
    }
    let corda = (a.x - da.x).to_radians().hypot((a.y - da.y).to_radians());
    let scarto = corda * corda / 8.0 * curvatura / rotazione.cos().powi(3);
    let scarto = scarto.to_degrees() * (1.0 + 1e-6);
    if scarto.is_finite() {
        Ok(scarto)
    } else {
        Err(LATO_TROPPO_LUNGO)
    }
}

/// `distanza > soglia`, falso anche per un NaN.
fn oltre(distanza: f64, soglia: f64) -> bool {
    distanza.partial_cmp(&soglia) == Some(std::cmp::Ordering::Greater)
}

/// Verifica che gli anelli, letti come geodetiche, abbiano la topologia che
/// hanno nel piano lon/lat (dove la validazione OGC li ha accettati): anelli
/// semplici, che non si incrociano, con le stesse relazioni di contenimento
/// (buchi dentro l'esterno, parti disgiunte).
///
/// Regola, dimostrabile: ogni geodetica sta entro il suo `scarto` dalla
/// corda ([`scarto_del_lato`]), e
///
/// - due lati senza estremi comuni hanno corde a distanza maggiore della
///   somma degli scarti (i tubi non si toccano);
/// - due lati con un estremo comune `v`: l'altro estremo di ciascuno dista
///   dalla corda dell'altro piu' del suo scarto. Due geodetiche minime
///   uscenti da `v` non si incontrano di nuovo se non in un estremo (oltre
///   un punto d'incontro nessuna delle due sarebbe minima), e un estremo
///   sulla geodetica dell'altro starebbe entro il suo scarto;
/// - due lati con entrambi gli estremi comuni si rifiutano.
///
/// Allora gli anelli geodetici sono semplici e si toccano solo nei vertici
/// comuni, e la deformazione di ogni corda nella sua geodetica, dentro il
/// suo tubo, non attraversa i vertici non comuni degli altri anelli: il
/// numero di avvolgimento di ognuno rispetto a ogni anello e' quello del
/// piano, e il contenimento e' lo stesso. Il segno dell'area di ogni anello
/// ([`crate::extended_algorithms`]) conferma il verso.
///
/// # Errors
///
/// Un messaggio senza coordinate se un lato e' troppo lungo per la verifica
/// o se due lati non rispettano la regola.
pub(crate) fn verifica_topologia_geodetica(
    anelli: &[&LineString<f64>],
    ellissoide: &EllissoideGeodetico,
) -> Result<(), &'static str> {
    use geo::algorithm::line_measures::{Distance, Euclidean};
    use rstar::{primitives::GeomWithData, primitives::Rectangle, RTree, AABB};
    const VICINI: &str = "due lati del poligono sono piu' vicini dello scarto fra le loro \
         geodetiche e le corde nel piano lon/lat: la topologia delle geodetiche non e' \
         garantita";

    let mut lati: Vec<LatoGeodetico> = Vec::new();
    for anello in anelli {
        for lato in anello.lines() {
            if lato.start == lato.end {
                continue;
            }
            lati.push(LatoGeodetico {
                da: lato.start,
                a: lato.end,
                scarto: scarto_del_lato(ellissoide, lato.start, lato.end)?,
            });
        }
    }
    let riquadro = |lato: &LatoGeodetico| {
        let bordo = lato.scarto + MARGINE_GRADI;
        AABB::from_corners(
            [
                lato.da.x.min(lato.a.x) - bordo,
                lato.da.y.min(lato.a.y) - bordo,
            ],
            [
                lato.da.x.max(lato.a.x) + bordo,
                lato.da.y.max(lato.a.y) + bordo,
            ],
        )
    };
    let albero: RTree<GeomWithData<Rectangle<[f64; 2]>, usize>> = RTree::bulk_load(
        lati.iter()
            .enumerate()
            .map(|(indice, lato)| {
                let aabb = riquadro(lato);
                GeomWithData::new(Rectangle::from_aabb(aabb), indice)
            })
            .collect(),
    );
    let segmento = |lato: &LatoGeodetico| geo::Line::new(lato.da, lato.a);
    for (i, lato) in lati.iter().enumerate() {
        for vicino in albero.locate_in_envelope_intersecting(&riquadro(lato)) {
            let j = vicino.data;
            if j <= i {
                continue;
            }
            let altro = &lati[j];
            let comuni = [
                (lato.da == altro.da, lato.a, altro.a),
                (lato.da == altro.a, lato.a, altro.da),
                (lato.a == altro.da, lato.da, altro.a),
                (lato.a == altro.a, lato.da, altro.da),
            ];
            let condivisi: Vec<(Coord<f64>, Coord<f64>)> = comuni
                .iter()
                .filter(|(comune, _, _)| *comune)
                .map(|&(_, mio, suo)| (mio, suo))
                .collect();
            match condivisi.as_slice() {
                [] => {
                    let distanza = Euclidean.distance(&segmento(lato), &segmento(altro));
                    if !oltre(distanza, lato.scarto + altro.scarto + MARGINE_GRADI) {
                        return Err(VICINI);
                    }
                }
                [(mio, suo)] => {
                    let mio_dall_altro =
                        Euclidean.distance(&geo::Point::from(*mio), &segmento(altro));
                    let suo_dal_mio = Euclidean.distance(&geo::Point::from(*suo), &segmento(lato));
                    if !(oltre(mio_dall_altro, altro.scarto + MARGINE_GRADI)
                        && oltre(suo_dal_mio, lato.scarto + MARGINE_GRADI))
                    {
                        return Err(VICINI);
                    }
                }
                _ => return Err(VICINI),
            }
        }
    }
    Ok(())
}

/// L'ellissoide WGS 84 della tabella integrata, per i test dei kernel.
#[cfg(test)]
pub(crate) fn wgs84_di_prova() -> EllissoideGeodetico {
    plenora_core::crs::resolve_crs("EPSG:4326", "crs")
        .ok()
        .and_then(|crs| EllissoideGeodetico::da_crs(&crs).ok())
        .unwrap_or_else(|| unreachable!("EPSG:4326 e' nella tabella integrata"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn i_parametri_fuori_dominio_non_danno_un_ellissoide() {
        for (a, inverso) in [
            (0.0, 298.0),
            (-1.0, 298.0),
            (f64::NAN, 298.0),
            (f64::INFINITY, 298.0),
            (6_378_137.0, 1.0),
            (6_378_137.0, 0.5),
            (6_378_137.0, f64::INFINITY),
            (6_378_137.0, f64::NAN),
        ] {
            assert!(
                EllissoideGeodetico::nuovo(Ellipsoid {
                    semi_major_axis_metre: a,
                    inverse_flattening: inverso,
                })
                .is_none(),
                "{a} {inverso}"
            );
        }
    }

    #[test]
    fn un_crs_senza_ellissoide_si_rifiuta() {
        let crs = ResolvedCrs::from_resolved_parts(
            "EPSG:4326".to_owned(),
            serde_json::json!({}),
            plenora_core::crs::CrsKind::Geographic,
            None,
        );
        assert!(matches!(
            EllissoideGeodetico::da_crs(&crs),
            Err(CrsError::EllipsoidRequired)
        ));
    }

    #[test]
    fn il_raggio_medio_di_wgs_84_e_quello_iugg() {
        let crs = plenora_core::crs::resolve_crs("EPSG:4326", "crs").expect("crs");
        let ellissoide = EllissoideGeodetico::da_crs(&crs).expect("ellissoide");
        // (2a + b) / 3 con b = 6 356 752,314 245 m.
        assert!((ellissoide.raggio_medio_m() - 6_371_008.771_415).abs() < 1e-5);
    }
    /// Oracolo sul percorso generico: con l'ellissoide WGS 84 della tabella
    /// i kernel rendono, bit per bit, cio' che rendevano con `Geodesic`,
    /// `Bearing` e `GeodesicArea` di `geo` (WGS 84 fisso), su punti,
    /// linee e poligoni deterministici.
    #[test]
    #[allow(clippy::float_cmp)] // Uguaglianza al bit: e' il contratto.
    fn con_wgs84_i_kernel_coincidono_con_geo_al_bit() {
        use geo::algorithm::line_measures::{Bearing, Distance, Geodesic, Length};
        use geo::algorithm::orient::{Direction, Orient};
        use geo::{GeodesicArea, Geometry, LineString, Point, Polygon};

        let e = wgs84_di_prova();
        let mut punti = Vec::new();
        for i in 0..12_u32 {
            for j in 0..9_u32 {
                let lon = f64::from(i).mul_add(29.7, -178.3);
                let lat = f64::from(j).mul_add(19.3, -77.9);
                punti.push(Point::new(lon, lat));
            }
        }
        for (k, &da) in punti.iter().enumerate() {
            let a = punti[(k * 7 + 3) % punti.len()];
            let attesa = Geodesic.distance(da, a);
            let calcolata = crate::extended::geodesic_distance_m(da, a, &e).expect("distanza");
            assert_eq!(calcolata.to_bits(), attesa.to_bits(), "{da:?} {a:?}");
            if let Ok(azimut) = crate::extended_algorithms::geodesic_bearing_degrees(da, a, &e) {
                assert_eq!(azimut.to_bits(), Geodesic.bearing(da, a).to_bits());
            }
        }
        let linea = LineString::from(punti.iter().take(20).map(|p| p.0).collect::<Vec<_>>());
        assert_eq!(
            crate::extended::geodesic_line_length_m(&linea, &e)
                .expect("lunghezza")
                .to_bits(),
            Geodesic.length(&linea).to_bits()
        );
        for (x, y, lato) in [(9.0, 45.0, 0.5), (-60.0, -30.0, 12.0), (100.0, 10.0, 4.0)] {
            let esterno = LineString::from(vec![
                (x, y),
                (x + lato, y),
                (x + lato, y + lato),
                (x, y + lato),
                (x, y),
            ]);
            let buco = LineString::from(vec![
                (x + lato / 4.0, y + lato / 4.0),
                (x + lato / 4.0, y + lato / 2.0),
                (x + lato / 2.0, y + lato / 2.0),
                (x + lato / 2.0, y + lato / 4.0),
                (x + lato / 4.0, y + lato / 4.0),
            ]);
            for poligono in [
                Polygon::new(esterno.clone(), vec![]),
                Polygon::new(esterno, vec![buco]),
            ] {
                let attesa = poligono.orient(Direction::Default).geodesic_area_unsigned();
                let calcolata =
                    crate::extended_algorithms::geodesic_area_m2(&Geometry::Polygon(poligono), &e)
                        .expect("area");
                assert_eq!(calcolata.to_bits(), attesa.to_bits());
            }
        }
    }

    #[test]
    fn la_sfera_agli_antipodi_non_rende_nan() {
        let e = wgs84_di_prova();
        let semicirconferenza = std::f64::consts::PI * e.raggio_medio_m();
        for (da, a) in [
            (geo::Point::new(0.0, 0.0), geo::Point::new(180.0, 0.0)),
            (geo::Point::new(-180.0, 0.0), geo::Point::new(0.0, 0.0)),
            (geo::Point::new(10.0, 45.0), geo::Point::new(-170.0, -45.0)),
            (geo::Point::new(0.0, 90.0), geo::Point::new(123.0, -90.0)),
            // Quasi antipodi: l'emiseno arrotondato supera 1 (NaN con
            // `2 asin(sqrt h)`).
            (geo::Point::new(0.0, 0.0), geo::Point::new(180.0, 1e-300)),
            (
                geo::Point::new(37.123_456_789, 12.5),
                geo::Point::new(-142.876_543_211, -12.5),
            ),
        ] {
            let distanza = crate::extended::haversine_distance_m(da, a, &e).expect("distanza");
            assert!(distanza.is_finite());
            assert!(
                (distanza - semicirconferenza).abs() < 1e-6,
                "{da:?} {a:?}: {distanza}"
            );
        }
    }

    #[test]
    fn l_azimut_non_definito_si_rifiuta() {
        use crate::extended_algorithms::{geodesic_bearing_degrees, ExtendedAlgorithmError};
        use geo::Point;

        let e = wgs84_di_prova();
        for (da, a) in [
            // Coincidenti, anche con -180 e 180 alla stessa latitudine.
            (Point::new(7.0, 44.0), Point::new(7.0, 44.0)),
            (Point::new(-180.0, 12.0), Point::new(180.0, 12.0)),
            // Origine su un polo.
            (Point::new(0.0, 90.0), Point::new(10.0, 45.0)),
            (Point::new(0.0, -90.0), Point::new(10.0, 45.0)),
            // Antipodi e luogo di taglio.
            (Point::new(0.0, 30.0), Point::new(180.0, -30.0)),
            (Point::new(0.0, 0.0), Point::new(180.0, 0.0)),
            (Point::new(0.0, 0.0), Point::new(179.9, 0.0)),
        ] {
            assert!(
                matches!(
                    geodesic_bearing_degrees(da, a, &e),
                    Err(ExtendedAlgorithmError::AzimutNonDefinito(_))
                ),
                "{da:?} {a:?}"
            );
        }
        // Verso un polo, latitudini opposte ma geodetica unica: definiti.
        for (da, a) in [
            (Point::new(12.0, 60.0), Point::new(12.0, 90.0)),
            (Point::new(10.0, 20.0), Point::new(30.0, -20.0)),
            (Point::new(0.0, 30.0), Point::new(179.0, -30.0)),
        ] {
            assert!(geodesic_bearing_degrees(da, a, &e).is_ok(), "{da:?} {a:?}");
        }
    }

    /// Il triangolo (0 30, 170 30, 85 31) e' valido e antiorario nel piano,
    /// ma il lato geodetico (0 30)-(170 30) sale oltre 80 gradi, sopra il
    /// terzo vertice: le geodetiche girano in verso orario, e l'area senza
    /// segno sarebbe quella del resto del globo. Si rifiuta; lo stesso
    /// anello come buco, anche.
    #[test]
    fn un_anello_che_gira_al_contrario_sul_globo_si_rifiuta() {
        use geo::{Geometry, LineString, Polygon};
        let e = wgs84_di_prova();
        let triangolo =
            LineString::from(vec![(0.0, 30.0), (170.0, 30.0), (85.0, 31.0), (0.0, 30.0)]);
        let poligono = Geometry::Polygon(Polygon::new(triangolo, vec![]));
        assert!(matches!(
            crate::extended_algorithms::geodesic_area_m2(&poligono, &e),
            Err(crate::extended_algorithms::ExtendedAlgorithmError::InvalidInput(_))
        ));
    }

    /// Il controesempio della seconda lettura: valido nel piano, nessun lato
    /// oltre 180 gradi, versi giusti, ma la geodetica del lato inferiore
    /// dell'esterno sale oltre 70 gradi nord vicino alla longitudine 0, e il
    /// «buco» a 40-41 gradi sta fuori dall'esterno geodetico. Prima si
    /// sottraeva e l'area era un numero plausibile e sbagliato.
    #[test]
    fn un_buco_fuori_dall_esterno_geodetico_si_rifiuta() {
        use geo::{Geometry, Polygon};
        let e = wgs84_di_prova();
        let poligono: Polygon<f64> = wkt::TryFromWkt::try_from_wkt_str(
            "POLYGON((-80 30,80 30,80 80,-80 80,-80 30),(-1 40,-1 41,1 41,1 40,-1 40))",
        )
        .expect("wkt");
        assert!(matches!(
            crate::extended_algorithms::geodesic_area_m2(&Geometry::Polygon(poligono), &e),
            Err(crate::extended_algorithms::ExtendedAlgorithmError::InvalidInput(_))
        ));
    }

    /// Stessa classe fra le parti di un multi-poligono: disgiunte nel piano,
    /// sovrapposte sul globo (il lato superiore della fascia sale a 38,1
    /// gradi a longitudine 0, dentro il quadratino a 38-39 gradi).
    #[test]
    fn parti_sovrapposte_solo_sul_globo_si_rifiutano() {
        use geo::{Geometry, MultiPolygon};
        let e = wgs84_di_prova();
        let parti: MultiPolygon<f64> = wkt::TryFromWkt::try_from_wkt_str(
            "MULTIPOLYGON(((-40 29,40 29,40 31,-40 31,-40 29)),((-1 38,1 38,1 39,-1 39,-1 38)))",
        )
        .expect("wkt");
        assert!(matches!(
            crate::extended_algorithms::geodesic_area_m2(&Geometry::MultiPolygon(parti), &e),
            Err(crate::extended_algorithms::ExtendedAlgorithmError::InvalidInput(_))
        ));
    }

    /// Lati corti: un buco a 1 m dall'esterno, con lati di circa 100 m,
    /// passa (lo scarto geodetica-corda e' sotto il millimetro); a 1 mm, con
    /// lati di 100 km, si rifiuta.
    #[test]
    fn la_verifica_accetta_i_dati_ordinari_e_rifiuta_i_vicini() {
        use geo::{Geometry, Polygon};
        let e = wgs84_di_prova();
        let grado = 1.0 / 111_000.0;
        let (x, y, lato) = (11.0, 44.0, 100.0 * grado);
        let esterno = format!(
            "({x} {y},{x2} {y},{x2} {y2},{x} {y2},{x} {y})",
            x2 = x + lato,
            y2 = y + lato
        );
        let d = grado; // 1 m
        let buco = format!(
            "({a} {b},{a} {c},{d2} {c},{d2} {b},{a} {b})",
            a = x + d,
            b = y + d,
            c = y + lato / 2.0,
            d2 = x + lato / 2.0
        );
        let ordinario: Polygon<f64> =
            wkt::TryFromWkt::try_from_wkt_str(&format!("POLYGON({esterno},{buco})")).expect("wkt");
        crate::extended_algorithms::geodesic_area_m2(&Geometry::Polygon(ordinario), &e)
            .expect("dati ordinari");
        let lato = 100_000.0 * grado;
        let d = grado / 1000.0; // 1 mm
        let esterno = format!(
            "({x} {y},{x2} {y},{x2} {y2},{x} {y2},{x} {y})",
            x2 = x + lato,
            y2 = y + lato
        );
        let buco = format!(
            "({a} {b},{a} {c},{d2} {c},{d2} {b},{a} {b})",
            a = x + lato / 4.0,
            b = y + d,
            c = y + lato / 2.0,
            d2 = x + lato / 2.0
        );
        let vicino: Polygon<f64> =
            wkt::TryFromWkt::try_from_wkt_str(&format!("POLYGON({esterno},{buco})")).expect("wkt");
        assert!(matches!(
            crate::extended_algorithms::geodesic_area_m2(&Geometry::Polygon(vicino), &e),
            Err(crate::extended_algorithms::ExtendedAlgorithmError::InvalidInput(_))
        ));
    }

    #[test]
    fn un_poligono_sull_antimeridiano_si_rifiuta() {
        use geo::{Geometry, LineString, Polygon};
        let e = wgs84_di_prova();
        for anello in [
            vec![
                (170.0, -5.0),
                (-170.0, -5.0),
                (-170.0, 5.0),
                (170.0, 5.0),
                (170.0, -5.0),
            ],
            vec![
                (-180.0, 80.0),
                (180.0, 80.0),
                (180.0, 90.0),
                (-180.0, 90.0),
                (-180.0, 80.0),
            ],
        ] {
            let poligono = Geometry::Polygon(Polygon::new(LineString::from(anello), vec![]));
            assert!(matches!(
                crate::extended_algorithms::geodesic_area_m2(&poligono, &e),
                Err(crate::extended_algorithms::ExtendedAlgorithmError::InvalidInput(_))
            ));
        }
    }
}
