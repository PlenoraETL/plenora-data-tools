//! Latitudini ausiliarie sull'ellissoide: isometrica, conforme, autalica.
//!
//! Le funzioni `taup`/`tau_da_taup` sono quelle di Karney (`GeographicLib`,
//! `Math::taupf` e `Math::tauf`; Karney 2011, "Transverse Mercator with an
//! accuracy of a few nanometers", eq. 7-9 e 19-21): `tau = tan(phi)`,
//! `taup = tan(chi)` con `chi` latitudine conforme, e l'inversa con il
//! metodo di Newton a precisione di macchina. Da qui passano Transverse
//! Mercator, Mercator, Lambert conica, stereografica e Hotine.

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

/// Latitudine geodetica da `q` ([`q_autalica`]), per Newton (Snyder 3-16).
///
/// Converge a precisione di macchina; oltre `qp` (polo) restituisce il
/// polo. `None` se non converge: non si inventa una latitudine.
pub(super) fn da_q_autalica(q: f64, qp: f64, e: f64) -> Option<f64> {
    let e2 = e * e;
    if q.abs() >= qp {
        return Some(std::f64::consts::FRAC_PI_2.copysign(q));
    }
    let mut phi = (q / 2.0).asin();
    for _ in 0..30 {
        let s = phi.sin();
        let c = phi.cos();
        let uno_meno = (e2 * s).mul_add(-s, 1.0);
        let delta =
            uno_meno * uno_meno / (2.0 * c) * (q / (1.0 - e2) - s / uno_meno - (e * s).atanh() / e);
        phi += delta;
        if delta.abs() <= 1e-15 {
            return Some(phi);
        }
    }
    None
}
