//! Latitudini ausiliarie sull'ellissoide: isometrica, conforme, autalica.
//!
//! Le funzioni `taup`/`tau_da_taup` sono quelle di Karney (`GeographicLib`,
//! `Math::taupf` e `Math::tauf`; Karney 2011, "Transverse Mercator with an
//! accuracy of a few nanometers", eq. 7-9 e 19-21): `tau = tan(phi)`,
//! `taup = tan(chi)` con `chi` latitudine conforme, e l'inversa con il
//! metodo di Newton a precisione di macchina. Da qui passano Transverse
//! Mercator, Mercator, Lambert conica, stereografica e Hotine.

use std::f64::consts::FRAC_PI_2;

/// `e * atanh(e * x)`, con `e` eccentricita' (sempre positiva nella tabella).
pub(super) fn eatanhe(x: f64, e: f64) -> f64 {
    e * (e * x).atanh()
}

/// `tan(chi)` dalla `tan(phi)`: latitudine conforme.
// `sig.hypot(1) * tau - sig * tau1` e' la forma di Karney: `mul_add`
// cambierebbe l'arrotondamento rispetto alla fonte.
#[allow(clippy::suboptimal_flops)]
pub(super) fn taup(tau: f64, e: f64) -> f64 {
    let tau1 = tau.hypot(1.0);
    let sig = eatanhe(tau / tau1, e).sinh();
    sig.hypot(1.0) * tau - sig * tau1
}

/// `tan(phi)` dalla `tan(chi)`, inversa di [`taup`] (Newton, al piu' dieci
/// passi; converge in due o tre sul dominio d'uso).
pub(super) fn tau_da_taup(taup_dato: f64, e: f64) -> f64 {
    let e2m = e.mul_add(-e, 1.0);
    let tol = f64::EPSILON.sqrt() / 10.0;
    let taumax = 2.0 / f64::EPSILON.sqrt();
    let mut tau = if taup_dato.abs() > 70.0 {
        taup_dato * eatanhe(1.0, e).exp()
    } else {
        taup_dato / e2m
    };
    if tau.is_nan() || tau.abs() >= taumax {
        return tau;
    }
    let stol = tol * taup_dato.abs().max(1.0);
    for _ in 0..10 {
        let taupa = taup(tau, e);
        let dtau = (taup_dato - taupa) * (e2m * tau).mul_add(tau, 1.0)
            / (e2m * tau.hypot(1.0) * taupa.hypot(1.0));
        tau += dtau;
        if dtau.is_nan() || dtau.abs() < stol {
            break;
        }
    }
    tau
}

/// Latitudine isometrica `psi` (radianti) dalla latitudine geodetica.
pub(super) fn isometrica(phi: f64, e: f64) -> f64 {
    taup(phi.tan(), e).asinh()
}

/// Latitudine geodetica (radianti) dalla latitudine isometrica.
pub(super) fn da_isometrica(psi: f64, e: f64) -> f64 {
    tau_da_taup(psi.sinh(), e).atan()
}

/// `q` della proiezione equivalente (Snyder 3-12, EPSG 9820): il doppio
/// dell'area della zona dall'equatore alla latitudine, a meno di `a^2`.
pub(super) fn q_autalica(phi: f64, e: f64) -> f64 {
    let e2 = e * e;
    let s = phi.sin();
    (1.0 - e2) * (s / (e2 * s).mul_add(-s, 1.0) + (e * s).atanh() / e)
}

/// `qp - q` in funzione della colatitudine `chi = pi/2 - |phi|`, senza
/// cancellazioni vicino al polo: con `u = 1 - sin|phi| = 2 sin^2(chi/2)`,
/// `1/(1-e^2) - s/(1-e^2 s^2) = u (1 + e^2 s) / ((1-e^2)(1-e^2 s^2))` e
/// `atanh(e) - atanh(e s) = atanh(e u / (1 - e^2 s))`.
fn qp_meno_q(chi: f64, e: f64) -> f64 {
    let e2 = e * e;
    let meta = (chi / 2.0).sin();
    let u = 2.0 * meta * meta;
    let s = 1.0 - u;
    (1.0 - e2)
        * (u * e2.mul_add(s, 1.0) / ((1.0 - e2) * (e2 * s).mul_add(-s, 1.0))
            + (e * u / e2.mul_add(-s, 1.0)).atanh() / e)
}

/// Seno e coseno della latitudine autalica `beta` (EPSG 9820), stabili
/// anche a pochi millimetri dal polo: si calcola `w = 1 - sin|beta| =
/// (qp - q) / qp` dalla colatitudine, non `q / qp` da `sin(phi)` (che
/// vicino al polo arrotonda a 1 e porta ogni punto sul polo).
pub(super) fn beta_autalica(phi: f64, qp: f64, e: f64) -> (f64, f64) {
    let chi = (FRAC_PI_2 - phi.abs()).max(0.0);
    let w = (qp_meno_q(chi, e) / qp).clamp(0.0, 1.0);
    ((1.0 - w).copysign(phi), (w * (2.0 - w)).sqrt())
}

/// Latitudine geodetica dalla latitudine autalica data come seno e coseno
/// (inversa di [`beta_autalica`]): Newton sulla colatitudine, con la stessa
/// forma senza cancellazioni. `None` se non converge.
pub(super) fn da_beta_autalica(seno: f64, coseno: f64, qp: f64, e: f64) -> Option<f64> {
    let e2 = e * e;
    // Colatitudine autalica, accurata anche vicino al polo (atan2).
    let chi_beta = coseno.abs().atan2(seno.abs());
    let meta = (chi_beta / 2.0).sin();
    let obiettivo = qp * 2.0 * meta * meta;
    if obiettivo == 0.0 {
        return Some(FRAC_PI_2.copysign(seno));
    }
    let mut chi = chi_beta;
    for _ in 0..40 {
        let (sc, cc) = chi.sin_cos();
        let denominatore = (e2 * cc).mul_add(-cc, 1.0);
        let derivata = 2.0 * (1.0 - e2) * sc / (denominatore * denominatore);
        let passo = (qp_meno_q(chi, e) - obiettivo) / derivata;
        if !passo.is_finite() {
            return None;
        }
        chi -= passo;
        // Relativa alla colatitudine: 1e-14 rad per rad, sotto 1e-7 m a terra.
        if passo.abs() <= 1e-14f64.mul_add(chi.abs(), f64::MIN_POSITIVE) {
            return Some((FRAC_PI_2 - chi).copysign(seno));
        }
    }
    None
}
