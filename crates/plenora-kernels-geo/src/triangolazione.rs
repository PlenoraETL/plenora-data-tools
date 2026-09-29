//! Triangolazione di Delaunay non vincolata con il caricamento in blocco di
//! `spade`, per `geo.delaunay` ([`crate::extended_algorithms::delaunay`]) e
//! `geo.voronoi` ([`crate::advanced::voronoi_cells`]).
//!
//! `geo` 0.33.1 (`unconstrained_triangulation_raw`) inserisce i punti in
//! `spade` uno alla volta; qui gli stessi punti entrano con
//! `DelaunayTriangulation::bulk_load`, un ordine di grandezza piu' veloce.
//! Si ricostruisce esattamente cio' che l'inserimento incrementale lascia nei
//! vertici, cosi' i chiamanti ne riproducono l'uscita:
//!
//! - **quali vertici**: `spade` fonde i punti con posizione uguale (`==` sui
//!   `f64`, quindi `-0.0 == 0.0`); qui i duplicati si tolgono prima, con la
//!   stessa uguaglianza, e il conteggio dei vertici dopo il caricamento si
//!   verifica (nessun punto perso o fuso in silenzio);
//! - **in che ordine**: l'indice di un vertice nell'incrementale e' l'ordine
//!   di prima comparsa della sua posizione; qui e' il [`Sito::rango`];
//! - **con quali bit**: un duplicato inserito piu' tardi sostituisce i dati
//!   del vertice, tranne quando i vertici distinti sono almeno due e ancora
//!   tutti collineari (`insert_when_all_vertices_on_line` di `spade` 2.15.1
//!   risponde `Updated` senza aggiornare). La differenza si vede solo fra
//!   `-0.0` e `0.0`, e qui si replica ([`siti_distinti`]);
//! - **quale errore**: le coordinate si validano in ordine d'ingresso con
//!   `spade::validate_vertex`, lo stesso controllo che `insert` fa punto per
//!   punto, quindi il primo punto fuori dominio da' lo stesso
//!   `InsertionError`.
//!
//! Perche' `bulk_load` e non `bulk_load_stable`: la variante stabile
//! (`try_bulk_load_cdt` di `spade` 2.15.1) reinserisce i vertici saltati
//! iterando un `std::collections::HashSet` a seme casuale, quindi in un
//! ordine che cambia da processo a processo; su ingressi degeneri (griglie,
//! punti cocircolari) la triangolazione e gli indici delle facce
//! cambierebbero con esso. `bulk_load` tiene i saltati in un `Vec`: stesso
//! ingresso, stessa triangolazione. L'ordine dei vertici che la variante
//! stabile conserverebbe qui si ricostruisce comunque con il rango.

use geo::{Coord, GeoNum, Kernel, Orientation};
use spade::{DelaunayTriangulation, HasPosition, InsertionError, Point2, Triangulation};

/// Un vertice della triangolazione: la coordinata con i bit che
/// l'inserimento incrementale le avrebbe lasciato, e il suo rango.
#[derive(Clone, Copy, Debug)]
pub struct Sito {
    /// La coordinata del vertice, con i bit che l'inserimento incrementale
    /// gli lascerebbe ([`siti_distinti`]).
    pub coordinata: Coord<f64>,
    /// Indice del vertice nell'inserimento incrementale: ordine di prima
    /// comparsa della posizione fra i punti d'ingresso.
    pub rango: usize,
}

impl HasPosition for Sito {
    type Scalar = f64;

    fn position(&self) -> Point2<f64> {
        Point2::new(self.coordinata.x, self.coordinata.y)
    }
}

/// Perche' la triangolazione non si e' costruita.
#[derive(Clone, Copy, Debug)]
pub enum ErroreTriangolazione {
    /// Coordinata fuori dal dominio di `spade` (`[2^-142, 2^201]` in
    /// modulo, o zero), NaN o infinita: lo stesso errore dell'incrementale.
    Inserimento(InsertionError),
    /// Il caricamento in blocco ha restituito un numero di vertici diverso
    /// dai siti distinti: non deve accadere, e se accade non si pubblica una
    /// triangolazione di punti diversi da quelli d'ingresso.
    VerticiInattesi,
}

/// La triangolazione con i siti distinti in ordine di rango.
pub struct Triangolazione {
    /// La triangolazione di `spade`, un vertice per sito distinto.
    pub triangolazione: DelaunayTriangulation<Sito>,
    /// `siti[r]` e' la coordinata del vertice di rango `r`.
    pub siti: Vec<Coord<f64>>,
}

/// Chiave d'uguaglianza di `spade`: `==` sui `f64`, cioe' i bit con lo zero
/// senza segno. I NaN non arrivano qui (li rifiuta `validate_vertex`).
fn chiave(coordinata: Coord<f64>) -> (u64, u64) {
    let senza_segno = |valore: f64| if valore == 0.0 { 0.0_f64 } else { valore };
    (
        senza_segno(coordinata.x).to_bits(),
        senza_segno(coordinata.y).to_bits(),
    )
}

/// I siti distinti in ordine di prima comparsa, con i bit che l'inserimento
/// incrementale di `spade` 2.15.1 lascia a ogni vertice.
///
/// Un duplicato all'indice `i` sostituisce i dati del vertice se in quel
/// momento i vertici distinti sono uno solo (`insert_second_vertex`) o se
/// la triangolazione ha gia' una faccia; non li sostituisce se i distinti
/// sono almeno due e tutti collineari. La collinearita' si decide con
/// l'orientazione esatta di `geo` rispetto ai primi due siti distinti, come
/// `spade` la decide con il predicato esatto rispetto a un lato della retta.
pub fn siti_distinti(coordinate: &[Coord<f64>]) -> Vec<Coord<f64>> {
    let mut ordine: Vec<usize> = (0..coordinate.len()).collect();
    ordine.sort_by_key(|&indice| coordinate.get(indice).copied().map(chiave));
    // gruppo[i]: il primo indice d'ingresso con la stessa posizione di i.
    let mut gruppo: Vec<usize> = vec![0; coordinate.len()];
    let mut precedente: Option<((u64, u64), usize)> = None;
    for &indice in &ordine {
        let Some(&corrente) = coordinate.get(indice) else {
            continue;
        };
        let corrente = chiave(corrente);
        let primo = match precedente {
            Some((stessa, primo)) if stessa == corrente => primo,
            _ => indice,
        };
        precedente = Some((corrente, primo));
        if let Some(posto) = gruppo.get_mut(indice) {
            *posto = primo;
        }
    }

    // Rango del gruppo e bit del suo vertice, in ordine d'ingresso.
    let mut rango_del_primo: Vec<usize> = vec![usize::MAX; coordinate.len()];
    let mut siti: Vec<Coord<f64>> = Vec::new();
    let mut collineari = true;
    for (indice, (&coordinata, &primo)) in coordinate.iter().zip(&gruppo).enumerate() {
        if primo == indice {
            if collineari && siti.len() >= 2 {
                if let (Some(&a), Some(&b)) = (siti.first(), siti.get(1)) {
                    collineari =
                        <f64 as GeoNum>::Ker::orient2d(a, b, coordinata) == Orientation::Collinear;
                }
            }
            if let Some(posto) = rango_del_primo.get_mut(indice) {
                *posto = siti.len();
            }
            siti.push(coordinata);
        } else if siti.len() == 1 || !collineari {
            if let Some(sito) = rango_del_primo
                .get(primo)
                .and_then(|&rango| siti.get_mut(rango))
            {
                *sito = coordinata;
            }
        }
    }
    siti
}

/// Triangola i punti come `unconstrained_triangulation_raw` di `geo` 0.33.1,
/// con il caricamento in blocco.
///
/// # Errors
///
/// - `Inserimento`: il primo punto, in ordine d'ingresso, che
///   `spade::validate_vertex` rifiuta;
/// - `VerticiInattesi`: vertici diversi dai siti distinti dopo il
///   caricamento.
pub fn triangola(coordinate: &[Coord<f64>]) -> Result<Triangolazione, ErroreTriangolazione> {
    for &coordinata in coordinate {
        spade::validate_vertex(&coordinata).map_err(ErroreTriangolazione::Inserimento)?;
    }
    let siti = siti_distinti(coordinate);
    let vertici: Vec<Sito> = siti
        .iter()
        .enumerate()
        .map(|(rango, &coordinata)| Sito { coordinata, rango })
        .collect();
    let triangolazione = DelaunayTriangulation::<Sito>::bulk_load(vertici)
        .map_err(ErroreTriangolazione::Inserimento)?;
    // Ogni rango una e una sola volta: nessun sito perso, fuso o ripetuto.
    let mut presenti = vec![false; siti.len()];
    let mut completi = triangolazione.num_vertices() == siti.len();
    for vertice in triangolazione.vertices() {
        match presenti.get_mut(vertice.data().rango) {
            Some(segno) if !*segno => *segno = true,
            _ => completi = false,
        }
    }
    if !completi {
        return Err(ErroreTriangolazione::VerticiInattesi);
    }
    Ok(Triangolazione {
        triangolazione,
        siti,
    })
}
