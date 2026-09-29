//! Griglie `NTv2` (EPSG 9615) fornite dall'utente.
//!
//! Lettura con la sola libreria standard, senza `unsafe`: il file si legge
//! intero (entro [`MAX_BYTE_GRIGLIA`]) e si interpreta record per record,
//! con ogni conteggio e ogni valore verificato. Formato (specifica `NTv2`,
//! Natural Resources Canada 1995): intestazione generale di 11 record da 16
//! byte (8 di chiave, 8 di valore), poi per ogni sottogriglia 11 record di
//! intestazione e `GS_COUNT` nodi da quattro `f32` (spostamento in
//! latitudine e in longitudine, accuratezze) in secondi d'arco, longitudini
//! positive verso ovest, nodi per righe da sud a nord e in ogni riga da est
//! a ovest. Endianness dal valore di `NUM_OREC` (11).
//!
//! Interpolazione bilineare sulla sottogriglia piu' fine che contiene il
//! punto (una radice che lo contiene, poi i figli), come PROJ. Il verso
//! inverso e' l'iterazione a punto fisso di PROJ (`pj_hgrid_apply`) fino a
//! 1e-12 radianti, qui con al piu' 20 passi e un errore se non converge.
//!
//! Gli errori nominano il difetto del file, mai un valore letto.

use std::path::Path;

use super::super::CrsError;

/// Dimensione massima di un file di griglia: 256 MiB (le griglie nazionali
/// piu' grandi del registro sono di decine di MiB).
pub const MAX_BYTE_GRIGLIA: u64 = 256 * 1024 * 1024;

const RECORD: usize = 16;
const SECONDI_PER_GRADO: f64 = 3600.0;
/// Tolleranza dell'inversa, in gradi: 1e-12 radianti come PROJ.
const TOLLERANZA_INVERSA_GRADI: f64 = 1e-12 * 180.0 / std::f64::consts::PI;

#[derive(Clone, Debug)]
struct Sottogriglia {
    nome: [u8; 8],
    padre: [u8; 8],
    /// Limiti in secondi d'arco, longitudine positiva verso ovest.
    sud: f64,
    nord: f64,
    est: f64,
    ovest: f64,
    passo_lat: f64,
    passo_lon: f64,
    righe: usize,
    colonne: usize,
    /// `(dlat, dlon)` in secondi, per nodo, riga per riga.
    spostamenti: Vec<(f32, f32)>,
    figli: Vec<usize>,
}

impl Sottogriglia {
    fn contiene(&self, lat_s: f64, lon_w: f64) -> bool {
        (self.sud..=self.nord).contains(&lat_s) && (self.est..=self.ovest).contains(&lon_w)
    }

    /// Spostamento bilineare `(dlat, dlon)` in secondi (lon positiva a
    /// ovest) nel punto, che deve stare nella sottogriglia.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    fn spostamento(&self, lat_s: f64, lon_w: f64) -> Option<(f64, f64)> {
        let fx = (lon_w - self.est) / self.passo_lon;
        let fy = (lat_s - self.sud) / self.passo_lat;
        // `contiene` garantisce 0 <= fx <= colonne - 1: il troncamento e'
        // un indice valido, e l'ultimo nodo usa la cella precedente.
        let ix = (fx.floor().max(0.0) as usize).min(self.colonne.checked_sub(2)?);
        let iy = (fy.floor().max(0.0) as usize).min(self.righe.checked_sub(2)?);
        let tx = fx - ix as f64;
        let ty = fy - iy as f64;
        let nodo = |i: usize, j: usize| -> Option<(f64, f64)> {
            let (dlat, dlon) = *self
                .spostamenti
                .get(j.checked_mul(self.colonne)?.checked_add(i)?)?;
            Some((f64::from(dlat), f64::from(dlon)))
        };
        let (a0, b0) = nodo(ix, iy)?;
        let (a1, b1) = nodo(ix + 1, iy)?;
        let (a2, b2) = nodo(ix, iy + 1)?;
        let (a3, b3) = nodo(ix + 1, iy + 1)?;
        let bil = |v0: f64, v1: f64, v2: f64, v3: f64| {
            let basso = (v1 - v0).mul_add(tx, v0);
            let alto = (v3 - v2).mul_add(tx, v2);
            (alto - basso).mul_add(ty, basso)
        };
        Some((bil(a0, a1, a2, a3), bil(b0, b1, b2, b3)))
    }
}

/// Una griglia `NTv2` letta e verificata.
#[derive(Clone, Debug)]
pub struct GrigliaNtv2 {
    sottogriglie: Vec<Sottogriglia>,
    radici: Vec<usize>,
}

const fn difetto(motivo: &'static str) -> CrsError {
    CrsError::GridInvalid { reason: motivo }
}

struct Lettore<'a> {
    byte: &'a [u8],
    posizione: usize,
    little_endian: bool,
}

impl Lettore<'_> {
    fn record(&mut self) -> Result<(&[u8], [u8; 8]), CrsError> {
        let fine = self
            .posizione
            .checked_add(RECORD)
            .ok_or_else(|| difetto("file troncato"))?;
        let record = self
            .byte
            .get(self.posizione..fine)
            .ok_or_else(|| difetto("file troncato"))?;
        self.posizione = fine;
        let (chiave, valore) = record.split_at(8);
        let mut v = [0_u8; 8];
        v.copy_from_slice(valore);
        Ok((chiave, v))
    }

    fn chiave(&mut self, attesa: [u8; 8]) -> Result<[u8; 8], CrsError> {
        let (chiave, valore) = self.record()?;
        if !chiave.eq_ignore_ascii_case(&attesa) {
            return Err(difetto("intestazione NTv2 con una chiave inattesa"));
        }
        Ok(valore)
    }

    fn intero_di(&mut self, chiave: [u8; 8]) -> Result<i32, CrsError> {
        let valore = self.chiave(chiave)?;
        Ok(self.intero(valore))
    }

    fn reale_di(&mut self, chiave: [u8; 8]) -> Result<f64, CrsError> {
        let valore = self.chiave(chiave)?;
        self.reale(valore)
    }

    const fn intero(&self, valore: [u8; 8]) -> i32 {
        let quattro = [valore[0], valore[1], valore[2], valore[3]];
        if self.little_endian {
            i32::from_le_bytes(quattro)
        } else {
            i32::from_be_bytes(quattro)
        }
    }

    const fn reale(&self, valore: [u8; 8]) -> Result<f64, CrsError> {
        let x = if self.little_endian {
            f64::from_le_bytes(valore)
        } else {
            f64::from_be_bytes(valore)
        };
        if x.is_finite() {
            Ok(x)
        } else {
            Err(difetto("valore non finito nell'intestazione"))
        }
    }

    fn f32_a(&self, posizione: usize) -> Result<f32, CrsError> {
        let quattro: [u8; 4] = self
            .byte
            .get(posizione..posizione + 4)
            .and_then(|fetta| fetta.try_into().ok())
            .ok_or_else(|| difetto("file troncato"))?;
        let x = if self.little_endian {
            f32::from_le_bytes(quattro)
        } else {
            f32::from_be_bytes(quattro)
        };
        if x.is_finite() {
            Ok(x)
        } else {
            Err(difetto("spostamento non finito"))
        }
    }
}

/// Numero di nodi lungo un asse: `(max - min) / passo + 1`, intero.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn nodi(minimo: f64, massimo: f64, passo: f64) -> Result<usize, CrsError> {
    // Anche NaN si rifiuta: le intestazioni sono gia' finite (`reale`).
    if passo <= 0.0 || massimo <= minimo {
        return Err(difetto("estensione o passo della sottogriglia non validi"));
    }
    let intervalli = (massimo - minimo) / passo;
    let arrotondati = intervalli.round();
    if (intervalli - arrotondati).abs() > 1e-6 || !(1.0..=1e7).contains(&arrotondati) {
        return Err(difetto("estensione non multipla del passo"));
    }
    Ok(arrotondati as usize + 1)
}

/// Intestazione e nodi di una sottogriglia.
fn leggi_sottogriglia(lettore: &mut Lettore<'_>, byte: &[u8]) -> Result<Sottogriglia, CrsError> {
    let nome = lettore.chiave(*b"SUB_NAME")?;
    let padre = lettore.chiave(*b"PARENT  ")?;
    lettore.chiave(*b"CREATED ")?;
    lettore.chiave(*b"UPDATED ")?;
    let sud = lettore.reale_di(*b"S_LAT   ")?;
    let nord = lettore.reale_di(*b"N_LAT   ")?;
    let est = lettore.reale_di(*b"E_LONG  ")?;
    let ovest = lettore.reale_di(*b"W_LONG  ")?;
    let passo_lat = lettore.reale_di(*b"LAT_INC ")?;
    let passo_lon = lettore.reale_di(*b"LONG_INC")?;
    let conteggio = lettore.intero_di(*b"GS_COUNT")?;
    let righe = nodi(sud, nord, passo_lat)?;
    let colonne = nodi(est, ovest, passo_lon)?;
    let attesi = righe
        .checked_mul(colonne)
        .filter(|n| usize::try_from(conteggio).ok() == Some(*n))
        .ok_or_else(|| difetto("GS_COUNT diverso da righe per colonne"))?;
    let inizio = lettore.posizione;
    let fine = attesi
        .checked_mul(RECORD)
        .and_then(|n| n.checked_add(inizio))
        .filter(|fine| *fine <= byte.len())
        .ok_or_else(|| difetto("file troncato"))?;
    let mut spostamenti = Vec::with_capacity(attesi);
    let mut posizione = inizio;
    while posizione < fine {
        spostamenti.push((lettore.f32_a(posizione)?, lettore.f32_a(posizione + 4)?));
        posizione += RECORD;
    }
    lettore.posizione = fine;
    Ok(Sottogriglia {
        nome,
        padre,
        sud,
        nord,
        est,
        ovest,
        passo_lat,
        passo_lon,
        righe,
        colonne,
        spostamenti,
        figli: Vec::new(),
    })
}

/// Collega i figli ai padri e verifica che la gerarchia sia un albero;
/// restituisce le radici.
fn albero(sottogriglie: &mut [Sottogriglia]) -> Result<Vec<usize>, CrsError> {
    let mut radici = Vec::new();
    for indice in 0..sottogriglie.len() {
        let padre = sottogriglie[indice].padre;
        if sottogriglie[..indice]
            .iter()
            .any(|altra| altra.nome == sottogriglie[indice].nome)
        {
            return Err(difetto("due sottogriglie con lo stesso nome"));
        }
        // `NONE` seguito da spazi o da NUL, come lo scrivono i vari
        // produttori (PROJ confronta i primi quattro caratteri).
        if padre[..4].eq_ignore_ascii_case(b"NONE")
            && padre[4..].iter().all(|b| *b == b' ' || *b == 0)
        {
            radici.push(indice);
            continue;
        }
        let posizione_padre = sottogriglie
            .iter()
            .position(|candidata| candidata.nome == padre)
            .filter(|p| *p != indice)
            .ok_or_else(|| difetto("sottogriglia con un padre inesistente"))?;
        let (figlio, genitore) = (&sottogriglie[indice], &sottogriglie[posizione_padre]);
        if figlio.sud < genitore.sud
            || figlio.nord > genitore.nord
            || figlio.est < genitore.est
            || figlio.ovest > genitore.ovest
        {
            // Fuori dal padre non si raggiungerebbe mai: i punti avrebbero
            // lo spostamento del padre senza errore.
            return Err(difetto("sottogriglia fuori dal padre"));
        }
        sottogriglie[posizione_padre].figli.push(indice);
    }
    if radici.is_empty() {
        return Err(difetto("nessuna sottogriglia radice"));
    }
    // Un ciclo di padri lascerebbe sottogriglie irraggiungibili: ogni
    // sottogriglia deve discendere da una radice.
    let mut raggiunte = vec![false; sottogriglie.len()];
    let mut pila = radici.clone();
    while let Some(indice) = pila.pop() {
        if std::mem::replace(&mut raggiunte[indice], true) {
            return Err(difetto("gerarchia delle sottogriglie non ad albero"));
        }
        pila.extend(sottogriglie[indice].figli.iter().copied());
    }
    if raggiunte.iter().any(|r| !r) {
        return Err(difetto("gerarchia delle sottogriglie non ad albero"));
    }
    Ok(radici)
}

impl GrigliaNtv2 {
    /// Legge e verifica un file `NTv2`.
    ///
    /// # Errors
    ///
    /// [`CrsError::GridUnreadable`] se il file non si apre o supera
    /// [`MAX_BYTE_GRIGLIA`]; [`CrsError::GridInvalid`] per ogni difetto di
    /// formato (come [`Self::da_byte`]).
    pub fn leggi(percorso: &Path) -> Result<Self, CrsError> {
        let dimensione = std::fs::metadata(percorso)
            .map_err(|_| CrsError::GridUnreadable)?
            .len();
        if dimensione > MAX_BYTE_GRIGLIA {
            return Err(difetto("file oltre il limite di 256 MiB"));
        }
        let byte = std::fs::read(percorso).map_err(|_| CrsError::GridUnreadable)?;
        Self::da_byte(&byte)
    }

    /// Interpreta i byte di un file `NTv2`.
    ///
    /// # Errors
    ///
    /// [`CrsError::GridInvalid`]: intestazione non `NTv2`, unita' diverse dai
    /// secondi d'arco, sottogriglie con estensione, passo o conteggio
    /// incoerenti, padri inesistenti, spostamenti non finiti, file troncato.
    pub fn da_byte(byte: &[u8]) -> Result<Self, CrsError> {
        let mut lettore = Lettore {
            byte,
            posizione: 0,
            little_endian: true,
        };
        let (chiave, valore) = lettore.record()?;
        if !chiave.eq_ignore_ascii_case(b"NUM_OREC") {
            return Err(difetto("non e' un file NTv2"));
        }
        let le = i32::from_le_bytes([valore[0], valore[1], valore[2], valore[3]]);
        let be = i32::from_be_bytes([valore[0], valore[1], valore[2], valore[3]]);
        lettore.little_endian = if le == 11 {
            true
        } else if be == 11 {
            false
        } else {
            return Err(difetto("NUM_OREC diverso da 11"));
        };
        let num_srec = lettore.intero_di(*b"NUM_SREC")?;
        if num_srec != 11 {
            return Err(difetto("NUM_SREC diverso da 11"));
        }
        let num_file = lettore.intero_di(*b"NUM_FILE")?;
        let num_file = usize::try_from(num_file)
            .ok()
            .filter(|n| (1..=10_000).contains(n))
            .ok_or_else(|| difetto("NUM_FILE non valido"))?;
        let tipo = lettore.chiave(*b"GS_TYPE ")?;
        if !tipo.eq_ignore_ascii_case(b"SECONDS ") {
            return Err(difetto("unita' diverse dai secondi d'arco (GS_TYPE)"));
        }
        for chiave in [
            b"VERSION ",
            b"SYSTEM_F",
            b"SYSTEM_T",
            b"MAJOR_F ",
            b"MINOR_F ",
            b"MAJOR_T ",
            b"MINOR_T ",
        ] {
            lettore.chiave(*chiave)?;
        }
        let mut sottogriglie = Vec::with_capacity(num_file);
        for _ in 0..num_file {
            sottogriglie.push(leggi_sottogriglia(&mut lettore, byte)?);
        }
        let radici = albero(&mut sottogriglie)?;
        Ok(Self {
            sottogriglie,
            radici,
        })
    }

    /// La sottogriglia piu' fine che contiene il punto (secondi, lon a ovest).
    fn cerca(&self, lat_s: f64, lon_w: f64) -> Option<&Sottogriglia> {
        let mut corrente = self
            .radici
            .iter()
            .map(|i| &self.sottogriglie[*i])
            .find(|g| g.contiene(lat_s, lon_w))?;
        loop {
            match corrente
                .figli
                .iter()
                .map(|i| &self.sottogriglie[*i])
                .find(|g| g.contiene(lat_s, lon_w))
            {
                Some(figlio) => corrente = figlio,
                None => return Some(corrente),
            }
        }
    }

    /// Spostamento in gradi `(dlon, dlat)` (longitudine positiva a est) nel
    /// punto; `None` fuori dalla griglia.
    fn spostamento_gradi(&self, lon: f64, lat: f64) -> Option<(f64, f64)> {
        let lat_s = lat * SECONDI_PER_GRADO;
        let lon_w = -lon * SECONDI_PER_GRADO;
        let (dlat, dlon_w) = self.cerca(lat_s, lon_w)?.spostamento(lat_s, lon_w)?;
        Some((-dlon_w / SECONDI_PER_GRADO, dlat / SECONDI_PER_GRADO))
    }

    /// Dal datum sorgente della griglia al datum d'arrivo; `None` fuori
    /// dalla griglia.
    pub(in crate::crs) fn avanti(&self, lon: f64, lat: f64) -> Option<(f64, f64)> {
        let (dlon, dlat) = self.spostamento_gradi(lon, lat)?;
        Some((lon + dlon, lat + dlat))
    }

    /// Inversa di [`Self::avanti`] per iterazione; `Ok(None)` se un passo
    /// esce dalla griglia.
    ///
    /// # Errors
    ///
    /// [`CrsError::ReprojectionNotConverged`] se in 20 passi la correzione
    /// non scende sotto 1e-12 radianti.
    pub(in crate::crs) fn indietro(
        &self,
        lon: f64,
        lat: f64,
    ) -> Result<Option<(f64, f64)>, CrsError> {
        let Some((dlon, dlat)) = self.spostamento_gradi(lon, lat) else {
            return Ok(None);
        };
        let (mut x, mut y) = (lon - dlon, lat - dlat);
        for _ in 0..20 {
            let Some((dlon, dlat)) = self.spostamento_gradi(x, y) else {
                return Ok(None);
            };
            let (ex, ey) = (x + dlon - lon, y + dlat - lat);
            x -= ex;
            y -= ey;
            if ex.hypot(ey) <= TOLLERANZA_INVERSA_GRADI {
                return Ok(Some((x, y)));
            }
        }
        Err(CrsError::ReprojectionNotConverged)
    }
}
