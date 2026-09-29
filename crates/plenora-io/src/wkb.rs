//! Lettura strutturale del WKB delle colonne `GeoParquet`.
//!
//! `GeoParquet` 1.1 ammette solo il WKB ISO dei sette tipi lineari
//! (`Point` … `GeometryCollection`), 2D o 3D (codici 1–7 e 1001–1007):
//! niente M, niente curve, niente EWKB. Questo modulo cammina ogni cella
//! senza decodificarla in una geometria e ne ricava il tipo di primo
//! livello, la presenza di Z e il riquadro XY delle coordinate. Serve in
//! scrittura (per `geometry_types` e `bbox`) e in lettura (per verificare
//! che i `geometry_types` dichiarati dal file siano veri).
//!
//! La camminata è anche una validazione: byte order, codici di tipo,
//! conteggi limitati dai byte rimasti, figli coerenti con il genitore,
//! profondità, nessun byte in eccesso, coordinate finite. Un `Point` con
//! tutte le coordinate NaN è il punto vuoto e non entra nel riquadro; ogni
//! altra coordinata non finita è un errore. I messaggi non riportano mai
//! byte o coordinate.

use std::collections::BTreeSet;

use plenora_core::contract::GeometryType;
use plenora_core::{PlenoraError, Result};

/// Profondità massima di annidamento (collezioni dentro collezioni), la
/// stessa di `Limits::default().max_geometry_depth`.
pub const PROFONDITA_MASSIMA: usize = 64;

/// Riquadro XY di un insieme di coordinate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Riquadro {
    pub xmin: f64,
    pub ymin: f64,
    pub xmax: f64,
    pub ymax: f64,
}

/// Ciò che la camminata ricava da una colonna.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sommario {
    /// Coppie (tipo di primo livello, Z) presenti, nell'ordine canonico.
    pub tipi: BTreeSet<(GeometryType, bool)>,
    /// Riquadro XY di tutte le coordinate non vuote; `None` se non ce n'è.
    pub riquadro: Option<Riquadro>,
}

impl Sommario {
    fn aggiungi_punto(&mut self, x: f64, y: f64) {
        let nuovo = self.riquadro.map_or(
            Riquadro {
                xmin: x,
                ymin: y,
                xmax: x,
                ymax: y,
            },
            |r| Riquadro {
                xmin: r.xmin.min(x),
                ymin: r.ymin.min(y),
                xmax: r.xmax.max(x),
                ymax: r.ymax.max(y),
            },
        );
        self.riquadro = Some(nuovo);
    }
}

fn non_valido(motivo: &str) -> PlenoraError {
    PlenoraError::DataMapping(format!("WKB non ammesso da GeoParquet 1.1: {motivo}"))
}

/// Il tipo canonico di un codice base 1–7.
const fn tipo_da_base(base: u32) -> Option<GeometryType> {
    match base {
        1 => Some(GeometryType::Point),
        2 => Some(GeometryType::LineString),
        3 => Some(GeometryType::Polygon),
        4 => Some(GeometryType::MultiPoint),
        5 => Some(GeometryType::MultiLineString),
        6 => Some(GeometryType::MultiPolygon),
        7 => Some(GeometryType::GeometryCollection),
        _ => None,
    }
}

struct Lettore<'a> {
    byte: &'a [u8],
    pos: usize,
}

impl Lettore<'_> {
    const fn rimasti(&self) -> usize {
        self.byte.len() - self.pos
    }

    fn prendi<const N: usize>(&mut self) -> Result<[u8; N]> {
        let fine = self
            .pos
            .checked_add(N)
            .filter(|fine| *fine <= self.byte.len())
            .ok_or_else(|| non_valido("cella troncata"))?;
        let mut uscita = [0_u8; N];
        uscita.copy_from_slice(&self.byte[self.pos..fine]);
        self.pos = fine;
        Ok(uscita)
    }

    fn u32(&mut self, little: bool) -> Result<u32> {
        let byte = self.prendi::<4>()?;
        Ok(if little {
            u32::from_le_bytes(byte)
        } else {
            u32::from_be_bytes(byte)
        })
    }

    fn f64(&mut self, little: bool) -> Result<f64> {
        let byte = self.prendi::<8>()?;
        Ok(if little {
            f64::from_le_bytes(byte)
        } else {
            f64::from_be_bytes(byte)
        })
    }

    /// Conteggio di elementi da almeno `minimo` byte ciascuno: rifiutato se
    /// i byte rimasti non possono contenerli (niente cicli su conteggi
    /// inventati).
    fn conteggio(&mut self, little: bool, minimo: usize) -> Result<usize> {
        let n = usize::try_from(self.u32(little)?)
            .map_err(|_| non_valido("conteggio non rappresentabile"))?;
        let servono = n
            .checked_mul(minimo)
            .ok_or_else(|| non_valido("conteggio oltre i byte della cella"))?;
        if servono > self.rimasti() {
            return Err(non_valido("conteggio oltre i byte della cella"));
        }
        Ok(n)
    }
}

/// Una coordinata (X, Y e, con `z`, Z): X e Y, e se è il punto vuoto
/// (tutte NaN).
fn coordinata(lettore: &mut Lettore<'_>, little: bool, z: bool) -> Result<(f64, f64, bool)> {
    let x = lettore.f64(little)?;
    let y = lettore.f64(little)?;
    let terza = if z { Some(lettore.f64(little)?) } else { None };
    let tutte_nan = x.is_nan() && y.is_nan() && terza.is_none_or(f64::is_nan);
    if tutte_nan {
        return Ok((x, y, true));
    }
    if !x.is_finite() || !y.is_finite() || terza.is_some_and(|valore| !valore.is_finite()) {
        return Err(non_valido("coordinata non finita"));
    }
    Ok((x, y, false))
}

fn sequenza(
    lettore: &mut Lettore<'_>,
    little: bool,
    z: bool,
    sommario: &mut Sommario,
) -> Result<usize> {
    let passo = if z { 24 } else { 16 };
    let n = lettore.conteggio(little, passo)?;
    for _ in 0..n {
        let (x, y, vuota) = coordinata(lettore, little, z)?;
        if vuota {
            return Err(non_valido("coordinata non finita"));
        }
        sommario.aggiungi_punto(x, y);
    }
    Ok(n)
}

/// Il genitore che vincola il figlio: tipo base atteso (per i multi) e Z.
#[derive(Clone, Copy)]
struct Vincolo {
    base: Option<u32>,
    z: bool,
}

fn geometria(
    lettore: &mut Lettore<'_>,
    profondita: usize,
    vincolo: Option<Vincolo>,
    sommario: &mut Sommario,
) -> Result<(u32, bool)> {
    if profondita > PROFONDITA_MASSIMA {
        return Err(non_valido("annidamento oltre il limite"));
    }
    let little = match lettore.prendi::<1>()?[0] {
        0 => false,
        1 => true,
        _ => return Err(non_valido("byte order non valido")),
    };
    let codice = lettore.u32(little)?;
    if codice & 0xE000_0000 != 0 {
        return Err(non_valido("EWKB (flag SRID, Z o M nel codice di tipo)"));
    }
    let base = codice % 1000;
    let z = match codice / 1000 {
        0 => false,
        1 => true,
        2 | 3 => return Err(non_valido("coordinate M")),
        _ => return Err(non_valido("codice di tipo sconosciuto")),
    };
    if tipo_da_base(base).is_none() {
        return Err(non_valido("tipo geometrico fuori dai sette tipi lineari"));
    }
    if let Some(vincolo) = vincolo {
        if vincolo.z != z || vincolo.base.is_some_and(|atteso| atteso != base) {
            return Err(non_valido("figlio incoerente con la multi-geometria"));
        }
    }
    match base {
        1 => {
            let (x, y, vuoto) = coordinata(lettore, little, z)?;
            if !vuoto {
                sommario.aggiungi_punto(x, y);
            }
        }
        2 => {
            if sequenza(lettore, little, z, sommario)? == 1 {
                return Err(non_valido("LineString con una sola coordinata"));
            }
        }
        3 => {
            let anelli = lettore.conteggio(little, 4)?;
            for _ in 0..anelli {
                let n = sequenza(lettore, little, z, sommario)?;
                if n > 0 && n < 4 {
                    return Err(non_valido("anello con meno di quattro coordinate"));
                }
            }
        }
        4..=6 => {
            let figlio = base - 3;
            let n = lettore.conteggio(little, 5)?;
            for _ in 0..n {
                geometria(
                    lettore,
                    profondita + 1,
                    Some(Vincolo {
                        base: Some(figlio),
                        z,
                    }),
                    sommario,
                )?;
            }
        }
        _ => {
            let n = lettore.conteggio(little, 5)?;
            for _ in 0..n {
                geometria(
                    lettore,
                    profondita + 1,
                    Some(Vincolo { base: None, z }),
                    sommario,
                )?;
            }
        }
    }
    Ok((base, z))
}

/// Cammina una cella e aggiunge al sommario il suo tipo e le sue
/// coordinate.
///
/// # Errors
///
/// `DataMapping` se la cella non è WKB ISO 2D/3D dei sette tipi lineari o
/// ha coordinate non finite (il punto vuoto è ammesso).
pub fn scansiona_cella(cella: &[u8], sommario: &mut Sommario) -> Result<()> {
    let mut lettore = Lettore {
        byte: cella,
        pos: 0,
    };
    let (base, z) = geometria(&mut lettore, 0, None, sommario)?;
    if lettore.rimasti() != 0 {
        return Err(non_valido("byte in eccesso dopo la geometria"));
    }
    let tipo = tipo_da_base(base).ok_or_else(|| non_valido("tipo geometrico sconosciuto"))?;
    sommario.tipi.insert((tipo, z));
    Ok(())
}

/// Nome `GeoParquet` di un tipo (`"Point"`, `"LineString Z"`, …); `None` per i
/// tipi che `GeoParquet` 1.1 non ammette.
#[must_use]
pub fn nome_geoparquet(tipo: GeometryType, z: bool) -> Option<String> {
    let base = match tipo {
        GeometryType::Point => "Point",
        GeometryType::LineString => "LineString",
        GeometryType::Polygon => "Polygon",
        GeometryType::MultiPoint => "MultiPoint",
        GeometryType::MultiLineString => "MultiLineString",
        GeometryType::MultiPolygon => "MultiPolygon",
        GeometryType::GeometryCollection => "GeometryCollection",
        _ => return None,
    };
    Some(if z {
        format!("{base} Z")
    } else {
        base.to_owned()
    })
}

/// Tipo e Z da un nome `GeoParquet`; `None` per un nome fuori dalla specifica.
#[must_use]
pub fn tipo_da_nome(nome: &str) -> Option<(GeometryType, bool)> {
    let (base, z) = nome
        .strip_suffix(" Z")
        .map_or((nome, false), |base| (base, true));
    let tipo = match base {
        "Point" => GeometryType::Point,
        "LineString" => GeometryType::LineString,
        "Polygon" => GeometryType::Polygon,
        "MultiPoint" => GeometryType::MultiPoint,
        "MultiLineString" => GeometryType::MultiLineString,
        "MultiPolygon" => GeometryType::MultiPolygon,
        "GeometryCollection" => GeometryType::GeometryCollection,
        _ => return None,
    };
    Some((tipo, z))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn punto(x: f64, y: f64) -> Vec<u8> {
        let mut v = vec![1_u8];
        v.extend_from_slice(&1_u32.to_le_bytes());
        v.extend_from_slice(&x.to_le_bytes());
        v.extend_from_slice(&y.to_le_bytes());
        v
    }

    #[test]
    fn punto_little_e_big_endian() {
        let mut s = Sommario::default();
        scansiona_cella(&punto(1.0, 2.0), &mut s).expect("punto");
        let mut big = vec![0_u8];
        big.extend_from_slice(&1_u32.to_be_bytes());
        big.extend_from_slice(&3.0_f64.to_be_bytes());
        big.extend_from_slice(&(-4.0_f64).to_be_bytes());
        scansiona_cella(&big, &mut s).expect("punto big endian");
        assert_eq!(
            s.riquadro,
            Some(Riquadro {
                xmin: 1.0,
                ymin: -4.0,
                xmax: 3.0,
                ymax: 2.0
            })
        );
        assert_eq!(
            s.tipi.into_iter().collect::<Vec<_>>(),
            vec![(GeometryType::Point, false)]
        );
    }

    #[test]
    fn punto_vuoto_non_entra_nel_riquadro() {
        let mut s = Sommario::default();
        scansiona_cella(&punto(f64::NAN, f64::NAN), &mut s).expect("punto vuoto");
        assert_eq!(s.riquadro, None);
        assert!(scansiona_cella(&punto(f64::NAN, 1.0), &mut s).is_err());
        assert!(scansiona_cella(&punto(f64::INFINITY, 1.0), &mut s).is_err());
    }

    #[test]
    fn rifiuti() {
        let mut s = Sommario::default();
        // Troncata, byte in eccesso, byte order, EWKB, M, curva.
        assert!(scansiona_cella(&punto(1.0, 2.0)[..10], &mut s).is_err());
        let mut lungo = punto(1.0, 2.0);
        lungo.push(0);
        assert!(scansiona_cella(&lungo, &mut s).is_err());
        let mut ordine = punto(1.0, 2.0);
        ordine[0] = 2;
        assert!(scansiona_cella(&ordine, &mut s).is_err());
        for codice in [0x2000_0001_u32, 0x8000_0001, 2001, 3001, 8, 1017] {
            let mut cella = punto(1.0, 2.0);
            cella[1..5].copy_from_slice(&codice.to_le_bytes());
            assert!(scansiona_cella(&cella, &mut s).is_err(), "codice {codice}");
        }
        // Conteggio enorme: rifiutato senza iterare.
        let mut linea = vec![1_u8];
        linea.extend_from_slice(&2_u32.to_le_bytes());
        linea.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(scansiona_cella(&linea, &mut s).is_err());
        // MultiPoint con un figlio LineString.
        let mut multi = vec![1_u8];
        multi.extend_from_slice(&4_u32.to_le_bytes());
        multi.extend_from_slice(&1_u32.to_le_bytes());
        let mut figlio = vec![1_u8];
        figlio.extend_from_slice(&2_u32.to_le_bytes());
        figlio.extend_from_slice(&0_u32.to_le_bytes());
        multi.extend_from_slice(&figlio);
        assert!(scansiona_cella(&multi, &mut s).is_err());
        assert!(s.tipi.is_empty());
    }

    #[test]
    fn annidamento_limitato() {
        let mut cella = Vec::new();
        for _ in 0..=PROFONDITA_MASSIMA + 1 {
            cella.push(1_u8);
            cella.extend_from_slice(&7_u32.to_le_bytes());
            cella.extend_from_slice(&1_u32.to_le_bytes());
        }
        cella.extend_from_slice(&punto(0.0, 0.0));
        assert!(scansiona_cella(&cella, &mut Sommario::default()).is_err());
    }

    #[test]
    fn nomi_andata_e_ritorno() {
        for tipo in GeometryType::ALL {
            for z in [false, true] {
                if let Some(nome) = nome_geoparquet(*tipo, z) {
                    assert_eq!(tipo_da_nome(&nome), Some((*tipo, z)));
                }
            }
        }
        assert_eq!(tipo_da_nome("Point M"), None);
        assert_eq!(tipo_da_nome("point"), None);
        assert_eq!(tipo_da_nome("Point  Z"), None);
    }
}
