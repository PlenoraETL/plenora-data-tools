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
        for (x, y, lato) in [(9.0, 45.0, 0.5), (-60.0, -30.0, 12.0), (100.0, 10.0, 40.0)] {
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
        // Un quarto di globo, nel verso giusto: accettato.
        let emisfero = LineString::from(vec![
            (-90.0, -1.0),
            (0.0, -1.0),
            (90.0, -1.0),
            (90.0, 89.0),
            (0.0, 89.0),
            (-90.0, 89.0),
            (-90.0, -1.0),
        ]);
        let area = crate::extended_algorithms::geodesic_area_m2(
            &Geometry::Polygon(Polygon::new(emisfero, vec![])),
            &e,
        );
        assert!(area.is_ok(), "{area:?}");
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
