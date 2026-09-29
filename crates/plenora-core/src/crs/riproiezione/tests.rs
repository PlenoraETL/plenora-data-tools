//! Prove interne della riproiezione: tabella, percorsi, regola
//! dell'accuratezza, domini, griglie `NTv2`. L'oracolo contro PROJ e' in
//! [`super::oracolo`].

// Confronti esatti voluti: valori del registro (accuratezze) e identita'
// al bit delle coordinate.
#![allow(clippy::float_cmp)]

use std::collections::BTreeMap;

use super::super::{builtin_crs_identifiers, resolve_crs, CrsKind};
use super::*;
use crate::crs::integrati::Identificativo;

fn crs(definizione: &str) -> ResolvedCrs {
    resolve_crs(definizione, "crs").expect("CRS integrato")
}

fn piano(da: &str, a: &str, opzioni: &OpzioniRiproiezione) -> Result<PianoRiproiezione, CrsError> {
    PianoRiproiezione::nuovo(&crs(da), &crs(a), opzioni)
}

fn piano_err(da: &str, a: &str, opzioni: &OpzioniRiproiezione) -> CrsError {
    piano(da, a, opzioni).expect_err("atteso un rifiuto")
}

fn accetta(metri: f64) -> OpzioniRiproiezione {
    OpzioniRiproiezione {
        accuratezza_accettata_m: Some(metri),
        ..OpzioniRiproiezione::default()
    }
}

fn riproiettore(da: &str, a: &str, opzioni: &OpzioniRiproiezione) -> Riproiettore {
    Riproiettore::nuovo(piano(da, a, opzioni).expect("piano"), BTreeMap::new())
        .expect("riproiettore")
}

fn percorso_griglia() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/riproiezione/griglia_sintetica.gsb")
}

fn griglia_sintetica() -> GrigliaNtv2 {
    GrigliaNtv2::leggi(&percorso_griglia()).expect("griglia sintetica")
}

fn byte_griglia_sintetica() -> Vec<u8> {
    std::fs::read(percorso_griglia()).expect("griglia sintetica")
}

fn gauss_boaga(lon: f64, lat: f64) -> (f64, f64) {
    let definizione = tabella::definizione(Identificativo::Epsg(3003)).expect("3003");
    let datum = tabella::datum(definizione.datum).expect("datum");
    Proiezione::nuova(&definizione.metodo, datum.ellissoide)
        .expect("proiezione")
        .avanti(lon, lat)
        .expect("avanti")
}

#[test]
fn ogni_crs_integrato_ha_una_definizione_e_una_proiezione() {
    let mut contati = 0;
    for definizione in builtin_crs_identifiers() {
        let risolto = crs(&definizione);
        assert!(riproiettabile(&risolto), "{definizione}");
        let identificativo = risolto.integrato.expect("riga della tabella");
        let voce = tabella::definizione(identificativo).expect("definizione");
        let datum = tabella::datum(voce.datum).expect("datum");
        Proiezione::nuova(&voce.metodo, datum.ellissoide).expect("proiezione");
        assert_eq!(
            risolto.kind() == CrsKind::Geographic,
            voce.metodo == tabella::MetodoProiezione::Geografico,
            "{definizione}"
        );
        contati += 1;
    }
    assert_eq!(contati, epsg::DEFINIZIONI.len(), "definizioni senza CRS");
    assert_eq!(contati, 169, "168 codici EPSG piu' OGC:CRS84");
    for trasformazione in tabella::trasformazioni() {
        assert!(
            tabella::datum(trasformazione.da).is_some(),
            "{}",
            trasformazione.codice
        );
        assert!(
            tabella::datum(trasformazione.a).is_some(),
            "{}",
            trasformazione.codice
        );
        assert!(
            trasformazione.accuratezza_m >= 0.0,
            "{}",
            trasformazione.codice
        );
    }
    // Un CRS risolto dal chiamante non si riproietta.
    let esterno = ResolvedCrs::from_resolved_parts(
        "EPSG:4326".to_owned(),
        serde_json::json!({"type": "GeographicCRS"}),
        CrsKind::Geographic,
        None,
    );
    assert!(!riproiettabile(&esterno));
    assert!(matches!(
        PianoRiproiezione::nuovo(&esterno, &crs("EPSG:4326"), &OpzioniRiproiezione::default()),
        Err(CrsError::NotBuiltin)
    ));
}

#[test]
fn stesso_datum_senza_accuratezza_e_identita_esatta_sullo_stesso_crs() {
    let stesso =
        piano("EPSG:4326", "EPSG:32632", &OpzioniRiproiezione::default()).expect("stesso datum");
    assert_eq!(stesso.percorsi().len(), 1);
    assert!(stesso.percorsi()[0].passi().is_empty());
    assert_eq!(stesso.accuratezza_garantita_m(), 0.0);
    assert_eq!(
        stesso.datum(),
        (
            "World Geodetic System 1984 ensemble",
            "World Geodetic System 1984 ensemble"
        )
    );
    // Stesso CRS e varianti che differiscono solo per l'ordine d'autorita'
    // degli assi: coordinate invariate al bit.
    for (da, a) in [
        ("EPSG:32632", "EPSG:32632"),
        ("EPSG:6707", "EPSG:7791"),
        ("EPSG:4326", "OGC:CRS84"),
    ] {
        let r = riproiettore(da, a, &OpzioniRiproiezione::default());
        let punto = if crs(da).kind() == CrsKind::Geographic {
            (11.123_456_789, 44.987_654_321)
        } else {
            (500_123.456_789, 4_800_987.654_321)
        };
        let uscita = r
            .trasforma(0, punto.0, punto.1)
            .expect("trasforma")
            .expect("area");
        assert_eq!(uscita.0.to_bits(), punto.0.to_bits(), "{da} -> {a}");
        assert_eq!(uscita.1.to_bits(), punto.1.to_bits(), "{da} -> {a}");
    }
    // Accettare un'accuratezza qui non ha effetto: si rifiuta.
    assert!(matches!(
        piano_err("EPSG:4326", "EPSG:32632", &accetta(1.0)),
        CrsError::ReprojectionConfig(_)
    ));
}

#[test]
fn datum_equivalenti_entro_un_centimetro_non_chiedono_accuratezza() {
    // RDN2008 -> ETRS89 (1), EPSG:6710: accuratezza 0 nel registro.
    let equivalenti = piano("EPSG:7791", "EPSG:25832", &OpzioniRiproiezione::default())
        .expect("RDN2008 = ETRS89");
    assert_eq!(equivalenti.percorsi()[0].codici(), vec![6710]);
    assert_eq!(equivalenti.accuratezza_garantita_m(), 0.0);
    // GDA94 -> GDA2020 (1), EPSG:8048: 1 cm, uguale alla precisione.
    let centimetro =
        piano("EPSG:4283", "EPSG:7844", &OpzioniRiproiezione::default()).expect("1 cm");
    assert_eq!(centimetro.percorsi()[0].codici(), vec![8048]);
}

#[test]
fn oltre_il_centimetro_serve_accuratezza_accettata_almeno_pari() {
    // Gauss-Boaga -> RDN2008 / UTM 32N: il percorso migliore senza griglie
    // e' Monte Mario to ETRS89 (1) (4 m) piu' RDN2008 to ETRS89 inverso.
    match piano_err("EPSG:3003", "EPSG:7791", &OpzioniRiproiezione::default()) {
        CrsError::ReprojectionAccuracyNotAccepted { accuracy_m } => assert_eq!(accuracy_m, 4.0),
        altro => panic!("atteso il rifiuto dell'accuratezza: {altro}"),
    }
    assert!(matches!(
        piano_err("EPSG:3003", "EPSG:7791", &accetta(3.99)),
        CrsError::ReprojectionAccuracyNotAccepted { .. }
    ));
    let ammesso = piano("EPSG:3003", "EPSG:7791", &accetta(4.0)).expect("accettata 4 m");
    // A parita' di accuratezza vince l'area d'uso piu' piccola: Sardegna,
    // Sicilia, poi l'Italia continentale.
    let codici: Vec<Vec<u32>> = ammesso
        .percorsi()
        .iter()
        .map(PercorsoDatum::codici)
        .collect();
    assert_eq!(
        codici,
        vec![vec![1661, 6710], vec![1663, 6710], vec![1659, 6710]]
    );
    assert!(ammesso.percorsi()[2].passi()[1].inversa());
    assert!(ammesso.percorsi().iter().all(|p| p.accuratezza_m() <= 4.0));
    // Con piu' tolleranza entrano altri percorsi (Sardegna, Sicilia, via
    // WGS 84), sempre in ordine di accuratezza.
    let largo = piano("EPSG:3003", "EPSG:7791", &accetta(50.0)).expect("50 m");
    assert!(largo.percorsi().len() > ammesso.percorsi().len());
    assert!(largo
        .percorsi()
        .windows(2)
        .all(|coppia| coppia[0].accuratezza_m() <= coppia[1].accuratezza_m()));
    assert!(largo.accuratezza_garantita_m() <= 50.0);
    for valore in [f64::NAN, f64::INFINITY, -1.0] {
        assert!(matches!(
            piano_err("EPSG:3003", "EPSG:7791", &accetta(valore)),
            CrsError::ReprojectionConfig(_)
        ));
    }
}

#[test]
fn senza_percorso_nel_registro_e_un_errore_esplicito() {
    // CGCS2000 non ha trasformazioni EPSG verso gli altri datum della tabella.
    assert!(matches!(
        piano_err("EPSG:4490", "EPSG:4326", &accetta(1000.0)),
        CrsError::ReprojectionPathUnavailable
    ));
}

#[test]
fn le_trasformazioni_imposte_devono_formare_un_percorso() {
    let imposto = OpzioniRiproiezione {
        accuratezza_accettata_m: Some(5.0),
        trasformazioni: Some(vec![1660, 1149]),
        ..OpzioniRiproiezione::default()
    };
    let scelto =
        piano("EPSG:3003", "EPSG:25832", &imposto).expect("Monte Mario -> WGS 84 -> ETRS89");
    assert_eq!(scelto.percorsi().len(), 1);
    assert!(!scelto.percorsi()[0].passi()[0].inversa());
    assert!(scelto.percorsi()[0].passi()[1].inversa());
    assert_eq!(scelto.percorsi()[0].accuratezza_m(), 5.0);
    for codici in [vec![1149, 1660], vec![1660], vec![], vec![999_999]] {
        let opzioni = OpzioniRiproiezione {
            trasformazioni: Some(codici.clone()),
            ..accetta(100.0)
        };
        assert!(
            matches!(
                piano_err("EPSG:3003", "EPSG:25832", &opzioni),
                CrsError::ReprojectionConfig(_)
            ),
            "{codici:?}"
        );
    }
}

#[test]
fn le_griglie_entrano_solo_se_fornite_e_se_servono() {
    let con_griglia = OpzioniRiproiezione {
        accuratezza_accettata_m: Some(0.1),
        griglie: vec![9734],
        ..OpzioniRiproiezione::default()
    };
    let piano_griglia = piano("EPSG:3003", "EPSG:7791", &con_griglia).expect("griglia IGM");
    assert_eq!(piano_griglia.percorsi()[0].codici(), vec![9734]);
    assert_eq!(
        piano_griglia.percorsi()[0].passi()[0].file_registro(),
        Some("35160622_47161840_R40_F00.gsb")
    );
    assert_eq!(
        piano_griglia.griglie().into_iter().collect::<Vec<_>>(),
        vec![9734]
    );
    // Senza il file il riproiettore non nasce; con un file in piu' neppure.
    assert!(matches!(
        Riproiettore::nuovo(piano_griglia.clone(), BTreeMap::new()),
        Err(CrsError::ReprojectionConfig(_))
    ));
    let mut due = BTreeMap::new();
    due.insert(9734, griglia_sintetica());
    due.insert(7709, griglia_sintetica());
    assert!(Riproiettore::nuovo(piano_griglia.clone(), due).is_err());
    // Codice che non e' una griglia, ripetuto, o inutile per la coppia.
    for griglie in [vec![1659], vec![9734, 9734], vec![7709], vec![1]] {
        let opzioni = OpzioniRiproiezione {
            griglie: griglie.clone(),
            ..accetta(100.0)
        };
        assert!(
            matches!(
                piano_err("EPSG:3003", "EPSG:7791", &opzioni),
                CrsError::ReprojectionConfig(_)
            ),
            "{griglie:?}"
        );
    }
    // Fuori dalla griglia il percorso non si applica (Ok(None)).
    let mut una = BTreeMap::new();
    una.insert(9734, griglia_sintetica());
    let r = Riproiettore::nuovo(piano_griglia, una).expect("riproiettore");
    let (x, y) = gauss_boaga(12.5, 47.5);
    assert!(
        r.trasforma(0, x, y).expect("dominio").is_none(),
        "fuori dalla griglia sintetica"
    );
    let (x, y) = gauss_boaga(11.0, 44.0);
    assert!(
        r.trasforma(0, x, y).expect("dominio").is_some(),
        "dentro la griglia sintetica"
    );
}

#[test]
fn fuori_dall_area_d_uso_di_un_passo_il_percorso_non_si_applica() {
    // Monte Mario to WGS 84 (2) vale per la Sardegna: un punto di Roma no.
    let sardegna = OpzioniRiproiezione {
        accuratezza_accettata_m: Some(4.0),
        trasformazioni: Some(vec![1662]),
        ..OpzioniRiproiezione::default()
    };
    let r = riproiettore("EPSG:4265", "EPSG:4326", &sardegna);
    assert!(r.trasforma(0, 12.5, 41.9).expect("dominio").is_none());
    assert!(r.trasforma(0, 9.0, 40.0).expect("dominio").is_some());
}

#[test]
fn dominio_e_regione_si_controllano_su_entrambi_i_lati() {
    let r = riproiettore("EPSG:4326", "EPSG:3857", &OpzioniRiproiezione::default());
    // Oltre la latitudine limite di Mercator: rifiuto, mai un valore.
    assert!(matches!(
        r.trasforma(0, 10.0, 86.0),
        Err(CrsError::CoordinateOutOfDomain {
            violation: CoordinateDomainViolation::OutsideProjectionRegion
        })
    ));
    assert!(matches!(
        r.trasforma(0, f64::NAN, 10.0),
        Err(CrsError::CoordinateOutOfDomain {
            violation: CoordinateDomainViolation::NonFinite
        })
    ));
    assert!(matches!(
        r.trasforma(0, 181.0, 10.0),
        Err(CrsError::CoordinateOutOfDomain { .. })
    ));
    // UTM 32N: un punto nel rettangolo del dominio ma oltre 15 gradi dal
    // meridiano centrale (vicino al polo) si rifiuta in lon/lat.
    let r = riproiettore("EPSG:32632", "EPSG:4326", &OpzioniRiproiezione::default());
    assert!(matches!(
        r.trasforma(0, 700_000.0, 9_300_000.0),
        Err(CrsError::CoordinateOutOfDomain {
            violation: CoordinateDomainViolation::OutsideProjectionRegion
        })
    ));
    // Verso un fuso UTM: la longitudine deve stare nella regione del fuso.
    let r = riproiettore("EPSG:4326", "EPSG:32632", &OpzioniRiproiezione::default());
    assert!(r.trasforma(0, 30.0, 45.0).is_err());
    assert!(r.trasforma(0, 9.0, 45.0).expect("dentro").is_some());
}

#[test]
fn i_fusi_vicini_all_antimeridiano_accettano_le_longitudini_oltre_180() {
    // UTM 1N ha il meridiano centrale a -177: la regione va da -192 a -162,
    // e 179 E (cioe' -181) ci sta.
    let r = riproiettore("EPSG:4326", "EPSG:32601", &OpzioniRiproiezione::default());
    let (x, _) = r.trasforma(0, 179.0, 50.0).expect("dentro").expect("area");
    assert!(x < 500_000.0, "a ovest del meridiano centrale");
    let (x, _) = r.trasforma(0, -170.0, 50.0).expect("dentro").expect("area");
    assert!(x > 500_000.0);
    assert!(r.trasforma(0, 160.0, 50.0).is_err());
}

#[test]
fn i_messaggi_non_riportano_coordinate() {
    let r = riproiettore("EPSG:4326", "EPSG:3857", &OpzioniRiproiezione::default());
    let errore = r
        .trasforma(0, 12.345_678, 88.765_432)
        .expect_err("fuori regione");
    let testo = errore.to_string();
    for frammento in ["12.34", "88.76", "12,34", "88,76"] {
        assert!(!testo.contains(frammento), "{testo}");
    }
}

#[test]
fn andata_e_ritorno_con_cambio_di_datum_torna_al_punto() {
    // Helmert a 7 parametri (Position Vector) avanti e con l'inversa
    // algebrica. Il giro non torna al nanometro: l'altezza ellissoidica che
    // il cambio di datum produce si scarta (CRS 2D, come PROJ), e al
    // ritorno si riparte da altezza nulla; resta sotto il millimetro.
    let opzioni = OpzioniRiproiezione {
        accuratezza_accettata_m: Some(4.0),
        trasformazioni: Some(vec![1660]),
        ..OpzioniRiproiezione::default()
    };
    let andata = riproiettore("EPSG:3003", "EPSG:32632", &opzioni);
    let ritorno = riproiettore("EPSG:32632", "EPSG:3003", &opzioni);
    for (x, y) in [
        (1_500_000.0, 5_000_000.0),
        (1_700_000.0, 4_700_000.0),
        (1_420_000.0, 5_150_000.0),
    ] {
        let (u, v) = andata.trasforma(0, x, y).expect("andata").expect("area");
        let (xr, yr) = ritorno.trasforma(0, u, v).expect("ritorno").expect("area");
        let scarto = (xr - x).hypot(yr - y);
        assert!(scarto < 1e-3, "({x}, {y}): {scarto} m");
        // Il cambio di datum sposta davvero il punto (decine di metri): i
        // falsi est di Gauss-Boaga e UTM 32N differiscono di 1000 km.
        assert!((u - (x - 1_000_000.0)).hypot(v - y) > 10.0);
    }
}

#[test]
fn trasformazione_nulla_non_cambia_lon_lat_fra_ellissoidi_diversi() {
    // NAD83 to WGS 84 (1): traslazioni nulle, GRS 1980 -> WGS 84. Come PROJ
    // (`+proj=noop`), lon/lat restano al bit.
    let r = riproiettore("EPSG:4269", "EPSG:4326", &accetta(4.0));
    let indice = r
        .piano()
        .percorsi()
        .iter()
        .position(|p| p.codici() == vec![1188])
        .expect("NAD83 to WGS 84 (1)");
    let (lon, lat) = r
        .trasforma(indice, -100.123_456_7, 40.765_432_1)
        .expect("dominio")
        .expect("area");
    assert_eq!((lon, lat), (-100.123_456_7, 40.765_432_1));
}

/// La griglia sintetica riscritta big-endian: chiavi intatte, valori girati.
fn big_endian(le: &[u8]) -> Vec<u8> {
    let mut be = Vec::with_capacity(le.len());
    let mut nodi_restanti = 0_usize;
    for record in le.chunks(16) {
        if nodi_restanti > 0 {
            for quattro in record.chunks(4) {
                be.extend(quattro.iter().rev());
            }
            nodi_restanti -= 1;
            continue;
        }
        let (chiave, valore) = record.split_at(8);
        be.extend_from_slice(chiave);
        if matches!(
            chiave,
            b"NUM_OREC" | b"NUM_SREC" | b"NUM_FILE" | b"GS_COUNT"
        ) {
            be.extend(valore[..4].iter().rev());
            be.extend_from_slice(&valore[4..]);
        } else if matches!(
            chiave,
            b"S_LAT   "
                | b"N_LAT   "
                | b"E_LONG  "
                | b"W_LONG  "
                | b"LAT_INC "
                | b"LONG_INC"
                | b"MAJOR_F "
                | b"MINOR_F "
                | b"MAJOR_T "
                | b"MINOR_T "
        ) {
            be.extend(valore.iter().rev());
        } else {
            be.extend_from_slice(valore);
        }
        if chiave == b"GS_COUNT" {
            nodi_restanti = usize::try_from(i32::from_le_bytes([
                valore[0], valore[1], valore[2], valore[3],
            ]))
            .expect("conteggio");
        }
    }
    be
}

#[test]
fn griglia_ntv2_big_endian_uguale_alla_little_endian() {
    let le = byte_griglia_sintetica();
    let piccola = GrigliaNtv2::da_byte(&le).expect("LE");
    let grande = GrigliaNtv2::da_byte(&big_endian(&le)).expect("BE");
    for (lon, lat) in [(7.0, 40.0), (11.0, 44.0), (18.0, 47.0)] {
        assert_eq!(piccola.avanti(lon, lat), grande.avanti(lon, lat));
    }
}

#[test]
fn griglia_ntv2_difettosa_si_rifiuta_senza_valori() {
    let buona = byte_griglia_sintetica();
    let mut casi: Vec<(&str, Vec<u8>)> = vec![
        ("vuoto", Vec::new()),
        ("troncato", buona[..buona.len() / 2].to_vec()),
    ];
    let mut orec = buona.clone();
    orec[8] = 12;
    casi.push(("NUM_OREC", orec));
    let mut tipo = buona.clone();
    tipo[3 * 16 + 8..3 * 16 + 16].copy_from_slice(b"MINUTES ");
    casi.push(("GS_TYPE", tipo));
    // GS_COUNT della prima sottogriglia: record 11 + 10.
    let mut conteggio = buona.clone();
    conteggio[21 * 16 + 8] ^= 1;
    casi.push(("GS_COUNT", conteggio));
    // Primo spostamento non finito.
    let mut nan = buona.clone();
    nan[22 * 16..22 * 16 + 4].copy_from_slice(&f32::NAN.to_le_bytes());
    casi.push(("NaN", nan));
    // Padre del figlio inesistente.
    let posizione_padre = buona
        .windows(8)
        .enumerate()
        .filter(|(_, finestra)| *finestra == b"PARENT  ")
        .map(|(i, _)| i)
        .nth(1)
        .expect("secondo PARENT");
    let mut padre = buona.clone();
    padre[posizione_padre + 8..posizione_padre + 16].copy_from_slice(b"ASSENTE ");
    casi.push(("padre", padre));
    // Due sottogriglie con lo stesso nome: il figlio si chiama RADICE.
    let mut doppione = buona.clone();
    let nome_figlio = posizione_padre - 16;
    doppione[nome_figlio + 8..nome_figlio + 16].copy_from_slice(b"RADICE  ");
    casi.push(("doppione", doppione));
    // Figlio fuori dal padre: il suo sud (record S_LAT del figlio) a 30 N.
    let mut fuori = buona;
    let sud_figlio = posizione_padre + 3 * 16;
    fuori[sud_figlio + 8..sud_figlio + 16].copy_from_slice(&(30.0_f64 * 3600.0).to_le_bytes());
    casi.push(("fuori dal padre", fuori));
    for (nome, byte) in casi {
        let errore = GrigliaNtv2::da_byte(&byte).expect_err(nome);
        assert!(
            matches!(errore, CrsError::GridInvalid { .. }),
            "{nome}: {errore}"
        );
    }
    assert!(matches!(
        GrigliaNtv2::leggi(std::path::Path::new("non-esiste.gsb")),
        Err(CrsError::GridUnreadable)
    ));
}

#[test]
fn la_sottogriglia_piu_fine_vince() {
    let griglia = griglia_sintetica();
    // Dentro il figlio (10..12 E, 43..45 N) lo spostamento e' quello del
    // figlio: la funzione del figlio aggiunge termini alla radice.
    let nel_figlio = griglia.avanti(11.05, 44.05).expect("dentro");
    let byte = byte_griglia_sintetica();
    // Solo la radice: NUM_FILE = 1 e i byte oltre la radice tagliati.
    let nodi_radice = 27 * 27;
    let fine_radice = (11 + 11 + nodi_radice) * 16;
    let mut solo_radice = byte[..fine_radice].to_vec();
    solo_radice[2 * 16 + 8] = 1;
    let radice = GrigliaNtv2::da_byte(&solo_radice).expect("radice sola");
    let nella_radice = radice.avanti(11.05, 44.05).expect("dentro");
    assert_ne!(nel_figlio, nella_radice);
    // Fuori da ogni sottogriglia: nessuno spostamento.
    assert!(griglia.avanti(20.0, 44.0).is_none());
    assert!(griglia
        .indietro(20.0, 44.0)
        .expect("nessun errore")
        .is_none());
}
