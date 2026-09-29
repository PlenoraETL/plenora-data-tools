//! Gli errori dei kernel geografici nella categoria di [`PlenoraError`].
//!
//! Stessa attribuzione di `ExtensionError::del_passo` e di
//! `From<RustBackendError>` dei kernel, e del passo geo dell'executor a
//! `190c493` (`errore_di_coppia`, `esito_kernel`): `Internal` cio' che non ha
//! concluso o un'invariante saltata (nessuno ha dimostrato che l'ingresso
//! sia sbagliato), `Unsupported` la precisione insufficiente (README,
//! «Precisione delle operazioni geografiche»), `InvalidPlan` il resto,
//! limiti di lavoro compresi. Il testo e' quello del kernel, che non porta
//! valori di cella (al piu' l'indice di una riga del passo).

use plenora_core::PlenoraError;
use plenora_kernels_geo::advanced::AdvancedError;
use plenora_kernels_geo::analysis::AnalysisError;
use plenora_kernels_geo::cluster::ClusterError;
use plenora_kernels_geo::construction::ConstructionError;
use plenora_kernels_geo::extended::ExtendedError;
use plenora_kernels_geo::extended_algorithms::ExtendedAlgorithmError;
use plenora_kernels_geo::extensions::ExtensionError;
use plenora_kernels_geo::operations::OperationError;
use plenora_kernels_geo::predicates::PredicateError;
use plenora_kernels_geo::spatial_join::SpatialJoinError;
use plenora_kernels_geo::topology::TopologyError;

/// Categoria di un errore di kernel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Classe {
    /// Calcolo o validazione non conclusi, invariante violata.
    Interna,
    /// Spostamento oltre la precisione dichiarata.
    Precisione,
    /// Ingresso o parametri rifiutati dal kernel.
    Piano,
}

/// Un errore di kernel geo con la sua categoria.
pub trait ErroreKernel: std::fmt::Display {
    /// La categoria dell'errore.
    fn classe(&self) -> Classe;
}

/// L'errore nella categoria giusta, con il nome dell'operazione.
pub fn del_kernel(op: &str, errore: &impl ErroreKernel) -> PlenoraError {
    match errore.classe() {
        Classe::Interna => PlenoraError::Internal(format!("{op}: {errore}")),
        Classe::Precisione => PlenoraError::Unsupported(format!("{op}: {errore}")),
        Classe::Piano => PlenoraError::InvalidPlan(format!("{op}: {errore}")),
    }
}

impl ErroreKernel for OperationError {
    fn classe(&self) -> Classe {
        match self {
            Self::Internal(_) | Self::ValidazioneNonConclusa(_) | Self::CalcoloNonConcluso(_) => {
                Classe::Interna
            }
            Self::PrecisionInsufficient => Classe::Precisione,
            Self::InvalidParameter { .. }
            | Self::InvalidOutput(_)
            | Self::InvalidInput(_)
            | Self::WktSerialization(_) => Classe::Piano,
        }
    }
}

impl ErroreKernel for ExtendedError {
    fn classe(&self) -> Classe {
        match self {
            Self::ValidazioneNonConclusa(_) | Self::CalcoloNonConcluso(_) => Classe::Interna,
            Self::InvalidParameter { .. }
            | Self::InvalidInput(_)
            | Self::InvalidOutput(_)
            | Self::CoordinateLimit { .. }
            | Self::WorkLimit { .. }
            | Self::InvalidGeographicCoordinate
            | Self::IndexOverflow => Classe::Piano,
        }
    }
}

impl ErroreKernel for ExtendedAlgorithmError {
    fn classe(&self) -> Classe {
        match self {
            Self::Internal(_) | Self::ValidazioneNonConclusa(_) | Self::CalcoloNonConcluso(_) => {
                Classe::Interna
            }
            Self::InvalidParameter { .. }
            | Self::InvalidInput(_)
            | Self::InvalidOutput(_)
            | Self::UnsupportedGeometry { .. }
            | Self::CoordinateLimit { .. }
            | Self::OutputLimit { .. }
            | Self::WorkLimit { .. }
            | Self::Triangulation(_)
            | Self::InvalidGeographicCoordinate
            | Self::IndexOverflow => Classe::Piano,
        }
    }
}

impl ErroreKernel for ExtensionError {
    fn classe(&self) -> Classe {
        if self.e_interna() {
            Classe::Interna
        } else if matches!(self, Self::PrecisionInsufficient) {
            Classe::Precisione
        } else {
            Classe::Piano
        }
    }
}

impl ErroreKernel for TopologyError {
    fn classe(&self) -> Classe {
        match self {
            Self::ValidazioneNonConclusa(_) | Self::CalcoloNonConcluso(_) => Classe::Interna,
            Self::PrecisionInsufficient => Classe::Precisione,
            Self::UnsupportedGeometry(_)
            | Self::InvalidGeometry(_)
            | Self::InvalidParameter { .. }
            | Self::ResourceLimit { .. }
            | Self::IndexOverflow => Classe::Piano,
        }
    }
}

impl ErroreKernel for AdvancedError {
    fn classe(&self) -> Classe {
        match self {
            Self::ValidazioneNonConclusa(_) | Self::CalcoloNonConcluso(_) => Classe::Interna,
            Self::PrecisionInsufficient | Self::VerticeMalCondizionato => Classe::Precisione,
            Self::InvalidPointLimit
            | Self::InsufficientPoints
            | Self::PointLimitExceeded { .. }
            | Self::ExpectedPoint { .. }
            | Self::InvalidPoint { .. }
            | Self::Voronoi(_)
            | Self::UnmatchedPoint(_)
            | Self::InvalidOutput(_) => Classe::Piano,
        }
    }
}

impl ErroreKernel for SpatialJoinError {
    fn classe(&self) -> Classe {
        match self {
            Self::Internal(_) | Self::ValidazioneNonConclusa(_) | Self::CalcoloNonConcluso(_) => {
                Classe::Interna
            }
            Self::IndexOverflow
            | Self::InvalidPairLimit
            | Self::PairLimitExceeded { .. }
            | Self::NonFiniteCoordinate { .. }
            | Self::InvalidGeometry { .. } => Classe::Piano,
        }
    }
}

impl ErroreKernel for AnalysisError {
    fn classe(&self) -> Classe {
        match self {
            Self::SpatialJoin(interno) => interno.classe(),
            Self::ValidazioneNonConclusa(_) | Self::CalcoloNonConcluso(_) => Classe::Interna,
            Self::InvalidWorkLimit
            | Self::WorkLimitExceeded { .. }
            | Self::ResultLimitExceeded { .. }
            | Self::InvalidMaximumDistance
            | Self::IndexOverflow
            | Self::InvalidGeometry { .. } => Classe::Piano,
        }
    }
}

impl ErroreKernel for ClusterError {
    fn classe(&self) -> Classe {
        match self {
            Self::InternalInvariant(_)
            | Self::ValidazioneNonConclusa(_)
            | Self::CalcoloNonConcluso(_) => Classe::Interna,
            Self::InvalidParameter { .. }
            | Self::UnsupportedGeometry { .. }
            | Self::InvalidGeometry { .. }
            | Self::NonFiniteCoordinate { .. }
            | Self::IndexOverflow => Classe::Piano,
        }
    }
}

impl ErroreKernel for ConstructionError {
    fn classe(&self) -> Classe {
        match self {
            Self::ValidazioneNonConclusa(_) => Classe::Interna,
            Self::NonFiniteCoordinate { .. }
            | Self::ExpectedPoint { .. }
            | Self::InvalidOutput(_)
            | Self::InvalidWkt(_)
            | Self::UnsupportedWktDimension => Classe::Piano,
        }
    }
}

impl ErroreKernel for PredicateError {
    fn classe(&self) -> Classe {
        match self {
            Self::ValidazioneNonConclusa(_) | Self::CalcoloNonConcluso(_) => Classe::Interna,
            Self::NonFiniteCoordinate { .. } | Self::InvalidGeometry { .. } => Classe::Piano,
        }
    }
}

#[cfg(test)]
mod tests {
    use plenora_core::ErrorCategory;

    use super::*;

    /// Cio' che non conclude e' interno, anche annidato; la precisione e'
    /// `Unsupported`; il resto e' del piano.
    #[test]
    fn le_categorie_seguono_la_regola_dei_kernel() {
        let interni = [
            del_kernel("op", &OperationError::CalcoloNonConcluso("forma")),
            del_kernel(
                "op",
                &AnalysisError::SpatialJoin(SpatialJoinError::CalcoloNonConcluso("forma")),
            ),
            del_kernel("op", &TopologyError::ValidazioneNonConclusa("forma")),
            del_kernel("op", &ExtensionError::Internal("forma")),
        ];
        for errore in interni {
            assert_eq!(errore.category(), ErrorCategory::Internal, "{errore}");
        }
        let precisione = del_kernel("op", &TopologyError::PrecisionInsufficient);
        assert!(matches!(precisione, PlenoraError::Unsupported(_)));
        let piano = del_kernel("op", &AnalysisError::InvalidWorkLimit);
        assert_eq!(piano.category(), ErrorCategory::InvalidPlan, "{piano}");
    }
}
