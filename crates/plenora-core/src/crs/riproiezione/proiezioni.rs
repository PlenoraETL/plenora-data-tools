//! Metodi di proiezione della tabella, in avanti (lon/lat in gradi ->
//! easting/northing in metri) e all'indietro.
//!
//! Formule:
//! - Transverse Mercator: serie di Krueger al sesto ordine in `n` nella
//!   forma di Karney (2011, eq. 35-36; `GeographicLib` `TransverseMercator`),
//!   errore di pochi nanometri entro 3900 km dal meridiano centrale;
//! - Mercator (variant A), Pseudo Mercator, Lambert Conic Conformal (2SP),
//!   Lambert Azimuthal Equal Area, Oblique Stereographic: EPSG Guidance
//!   Note 7-2, con le latitudini
//!   ausiliarie calcolate in forma chiusa o per Newton a precisione di
//!   macchina ([`super::latitudine`]) al posto delle serie troncate;
//! - Hotine Oblique Mercator (variant B) con azimut e angolo del reticolo di
//!   90 gradi (EPSG:2056): Swiss Oblique Mercator come PROJ ([`Svizzera`]).
//!
//! L'oracolo contro PROJ 9.5.1 e' in `tests/riproiezione_oracolo.rs`.

// Formule trascritte dalla nota EPSG 7-2 e da Karney: i nomi brevi sono
// quelli delle fonti, e la forma a*b+c resta quella delle fonti (mul_add
// cambierebbe l'arrotondamento senza guadagno misurabile).
#![allow(
    clippy::many_single_char_names,
    clippy::similar_names,
    clippy::suboptimal_flops
)]

use std::f64::consts::FRAC_PI_2;

use super::super::Ellipsoid;
use super::latitudine::{da_isometrica, da_q_autalica, isometrica, q_autalica, tau_da_taup, taup};
use super::tabella::MetodoProiezione;

/// Un metodo di proiezione con le costanti gia' calcolate.
#[derive(Clone, Debug)]
pub(in crate::crs) enum Proiezione {
    Geografico,
    Tm(Tm),
    Mercatore(Mercatore),
    PseudoMercatore(PseudoMercatore),
    Lcc(Lcc),
    Laea(Laea),
    Stereo(Stereo),
    Svizzera(Svizzera),
}

/// Perche' una proiezione non e' costruibile o un punto non e' calcolabile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::crs) enum ErroreProiezione {
    /// Parametri della tabella fuori dalle ipotesi del metodo.
    Parametri,
    /// Punto fuori dal dominio matematico del metodo (per esempio oltre 90
    /// gradi dal meridiano centrale in Transverse Mercator, o il polo in
    /// Mercator).
    FuoriDominio,
    /// Un'iterazione non ha raggiunto la precisione di macchina.
    NonConvergente,
}

fn eccentricita(ellissoide: Ellipsoid) -> (f64, f64, f64) {
    let f = 1.0 / ellissoide.inverse_flattening;
    let e2 = f * (2.0 - f);
    (ellissoide.semi_major_axis_metre, e2, e2.sqrt())
}

/// Differenza di longitudine ridotta a `[-pi, pi]`.
fn riduci(lambda: f64) -> f64 {
    let ridotta = lambda.rem_euclid(std::f64::consts::TAU);
    if ridotta > std::f64::consts::PI {
        ridotta - std::f64::consts::TAU
    } else {
        ridotta
    }
}

/// Longitudine in gradi ridotta a `[-180, 180]`.
pub(in crate::crs) fn riduci_gradi(lon: f64) -> f64 {
    if (-180.0..=180.0).contains(&lon) {
        return lon;
    }
    let ridotta = lon.rem_euclid(360.0);
    if ridotta > 180.0 {
        ridotta - 360.0
    } else {
        ridotta
    }
}

impl Proiezione {
    pub(in crate::crs) fn nuova(
        metodo: &MetodoProiezione,
        ellissoide: Ellipsoid,
    ) -> Result<Self, ErroreProiezione> {
        let (a, e2, e) = eccentricita(ellissoide);
        Ok(match *metodo {
            MetodoProiezione::Geografico => Self::Geografico,
            MetodoProiezione::TrasversaDiMercatore {
                lat0,
                lon0,
                k0,
                falsa_est,
                falsa_nord,
            } => Self::Tm(Tm::nuova(a, e2, e, lat0, lon0, k0, falsa_est, falsa_nord)),
            MetodoProiezione::MercatoreA {
                lat0,
                lon0,
                k0,
                falsa_est,
                falsa_nord,
            } => {
                if lat0 != 0.0 {
                    return Err(ErroreProiezione::Parametri);
                }
                Self::Mercatore(Mercatore {
                    a,
                    e,
                    lon0: lon0.to_radians(),
                    k0,
                    falsa_est,
                    falsa_nord,
                })
            }
            MetodoProiezione::PseudoMercatore {
                lat0,
                lon0,
                falsa_est,
                falsa_nord,
            } => {
                if lat0 != 0.0 {
                    return Err(ErroreProiezione::Parametri);
                }
                Self::PseudoMercatore(PseudoMercatore {
                    a,
                    lon0: lon0.to_radians(),
                    falsa_est,
                    falsa_nord,
                })
            }
            MetodoProiezione::LambertConicaConforme2Sp {
                lat_origine,
                lon_origine,
                lat1,
                lat2,
                est_origine,
                nord_origine,
            } => Self::Lcc(Lcc::nuova(
                a,
                e2,
                e,
                [lat_origine, lon_origine, lat1, lat2],
                est_origine,
                nord_origine,
            )?),
            MetodoProiezione::LambertAzimutaleEquivalente {
                lat0,
                lon0,
                falsa_est,
                falsa_nord,
            } => Self::Laea(Laea::nuova(a, e2, e, lat0, lon0, falsa_est, falsa_nord)),
            MetodoProiezione::StereograficaObliqua {
                lat0,
                lon0,
                k0,
                falsa_est,
                falsa_nord,
            } => Self::Stereo(Stereo::nuova(
                a, e2, e, lat0, lon0, k0, falsa_est, falsa_nord,
            )),
            MetodoProiezione::HotineObliquaB {
                lat_centro,
                lon_centro,
                azimut,
                angolo_reticolo,
                k_centro,
                est_centro,
                nord_centro,
            } => Self::Svizzera(Svizzera::nuova(
                a,
                e2,
                e,
                [lat_centro, lon_centro, azimut, angolo_reticolo, k_centro],
                est_centro,
                nord_centro,
            )?),
        })
    }

    /// Da longitudine e latitudine geodetiche (gradi) a coordinate
    /// proiettate (metri); identita' per i geografici.
    pub(in crate::crs) fn avanti(
        &self,
        lon: f64,
        lat: f64,
    ) -> Result<(f64, f64), ErroreProiezione> {
        let (lambda, phi) = (lon.to_radians(), lat.to_radians());
        match self {
            Self::Geografico => Ok((lon, lat)),
            Self::Tm(p) => p.avanti(lambda, phi),
            Self::Mercatore(p) => p.avanti(lambda, phi),
            Self::PseudoMercatore(p) => p.avanti(lambda, phi),
            Self::Lcc(p) => p.avanti(lambda, phi),
            Self::Laea(p) => p.avanti(lambda, phi),
            Self::Stereo(p) => Ok(p.avanti(lambda, phi)),
            Self::Svizzera(p) => p.avanti(lambda, phi),
        }
    }

    /// Da coordinate proiettate (metri) a longitudine e latitudine
    /// geodetiche (gradi, longitudine in `[-180, 180]`).
    pub(in crate::crs) fn indietro(&self, x: f64, y: f64) -> Result<(f64, f64), ErroreProiezione> {
        let (lambda, phi) = match self {
            Self::Geografico => return Ok((x, y)),
            Self::Tm(p) => p.indietro(x, y),
            Self::Mercatore(p) => p.indietro(x, y),
            Self::PseudoMercatore(p) => p.indietro(x, y),
            Self::Lcc(p) => p.indietro(x, y)?,
            Self::Laea(p) => p.indietro(x, y)?,
            Self::Stereo(p) => p.indietro(x, y),
            Self::Svizzera(p) => p.indietro(x, y),
        };
        if !(lambda.is_finite() && phi.is_finite()) {
            return Err(ErroreProiezione::FuoriDominio);
        }
        Ok((riduci(lambda).to_degrees(), phi.to_degrees()))
    }
}

// ---------------------------------------------------------------------------
// Transverse Mercator (Krueger/Karney, sesto ordine).
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub(in crate::crs) struct Tm {
    e: f64,
    lon0: f64,
    /// `k0 * A`, il raggio rettificante scalato.
    k0a: f64,
    alfa: [f64; 6],
    beta: [f64; 6],
    /// `xi` dell'origine (meridiano centrale, latitudine d'origine).
    xi0: f64,
    falsa_est: f64,
    falsa_nord: f64,
}

impl Tm {
    #[allow(clippy::too_many_arguments)]
    fn nuova(
        a: f64,
        _e2: f64,
        e: f64,
        lat0: f64,
        lon0: f64,
        k0: f64,
        falsa_est: f64,
        falsa_nord: f64,
    ) -> Self {
        let f = 1.0 - (1.0 - e * e).sqrt();
        let n = f / (2.0 - f);
        let n2 = n * n;
        let n3 = n2 * n;
        let n4 = n3 * n;
        let n5 = n4 * n;
        let n6 = n5 * n;
        // Karney 2011, eq. 14 (A) e 35-36 (alfa, beta).
        let raggio = a / (1.0 + n) * (1.0 + n2 / 4.0 + n4 / 64.0 + n6 / 256.0);
        let alfa = [
            n / 2.0 - 2.0 * n2 / 3.0 + 5.0 * n3 / 16.0 + 41.0 * n4 / 180.0 - 127.0 * n5 / 288.0
                + 7891.0 * n6 / 37800.0,
            13.0 * n2 / 48.0 - 3.0 * n3 / 5.0 + 557.0 * n4 / 1440.0 + 281.0 * n5 / 630.0
                - 1_983_433.0 * n6 / 1_935_360.0,
            61.0 * n3 / 240.0 - 103.0 * n4 / 140.0
                + 15061.0 * n5 / 26880.0
                + 167_603.0 * n6 / 181_440.0,
            49561.0 * n4 / 161_280.0 - 179.0 * n5 / 168.0 + 6_601_661.0 * n6 / 7_257_600.0,
            34729.0 * n5 / 80640.0 - 3_418_889.0 * n6 / 1_995_840.0,
            212_378_941.0 * n6 / 319_334_400.0,
        ];
        let beta = [
            n / 2.0 - 2.0 * n2 / 3.0 + 37.0 * n3 / 96.0 - n4 / 360.0 - 81.0 * n5 / 512.0
                + 96199.0 * n6 / 604_800.0,
            n2 / 48.0 + n3 / 15.0 - 437.0 * n4 / 1440.0 + 46.0 * n5 / 105.0
                - 1_118_711.0 * n6 / 3_870_720.0,
            17.0 * n3 / 480.0 - 37.0 * n4 / 840.0 - 209.0 * n5 / 4480.0 + 5569.0 * n6 / 90720.0,
            4397.0 * n4 / 161_280.0 - 11.0 * n5 / 504.0 - 830_251.0 * n6 / 7_257_600.0,
            4583.0 * n5 / 161_280.0 - 108_847.0 * n6 / 3_991_680.0,
            20_648_693.0 * n6 / 638_668_800.0,
        ];
        let mut tm = Self {
            e,
            lon0: lon0.to_radians(),
            k0a: k0 * raggio,
            alfa,
            beta,
            xi0: 0.0,
            falsa_est,
            falsa_nord,
        };
        let (xi0, _) = tm.xi_eta(lat0.to_radians(), 0.0);
        tm.xi0 = xi0;
        tm
    }

    /// `(xi, eta)` normalizzati sul raggio rettificante (Karney eq. 11 e
    /// 35), per `|lambda| <= pi/2`.
    fn xi_eta(&self, phi: f64, lambda: f64) -> (f64, f64) {
        let c = lambda.cos().max(0.0);
        let (xip, etap) = if phi.abs() >= FRAC_PI_2 {
            (FRAC_PI_2.copysign(phi), 0.0)
        } else {
            let tp = taup(phi.tan(), self.e);
            (tp.atan2(c), (lambda.sin() / tp.hypot(c)).asinh())
        };
        let mut xi = xip;
        let mut eta = etap;
        for (j, alfa) in (1..=6).zip(self.alfa) {
            let k = 2.0 * f64::from(j);
            xi += alfa * (k * xip).sin() * (k * etap).cosh();
            eta += alfa * (k * xip).cos() * (k * etap).sinh();
        }
        (xi, eta)
    }

    fn avanti(&self, lambda: f64, phi: f64) -> Result<(f64, f64), ErroreProiezione> {
        let dl = riduci(lambda - self.lon0);
        if dl.abs() >= FRAC_PI_2 {
            return Err(ErroreProiezione::FuoriDominio);
        }
        let (xi, eta) = self.xi_eta(phi, dl);
        Ok((
            self.falsa_est + self.k0a * eta,
            self.falsa_nord + self.k0a * (xi - self.xi0),
        ))
    }

    fn indietro(&self, x: f64, y: f64) -> (f64, f64) {
        let xi = (y - self.falsa_nord) / self.k0a + self.xi0;
        let eta = (x - self.falsa_est) / self.k0a;
        let mut xip = xi;
        let mut etap = eta;
        for (j, beta) in (1..=6).zip(self.beta) {
            let k = 2.0 * f64::from(j);
            xip -= beta * (k * xi).sin() * (k * eta).cosh();
            etap -= beta * (k * xi).cos() * (k * eta).sinh();
        }
        let s = etap.sinh();
        let c = xip.cos().max(0.0);
        let r = s.hypot(c);
        if r == 0.0 {
            return (self.lon0, FRAC_PI_2.copysign(xip));
        }
        let lambda = s.atan2(c);
        let phi = tau_da_taup(xip.sin() / r, self.e).atan();
        (self.lon0 + lambda, phi)
    }
}

// ---------------------------------------------------------------------------
// Mercator (variant A) e Pseudo Mercator.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub(in crate::crs) struct Mercatore {
    a: f64,
    e: f64,
    lon0: f64,
    k0: f64,
    falsa_est: f64,
    falsa_nord: f64,
}

impl Mercatore {
    fn avanti(&self, lambda: f64, phi: f64) -> Result<(f64, f64), ErroreProiezione> {
        if phi.abs() >= FRAC_PI_2 {
            return Err(ErroreProiezione::FuoriDominio);
        }
        let ak = self.a * self.k0;
        Ok((
            self.falsa_est + ak * riduci(lambda - self.lon0),
            self.falsa_nord + ak * isometrica(phi, self.e),
        ))
    }

    fn indietro(&self, x: f64, y: f64) -> (f64, f64) {
        let ak = self.a * self.k0;
        (
            self.lon0 + (x - self.falsa_est) / ak,
            da_isometrica((y - self.falsa_nord) / ak, self.e),
        )
    }
}

/// Popular Visualisation Pseudo Mercator (EPSG 1024): formule sferiche con
/// raggio `a` applicate alle coordinate geodetiche dell'ellissoide.
#[derive(Clone, Debug)]
pub(in crate::crs) struct PseudoMercatore {
    a: f64,
    lon0: f64,
    falsa_est: f64,
    falsa_nord: f64,
}

impl PseudoMercatore {
    fn avanti(&self, lambda: f64, phi: f64) -> Result<(f64, f64), ErroreProiezione> {
        if phi.abs() >= FRAC_PI_2 {
            return Err(ErroreProiezione::FuoriDominio);
        }
        Ok((
            self.falsa_est + self.a * riduci(lambda - self.lon0),
            self.falsa_nord + self.a * phi.tan().asinh(),
        ))
    }

    fn indietro(&self, x: f64, y: f64) -> (f64, f64) {
        (
            self.lon0 + (x - self.falsa_est) / self.a,
            ((y - self.falsa_nord) / self.a).sinh().atan(),
        )
    }
}

// ---------------------------------------------------------------------------
// Lambert Conic Conformal (2SP).
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub(in crate::crs) struct Lcc {
    e: f64,
    lon_origine: f64,
    n: f64,
    /// `a * F`.
    af: f64,
    /// Raggio del parallelo d'origine, `r_F`.
    rf: f64,
    est_origine: f64,
    nord_origine: f64,
}

impl Lcc {
    fn nuova(
        a: f64,
        e2: f64,
        e: f64,
        [lat_origine, lon_origine, lat1, lat2]: [f64; 4],
        est_origine: f64,
        nord_origine: f64,
    ) -> Result<Self, ErroreProiezione> {
        let m = |phi: f64| phi.cos() / (1.0 - e2 * phi.sin() * phi.sin()).sqrt();
        let (phi1, phi2) = (lat1.to_radians(), lat2.to_radians());
        let (psi1, psi2) = (isometrica(phi1, e), isometrica(phi2, e));
        // Paralleli coincidenti: e' il metodo 1SP, non questo (confronto
        // esatto voluto: basta che differiscano perche' `n` sia definito).
        #[allow(clippy::float_cmp)]
        let coincidenti = psi1 == psi2;
        if coincidenti {
            return Err(ErroreProiezione::Parametri);
        }
        // EPSG: n = (ln m1 - ln m2) / (ln t1 - ln t2), con ln t = -psi.
        let n = (m(phi1).ln() - m(phi2).ln()) / (psi2 - psi1);
        let f = m(phi1) * (n * psi1).exp() / n;
        let af = a * f;
        let rf = af * (-n * isometrica(lat_origine.to_radians(), e)).exp();
        Ok(Self {
            e,
            lon_origine: lon_origine.to_radians(),
            n,
            af,
            rf,
            est_origine,
            nord_origine,
        })
    }

    fn avanti(&self, lambda: f64, phi: f64) -> Result<(f64, f64), ErroreProiezione> {
        if phi.abs() >= FRAC_PI_2 {
            return Err(ErroreProiezione::FuoriDominio);
        }
        let r = self.af * (-self.n * isometrica(phi, self.e)).exp();
        let theta = self.n * riduci(lambda - self.lon_origine);
        Ok((
            self.est_origine + r * theta.sin(),
            self.nord_origine + self.rf - r * theta.cos(),
        ))
    }

    fn indietro(&self, x: f64, y: f64) -> Result<(f64, f64), ErroreProiezione> {
        let segno = self.n.signum();
        let dx = x - self.est_origine;
        let dy = self.rf - (y - self.nord_origine);
        let r = segno * dx.hypot(dy);
        if r == 0.0 {
            return Ok((self.lon_origine, FRAC_PI_2.copysign(self.n)));
        }
        let theta = (segno * dx).atan2(segno * dy);
        let psi = -(r / self.af).ln() / self.n;
        if !psi.is_finite() {
            return Err(ErroreProiezione::FuoriDominio);
        }
        Ok((
            theta / self.n + self.lon_origine,
            da_isometrica(psi, self.e),
        ))
    }
}

// ---------------------------------------------------------------------------
// Lambert Azimuthal Equal Area (aspetto obliquo, EPSG 9820).
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub(in crate::crs) struct Laea {
    e: f64,
    lon0: f64,
    qp: f64,
    rq: f64,
    d: f64,
    seno_beta0: f64,
    coseno_beta0: f64,
    falsa_est: f64,
    falsa_nord: f64,
}

impl Laea {
    fn nuova(
        a: f64,
        e2: f64,
        e: f64,
        lat0: f64,
        lon0: f64,
        falsa_est: f64,
        falsa_nord: f64,
    ) -> Self {
        let phi0 = lat0.to_radians();
        let qp = q_autalica(FRAC_PI_2, e);
        let q0 = q_autalica(phi0, e);
        let beta0 = (q0 / qp).clamp(-1.0, 1.0).asin();
        let rq = a * (qp / 2.0).sqrt();
        let d = a * (phi0.cos() / (1.0 - e2 * phi0.sin() * phi0.sin()).sqrt()) / (rq * beta0.cos());
        Self {
            e,
            lon0: lon0.to_radians(),
            qp,
            rq,
            d,
            seno_beta0: beta0.sin(),
            coseno_beta0: beta0.cos(),
            falsa_est,
            falsa_nord,
        }
    }

    fn avanti(&self, lambda: f64, phi: f64) -> Result<(f64, f64), ErroreProiezione> {
        let q = q_autalica(phi, self.e);
        let beta = (q / self.qp).clamp(-1.0, 1.0).asin();
        let dl = riduci(lambda - self.lon0);
        let denominatore =
            1.0 + self.seno_beta0 * beta.sin() + self.coseno_beta0 * beta.cos() * dl.cos();
        if denominatore <= 0.0 {
            // Antipodo del centro: la proiezione non e' definita.
            return Err(ErroreProiezione::FuoriDominio);
        }
        let b = self.rq * (2.0 / denominatore).sqrt();
        Ok((
            self.falsa_est + b * self.d * beta.cos() * dl.sin(),
            self.falsa_nord
                + (b / self.d)
                    * (self.coseno_beta0 * beta.sin() - self.seno_beta0 * beta.cos() * dl.cos()),
        ))
    }

    fn indietro(&self, x: f64, y: f64) -> Result<(f64, f64), ErroreProiezione> {
        let dx = (x - self.falsa_est) / self.d;
        let dy = self.d * (y - self.falsa_nord);
        let rho = dx.hypot(dy);
        if rho == 0.0 {
            let phi = da_q_autalica(self.qp * self.seno_beta0, self.qp, self.e)
                .ok_or(ErroreProiezione::NonConvergente)?;
            return Ok((self.lon0, phi));
        }
        let rapporto = rho / (2.0 * self.rq);
        if rapporto > 1.0 {
            return Err(ErroreProiezione::FuoriDominio);
        }
        let c = 2.0 * rapporto.asin();
        let (sc, cc) = c.sin_cos();
        let beta = (cc * self.seno_beta0 + dy * sc * self.coseno_beta0 / rho)
            .clamp(-1.0, 1.0)
            .asin();
        let dl = (dx * self.d * sc)
            .atan2(self.d * rho * self.coseno_beta0 * cc - self.d * dy * self.seno_beta0 * sc);
        let phi = da_q_autalica(self.qp * beta.sin(), self.qp, self.e)
            .ok_or(ErroreProiezione::NonConvergente)?;
        Ok((self.lon0 + dl, phi))
    }
}

// ---------------------------------------------------------------------------
// Oblique Stereographic (EPSG 9809): sfera conforme di Gauss e
// stereografica obliqua sulla sfera.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub(in crate::crs) struct Stereo {
    e: f64,
    lon0: f64,
    n: f64,
    /// `ln(c) / 2`: latitudine isometrica sulla sfera = `n psi + ln(c)/2`.
    mezzo_ln_c: f64,
    seno_chi0: f64,
    coseno_chi0: f64,
    /// `2 R k0`.
    due_rk: f64,
    falsa_est: f64,
    falsa_nord: f64,
}

impl Stereo {
    #[allow(clippy::too_many_arguments)]
    fn nuova(
        a: f64,
        e2: f64,
        e: f64,
        lat0: f64,
        lon0: f64,
        k0: f64,
        falsa_est: f64,
        falsa_nord: f64,
    ) -> Self {
        let phi0 = lat0.to_radians();
        let s0 = phi0.sin();
        let c0 = phi0.cos();
        let raggio = a * (1.0 - e2).sqrt() / (1.0 - e2 * s0 * s0);
        let n = (1.0 + e2 * c0.powi(4) / (1.0 - e2)).sqrt();
        let psi0 = isometrica(phi0, e);
        // EPSG: sin chi0' = (w1 - 1)/(w1 + 1) con ln w1 = 2 n psi0.
        let seno_chi0_primo = (n * psi0).tanh();
        let c = (n + s0) * (1.0 - seno_chi0_primo) / ((n - s0) * (1.0 + seno_chi0_primo));
        let mezzo_ln_c = c.ln() / 2.0;
        let chi0 = (n * psi0 + mezzo_ln_c).sinh().atan();
        Self {
            e,
            lon0: lon0.to_radians(),
            n,
            mezzo_ln_c,
            seno_chi0: chi0.sin(),
            coseno_chi0: chi0.cos(),
            due_rk: 2.0 * raggio * k0,
            falsa_est,
            falsa_nord,
        }
    }

    fn avanti(&self, lambda: f64, phi: f64) -> (f64, f64) {
        let chi = if phi.abs() >= FRAC_PI_2 {
            FRAC_PI_2.copysign(phi)
        } else {
            (self.n * isometrica(phi, self.e) + self.mezzo_ln_c)
                .sinh()
                .atan()
        };
        let dl = self.n * riduci(lambda - self.lon0);
        let (sc, cc) = chi.sin_cos();
        let b = 1.0 + sc * self.seno_chi0 + cc * self.coseno_chi0 * dl.cos();
        (
            self.falsa_est + self.due_rk * cc * dl.sin() / b,
            self.falsa_nord
                + self.due_rk * (sc * self.coseno_chi0 - cc * self.seno_chi0 * dl.cos()) / b,
        )
    }

    fn indietro(&self, x: f64, y: f64) -> (f64, f64) {
        let dx = x - self.falsa_est;
        let dy = y - self.falsa_nord;
        let rho = dx.hypot(dy);
        let (chi, dl) = if rho == 0.0 {
            (self.seno_chi0.atan2(self.coseno_chi0), 0.0)
        } else {
            let c = 2.0 * (rho / self.due_rk).atan();
            let (sc, cc) = c.sin_cos();
            let chi = (cc * self.seno_chi0 + dy * sc * self.coseno_chi0 / rho)
                .clamp(-1.0, 1.0)
                .asin();
            let dl = (dx * sc).atan2(rho * self.coseno_chi0 * cc - dy * self.seno_chi0 * sc);
            (chi, dl)
        };
        let phi = if chi.abs() >= FRAC_PI_2 {
            FRAC_PI_2.copysign(chi)
        } else {
            da_isometrica((chi.tan().asinh() - self.mezzo_ln_c) / self.n, self.e)
        };
        (self.lon0 + dl / self.n, phi)
    }
}

// ---------------------------------------------------------------------------
// Hotine Oblique Mercator (variant B, EPSG 9815) con azimut e angolo del
// reticolo di 90 gradi: la Swiss Oblique Mercator (EPSG 9814, Rosenmund).
// ---------------------------------------------------------------------------

/// Swiss Oblique Mercator: sfera conforme di Gauss, rotazione che porta il
/// centro sull'equatore, Mercator sulla sfera. Sono le formule ufficiali di
/// swisstopo e quelle di PROJ (`somerc`), che PROJ 9.5.1 usa per EPSG:2056
/// benche' il registro lo descriva come Hotine variant B con azimut e
/// angolo del reticolo di 90 gradi: le formule letterali di Hotine B
/// (aposfera) ne differiscono fino a circa 9 cm ai bordi dell'area d'uso
/// (misura dell'oracolo). Qui si seguono swisstopo e PROJ; ogni altra
/// Hotine B si rifiuta (nessuna nella tabella).
#[derive(Clone, Debug)]
pub(in crate::crs) struct Svizzera {
    e: f64,
    lon0: f64,
    /// `alpha` della sfera di Gauss (`c` in PROJ).
    c: f64,
    /// Costante `K` della latitudine sulla sfera.
    k: f64,
    seno_p0: f64,
    coseno_p0: f64,
    /// `a * k0 * sqrt(1 - e^2) / (1 - e^2 sin^2 phi0)`.
    kr: f64,
    est_centro: f64,
    nord_centro: f64,
}

impl Svizzera {
    fn nuova(
        a: f64,
        e2: f64,
        e: f64,
        [lat_centro, lon_centro, azimut, angolo_reticolo, k_centro]: [f64; 5],
        est_centro: f64,
        nord_centro: f64,
    ) -> Result<Self, ErroreProiezione> {
        // Valori esatti del registro (generatore): solo 90 e 90.
        #[allow(clippy::float_cmp)]
        let svizzera = azimut == 90.0 && angolo_reticolo == 90.0;
        if !svizzera {
            return Err(ErroreProiezione::Parametri);
        }
        let phi0 = lat_centro.to_radians();
        let cp2 = phi0.cos() * phi0.cos();
        let c = (1.0 + e2 * cp2 * cp2 / (1.0 - e2)).sqrt();
        let seno_p0 = phi0.sin() / c;
        let phip0 = seno_p0.asin();
        let k = phip0.tan().asinh() - c * isometrica(phi0, e);
        let esp = e * phi0.sin();
        let kr = a * k_centro * (1.0 - e2).sqrt() / (1.0 - esp * esp);
        Ok(Self {
            e,
            lon0: lon_centro.to_radians(),
            c,
            k,
            seno_p0,
            coseno_p0: phip0.cos(),
            kr,
            est_centro,
            nord_centro,
        })
    }

    fn avanti(&self, lambda: f64, phi: f64) -> Result<(f64, f64), ErroreProiezione> {
        if phi.abs() >= FRAC_PI_2 {
            return Err(ErroreProiezione::FuoriDominio);
        }
        // Latitudine sulla sfera: psi' = c psi + K.
        let phip = (self.c * isometrica(phi, self.e) + self.k).sinh().atan();
        let lamp = self.c * riduci(lambda - self.lon0);
        let cp = phip.cos();
        let phipp = (self.coseno_p0 * phip.sin() - self.seno_p0 * cp * lamp.cos())
            .clamp(-1.0, 1.0)
            .asin();
        let lampp = (cp * lamp.sin() / phipp.cos()).clamp(-1.0, 1.0).asin();
        Ok((
            self.est_centro + self.kr * lampp,
            self.nord_centro + self.kr * phipp.tan().asinh(),
        ))
    }

    fn indietro(&self, x: f64, y: f64) -> (f64, f64) {
        let phipp = ((y - self.nord_centro) / self.kr).sinh().atan();
        let lampp = (x - self.est_centro) / self.kr;
        let cp = phipp.cos();
        let phip = (self.coseno_p0 * phipp.sin() + self.seno_p0 * cp * lampp.cos())
            .clamp(-1.0, 1.0)
            .asin();
        let lamp = (cp * lampp.sin() / phip.cos()).clamp(-1.0, 1.0).asin();
        let psi = (phip.tan().asinh() - self.k) / self.c;
        (self.lon0 + lamp / self.c, da_isometrica(psi, self.e))
    }
}
