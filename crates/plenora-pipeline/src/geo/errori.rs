//! Gli errori dei kernel geografici nella categoria di [`PlenoraError`].
//!
//! Stessa attribuzione di `ExtensionError::del_passo` e di
//! `From<RustBackendError>` dei kernel: `Internal` cio' che non ha concluso
//! o un'invariante saltata (nessuno ha dimostrato che l'ingresso sia
//! sbagliato), `Unsupported` la precisione insufficiente (docs/limiti.md,
//! «Precisione delle operazioni geografiche»), `ResourceLimit` un limite di
//! lavoro, d'uscita o di coppie superato (il piano e' corretto, sono i dati a
//! non entrarci: la definizione di `PlenoraError::ResourceLimit`, come le
//! tabellari; a `190c493` erano `InvalidPlan`), `InvalidPlan` i rifiuti del piano. La
//! scadenza e' sempre `Timeout`, l'annullamento `Cancelled`. Il
//! testo e' quello del kernel, che non porta valori di cella (al piu'
//! l'indice di una riga del passo).

use plenora_core::PlenoraError;
use plenora_kernels_geo::coordinate_api::{CoordinateApiError, CoordinateErrorKind};
use plenora_kernels_geo::geodesic_densify::GeodesicDensifyError;
use plenora_kernels_geo::geodesic_distance::GeodesicDistanceError;
use plenora_kernels_geo::preserve_shape_measure::ShapeMeasureError;

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
    /// Certificazione del calcolo non conclusa.
    Esecuzione,
    /// Calcolo o validazione non conclusi, invariante violata.
    Interna,
    /// Dominio non supportato o precisione insufficiente.
    Precisione,
    /// Un limite di lavoro, d'uscita o di coppie superato.
    Limite,
    /// Scadenza raggiunta, indipendentemente dal punto di controllo.
    Scaduta,
    /// Configurazione non valida.
    Configurazione,
    /// Dominio CRS non valido.
    Crs,
    /// Dati non validi.
    Dati,
    /// Calcolo annullato dal chiamante.
    Annullata,
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
        Classe::Esecuzione => PlenoraError::Execution {
            node: String::new(),
            operation: op.to_owned(),
            execution_id: String::new(),
            reason: errore.to_string(),
        },
        Classe::Interna => PlenoraError::Internal(format!("{op}: {errore}")),
        Classe::Precisione => PlenoraError::Unsupported(format!("{op}: {errore}")),
        Classe::Limite => PlenoraError::ResourceLimit(format!("{op}: {errore}")),
        Classe::Scaduta => PlenoraError::Timeout(format!("{op}: {errore}")),
        Classe::Configurazione => PlenoraError::InvalidConfiguration(format!("{op}: {errore}")),
        Classe::Crs => PlenoraError::Crs(format!("{op}: {errore}")),
        Classe::Dati => PlenoraError::DataMapping(format!("{op}: {errore}")),
        Classe::Annullata => PlenoraError::Cancelled(format!("{op}: {errore}")),
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
            Self::MargineMemoria(_) => Classe::Limite,
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
            Self::CoordinateLimit { .. } | Self::WorkLimit { .. } => Classe::Limite,
            Self::InvalidParameter { .. }
            | Self::InvalidInput(_)
            | Self::InvalidOutput(_)
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
            Self::CoordinateLimit { .. } | Self::OutputLimit { .. } | Self::WorkLimit { .. } => {
                Classe::Limite
            }
            Self::InvalidParameter { .. }
            | Self::InvalidInput(_)
            | Self::InvalidOutput(_)
            | Self::UnsupportedGeometry { .. }
            | Self::Triangulation(_)
            | Self::InvalidGeographicCoordinate
            | Self::AzimutNonDefinito(_)
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
        } else if self.e_un_limite() {
            Classe::Limite
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
            Self::ResourceLimit { .. } => Classe::Limite,
            Self::UnsupportedGeometry(_)
            | Self::InvalidGeometry(_)
            | Self::InvalidParameter { .. }
            | Self::IndexOverflow => Classe::Piano,
        }
    }
}

impl ErroreKernel for AdvancedError {
    fn classe(&self) -> Classe {
        match self {
            Self::ValidazioneNonConclusa(_) | Self::CalcoloNonConcluso(_) => Classe::Interna,
            Self::PrecisionInsufficient | Self::VerticeMalCondizionato => Classe::Precisione,
            Self::PointLimitExceeded { .. } => Classe::Limite,
            Self::InvalidPointLimit
            | Self::InsufficientPoints
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
            Self::PairLimitExceeded { .. } | Self::MargineMemoria(_) => Classe::Limite,
            Self::IndexOverflow
            | Self::InvalidPairLimit
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
            Self::WorkLimitExceeded { .. }
            | Self::ResultLimitExceeded { .. }
            | Self::MargineMemoria(_) => Classe::Limite,
            Self::InvalidWorkLimit
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

// Tabella unica GOP-028: match esaustivi, nessuna variante futura ignorata.
// Ogni famiglia dichiara qui le interruzioni e le altre categorie.
macro_rules! mappa_interruzioni {
    ($($tipo:ident { $($pattern:pat => $classe:ident),* $(,)? }),* $(,)?) => {
        $(impl ErroreKernel for $tipo {
            fn classe(&self) -> Classe {
                match self {
                    Self::DeadlineExceeded => Classe::Scaduta,
                    Self::Cancelled => Classe::Annullata,
                    $($pattern => Classe::$classe),*
                }
            }
        })*

        #[cfg(test)]
        #[test]
        #[deny(clippy::disallowed_methods)]
        fn tutte_le_varianti_di_interruzione_kernel() {
            $(for (errore, categoria, codice) in [
                ($tipo::DeadlineExceeded, plenora_core::ErrorCategory::Timeout,
                 plenora_core::error::CODE_DEADLINE_EXCEEDED),
                ($tipo::Cancelled, plenora_core::ErrorCategory::Cancelled,
                 plenora_core::error::CODE_CANCELLED),
            ] {
                let errore = del_kernel("op", &errore);
                assert_eq!(errore.category(), categoria);
                assert_eq!(errore.public_projection().code(), Some(codice));
            })*
        }
    };
}

mappa_interruzioni! {
    PredicateError {
        Self::WorkLimit | Self::ReservationLimit | Self::AllocationFailed => Limite,
        Self::ValidazioneNonConclusa(_) | Self::CalcoloNonConcluso(_) => Interna,
        Self::InvalidRoundedTopology | Self::UnsupportedGeometry => Precisione,
        Self::AccuracyUnproven => Esecuzione,
        Self::NonFiniteCoordinate { .. } | Self::InvalidGeometry { .. } => Piano,
    },
    CoordinateErrorKind {
        Self::InvalidConfiguration => Configurazione,
        Self::Crs => Crs,
        Self::DataMapping => Dati,
        Self::ResourceLimit => Limite,
        Self::Unsupported => Precisione,
        Self::Execution => Esecuzione,
    },
    GeodesicDistanceError {
        Self::WorkLimit | Self::MemoryLimit | Self::Allocation => Limite,
        Self::AccuracyUnproven => Esecuzione,
        Self::GeographicRequired => Crs,
        Self::InvalidGeometry | Self::EmptyGeometry => Dati,
        Self::UnsupportedEllipsoid | Self::UnsupportedGeometry |
        Self::AmbiguousGeodesic | Self::UnsupportedSurface => Precisione,
    },
    GeodesicDensifyError {
        Self::WorkLimit | Self::PointLimit | Self::MemoryLimit | Self::Allocation => Limite,
        Self::AccuracyUnproven => Esecuzione,
        Self::GeographicRequired => Crs,
        Self::InvalidMaximum => Configurazione,
        Self::InvalidGeometry | Self::InvalidCoordinate => Dati,
        Self::UnsupportedEllipsoid | Self::UnsupportedGeometry |
        Self::AmbiguousGeodesic | Self::UnsupportedSurface => Precisione,
    },
    ShapeMeasureError {
        Self::WorkLimit => Limite,
        Self::AccuracyUnproven | Self::CertifiedDomain => Esecuzione,
        Self::InvalidTolerance => Configurazione,
        Self::CrsDomain | Self::UnsupportedEllipsoid |
        Self::UnsupportedTopology | Self::UnsupportedSurface => Precisione,
    },
}

impl ErroreKernel for CoordinateApiError {
    fn classe(&self) -> Classe {
        self.kind.classe()
    }
}

// Le famiglie che incapsulano PredicateError delegano alla stessa tabella.
impl ErroreKernel for plenora_kernels_geo::point_boolean::PointBooleanError {
    fn classe(&self) -> Classe {
        match self {
            Self::Predicate(errore) => errore.classe(),
            Self::UnsupportedOperands | Self::InvalidOutputDimension => Classe::Piano,
        }
    }
}

impl ErroreKernel for plenora_kernels_geo::line_boolean::LineBooleanError {
    fn classe(&self) -> Classe {
        match self {
            Self::Predicate(errore) => errore.classe(),
            Self::UnsupportedOperands | Self::InvalidOutputDimension => Classe::Piano,
            Self::InvalidRoundedTopology | Self::CoordinateOutOfRange => Classe::Precisione,
        }
    }
}

impl ErroreKernel for plenora_kernels_geo::polygon_boolean::PolygonBooleanError {
    fn classe(&self) -> Classe {
        match self {
            Self::Predicate(errore) => errore.classe(),
            Self::UnsupportedOperands | Self::InvalidOutputDimension => Classe::Piano,
            Self::InvalidRoundedTopology | Self::CoordinateOutOfRange => Classe::Precisione,
        }
    }
}

impl ErroreKernel for plenora_kernels_geo::polygon_completion::PolygonCompletionError {
    fn classe(&self) -> Classe {
        match self {
            Self::Predicate(errore) => errore.classe(),
            Self::UnsupportedLinework | Self::OverlappingInteriors | Self::AmbiguousProvenance => {
                Classe::Piano
            }
            Self::InvalidRoundedTopology | Self::CoordinateOutOfRange => Classe::Precisione,
        }
    }
}

impl ErroreKernel for plenora_kernels_geo::reshape::ReshapeError {
    fn classe(&self) -> Classe {
        match self {
            Self::Predicate(errore) => errore.classe(),
            Self::InvalidGeometry => Classe::Dati,
            Self::Unsupported | Self::InvalidRoundedTopology | Self::CoordinateOutOfRange => {
                Classe::Precisione
            }
        }
    }
}

impl ErroreKernel for plenora_kernels_geo::trim_extend::TrimExtendError {
    fn classe(&self) -> Classe {
        match self {
            Self::Predicate(errore) => errore.classe(),
            Self::InvalidGeometry => Classe::Dati,
            Self::Unsupported | Self::InvalidRoundedTopology | Self::CoordinateOutOfRange => {
                Classe::Precisione
            }
        }
    }
}

impl ErroreKernel for plenora_kernels_geo::offset::OffsetError {
    fn classe(&self) -> Classe {
        match self {
            Self::Predicate(errore) => errore.classe(),
            Self::InvalidConfiguration => Classe::Configurazione,
            Self::InvalidGeometry => Classe::Dati,
            Self::Unsupported | Self::InvalidRoundedTopology | Self::CoordinateOutOfRange => {
                Classe::Precisione
            }
            Self::AccuracyUnproven => Classe::Esecuzione,
        }
    }
}

#[cfg(test)]
macro_rules! verifica {
    ($errore:expr, $categoria:expr) => {
        assert_eq!(
            del_kernel(
                "op",
                &plenora_kernels_geo::point_boolean::PointBooleanError::Predicate($errore)
            )
            .category(),
            $categoria
        );
        assert_eq!(
            del_kernel(
                "op",
                &plenora_kernels_geo::line_boolean::LineBooleanError::Predicate($errore)
            )
            .category(),
            $categoria
        );
        assert_eq!(
            del_kernel(
                "op",
                &plenora_kernels_geo::polygon_boolean::PolygonBooleanError::Predicate($errore)
            )
            .category(),
            $categoria
        );
        assert_eq!(
            del_kernel(
                "op",
                &plenora_kernels_geo::polygon_completion::PolygonCompletionError::Predicate(
                    $errore
                )
            )
            .category(),
            $categoria
        );
        assert_eq!(
            del_kernel(
                "op",
                &plenora_kernels_geo::reshape::ReshapeError::Predicate($errore)
            )
            .category(),
            $categoria
        );
        assert_eq!(
            del_kernel(
                "op",
                &plenora_kernels_geo::trim_extend::TrimExtendError::Predicate($errore)
            )
            .category(),
            $categoria
        );
        assert_eq!(
            del_kernel(
                "op",
                &plenora_kernels_geo::offset::OffsetError::Predicate($errore)
            )
            .category(),
            $categoria
        );
        assert_eq!(
            del_kernel(
                "op",
                &plenora_kernels_geo::general_boolean::GeneralBooleanError::Predicate($errore)
            )
            .category(),
            $categoria
        );
    };
}

#[cfg(test)]
#[test]
#[deny(clippy::disallowed_methods)]
fn interruzioni_annidate_e_limiti_mantengono_categoria() {
    use plenora_core::ErrorCategory;
    verifica!(PredicateError::DeadlineExceeded, ErrorCategory::Timeout);
    verifica!(PredicateError::Cancelled, ErrorCategory::Cancelled);
    verifica!(PredicateError::WorkLimit, ErrorCategory::ResourceLimit);
    verifica!(
        PredicateError::ReservationLimit,
        ErrorCategory::ResourceLimit
    );
    verifica!(
        PredicateError::AllocationFailed,
        ErrorCategory::ResourceLimit
    );
    for (kind, categoria) in [
        (
            CoordinateErrorKind::DeadlineExceeded,
            ErrorCategory::Timeout,
        ),
        (CoordinateErrorKind::Cancelled, ErrorCategory::Cancelled),
        (
            CoordinateErrorKind::ResourceLimit,
            ErrorCategory::ResourceLimit,
        ),
    ] {
        assert_eq!(
            del_kernel(
                "op",
                &CoordinateApiError {
                    kind,
                    index: Some(0)
                }
            )
            .category(),
            categoria
        );
    }
}

#[deny(clippy::disallowed_methods)]
#[cfg(test)]
mod tests {
    use plenora_core::ErrorCategory;

    use super::*;

    /// I rifiuti di risorse e l'annullamento DE-9IM mantengono la categoria.
    #[test]
    fn de9im_resource_and_cancellation_categories() {
        for error in [
            PredicateError::WorkLimit,
            PredicateError::ReservationLimit,
            PredicateError::AllocationFailed,
        ] {
            assert_eq!(
                del_kernel("op", &error).category(),
                ErrorCategory::ResourceLimit
            );
        }
        assert_eq!(
            del_kernel("op", &PredicateError::Cancelled).category(),
            ErrorCategory::Cancelled
        );
    }

    #[test]
    #[deny(clippy::disallowed_methods)]
    fn unsupported_and_accuracy_categories() {
        assert_eq!(
            del_kernel("op", &PredicateError::UnsupportedGeometry).category(),
            ErrorCategory::Unsupported
        );
        assert_eq!(
            del_kernel("op", &PredicateError::AccuracyUnproven).category(),
            ErrorCategory::Execution
        );
    }

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
        // Ogni limite superato e' `ResourceLimit`, anche annidato: il piano
        // e' corretto, sono i dati a non entrarci.
        let limiti = [
            del_kernel("op", &GeodesicDistanceError::WorkLimit),
            del_kernel("op", &GeodesicDistanceError::MemoryLimit),
            del_kernel("op", &GeodesicDistanceError::Allocation),
            del_kernel("op", &GeodesicDensifyError::WorkLimit),
            del_kernel("op", &GeodesicDensifyError::PointLimit),
            del_kernel("op", &GeodesicDensifyError::MemoryLimit),
            del_kernel("op", &GeodesicDensifyError::Allocation),
            del_kernel("op", &ShapeMeasureError::WorkLimit),
            del_kernel("op", &AnalysisError::WorkLimitExceeded { limit: 1 }),
            del_kernel("op", &AnalysisError::ResultLimitExceeded { limit: 1 }),
            del_kernel(
                "op",
                &AnalysisError::SpatialJoin(SpatialJoinError::PairLimitExceeded { limit: 1 }),
            ),
            del_kernel(
                "op",
                &TopologyError::ResourceLimit {
                    name: "candidate_pairs",
                    actual: 2,
                    limit: 1,
                },
            ),
            del_kernel("op", &ExtensionError::IssueLimit { limit: 1 }),
            del_kernel(
                "op",
                &ExtendedError::WorkLimit {
                    actual: 2,
                    limit: 1,
                },
            ),
            del_kernel(
                "op",
                &ExtendedAlgorithmError::OutputLimit {
                    actual: 2,
                    limit: 1,
                },
            ),
            del_kernel(
                "op",
                &AdvancedError::PointLimitExceeded {
                    actual: 2,
                    limit: 1,
                },
            ),
        ];
        for errore in limiti {
            assert_eq!(errore.category(), ErrorCategory::ResourceLimit, "{errore}");
        }
        // Le celle della griglia dipendono solo dalla config: piano.
        let celle = del_kernel(
            "op",
            &ExtensionError::CellLimit {
                actual: 2,
                limit: 1,
            },
        );
        assert_eq!(celle.category(), ErrorCategory::InvalidPlan, "{celle}");
    }
}
