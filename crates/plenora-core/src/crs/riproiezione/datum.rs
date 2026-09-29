//! Cambi di datum senza griglia: coordinate geodetiche -> geocentriche ->
//! Helmert -> geocentriche -> geodetiche nell'ellissoide d'arrivo.
//!
//! Come PROJ per i CRS 2D: altezza ellissoidica nulla in ingresso, altezza
//! d'uscita scartata. Le formule di Helmert sono quelle EPSG (linearizzate
//! nelle rotazioni): Geocentric translations (9603), Position Vector (9606)
//! e Coordinate Frame (9607), con la matrice
//!
//! ```text
//! Coordinate Frame:  R = [[1, rz, -ry], [-rz, 1, rx], [ry, -rx, 1]]
//! Position Vector:   R trasposta
//! X' = (1 + ds * 1e-6) * R * X + T
//! ```
//!
//! **Verso inverso.** Il registro EPSG ottiene il verso inverso cambiando
//! il segno dei parametri, un'approssimazione; PROJ 9.5.1 applica invece
//! l'inversa algebrica della stessa formula (`+inv +proj=helmert`:
//! `X = R^T (X' - T) / (1 + ds)`). Qui si fa come PROJ: il giro avanti e
//! indietro torna al punto di partenza, e la differenza dal cambio di segno
//! (pochi millimetri con rotazioni di secondi d'arco e traslazioni di
//! centinaia di metri) resta molto sotto l'accuratezza EPSG di ogni
//! trasformazione della tabella (almeno 1 cm, e 1 cm solo per GDA94 ->
//! GDA2020, con parametri di centimetri). Con le sole traslazioni i due
//! versi coincidono.

// Formule EPSG: nomi brevi e forma a*b+c delle fonti.
#![allow(clippy::many_single_char_names, clippy::suboptimal_flops)]

use super::super::Ellipsoid;
use super::tabella::MetodoTrasformazione;

/// Un ellissoide con le costanti della conversione geocentrica.
#[derive(Clone, Copy, Debug)]
pub(in crate::crs) struct Geocentrico {
    a: f64,
    b: f64,
    e2: f64,
    /// Seconda eccentricita' al quadrato.
    ep2: f64,
    f: f64,
}

impl Geocentrico {
    pub(in crate::crs) fn nuovo(ellissoide: Ellipsoid) -> Self {
        let a = ellissoide.semi_major_axis_metre;
        let f = 1.0 / ellissoide.inverse_flattening;
        let e2 = f * (2.0 - f);
        Self {
            a,
            b: a * (1.0 - f),
            e2,
            ep2: e2 / (1.0 - e2),
            f,
        }
    }

    /// Da longitudine e latitudine (gradi), altezza nulla, a `X, Y, Z`.
    pub(in crate::crs) fn avanti(&self, lon: f64, lat: f64) -> [f64; 3] {
        let (sl, cl) = lon.to_radians().sin_cos();
        let (sp, cp) = lat.to_radians().sin_cos();
        let n = self.a / (1.0 - self.e2 * sp * sp).sqrt();
        [n * cp * cl, n * cp * sl, n * (1.0 - self.e2) * sp]
    }

    /// Da `X, Y, Z` a longitudine e latitudine (gradi), altezza scartata.
    ///
    /// Formula di Bowring (1976) iterata sulla latitudine ridotta: con
    /// altezze di qualche chilometro converge a precisione di macchina in
    /// due passi; se ne fanno cinque.
    pub(in crate::crs) fn indietro(&self, [x, y, z]: [f64; 3]) -> (f64, f64) {
        let p = x.hypot(y);
        let lon = y.atan2(x);
        let mut theta = (z * self.a).atan2(p * self.b);
        let mut phi = theta;
        for _ in 0..5 {
            let (s, c) = theta.sin_cos();
            phi = (z + self.ep2 * self.b * s * s * s).atan2(p - self.e2 * self.a * c * c * c);
            theta = ((1.0 - self.f) * phi.sin()).atan2(phi.cos());
        }
        (lon.to_degrees(), phi.to_degrees())
    }
}

/// Una trasformazione di Helmert in radianti e scala adimensionale.
#[derive(Clone, Copy, Debug)]
pub(in crate::crs) struct Helmert {
    t: [f64; 3],
    /// Matrice di rotazione linearizzata, gia' nella convenzione del metodo.
    r: [[f64; 3]; 3],
    /// `1 + ds * 1e-6`.
    scala: f64,
}

const SECONDO_D_ARCO: f64 = std::f64::consts::PI / 648_000.0;

impl Helmert {
    /// `None` per un metodo a griglia.
    pub(in crate::crs) fn da_metodo(metodo: &MetodoTrasformazione) -> Option<Self> {
        match *metodo {
            MetodoTrasformazione::Traslazioni { tx, ty, tz } => Some(Self {
                t: [tx, ty, tz],
                r: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
                scala: 1.0,
            }),
            MetodoTrasformazione::TelaioCoordinate {
                tx,
                ty,
                tz,
                rx,
                ry,
                rz,
                ds,
            } => Some(Self::sette([tx, ty, tz], [rx, ry, rz], ds, false)),
            MetodoTrasformazione::VettorePosizione {
                tx,
                ty,
                tz,
                rx,
                ry,
                rz,
                ds,
            } => Some(Self::sette([tx, ty, tz], [rx, ry, rz], ds, true)),
            MetodoTrasformazione::GrigliaNtv2 { .. } => None,
        }
    }

    fn sette(t: [f64; 3], rotazioni: [f64; 3], ds: f64, vettore_posizione: bool) -> Self {
        let [rx, ry, rz] = rotazioni.map(|r| r * SECONDO_D_ARCO);
        let telaio = [[1.0, rz, -ry], [-rz, 1.0, rx], [ry, -rx, 1.0]];
        let r = if vettore_posizione {
            [
                [telaio[0][0], telaio[1][0], telaio[2][0]],
                [telaio[0][1], telaio[1][1], telaio[2][1]],
                [telaio[0][2], telaio[1][2], telaio[2][2]],
            ]
        } else {
            telaio
        };
        Self {
            t,
            r,
            scala: 1.0 + ds * 1e-6,
        }
    }

    pub(in crate::crs) fn avanti(&self, [x, y, z]: [f64; 3]) -> [f64; 3] {
        let r = &self.r;
        [
            self.scala * (r[0][0] * x + r[0][1] * y + r[0][2] * z) + self.t[0],
            self.scala * (r[1][0] * x + r[1][1] * y + r[1][2] * z) + self.t[1],
            self.scala * (r[2][0] * x + r[2][1] * y + r[2][2] * z) + self.t[2],
        ]
    }

    /// Inversa algebrica di [`Self::avanti`], come PROJ (modulo).
    pub(in crate::crs) fn indietro(&self, [x, y, z]: [f64; 3]) -> [f64; 3] {
        let r = &self.r;
        let (x, y, z) = (
            (x - self.t[0]) / self.scala,
            (y - self.t[1]) / self.scala,
            (z - self.t[2]) / self.scala,
        );
        [
            r[0][0] * x + r[1][0] * y + r[2][0] * z,
            r[0][1] * x + r[1][1] * y + r[2][1] * z,
            r[0][2] * x + r[1][2] * y + r[2][2] * z,
        ]
    }
}
