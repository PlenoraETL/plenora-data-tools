//! GENERATO da `scripts/genera_projjson_integrati.py`: non modificare a mano.
//! Mappa di controllo tra codici EPSG e campi della riproiezione.
use crate::crs::riproiezione::tabella::MetodoProiezione;
// La corrispondenza generata mantiene insieme tutti i metodi del registro.
#[allow(clippy::too_many_lines)]
pub(super) fn parametri(metodo: MetodoProiezione) -> (u32, Vec<(u32, f64)>) {
    match metodo {
        MetodoProiezione::Geografico => (0, vec![]),
        MetodoProiezione::TrasversaDiMercatore {
            lat0,
            lon0,
            k0,
            falsa_est,
            falsa_nord,
        } => (
            9807,
            vec![
                (8801, lat0),
                (8802, lon0),
                (8805, k0),
                (8806, falsa_est),
                (8807, falsa_nord),
            ],
        ),
        MetodoProiezione::MercatoreA {
            lat0,
            lon0,
            k0,
            falsa_est,
            falsa_nord,
        } => (
            9804,
            vec![
                (8801, lat0),
                (8802, lon0),
                (8805, k0),
                (8806, falsa_est),
                (8807, falsa_nord),
            ],
        ),
        MetodoProiezione::PseudoMercatore {
            lat0,
            lon0,
            falsa_est,
            falsa_nord,
        } => (
            1024,
            vec![
                (8801, lat0),
                (8802, lon0),
                (8806, falsa_est),
                (8807, falsa_nord),
            ],
        ),
        MetodoProiezione::LambertConicaConforme2Sp {
            lat_origine,
            lon_origine,
            lat1,
            lat2,
            est_origine,
            nord_origine,
        } => (
            9802,
            vec![
                (8821, lat_origine),
                (8822, lon_origine),
                (8823, lat1),
                (8824, lat2),
                (8826, est_origine),
                (8827, nord_origine),
            ],
        ),
        MetodoProiezione::LambertAzimutaleEquivalente {
            lat0,
            lon0,
            falsa_est,
            falsa_nord,
        } => (
            9820,
            vec![
                (8801, lat0),
                (8802, lon0),
                (8806, falsa_est),
                (8807, falsa_nord),
            ],
        ),
        MetodoProiezione::StereograficaObliqua {
            lat0,
            lon0,
            k0,
            falsa_est,
            falsa_nord,
        } => (
            9809,
            vec![
                (8801, lat0),
                (8802, lon0),
                (8805, k0),
                (8806, falsa_est),
                (8807, falsa_nord),
            ],
        ),
        MetodoProiezione::HotineObliquaB {
            lat_centro,
            lon_centro,
            azimut,
            angolo_reticolo,
            k_centro,
            est_centro,
            nord_centro,
        } => (
            9815,
            vec![
                (8811, lat_centro),
                (8812, lon_centro),
                (8813, azimut),
                (8814, angolo_reticolo),
                (8815, k_centro),
                (8816, est_centro),
                (8817, nord_centro),
            ],
        ),
    }
}
