//! Politiche condivise dai codec di celle testuali (GOP-024).

/// Politica obbligatoria per il punto decodificato.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellRepresentative {
    /// Punto medio nelle coordinate della griglia sorgente.
    Center,
    /// Angolo minimo nelle coordinate della griglia sorgente.
    Southwest,
}
