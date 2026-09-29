//! Oracolo differenziale contro PROJ 9.5.1.
//!
//! Le fixture in `tests/fixtures/riproiezione/` sono di
//! `scripts/genera_oracolo_riproiezione.py` (pyproj 3.7.2, EPSG v11.022):
//! PROJ obbligato alla stessa operazione EPSG, senza griglie di rete. Ogni
//! confronto e' entro 1 mm a terra; gli scarti massimi per metodo si
//! stampano (`cargo test -- --nocapture oracolo`).

use std::collections::BTreeMap;
use std::path::PathBuf;

use super::super::{resolve_crs, CrsKind};
use super::datum::{Geocentrico, Helmert};
use super::proiezioni::Proiezione;
use super::tabella::{self, MetodoProiezione, MetodoTrasformazione};
use super::{GrigliaNtv2, OpzioniRiproiezione, PianoRiproiezione, Riproiettore};
use crate::crs::integrati::Identificativo;

/// Un millimetro, la tolleranza dell'oracolo.
const MM: f64 = 1e-3;
/// Metri per grado di latitudine (per eccesso: la tolleranza resta severa).
const METRI_PER_GRADO: f64 = 111_320.0;

fn cartella() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/riproiezione")
}

fn righe(nome: &str) -> Vec<Vec<String>> {
    let testo = std::fs::read_to_string(cartella().join(nome)).expect("fixture");
    testo
        .lines()
        .skip(1)
        .filter(|riga| !riga.is_empty())
        .map(|riga| riga.split(',').map(str::to_owned).collect())
        .collect()
}

fn numero(testo: &str) -> f64 {
    testo.parse().expect("numero")
}

/// Distanza a terra fra due punti lon/lat vicini, in metri.
fn metri_geografici(lon: f64, lat: f64, lon2: f64, lat2: f64) -> f64 {
    let mut dl = lon2 - lon;
    if dl > 180.0 {
        dl -= 360.0;
    } else if dl < -180.0 {
        dl += 360.0;
    }
    let dx = dl * METRI_PER_GRADO * lat.to_radians().cos();
    let dy = (lat2 - lat) * METRI_PER_GRADO;
    dx.hypot(dy)
}

fn codice_epsg(testo: &str) -> u32 {
    testo
        .strip_prefix("EPSG:")
        .and_then(|c| c.parse().ok())
        .expect("codice EPSG")
}

fn nome_metodo(metodo: &MetodoProiezione) -> &'static str {
    match metodo {
        MetodoProiezione::Geografico => "geografico",
        MetodoProiezione::TrasversaDiMercatore { .. } => "Transverse Mercator",
        MetodoProiezione::MercatoreA { .. } => "Mercator (variant A)",
        MetodoProiezione::PseudoMercatore { .. } => "Pseudo Mercator",
        MetodoProiezione::LambertConicaConforme2Sp { .. } => "Lambert Conic Conformal (2SP)",
        MetodoProiezione::LambertAzimutaleEquivalente { .. } => "Lambert Azimuthal Equal Area",
        MetodoProiezione::StereograficaObliqua { .. } => "Oblique Stereographic",
        MetodoProiezione::HotineObliquaB { .. } => "Hotine Oblique Mercator (variant B)",
    }
}

fn aggiorna(massimi: &mut BTreeMap<String, f64>, chiave: &str, valore: f64) {
    let voce = massimi.entry(chiave.to_owned()).or_insert(0.0);
    *voce = voce.max(valore);
}

#[test]
fn oracolo_proiezioni_entro_un_millimetro_da_proj() {
    let mut avanti: BTreeMap<String, f64> = BTreeMap::new();
    let mut indietro: BTreeMap<String, f64> = BTreeMap::new();
    let mut andata_ritorno: BTreeMap<String, f64> = BTreeMap::new();
    let mut crs_visti = std::collections::BTreeSet::new();
    for riga in righe("proiezioni.csv") {
        let codice = codice_epsg(&riga[0]);
        crs_visti.insert(codice);
        let (lon, lat, x, y) = (
            numero(&riga[1]),
            numero(&riga[2]),
            numero(&riga[3]),
            numero(&riga[4]),
        );
        let definizione = tabella::definizione(Identificativo::Epsg(codice)).expect("CRS");
        let datum = tabella::datum(definizione.datum).expect("datum");
        let proiezione =
            Proiezione::nuova(&definizione.metodo, datum.ellissoide).expect("proiezione");
        let metodo = nome_metodo(&definizione.metodo);

        let (xa, ya) = proiezione.avanti(lon, lat).expect("avanti");
        let errore = (xa - x).hypot(ya - y);
        assert!(errore <= MM, "{codice} avanti ({lon}, {lat}): {errore} m");
        aggiorna(&mut avanti, metodo, errore);

        let (loni, lati) = proiezione.indietro(x, y).expect("indietro");
        let errore = metri_geografici(lon, lat, loni, lati);
        assert!(errore <= MM, "{codice} indietro ({x}, {y}): {errore} m");
        aggiorna(&mut indietro, metodo, errore);

        let (xr, yr) = proiezione.avanti(loni, lati).expect("ritorno");
        let errore = (xr - x).hypot(yr - y);
        assert!(errore <= MM, "{codice} andata e ritorno: {errore} m");
        aggiorna(&mut andata_ritorno, metodo, errore);
    }
    // Ogni CRS proiettato della tabella e' nell'oracolo.
    let proiettati: std::collections::BTreeSet<u32> = super::epsg::DEFINIZIONI
        .iter()
        .filter(|d| !matches!(d.metodo, MetodoProiezione::Geografico))
        .filter_map(|d| match d.crs {
            Identificativo::Epsg(codice) => Some(codice),
            Identificativo::OgcCrs84 => None,
        })
        .collect();
    assert_eq!(crs_visti, proiettati, "CRS proiettati senza oracolo");
    for (metodo, massimo) in &avanti {
        eprintln!(
            "oracolo proiezioni {metodo}: avanti {massimo:.3e} m, indietro {:.3e} m, \
             andata e ritorno {:.3e} m",
            indietro[metodo], andata_ritorno[metodo]
        );
    }
}

fn applica_trasformazione(codice: u32, inversa: bool, lon: f64, lat: f64) -> (f64, f64) {
    let trasformazione = tabella::trasformazione(codice).expect("trasformazione");
    if let MetodoTrasformazione::Traslazioni { tx, ty, tz } = trasformazione.metodo {
        if tx == 0.0 && ty == 0.0 && tz == 0.0 {
            return (lon, lat);
        }
    }
    let helmert = Helmert::da_metodo(&trasformazione.metodo).expect("helmert");
    let da = Geocentrico::nuovo(tabella::datum(trasformazione.da).expect("da").ellissoide);
    let a = Geocentrico::nuovo(tabella::datum(trasformazione.a).expect("a").ellissoide);
    if inversa {
        da.indietro(helmert.indietro(a.avanti(lon, lat)))
    } else {
        a.indietro(helmert.avanti(da.avanti(lon, lat)))
    }
}

#[test]
fn oracolo_trasformazioni_entro_un_millimetro_da_proj() {
    let mut massimi: BTreeMap<String, f64> = BTreeMap::new();
    let mut provate = std::collections::BTreeSet::new();
    for riga in righe("trasformazioni.csv") {
        let codice: u32 = riga[0].parse().expect("codice");
        provate.insert(codice);
        let inversa = riga[1] == "1";
        let (lon, lat, lon_attesa, lat_attesa) = (
            numero(&riga[2]),
            numero(&riga[3]),
            numero(&riga[4]),
            numero(&riga[5]),
        );
        let (lon_calcolata, lat_calcolata) = applica_trasformazione(codice, inversa, lon, lat);
        let errore = metri_geografici(lon_attesa, lat_attesa, lon_calcolata, lat_calcolata);
        assert!(
            errore <= MM,
            "EPSG:{codice} {} ({lon}, {lat}): {errore} m",
            if inversa { "inversa" } else { "avanti" }
        );
        let metodo = match tabella::trasformazione(codice).expect("t").metodo {
            MetodoTrasformazione::Traslazioni { .. } => "Geocentric translations",
            MetodoTrasformazione::VettorePosizione { .. } => "Position Vector",
            MetodoTrasformazione::TelaioCoordinate { .. } => "Coordinate Frame",
            MetodoTrasformazione::GrigliaNtv2 { .. } => "NTv2",
        };
        aggiorna(
            &mut massimi,
            &format!("{metodo}{}", if inversa { " (inversa)" } else { "" }),
            errore,
        );
    }
    // Andata e ritorno con le sole formule nostre: l'altezza prodotta
    // dall'andata si scarta (CRS 2D, come PROJ), e il ritorno riparte da
    // altezza nulla, quindi il giro non e' un'identita' (circa 1 mm con
    // altezze di decine di metri). Lo scarto massimo si stampa; il limite
    // e' la precisione di 1 cm, molto sotto l'accuratezza EPSG di ogni
    // cambio di datum.
    let mut giro = 0.0_f64;
    for riga in righe("trasformazioni.csv") {
        if riga[1] == "1" {
            continue;
        }
        let codice: u32 = riga[0].parse().expect("codice");
        let (lon, lat) = (numero(&riga[2]), numero(&riga[3]));
        let (lon_a, lat_a) = applica_trasformazione(codice, false, lon, lat);
        let (lon_r, lat_r) = applica_trasformazione(codice, true, lon_a, lat_a);
        let scarto = metri_geografici(lon, lat, lon_r, lat_r);
        assert!(
            scarto <= 10.0 * MM,
            "EPSG:{codice} andata e ritorno ({lon}, {lat}): {scarto} m"
        );
        giro = giro.max(scarto);
    }
    eprintln!("oracolo trasformazioni andata e ritorno (senza PROJ): {giro:.3e} m");
    let senza_griglia: std::collections::BTreeSet<u32> = tabella::trasformazioni()
        .iter()
        .filter(|t| !t.a_griglia())
        .map(|t| t.codice)
        .collect();
    assert_eq!(provate, senza_griglia, "trasformazioni senza oracolo");
    for (metodo, massimo) in &massimi {
        eprintln!("oracolo trasformazioni {metodo}: {massimo:.3e} m");
    }
}

fn griglia_sintetica() -> GrigliaNtv2 {
    GrigliaNtv2::leggi(&cartella().join("griglia_sintetica.gsb")).expect("griglia sintetica")
}

#[test]
fn oracolo_griglia_ntv2_entro_un_millimetro_da_proj() {
    let griglia = griglia_sintetica();
    let mut massimo = [0.0_f64; 2];
    for riga in righe("griglia.csv") {
        let inversa = riga[0] == "1";
        let (lon, lat, lon_attesa, lat_attesa) = (
            numero(&riga[1]),
            numero(&riga[2]),
            numero(&riga[3]),
            numero(&riga[4]),
        );
        let (lon_calcolata, lat_calcolata) = if inversa {
            griglia
                .indietro(lon, lat)
                .expect("converge")
                .expect("dentro")
        } else {
            griglia.avanti(lon, lat).expect("dentro")
        };
        let errore = metri_geografici(lon_attesa, lat_attesa, lon_calcolata, lat_calcolata);
        assert!(
            errore <= MM,
            "griglia ({lon}, {lat}) inversa {inversa}: {errore} m"
        );
        let indice = usize::from(inversa);
        massimo[indice] = massimo[indice].max(errore);
    }
    eprintln!(
        "oracolo NTv2: avanti {:.3e} m, inversa {:.3e} m",
        massimo[0], massimo[1]
    );
}

#[test]
fn oracolo_catene_entro_un_millimetro_da_proj() {
    let mut massimi: BTreeMap<String, f64> = BTreeMap::new();
    let mut piani: BTreeMap<(String, String, String), Riproiettore> = BTreeMap::new();
    for riga in righe("catene.csv") {
        let (da, a, percorso) = (riga[0].clone(), riga[1].clone(), riga[2].clone());
        let accuratezza = numero(&riga[3]);
        let (x, y, x_attesa, y_attesa) = (
            numero(&riga[4]),
            numero(&riga[5]),
            numero(&riga[6]),
            numero(&riga[7]),
        );
        let chiave = (da.clone(), a.clone(), percorso.clone());
        if !piani.contains_key(&chiave) {
            let codici: Vec<u32> = percorso
                .split('+')
                .filter(|p| !p.is_empty())
                .map(|p| p.trim_end_matches('i').parse().expect("codice"))
                .collect();
            let griglie: Vec<u32> = codici.iter().copied().filter(|c| *c == 9734).collect();
            let opzioni = OpzioniRiproiezione {
                accuratezza_accettata_m: (accuratezza > 0.01).then_some(accuratezza),
                griglie: griglie.clone(),
                trasformazioni: Some(codici),
            };
            let sorgente = resolve_crs(&da, "da").expect("da");
            let destinazione = resolve_crs(&a, "a").expect("a");
            let deciso = PianoRiproiezione::nuovo(&sorgente, &destinazione, &opzioni)
                .unwrap_or_else(|errore| panic!("{da} -> {a} {percorso}: {errore}"));
            assert_eq!(deciso.percorsi().len(), 1, "percorso imposto");
            let file = griglie.iter().map(|c| (*c, griglia_sintetica())).collect();
            piani.insert(
                chiave.clone(),
                Riproiettore::nuovo(deciso, file).expect("riproiettore"),
            );
        }
        let riproiettore = &piani[&chiave];
        let (xc, yc) = riproiettore
            .trasforma(0, x, y)
            .unwrap_or_else(|errore| panic!("{da} -> {a} ({x}, {y}): {errore}"))
            .unwrap_or_else(|| panic!("{da} -> {a} ({x}, {y}): fuori area"));
        let errore = match riproiettore.piano().destinazione().kind() {
            CrsKind::Geographic => metri_geografici(x_attesa, y_attesa, xc, yc),
            CrsKind::Projected => (xc - x_attesa).hypot(yc - y_attesa),
        };
        assert!(
            errore <= MM,
            "{da} -> {a} {percorso} ({x}, {y}): {errore} m"
        );
        let famiglia = if percorso.is_empty() {
            "stesso datum".to_owned()
        } else if percorso.contains("9734") {
            "griglia NTv2".to_owned()
        } else {
            "cambio di datum".to_owned()
        };
        aggiorna(&mut massimi, &famiglia, errore);
    }
    for (famiglia, massimo) in &massimi {
        eprintln!("oracolo catene {famiglia}: {massimo:.3e} m");
    }
}
