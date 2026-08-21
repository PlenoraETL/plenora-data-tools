//! Catalogo unificato delle operazioni (architettura.md, piano-v5.md#identita-e-fingerprint).
//!
//! Ogni operazione dichiara il proprio contratto in modo machine-readable;
//! il fingerprint del catalogo deriva dalle versioni esplicite per-componente,
//! mai da hash del binario.

/// Famiglia di appartenenza (namespace dell'`id`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Family {
    Table,
    Geo,
}

/// Provenienza dell'operazione.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Origin {
    /// Compatibile con il tool Python Manipola di riferimento.
    ManipolaCompat,
    /// Estensione nativa.
    Extension,
}

/// Arietà del nodo nel DAG.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Arity {
    Unary,
    /// Binaria ordinata (left, right).
    BinaryOrdered,
    /// N-aria (es. concat di 3+ input).
    NAry,
}

/// Classe di esecuzione, usata dal planner per segmenti e materializzazioni.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExecutionClass {
    /// 1:1 sulle righe, eseguibile batch-per-batch.
    Streaming,
    /// Richiede l'intero input.
    Blocking,
    /// Blocking con due input.
    BinaryBlocking,
}

/// Comportamento alla cancellazione (errori-e-limiti.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CancellationBehavior {
    Cooperative,
    BoundaryOnly,
    NonInterruptible,
}

/// Fondibilita' di un'operazione geo nella fusione dei segmenti (architettura.md#geometrie
/// D12.2).
///
/// Capability dichiarativa FISICA, stesso principio di
/// [`CancellationBehavior`]. Resta FUORI da `descriptor_canonical` e quindi
/// dal `catalog_fingerprint` (decisione deliberata: il fingerprint guarda la
/// compatibilita' semantica dei piani, la fondibilita' e' fisica — architettura.md#planner-ed-executor).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GeoFusion {
    /// Non fondibile: esecuzione nodo-per-nodo (default). Tutte le op
    /// tabellari e le geo non revistate per il perimetro (M1+M3).
    NotFusible,
    /// Trasformazione 1:1 sul posto: fondibile in un gruppo di nodi unari
    /// consecutivi a parita' di colonna geometria e ruolo (le 14 op di M1
    /// piu' `reproject`/`make_valid` di M3).
    TransformInPlace,
    /// Misura terminale: consuma la geometria producendo un valore non
    /// geometrico (`area`, `length`, `perimeter`, `vertex_count`, `to_wkt`
    /// — perimetro M2); chiude un eventuale gruppo fuso a monte.
    TerminalMeasure,
}

impl GeoFusion {
    /// Nome stabile `snake_case` della variante: unica fonte per il JSON
    /// delle capability (`capabilities.rs`) e per lo snapshot di catalogo.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotFusible => "not_fusible",
            Self::TransformInPlace => "transform_in_place",
            Self::TerminalMeasure => "terminal_measure",
        }
    }
}

/// Forma del risultato rispetto alle righe di input (da geo-tools-arrow).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResultShape {
    OneToOne,
    OneToMany,
    ManyToOne,
    Collective,
    WholeToMany,
    FromCoords,
    Diagnostic,
}

/// Osservabilita' dell'indice sorgente attraverso un nodo.
///
/// `Preserved` significa che ogni configurazione valida del descrittore
/// mantiene cardinalita' e ordine delle righe. Ogni altra operazione e'
/// `Unavailable`: senza un sidecar di lineage il runtime non puo' ricostruire
/// un indice sorgente originale e deve rifiutare i consumer row-diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourceRowProvenance {
    Preserved,
    Unavailable,
}

/// Requisito CRS (solo operazioni geo).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CrsRequirement {
    Known,
    Projected,
    Geographic,
    SameProjected,
    Reprojection,
}

/// Politica di determinismo per operazioni con ordine non definito (architettura.md#determinismo).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeterminismPolicy {
    /// L'output ha ordine definito dalla semantica dell'operazione.
    DefinedOrder,
    /// Ordine di arrivo dei batch (stabile, deterministico).
    InputOrder,
    /// Ordinamento stabile su chiave dichiarata.
    StableKeyOrder,
    /// Ordinamento canonico dei valori (set operations).
    CanonicalOrder,
}

/// Vincolo di espansione vincolante per un'operazione binaria (errori-e-limiti.md).
///
/// Per le operazioni binarie nessuna base singola e' adeguata: il runtime
/// calcola tutte le metriche di [`JoinExpansion`] e il catalogo dichiara
/// quale e' vincolante. La soglia di confronto e' `max_expansion_factor`
/// dei limiti effettivi, tranne per [`ExpansionConstraint::Custom`] che la
/// sovrascrive per la singola operazione.
///
/// `PartialEq`/`Eq`/`Hash` sono implementati a mano: il fattore di `Custom`
/// e' confrontato e hashato per bit (`f64::to_bits`), mai per valore —
/// nessuna ambiguita' su NaN/-0.0 nel fingerprint del catalogo (piano-v5.md#identita-e-fingerprint).
#[derive(Debug, Clone, Copy)]
pub enum ExpansionConstraint {
    /// `output / (left + right)`: default, retrocompatibile con la base
    /// fissa left+right della prima implementazione.
    SumRelative,
    /// `output / left`: operazioni lookup-style (output <= left).
    LeftRelative,
    /// `output / right`.
    RightRelative,
    /// `max(output / left, output / right)`: join molti-a-molti.
    MaxRelative,
    /// Stima a priori da statistiche (errori-e-limiti.md/architettura.md#planner-ed-executor), per operazioni la cui
    /// semantica di output non e' caratterizzabile con una base fissa.
    ///
    /// Semantica scelta: la **metrica** vincolante resta
    /// `output_over_sum_inputs` (la base piu' conservativa e stabile), ma la
    /// **soglia** effettiva e' il fattore dichiarato, che sovrascrive
    /// `max_expansion_factor` per la sola operazione — vedi
    /// [`ExpansionConstraint::binding_threshold`]. Il fattore deve essere
    /// finito e positivo (costante di catalogo, verificata in review; non
    /// e' un input esterno). Nessuna op v1 lo usa: e' riservato a op
    /// future guidate da stime.
    Custom(f64),
}

impl PartialEq for ExpansionConstraint {
    fn eq(&self, other: &Self) -> bool {
        match (*self, *other) {
            (Self::SumRelative, Self::SumRelative)
            | (Self::LeftRelative, Self::LeftRelative)
            | (Self::RightRelative, Self::RightRelative)
            | (Self::MaxRelative, Self::MaxRelative) => true,
            (Self::Custom(a), Self::Custom(b)) => a.to_bits() == b.to_bits(),
            _ => false,
        }
    }
}

impl Eq for ExpansionConstraint {}

impl std::hash::Hash for ExpansionConstraint {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            Self::SumRelative => state.write_u8(0),
            Self::LeftRelative => state.write_u8(1),
            Self::RightRelative => state.write_u8(2),
            Self::MaxRelative => state.write_u8(3),
            Self::Custom(factor) => {
                state.write_u8(4);
                state.write_u64(factor.to_bits());
            }
        }
    }
}

impl ExpansionConstraint {
    /// Soglia effettiva del fattore di espansione per questo vincolo (errori-e-limiti.md):
    /// il fattore custom per [`ExpansionConstraint::Custom`] (override di
    /// `max_expansion_factor` per la singola operazione), altrimenti il
    /// `max_expansion_factor` dei limiti effettivi passato dal chiamante.
    #[must_use]
    pub const fn binding_threshold(self, max_expansion_factor: f64) -> f64 {
        match self {
            Self::Custom(factor) => factor,
            _ => max_expansion_factor,
        }
    }

    /// `true` se l'espansione osservata supera la soglia di questo vincolo.
    ///
    /// E' la DECISIONE del limite, e non passa per le metriche `f64` di
    /// [`JoinExpansion`]: i conteggi restano interi e il fattore viene
    /// decomposto ([`crate::limits::expansion_exceeded`]). Decidere sul
    /// rapporto in doppia precisione arrotonda i conteggi, e con
    /// `left = right = 2^53` e `output = 2^53+1` il rapporto reale — maggiore
    /// di 1 — diventava esattamente `1.0`: il limite non scattava. Le
    /// metriche restano osservabili, ma non decidono.
    ///
    /// Base per vincolo: la somma degli input (`SumRelative`, `Custom`), il
    /// solo lato sinistro o destro, e per `MaxRelative` il massimo delle tre
    /// metriche — che supera la soglia **se e solo se** almeno una la supera,
    /// e la metrica sulla somma e' sempre dominata dalle altre due.
    ///
    /// Denominatore nullo: coerente con [`JoinExpansion::compute`] — con
    /// output non nullo la metrica e' infinita, quindi il vincolo scatta; con
    /// output nullo vale zero e non scatta.
    #[must_use]
    pub fn exceeded(
        self,
        output_rows: u64,
        left_rows: u64,
        right_rows: u64,
        max_expansion_factor: f64,
    ) -> bool {
        let threshold = self.binding_threshold(max_expansion_factor);
        let left = u128::from(left_rows);
        let right = u128::from(right_rows);
        let exceeded =
            |base: u128| crate::limits::expansion_exceeded_wide(output_rows, base, threshold);
        match self {
            // La somma in `u128` non satura: due conteggi a 64 bit ci stanno
            // sempre, e saturare abbasserebbe il denominatore proprio dove
            // deciderebbe il limite.
            Self::SumRelative | Self::Custom(_) => exceeded(left + right),
            Self::LeftRelative => exceeded(left),
            Self::RightRelative => exceeded(right),
            Self::MaxRelative => exceeded(left) || exceeded(right),
        }
    }
}

/// Metriche di espansione di un'operazione binaria (errori-e-limiti.md).
///
/// Il runtime le calcola tutte e tre; il vincolo dichiarato in catalogo
/// ([`ExpansionConstraint`]) seleziona quella vincolante.
///
/// Sono metriche **osservabili**, non la base della decisione: i rapporti
/// sono `f64` e sopra 2^53 righe arrotondano i conteggi. Il limite si decide
/// in aritmetica esatta con [`ExpansionConstraint::exceeded`]; questi valori
/// servono a raccontare l'esito, non a stabilirlo.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JoinExpansion {
    /// Righe output / (righe left + righe right).
    pub output_over_sum_inputs: f64,
    /// Righe output / righe left.
    pub output_over_left: f64,
    /// Righe output / righe right.
    pub output_over_right: f64,
}

impl JoinExpansion {
    /// Calcola le tre metriche dalle righe output/left/right.
    ///
    /// Denominatore nullo: la metrica e' infinita se l'output e' non nullo
    /// (espansione da input vuoto), zero altrimenti.
    #[must_use]
    #[allow(clippy::cast_precision_loss)] // Metriche f64 per contratto (errori-e-limiti.md); sotto 2^53 righe il confronto e' esatto.
    pub fn compute(output_rows: u64, left_rows: u64, right_rows: u64) -> Self {
        fn ratio(numerator: u64, denominator: u64) -> f64 {
            if denominator == 0 {
                if numerator == 0 {
                    0.0
                } else {
                    f64::INFINITY
                }
            } else {
                numerator as f64 / denominator as f64
            }
        }
        Self {
            // Somma SATURANTE: e' un denominatore di una metrica che decide
            // un limite, e avvolgere lo abbasserebbe — cioe' alzerebbe il
            // rapporto e potrebbe far scattare il vincolo a sproposito (o in
            // debug far abortire con `overflow-checks`).
            output_over_sum_inputs: ratio(output_rows, left_rows.saturating_add(right_rows)),
            output_over_left: ratio(output_rows, left_rows),
            output_over_right: ratio(output_rows, right_rows),
        }
    }

    /// Restituisce la metrica vincolante per il vincolo dichiarato in
    /// catalogo. `MaxRelative` e' il massimo delle tre (la metrica sulla
    /// somma e' sempre dominata dalle altre due, quindi includerla non
    /// cambia il risultato). `Custom` usa la metrica sulla somma degli
    /// input: la specificita' del vincolo e' nella soglia
    /// ([`ExpansionConstraint::binding_threshold`]), non nella base.
    #[must_use]
    pub const fn binding_metric(&self, constraint: ExpansionConstraint) -> f64 {
        match constraint {
            ExpansionConstraint::SumRelative | ExpansionConstraint::Custom(_) => {
                self.output_over_sum_inputs
            }
            ExpansionConstraint::LeftRelative => self.output_over_left,
            ExpansionConstraint::RightRelative => self.output_over_right,
            ExpansionConstraint::MaxRelative => self
                .output_over_sum_inputs
                .max(self.output_over_left)
                .max(self.output_over_right),
        }
    }
}

/// Livello di maturità (pipeline di promozione).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Maturity {
    Planned,
    /// Kernel implementato ma in attesa del backend richiesto
    /// (da `backend_pending` di geo-tools-arrow, op feature-gated geos/proj).
    BackendPending,
    KernelValidated,
    PublicProtocol,
}

/// Contratto machine-readable di un'operazione (piano-v5.md#identita-e-fingerprint).
///
/// Le versioni per-componente alimentano il `catalog_fingerprint`:
/// ogni modifica incompatibile incrementa la versione pertinente.
#[derive(Debug, Clone)]
pub struct OperationDescriptor {
    /// Id namespaced: `table.*` / `geo.*`.
    pub id: &'static str,
    pub family: Family,
    pub origin: Origin,
    pub arity: Arity,
    pub execution_class: ExecutionClass,
    pub cancellation_behavior: CancellationBehavior,
    /// Fondibilita' nella fusione dei segmenti geo (architettura.md#geometrie D12.2):
    /// capability fisica, NON entra in `descriptor_canonical` ne' nel
    /// `catalog_fingerprint`. `NotFusible` per tutte le op tabellari.
    pub geo_fusion: GeoFusion,
    pub result_shape: Option<ResultShape>,
    pub crs_requirement: Option<CrsRequirement>,
    /// Backend/feature richiesti (es. `geos`, `proj`).
    pub required_capabilities: &'static [&'static str],
    pub determinism: DeterminismPolicy,
    /// Vincolo di espansione vincolante per le operazioni binarie (errori-e-limiti.md);
    /// irrilevante per unarie/N-arie, che restano sulla base input
    /// (default `SumRelative`, retrocompatibile).
    pub expansion_constraint: ExpansionConstraint,
    /// Esenzione da `max_expansion_factor` dichiarata in catalogo (errori-e-limiti.md):
    /// le op che espandono per contratto ogni elemento di input in molti
    /// output (`WholeToMany`: generative/diagnostiche) non sono soggette al
    /// fattore; restano vincolate da `max_rows_per_edge` e dagli altri
    /// limiti di righe.
    pub expansion_factor_exempt: bool,
    pub maturity: Maturity,
    // Versioni esplicite per-componente (piano-v5.md#identita-e-fingerprint): disciplina di incremento in CI.
    pub semantic_version: u32,
    pub config_schema_version: u32,
    pub contract_analysis_version: u32,
    pub kernel_version: u32,
}

impl OperationDescriptor {
    /// Dichiara se la posizione sorgente resta osservabile per tutte le
    /// configurazioni valide dell'operazione.
    ///
    /// La classificazione e' conservativa: una sola modalita' capace di
    /// selezionare, riordinare, espandere o aggregare rende il descrittore
    /// `Unavailable`. Questo evita provenance inventata nei sibling paths.
    #[must_use]
    pub fn source_row_provenance(&self) -> SourceRowProvenance {
        match self.family {
            Family::Table => {
                if matches!(
                    self.id,
                    "table.bin"
                        | "table.add_row_number"
                        | "table.assert_unique"
                        | "table.assert_foreign_key"
                ) || (matches!(self.arity, Arity::Unary)
                    && !matches!(
                        self.id,
                        "table.aggregate"
                            | "table.dedup_advanced"
                            | "table.distinct"
                            | "table.filter"
                            | "table.limit"
                            | "table.melt"
                            | "table.pivot"
                            | "table.sample"
                            | "table.sort"
                            | "table.statistics"
                            | "table.top_n"
                            | "table.transpose"
                            | "table.validate_rules"
                            | "table.window_function"
                            | "table.rolling_window"
                            | "table.explode"
                            | "table.unnest"
                    ))
                {
                    SourceRowProvenance::Preserved
                } else {
                    SourceRowProvenance::Unavailable
                }
            }
            Family::Geo => {
                if matches!(self.arity, Arity::Unary)
                    && matches!(
                        self.result_shape,
                        Some(ResultShape::OneToOne | ResultShape::FromCoords)
                    )
                {
                    SourceRowProvenance::Preserved
                } else {
                    SourceRowProvenance::Unavailable
                }
            }
        }
    }

    /// Dichiara se l'operazione, nella configurazione data, puo' rifiutare
    /// righe con diagnostica row-scoped (`plenora-row-diagnostics-v1`).
    ///
    /// Autorita' UNICA catalog-level (accanto a [`Self::source_row_provenance`]
    /// e `required_capabilities`): la usano il gate provenance del planner, il
    /// `prepare` (flag per kernel del machinery di segmento) e il gate dei
    /// piani legacy della CLI. Nessuna lista duplicata altrove: chiunque
    /// aggiunga un percorso di rifiuto row-scoped la dichiara QUI.
    ///
    /// La proprieta' e' config-sensitive per costruzione:
    /// - `table.type_cast`: solo i target con conversione fallibile
    ///   row-scoped (`int`, `float`, `bool`, `uint64`, `date`, `datetime`,
    ///   `date32`, `timestamp_millis`, `decimal128`) e solo con `errors`
    ///   assente/`coerce`/`raise`; gli altri target (es. `str`) sono totali;
    /// - `table.md5_hash`/`table.sha256_hash`: solo con `null_policy=error`
    ///   (P1-3: `empty`/`literal` hanno semantica storica dichiarata, nessun
    ///   rifiuto);
    /// - `table.hmac_sha256`: MAI (P2) — le `null_policy` legacy producono
    ///   output dichiarato, nessun rifiuto row-scoped possibile.
    ///
    /// Le op geo elencate sono quelle dispatchate nel DAG con raccolta
    /// row-scoped (ledger `diag-transport`/`diag-wkt`): le op solo-trasporto
    /// (es. `geo.geodesic_*`) non attraversano nessuno dei tre gate e restano
    /// coperte dal contratto del trasporto.
    #[must_use]
    pub fn emits_row_diagnostics(&self, config: &serde_json::Value) -> bool {
        match self.family {
            Family::Table => match self.id {
                "table.flatten_json"
                | "table.date_extract"
                | "table.date_format"
                | "table.date_add"
                | "table.date_diff"
                | "table.timezone_convert"
                | "table.formula"
                | "table.expression"
                | "table.assert_not_null"
                | "table.assert_unique"
                | "table.assert_range"
                | "table.assert_regex"
                | "table.assert_foreign_key" => true,
                "table.type_cast" => {
                    matches!(
                        config
                            .get("target_type")
                            .and_then(serde_json::Value::as_str),
                        Some(
                            "int"
                                | "float"
                                | "bool"
                                | "uint64"
                                | "date"
                                | "datetime"
                                | "date32"
                                | "timestamp_millis"
                                | "decimal128"
                        )
                    ) && matches!(
                        config.get("errors").and_then(serde_json::Value::as_str),
                        None | Some("coerce" | "raise")
                    )
                }
                "table.md5_hash" | "table.sha256_hash" => matches!(
                    config
                        .get("null_policy")
                        .and_then(serde_json::Value::as_str),
                    Some("error")
                ),
                _ => false,
            },
            Family::Geo => matches!(
                self.id,
                "geo.from_wkt"
                    | "geo.centroid"
                    | "geo.convex_hull"
                    | "geo.envelope"
                    | "geo.buffer"
                    | "geo.simplify"
                    | "geo.boundary"
                    | "geo.point_on_surface"
                    | "geo.make_valid"
                    | "geo.reproject"
                    | "geo.affine_transform"
                    | "geo.translate"
                    | "geo.scale"
                    | "geo.rotate"
                    | "geo.concave_hull"
                    | "geo.densify"
                    | "geo.snap_to_grid"
                    | "geo.line_substring"
                    | "geo.line_interpolate_point"
                    | "geo.area"
                    | "geo.length"
                    | "geo.perimeter"
                    | "geo.vertex_count"
                    | "geo.to_wkt"
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// Catalogo unificato delle 146 operazioni (Fase 1, decisione D17/D20; +4
// estensioni geo v1.1: from_wkt, geometry_accessors, collect,
// line_locate_point; +4 estensioni table v1.1: select_columns, limit, top_n,
// stable_fingerprint; +3 estensioni geo v1.2: generate_grid, subdivide,
// snap; +4 estensioni table v1.2: align_schema, concat_by_name,
// hmac_sha256, validate_rules; +3 estensioni geo v1.3: coverage_validate,
// shared_paths, cluster_dbscan; +1 estensione table v1.3: fuzzy_join).
//
// Sorgenti dei metadati:
// - `plenora-nogeo-tools/src/catalog.rs`  (62 op tabellari -> `table.*`);
// - `plenora-geo-tools-arrow/src/catalog.rs` (65 op geografiche -> `geo.*`).
//
// Mapping documentato in `docs/catalog-diff.md`:
// - tabellari: id storico invariato sotto il namespace `table.`;
// - geografiche `geo_*`: il prefisso storico diventa il namespace
//   (`geo_buffer` -> `geo.buffer`);
// - predicati DE-9IM: `predicate_*` -> `geo.predicate_*`;
// - estensioni geo nude: `<id>` -> `geo.<id>`.
//
// Scelte conservative dove il sorgente non dichiara il metadato:
// - `arity` geo: i descrittori sorgente non contano gli input; `BinaryOrdered`
//   solo per le op intrinsecamente binarie (join/overlay/filtri spaziali su
//   due input); predicati, distanze a due colonne e `split` restano `Unary`
//   (due colonne dello stesso input);
// - `execution_class` geo: derivato da `result_shape` (1:1 -> Streaming,
//   aggregazioni/tessellazioni -> Blocking, overlay/join -> BinaryBlocking);
// - `cancellation_behavior`: Cooperative per kernel puri streaming,
//   BoundaryOnly per blocking grandi, NonInterruptible per le op con
//   capability esterna (`geos`/`proj`, chiamate monolitiche, errori-e-limiti.md);
// - `result_shape`: `BinaryLineage` del sorgente geo non ha variante
//   equivalente: mappato su `OneToMany` (un left puo' produrre piu' righe);
// - `determinism`: `DefinedOrder` di default; `CanonicalOrder` per le set
//   operation tabellari e le aggregazioni senza ordine; `InputOrder` per
//   `concat` (ordine di arrivo dei rami).
// ---------------------------------------------------------------------------

macro_rules! op {
    // Nessuna versione esplicita: tutte e 4 le componenti a 1 (piano-v5.md#identita-e-fingerprint),
    // vincolo di espansione `SumRelative` e nessuna esenzione (errori-e-limiti.md).
    ($id:literal, $family:ident, $origin:ident, $arity:ident, $exec:ident,
     $cancel:ident, $shape:expr, $crs:expr, $caps:expr, $det:ident, $mat:ident) => {
        op!($id, $family, $origin, $arity, $exec, $cancel, $shape, $crs, $caps, $det, $mat,
            kernel_version = 1)
    };
    // Variante con chiavi opzionali: `semantic_version`,
    // `config_schema_version`, `contract_analysis_version`, `kernel_version`
    // (default 1), `expansion_constraint` (default `SumRelative`; accetta un
    // ident di variante oppure `Custom(fattore)` con il fattore f64, errori-e-limiti.md),
    // `expansion_factor_exempt` (default `false`) e `geo_fusion` (default
    // `NotFusible`, architettura.md#geometrie D12.2) sono ammesse in qualsiasi combinazione e
    // ordine; chiave duplicata o sconosciuta -> errore di compilazione.
    ($id:literal, $family:ident, $origin:ident, $arity:ident, $exec:ident,
     $cancel:ident, $shape:expr, $crs:expr, $caps:expr, $det:ident, $mat:ident,
     $($versions:tt)+) => {
        op!(@munch
            ($id, $family, $origin, $arity, $exec, $cancel, $shape, $crs, $caps, $det, $mat)
            (1, 1, 1, 1, ExpansionConstraint::SumRelative, false, GeoFusion::NotFusible)
            $($versions)+)
    };
    // Muncher: consuma una chiave per passo aggiornando l'accumulatore
    // (semantic, config_schema, contract_analysis, kernel,
    // expansion_constraint, expansion_factor_exempt, geo_fusion).
    (@munch ($($base:tt)*) ($s:expr, $c:expr, $a:expr, $k:expr, $x:expr, $e:expr, $g:expr)
        semantic_version = $v:expr) => {
        op!(@build ($($base)*) ($v, $c, $a, $k, $x, $e, $g))
    };
    (@munch ($($base:tt)*) ($s:expr, $c:expr, $a:expr, $k:expr, $x:expr, $e:expr, $g:expr)
        semantic_version = $v:expr, $($rest:tt)+) => {
        op!(@munch ($($base)*) ($v, $c, $a, $k, $x, $e, $g) $($rest)+)
    };
    (@munch ($($base:tt)*) ($s:expr, $c:expr, $a:expr, $k:expr, $x:expr, $e:expr, $g:expr)
        config_schema_version = $v:expr) => {
        op!(@build ($($base)*) ($s, $v, $a, $k, $x, $e, $g))
    };
    (@munch ($($base:tt)*) ($s:expr, $c:expr, $a:expr, $k:expr, $x:expr, $e:expr, $g:expr)
        config_schema_version = $v:expr, $($rest:tt)+) => {
        op!(@munch ($($base)*) ($s, $v, $a, $k, $x, $e, $g) $($rest)+)
    };
    (@munch ($($base:tt)*) ($s:expr, $c:expr, $a:expr, $k:expr, $x:expr, $e:expr, $g:expr)
        contract_analysis_version = $v:expr) => {
        op!(@build ($($base)*) ($s, $c, $v, $k, $x, $e, $g))
    };
    (@munch ($($base:tt)*) ($s:expr, $c:expr, $a:expr, $k:expr, $x:expr, $e:expr, $g:expr)
        contract_analysis_version = $v:expr, $($rest:tt)+) => {
        op!(@munch ($($base)*) ($s, $c, $v, $k, $x, $e, $g) $($rest)+)
    };
    (@munch ($($base:tt)*) ($s:expr, $c:expr, $a:expr, $k:expr, $x:expr, $e:expr, $g:expr)
        kernel_version = $v:expr) => {
        op!(@build ($($base)*) ($s, $c, $a, $v, $x, $e, $g))
    };
    (@munch ($($base:tt)*) ($s:expr, $c:expr, $a:expr, $k:expr, $x:expr, $e:expr, $g:expr)
        kernel_version = $v:expr, $($rest:tt)+) => {
        op!(@munch ($($base)*) ($s, $c, $a, $v, $x, $e, $g) $($rest)+)
    };
    // `expansion_constraint`: variante senza payload (ident) oppure
    // `Custom(fattore)` con fattore f64 esplicito (errori-e-limiti.md).
    (@munch ($($base:tt)*) ($s:expr, $c:expr, $a:expr, $k:expr, $x:expr, $e:expr, $g:expr)
        expansion_constraint = Custom($v:expr)) => {
        op!(@build ($($base)*) ($s, $c, $a, $k, ExpansionConstraint::Custom($v), $e, $g))
    };
    (@munch ($($base:tt)*) ($s:expr, $c:expr, $a:expr, $k:expr, $x:expr, $e:expr, $g:expr)
        expansion_constraint = Custom($v:expr), $($rest:tt)+) => {
        op!(@munch ($($base)*) ($s, $c, $a, $k, ExpansionConstraint::Custom($v), $e, $g) $($rest)+)
    };
    (@munch ($($base:tt)*) ($s:expr, $c:expr, $a:expr, $k:expr, $x:expr, $e:expr, $g:expr)
        expansion_constraint = $v:ident) => {
        op!(@build ($($base)*) ($s, $c, $a, $k, ExpansionConstraint::$v, $e, $g))
    };
    (@munch ($($base:tt)*) ($s:expr, $c:expr, $a:expr, $k:expr, $x:expr, $e:expr, $g:expr)
        expansion_constraint = $v:ident, $($rest:tt)+) => {
        op!(@munch ($($base)*) ($s, $c, $a, $k, ExpansionConstraint::$v, $e, $g) $($rest)+)
    };
    (@munch ($($base:tt)*) ($s:expr, $c:expr, $a:expr, $k:expr, $x:expr, $e:expr, $g:expr)
        expansion_factor_exempt = $v:expr) => {
        op!(@build ($($base)*) ($s, $c, $a, $k, $x, $v, $g))
    };
    (@munch ($($base:tt)*) ($s:expr, $c:expr, $a:expr, $k:expr, $x:expr, $e:expr, $g:expr)
        expansion_factor_exempt = $v:expr, $($rest:tt)+) => {
        op!(@munch ($($base)*) ($s, $c, $a, $k, $x, $v, $g) $($rest)+)
    };
    // `geo_fusion`: variante di [`GeoFusion`] senza payload (architettura.md#geometrie D12.2).
    (@munch ($($base:tt)*) ($s:expr, $c:expr, $a:expr, $k:expr, $x:expr, $e:expr, $g:expr)
        geo_fusion = $v:ident) => {
        op!(@build ($($base)*) ($s, $c, $a, $k, $x, $e, GeoFusion::$v))
    };
    (@munch ($($base:tt)*) ($s:expr, $c:expr, $a:expr, $k:expr, $x:expr, $e:expr, $g:expr)
        geo_fusion = $v:ident, $($rest:tt)+) => {
        op!(@munch ($($base)*) ($s, $c, $a, $k, $x, $e, GeoFusion::$v) $($rest)+)
    };
    (@build ($id:literal, $family:ident, $origin:ident, $arity:ident, $exec:ident,
     $cancel:ident, $shape:expr, $crs:expr, $caps:expr, $det:ident, $mat:ident)
     ($semantic:expr, $config_schema:expr, $contract_analysis:expr, $kernel:expr,
      $constraint:expr, $exempt:expr, $fusion:expr)) => {
        OperationDescriptor {
            id: $id,
            family: Family::$family,
            origin: Origin::$origin,
            arity: Arity::$arity,
            execution_class: ExecutionClass::$exec,
            cancellation_behavior: CancellationBehavior::$cancel,
            geo_fusion: $fusion,
            result_shape: $shape,
            crs_requirement: $crs,
            required_capabilities: $caps,
            determinism: DeterminismPolicy::$det,
            expansion_constraint: $constraint,
            expansion_factor_exempt: $exempt,
            maturity: Maturity::$mat,
            semantic_version: $semantic,
            config_schema_version: $config_schema,
            contract_analysis_version: $contract_analysis,
            kernel_version: $kernel,
        }
    };
}

/// Catalogo unificato: 71 operazioni tabellari + 75 geografiche.
pub static CATALOG: &[OperationDescriptor] = &[
    // --- Tabellari Manipola-compat (37) -----------------------------------
    op!(
        "table.add_row_number",
        Table,
        ManipolaCompat,
        Unary,
        Blocking,
        BoundaryOnly,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol
    ),
    op!(
        "table.aggregate",
        Table,
        ManipolaCompat,
        Unary,
        Blocking,
        BoundaryOnly,
        None,
        None,
        &[],
        CanonicalOrder,
        PublicProtocol,
        kernel_version = 2
    ),
    op!(
        "table.bin",
        Table,
        ManipolaCompat,
        Unary,
        Blocking,
        BoundaryOnly,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol
    ),
    op!(
        "table.concat",
        Table,
        ManipolaCompat,
        NAry,
        Blocking,
        BoundaryOnly,
        None,
        None,
        &[],
        InputOrder,
        PublicProtocol
    ),
    op!(
        "table.concat_columns",
        Table,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol
    ),
    op!(
        "table.conditional",
        Table,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol
    ),
    op!(
        "table.cross_join",
        Table,
        ManipolaCompat,
        BinaryOrdered,
        BinaryBlocking,
        BoundaryOnly,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        expansion_constraint = MaxRelative
    ),
    op!(
        "table.date_extract",
        Table,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        semantic_version = 2,
        kernel_version = 3
    ),
    op!(
        "table.dedup_advanced",
        Table,
        ManipolaCompat,
        Unary,
        Blocking,
        BoundaryOnly,
        None,
        None,
        &[],
        CanonicalOrder,
        PublicProtocol,
        kernel_version = 2
    ),
    op!(
        "table.distinct",
        Table,
        ManipolaCompat,
        Unary,
        Blocking,
        BoundaryOnly,
        None,
        None,
        &[],
        CanonicalOrder,
        PublicProtocol,
        kernel_version = 2
    ),
    op!(
        "table.drop_columns",
        Table,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol
    ),
    op!(
        "table.fill_na",
        Table,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        kernel_version = 2
    ),
    op!(
        "table.filter",
        Table,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        kernel_version = 2
    ),
    op!(
        "table.flatten_json",
        Table,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        semantic_version = 2,
        kernel_version = 3
    ),
    op!(
        "table.formula",
        Table,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        semantic_version = 2,
        kernel_version = 3
    ),
    // join generico: molti-a-molti possibile -> MaxRelative (errori-e-limiti.md).
    op!(
        "table.join",
        Table,
        ManipolaCompat,
        BinaryOrdered,
        BinaryBlocking,
        BoundaryOnly,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        kernel_version = 2,
        expansion_constraint = MaxRelative
    ),
    op!(
        "table.lookup",
        Table,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol
    ),
    op!(
        "table.melt",
        Table,
        ManipolaCompat,
        Unary,
        Blocking,
        BoundaryOnly,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        kernel_version = 2
    ),
    op!(
        "table.pivot",
        Table,
        ManipolaCompat,
        Unary,
        Blocking,
        BoundaryOnly,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        kernel_version = 2
    ),
    op!(
        "table.rename",
        Table,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol
    ),
    op!(
        "table.reorder_columns",
        Table,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol
    ),
    op!(
        "table.replace",
        Table,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol
    ),
    op!(
        "table.sample",
        Table,
        ManipolaCompat,
        Unary,
        Blocking,
        BoundaryOnly,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol
    ),
    op!(
        "table.sort",
        Table,
        ManipolaCompat,
        Unary,
        Blocking,
        BoundaryOnly,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        kernel_version = 2
    ),
    op!(
        "table.split_column",
        Table,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol
    ),
    op!(
        "table.statistics",
        Table,
        ManipolaCompat,
        Unary,
        Blocking,
        BoundaryOnly,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        kernel_version = 2
    ),
    op!(
        "table.string_extract",
        Table,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        kernel_version = 2
    ),
    op!(
        "table.string_length",
        Table,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol
    ),
    op!(
        "table.string_pad",
        Table,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol
    ),
    // diff: l'output (added/removed/changed) e' proporzionale a entrambi gli
    // input -> SumRelative (errori-e-limiti.md).
    op!(
        "table.table_diff",
        Table,
        ManipolaCompat,
        BinaryOrdered,
        BinaryBlocking,
        BoundaryOnly,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        kernel_version = 2,
        expansion_constraint = SumRelative
    ),
    op!(
        "table.text_normalize",
        Table,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        kernel_version = 2
    ),
    op!(
        "table.transpose",
        Table,
        ManipolaCompat,
        Unary,
        Blocking,
        BoundaryOnly,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol
    ),
    op!(
        "table.type_cast",
        Table,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        semantic_version = 2,
        kernel_version = 3
    ),
    op!(
        "table.uuid_generator",
        Table,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol
    ),
    op!(
        "table.window_function",
        Table,
        ManipolaCompat,
        Unary,
        Blocking,
        BoundaryOnly,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        kernel_version = 2
    ),
    op!(
        "table.mask_data",
        Table,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        kernel_version = 2
    ),
    op!(
        "table.md5_hash",
        Table,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        semantic_version = 2,
        kernel_version = 2
    ),
    // --- Tabellari estensioni (25) -----------------------------------------
    // anti_join: output <= left -> LeftRelative.
    op!(
        "table.anti_join",
        Table,
        Extension,
        BinaryOrdered,
        BinaryBlocking,
        BoundaryOnly,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        kernel_version = 2,
        expansion_constraint = LeftRelative
    ),
    // asof_join: una corrispondenza per riga left (lookup-style) -> LeftRelative.
    op!(
        "table.asof_join",
        Table,
        Extension,
        BinaryOrdered,
        BinaryBlocking,
        BoundaryOnly,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        expansion_constraint = LeftRelative
    ),
    op!(
        "table.assert_not_null",
        Table,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        semantic_version = 2,
        kernel_version = 2
    ),
    op!(
        "table.assert_range",
        Table,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        semantic_version = 2,
        kernel_version = 2
    ),
    op!(
        "table.assert_regex",
        Table,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        semantic_version = 2,
        kernel_version = 2
    ),
    op!(
        "table.assert_schema",
        Table,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol
    ),
    op!(
        "table.assert_unique",
        Table,
        Extension,
        Unary,
        Blocking,
        BoundaryOnly,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        semantic_version = 2,
        kernel_version = 3
    ),
    op!(
        "table.coalesce",
        Table,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        kernel_version = 2
    ),
    op!(
        "table.date_add",
        Table,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        semantic_version = 2,
        kernel_version = 3
    ),
    op!(
        "table.date_diff",
        Table,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        semantic_version = 2,
        kernel_version = 3
    ),
    op!(
        "table.date_format",
        Table,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        semantic_version = 2,
        kernel_version = 3
    ),
    // except: output <= left -> LeftRelative.
    op!(
        "table.except",
        Table,
        Extension,
        BinaryOrdered,
        BinaryBlocking,
        BoundaryOnly,
        None,
        None,
        &[],
        CanonicalOrder,
        PublicProtocol,
        kernel_version = 2,
        expansion_constraint = LeftRelative
    ),
    op!(
        "table.explode",
        Table,
        Extension,
        Unary,
        Blocking,
        BoundaryOnly,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        semantic_version = 2,
        kernel_version = 2
    ),
    // intersect: output <= left (e <= right) -> LeftRelative.
    op!(
        "table.intersect",
        Table,
        Extension,
        BinaryOrdered,
        BinaryBlocking,
        BoundaryOnly,
        None,
        None,
        &[],
        CanonicalOrder,
        PublicProtocol,
        kernel_version = 2,
        expansion_constraint = LeftRelative
    ),
    op!(
        "table.rolling_window",
        Table,
        Extension,
        Unary,
        Blocking,
        BoundaryOnly,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        kernel_version = 2
    ),
    // semi_join: output <= left -> LeftRelative.
    op!(
        "table.semi_join",
        Table,
        Extension,
        BinaryOrdered,
        BinaryBlocking,
        BoundaryOnly,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        kernel_version = 2,
        expansion_constraint = LeftRelative
    ),
    op!(
        "table.sha256_hash",
        Table,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        semantic_version = 2,
        kernel_version = 2
    ),
    op!(
        "table.timezone_convert",
        Table,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        semantic_version = 2,
        kernel_version = 3
    ),
    // union_distinct: output <= left + right -> SumRelative (esplicito).
    op!(
        "table.union_distinct",
        Table,
        Extension,
        BinaryOrdered,
        BinaryBlocking,
        BoundaryOnly,
        None,
        None,
        &[],
        CanonicalOrder,
        PublicProtocol,
        kernel_version = 2,
        expansion_constraint = SumRelative
    ),
    op!(
        "table.unnest",
        Table,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol
    ),
    // expression v2 (Fase estensione funzioni/temporali): nuove funzioni
    // (substring, regex_replace, between, in, greatest, least, floor, ceil,
    // power) e date_trunc con output Date32/TimestampMs nativi -> tutte e 4
    // le versioni incrementate (piano-v5.md#identita-e-fingerprint).
    op!(
        "table.expression",
        Table,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        semantic_version = 3,
        config_schema_version = 2,
        contract_analysis_version = 2,
        kernel_version = 4
    ),
    op!(
        "table.assert_cardinality",
        Table,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol
    ),
    op!(
        "table.assert_metadata",
        Table,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol
    ),
    // assert_foreign_key: validazione, output = left -> LeftRelative.
    op!(
        "table.assert_foreign_key",
        Table,
        Extension,
        BinaryOrdered,
        BinaryBlocking,
        BoundaryOnly,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        semantic_version = 2,
        kernel_version = 3,
        expansion_constraint = LeftRelative
    ),
    // reconcile: semantica di output non caratterizzata con certezza ->
    // SumRelative di default (da rivedere se emerge un vincolo piu' preciso).
    op!(
        "table.reconcile",
        Table,
        Extension,
        BinaryOrdered,
        BinaryBlocking,
        BoundaryOnly,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        kernel_version = 2
    ),
    // --- Geografiche Manipola-compat (33) -----------------------------------
    op!(
        "geo.centroid",
        Geo,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        PublicProtocol,
        geo_fusion = TransformInPlace,
        semantic_version = 2
    ),
    op!(
        "geo.convex_hull",
        Geo,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        PublicProtocol,
        geo_fusion = TransformInPlace,
        semantic_version = 2
    ),
    op!(
        "geo.envelope",
        Geo,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        PublicProtocol,
        geo_fusion = TransformInPlace,
        semantic_version = 2
    ),
    // sjoin: una geometria left puo' intersecare molte right (molti-a-molti)
    // -> MaxRelative (errori-e-limiti.md).
    op!(
        "geo.sjoin",
        Geo,
        ManipolaCompat,
        BinaryOrdered,
        BinaryBlocking,
        BoundaryOnly,
        Some(ResultShape::OneToMany),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        PublicProtocol,
        expansion_constraint = MaxRelative
    ),
    op!(
        "geo.area",
        Geo,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        KernelValidated,
        geo_fusion = TerminalMeasure,
        semantic_version = 2
    ),
    op!(
        "geo.boundary",
        Geo,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        KernelValidated,
        geo_fusion = TransformInPlace,
        semantic_version = 2
    ),
    op!(
        "geo.bounds_extractor",
        Geo,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        KernelValidated,
        semantic_version = 2
    ),
    op!(
        "geo.buffer",
        Geo,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        KernelValidated,
        geo_fusion = TransformInPlace,
        semantic_version = 2
    ),
    op!(
        "geo.clean_topology",
        Geo,
        ManipolaCompat,
        Unary,
        Blocking,
        BoundaryOnly,
        Some(ResultShape::OneToMany),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    // clip/difference: il taglio puo' spezzare una geometria left in piu'
    // pezzi (OneToMany) -> MaxRelative.
    op!(
        "geo.clip",
        Geo,
        ManipolaCompat,
        BinaryOrdered,
        BinaryBlocking,
        BoundaryOnly,
        Some(ResultShape::OneToMany),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated,
        expansion_constraint = MaxRelative
    ),
    op!(
        "geo.count_points_in_polygons",
        Geo,
        ManipolaCompat,
        BinaryOrdered,
        BinaryBlocking,
        BoundaryOnly,
        Some(ResultShape::OneToMany),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated,
        expansion_constraint = LeftRelative
    ),
    op!(
        "geo.difference",
        Geo,
        ManipolaCompat,
        BinaryOrdered,
        BinaryBlocking,
        BoundaryOnly,
        Some(ResultShape::OneToMany),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated,
        expansion_constraint = MaxRelative
    ),
    op!(
        "geo.dissolve",
        Geo,
        ManipolaCompat,
        Unary,
        Blocking,
        BoundaryOnly,
        Some(ResultShape::ManyToOne),
        Some(CrsRequirement::Projected),
        &[],
        CanonicalOrder,
        KernelValidated
    ),
    op!(
        "geo.distance",
        Geo,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    op!(
        "geo.explode",
        Geo,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToMany),
        Some(CrsRequirement::Known),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    op!(
        "geo.from_coords",
        Geo,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::FromCoords),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        KernelValidated,
        semantic_version = 2
    ),
    op!(
        "geo.intersection",
        Geo,
        ManipolaCompat,
        BinaryOrdered,
        BinaryBlocking,
        BoundaryOnly,
        Some(ResultShape::OneToMany),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated,
        expansion_constraint = MaxRelative
    ),
    op!(
        "geo.length",
        Geo,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        KernelValidated,
        geo_fusion = TerminalMeasure,
        semantic_version = 2
    ),
    op!(
        "geo.line_builder",
        Geo,
        ManipolaCompat,
        Unary,
        Blocking,
        BoundaryOnly,
        Some(ResultShape::ManyToOne),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    // nearest: una corrispondenza per riga left (lookup-style) -> LeftRelative.
    op!(
        "geo.nearest",
        Geo,
        ManipolaCompat,
        BinaryOrdered,
        BinaryBlocking,
        BoundaryOnly,
        Some(ResultShape::OneToMany),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated,
        expansion_constraint = LeftRelative
    ),
    // overlay: un left puo' produrre piu' pezzi (OneToMany) -> MaxRelative.
    op!(
        "geo.overlay",
        Geo,
        ManipolaCompat,
        BinaryOrdered,
        BinaryBlocking,
        BoundaryOnly,
        Some(ResultShape::OneToMany),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated,
        expansion_constraint = MaxRelative
    ),
    op!(
        "geo.perimeter",
        Geo,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        KernelValidated,
        geo_fusion = TerminalMeasure,
        semantic_version = 2
    ),
    op!(
        "geo.point_on_surface",
        Geo,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        KernelValidated,
        geo_fusion = TransformInPlace,
        semantic_version = 2
    ),
    op!(
        "geo.polygon_builder",
        Geo,
        ManipolaCompat,
        Unary,
        Blocking,
        BoundaryOnly,
        Some(ResultShape::ManyToOne),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    op!(
        "geo.simplify",
        Geo,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        KernelValidated,
        geo_fusion = TransformInPlace,
        semantic_version = 2
    ),
    op!(
        "geo.symmetric_difference",
        Geo,
        ManipolaCompat,
        BinaryOrdered,
        BinaryBlocking,
        BoundaryOnly,
        Some(ResultShape::OneToMany),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated,
        expansion_constraint = MaxRelative
    ),
    op!(
        "geo.to_wkt",
        Geo,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Known),
        &[],
        DefinedOrder,
        KernelValidated,
        geo_fusion = TerminalMeasure,
        semantic_version = 2
    ),
    // union: semantica di output non caratterizzata con certezza (unione
    // dissolta dei due input) -> SumRelative di default (da rivedere).
    op!(
        "geo.union",
        Geo,
        ManipolaCompat,
        BinaryOrdered,
        BinaryBlocking,
        BoundaryOnly,
        Some(ResultShape::OneToMany),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    op!(
        "geo.vertex_count",
        Geo,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Known),
        &[],
        DefinedOrder,
        KernelValidated,
        geo_fusion = TerminalMeasure,
        semantic_version = 2
    ),
    op!(
        "geo.voronoi",
        Geo,
        ManipolaCompat,
        Unary,
        Blocking,
        BoundaryOnly,
        Some(ResultShape::OneToMany),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    // within: filtro del left sul right -> output <= left -> LeftRelative.
    op!(
        "geo.within",
        Geo,
        ManipolaCompat,
        BinaryOrdered,
        BinaryBlocking,
        BoundaryOnly,
        Some(ResultShape::OneToMany),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated,
        expansion_constraint = LeftRelative
    ),
    // architettura.md#geometrie M3: make_valid entra nel perimetro di fusione come
    // TransformInPlace; l'ammissione di input OGC-invalido (trappola 1) e'
    // una proprieta' del suo gate di decode, gestita dal runner fuso con
    // l'eccezione documentata in architettura.md#geometrie D12.4-M3 — non richiede una
    // variante di capability dedicata (la relazione di raggruppamento e'
    // identica: 1:1 in place sulla stessa colonna).
    op!(
        "geo.make_valid",
        Geo,
        ManipolaCompat,
        Unary,
        Streaming,
        NonInterruptible,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Known),
        &["geos"],
        DefinedOrder,
        BackendPending,
        geo_fusion = TransformInPlace,
        semantic_version = 2
    ),
    op!(
        "geo.reproject",
        Geo,
        ManipolaCompat,
        Unary,
        Streaming,
        NonInterruptible,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Reprojection),
        &["proj"],
        DefinedOrder,
        BackendPending,
        geo_fusion = TransformInPlace,
        semantic_version = 2
    ),
    // --- Predicati DE-9IM, estensioni geo (11) ------------------------------
    op!(
        "geo.predicate_intersects",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    op!(
        "geo.predicate_disjoint",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    op!(
        "geo.predicate_contains",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    op!(
        "geo.predicate_within",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    op!(
        "geo.predicate_equals_topo",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    op!(
        "geo.predicate_covers",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    op!(
        "geo.predicate_covered_by",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    op!(
        "geo.predicate_contains_properly",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    op!(
        "geo.predicate_touches",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    op!(
        "geo.predicate_crosses",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    op!(
        "geo.predicate_overlaps",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    // --- Estensioni geo (21) -------------------------------------------------
    op!(
        "geo.affine_transform",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        KernelValidated,
        geo_fusion = TransformInPlace,
        semantic_version = 2
    ),
    op!(
        "geo.translate",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        KernelValidated,
        geo_fusion = TransformInPlace,
        semantic_version = 2
    ),
    op!(
        "geo.scale",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        KernelValidated,
        geo_fusion = TransformInPlace,
        semantic_version = 2
    ),
    op!(
        "geo.rotate",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        KernelValidated,
        geo_fusion = TransformInPlace,
        semantic_version = 2
    ),
    op!(
        "geo.concave_hull",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        KernelValidated,
        geo_fusion = TransformInPlace,
        semantic_version = 2
    ),
    op!(
        "geo.hausdorff_distance",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    op!(
        "geo.haversine_distance",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Geographic),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    op!(
        "geo.geodesic_distance",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Geographic),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    op!(
        "geo.geodesic_line_length",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Geographic),
        &[],
        DefinedOrder,
        KernelValidated,
        semantic_version = 2
    ),
    op!(
        "geo.densify",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        KernelValidated,
        geo_fusion = TransformInPlace,
        semantic_version = 2
    ),
    op!(
        "geo.snap_to_grid",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        KernelValidated,
        geo_fusion = TransformInPlace,
        semantic_version = 2
    ),
    op!(
        "geo.delaunay",
        Geo,
        Extension,
        Unary,
        Blocking,
        BoundaryOnly,
        Some(ResultShape::OneToMany),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    op!(
        "geo.polygonize",
        Geo,
        Extension,
        Unary,
        Blocking,
        NonInterruptible,
        Some(ResultShape::ManyToOne),
        Some(CrsRequirement::Projected),
        &["geos"],
        DefinedOrder,
        BackendPending
    ),
    op!(
        "geo.line_merge",
        Geo,
        Extension,
        Unary,
        Blocking,
        BoundaryOnly,
        Some(ResultShape::ManyToOne),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    op!(
        "geo.split",
        Geo,
        Extension,
        Unary,
        Streaming,
        NonInterruptible,
        Some(ResultShape::OneToMany),
        Some(CrsRequirement::SameProjected),
        &["geos"],
        DefinedOrder,
        BackendPending
    ),
    op!(
        "geo.line_substring",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        KernelValidated,
        semantic_version = 2
    ),
    op!(
        "geo.line_interpolate_point",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        KernelValidated,
        semantic_version = 2
    ),
    op!(
        "geo.frechet_distance",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    op!(
        "geo.bearing",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Geographic),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    op!(
        "geo.geodesic_area",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Geographic),
        &[],
        DefinedOrder,
        KernelValidated,
        semantic_version = 2
    ),
    op!(
        "geo.geometry_diagnostics",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::Diagnostic),
        Some(CrsRequirement::Known),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    // --- Estensioni geo v1.1 (4) ---------------------------------------------
    op!(
        "geo.from_wkt",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::FromCoords),
        Some(CrsRequirement::Known),
        &[],
        DefinedOrder,
        KernelValidated,
        semantic_version = 3,
        contract_analysis_version = 2,
        kernel_version = 2
    ),
    op!(
        "geo.geometry_accessors",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Known),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    op!(
        "geo.collect",
        Geo,
        Extension,
        Unary,
        Blocking,
        BoundaryOnly,
        Some(ResultShape::ManyToOne),
        Some(CrsRequirement::Known),
        &[],
        CanonicalOrder,
        KernelValidated
    ),
    op!(
        "geo.line_locate_point",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Known),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    // --- Estensioni geo v1.2 (3) ---------------------------------------------
    op!(
        "geo.generate_grid",
        Geo,
        Extension,
        Unary,
        Blocking,
        BoundaryOnly,
        Some(ResultShape::WholeToMany),
        Some(CrsRequirement::Known),
        &[],
        DefinedOrder,
        KernelValidated,
        expansion_factor_exempt = true,
        semantic_version = 2,
        contract_analysis_version = 2
    ),
    op!(
        "geo.subdivide",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToMany),
        Some(CrsRequirement::Known),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    // `snap`: il riferimento da config (`reference_wkb`) e' assunto nello
    // stesso CRS dell'input (convenzione D16): requisito SameProjected per
    // l'unica colonna, come le distanze "unarie".
    op!(
        "geo.snap",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    // --- Estensioni geo v1.3 (3) ---------------------------------------------
    // Coperture poligonali (piantine di edifici): entrambe consumano l'intero
    // input (Blocking) e producono una riga per issue/tratto condiviso
    // (WholeToMany, schema nuovo); aree e lunghezze in unita' di mappa,
    // quindi SameProjected. Esenti da `max_expansion_factor` (errori-e-limiti.md:
    // esenzione dichiarata in catalogo).
    op!(
        "geo.coverage_validate",
        Geo,
        Extension,
        Unary,
        Blocking,
        BoundaryOnly,
        Some(ResultShape::WholeToMany),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated,
        expansion_factor_exempt = true
    ),
    op!(
        "geo.shared_paths",
        Geo,
        Extension,
        Unary,
        Blocking,
        BoundaryOnly,
        Some(ResultShape::WholeToMany),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated,
        expansion_factor_exempt = true
    ),
    // `cluster_dbscan`: clustering globale per densita' (vicinati R-tree
    // sull'intero input) ma output allineato alle righe (un'etichetta UInt64
    // nullable per riga, noise -> null): Blocking con shape OneToOne; eps in
    // unita' di mappa, quindi Projected.
    op!(
        "geo.cluster_dbscan",
        Geo,
        Extension,
        Unary,
        Blocking,
        BoundaryOnly,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    // --- Estensioni table v1.1 (4) -------------------------------------------
    op!(
        "table.limit",
        Table,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        InputOrder,
        KernelValidated
    ),
    op!(
        "table.select_columns",
        Table,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        KernelValidated
    ),
    op!(
        "table.stable_fingerprint",
        Table,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        KernelValidated,
        kernel_version = 2
    ),
    op!(
        "table.top_n",
        Table,
        Extension,
        Unary,
        Blocking,
        BoundaryOnly,
        None,
        None,
        &[],
        DefinedOrder,
        KernelValidated
    ),
    // --- Estensioni table v1.2 (4) -------------------------------------------
    op!(
        "table.align_schema",
        Table,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        KernelValidated
    ),
    op!(
        "table.concat_by_name",
        Table,
        Extension,
        NAry,
        Blocking,
        BoundaryOnly,
        None,
        None,
        &[],
        InputOrder,
        KernelValidated
    ),
    op!(
        "table.hmac_sha256",
        Table,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        KernelValidated,
        kernel_version = 2
    ),
    op!(
        "table.validate_rules",
        Table,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        KernelValidated
    ),
    // --- Estensioni table v1.3 (1) -------------------------------------------
    // fuzzy_join: build/probe sui blocchi (prefix/soundex) come i join
    // esatti, ma scoring per coppia candidata -> BinaryBlocking; ordine di
    // output definito (scansione sinistra, indice destro).
    // fuzzy_join: build/probe sui blocchi (prefix/soundex) come i join
    // esatti, ma scoring per coppia candidata -> BinaryBlocking; ordine di
    // output definito (scansione sinistra, indice destro). Piu' candidati
    // per riga left possibili -> MaxRelative.
    op!(
        "table.fuzzy_join",
        Table,
        Extension,
        BinaryOrdered,
        BinaryBlocking,
        BoundaryOnly,
        None,
        None,
        &[],
        DefinedOrder,
        KernelValidated,
        expansion_constraint = MaxRelative
    ),
];

/// Tabella alias versionata (decisione D20, `docs/catalog-diff.md`).
///
/// Forma: `(schema_version, legacy_alias, canonical_id)`. Immutabile per le
/// versioni pubblicate: un alias introdotto non puo' mai essere riassegnato.
/// `schema_version` 3 copre sia i piani nogeo legacy sia gli id storici del
/// protocollo geo (v2/v3, `TransformArrowSchema`).
pub static ALIASES: &[(u16, &str, &str)] = &[
    // --- Piani nogeo legacy: id storico -> table.<id> (62) -----------------
    (3, "add_row_number", "table.add_row_number"),
    (3, "aggregate", "table.aggregate"),
    (3, "bin", "table.bin"),
    (3, "concat", "table.concat"),
    (3, "concat_columns", "table.concat_columns"),
    (3, "conditional", "table.conditional"),
    (3, "cross_join", "table.cross_join"),
    (3, "date_extract", "table.date_extract"),
    (3, "dedup_advanced", "table.dedup_advanced"),
    (3, "distinct", "table.distinct"),
    (3, "drop_columns", "table.drop_columns"),
    (3, "fill_na", "table.fill_na"),
    (3, "filter", "table.filter"),
    (3, "flatten_json", "table.flatten_json"),
    (3, "formula", "table.formula"),
    (3, "join", "table.join"),
    (3, "lookup", "table.lookup"),
    (3, "melt", "table.melt"),
    (3, "pivot", "table.pivot"),
    (3, "rename", "table.rename"),
    (3, "reorder_columns", "table.reorder_columns"),
    (3, "replace", "table.replace"),
    (3, "sample", "table.sample"),
    (3, "sort", "table.sort"),
    (3, "split_column", "table.split_column"),
    (3, "statistics", "table.statistics"),
    (3, "string_extract", "table.string_extract"),
    (3, "string_length", "table.string_length"),
    (3, "string_pad", "table.string_pad"),
    (3, "table_diff", "table.table_diff"),
    (3, "text_normalize", "table.text_normalize"),
    (3, "transpose", "table.transpose"),
    (3, "type_cast", "table.type_cast"),
    (3, "uuid_generator", "table.uuid_generator"),
    (3, "window_function", "table.window_function"),
    (3, "mask_data", "table.mask_data"),
    (3, "md5_hash", "table.md5_hash"),
    (3, "anti_join", "table.anti_join"),
    (3, "asof_join", "table.asof_join"),
    (3, "assert_not_null", "table.assert_not_null"),
    (3, "assert_range", "table.assert_range"),
    (3, "assert_regex", "table.assert_regex"),
    (3, "assert_schema", "table.assert_schema"),
    (3, "assert_unique", "table.assert_unique"),
    (3, "coalesce", "table.coalesce"),
    (3, "date_add", "table.date_add"),
    (3, "date_diff", "table.date_diff"),
    (3, "date_format", "table.date_format"),
    (3, "except", "table.except"),
    (3, "explode", "table.explode"),
    (3, "intersect", "table.intersect"),
    (3, "rolling_window", "table.rolling_window"),
    (3, "semi_join", "table.semi_join"),
    (3, "sha256_hash", "table.sha256_hash"),
    (3, "timezone_convert", "table.timezone_convert"),
    (3, "union_distinct", "table.union_distinct"),
    (3, "unnest", "table.unnest"),
    (3, "expression", "table.expression"),
    (3, "assert_cardinality", "table.assert_cardinality"),
    (3, "assert_metadata", "table.assert_metadata"),
    (3, "assert_foreign_key", "table.assert_foreign_key"),
    (3, "reconcile", "table.reconcile"),
    // --- Id geo storici: geo_* -> geo.<id senza prefisso> (33) -------------
    (3, "geo_centroid", "geo.centroid"),
    (3, "geo_convex_hull", "geo.convex_hull"),
    (3, "geo_envelope", "geo.envelope"),
    (3, "geo_area", "geo.area"),
    (3, "geo_boundary", "geo.boundary"),
    (3, "geo_bounds_extractor", "geo.bounds_extractor"),
    (3, "geo_buffer", "geo.buffer"),
    (3, "geo_clean_topology", "geo.clean_topology"),
    (3, "geo_clip", "geo.clip"),
    (
        3,
        "geo_count_points_in_polygons",
        "geo.count_points_in_polygons",
    ),
    (3, "geo_difference", "geo.difference"),
    (3, "geo_dissolve", "geo.dissolve"),
    (3, "geo_distance", "geo.distance"),
    (3, "geo_explode", "geo.explode"),
    (3, "geo_from_coords", "geo.from_coords"),
    (3, "geo_intersection", "geo.intersection"),
    (3, "geo_length", "geo.length"),
    (3, "geo_line_builder", "geo.line_builder"),
    (3, "geo_nearest", "geo.nearest"),
    (3, "geo_overlay", "geo.overlay"),
    (3, "geo_perimeter", "geo.perimeter"),
    (3, "geo_point_on_surface", "geo.point_on_surface"),
    (3, "geo_polygon_builder", "geo.polygon_builder"),
    (3, "geo_simplify", "geo.simplify"),
    (3, "geo_symmetric_difference", "geo.symmetric_difference"),
    (3, "geo_to_wkt", "geo.to_wkt"),
    (3, "geo_union", "geo.union"),
    (3, "geo_vertex_count", "geo.vertex_count"),
    (3, "geo_voronoi", "geo.voronoi"),
    (3, "geo_within", "geo.within"),
    (3, "geo_make_valid", "geo.make_valid"),
    (3, "geo_reproject", "geo.reproject"),
    // --- Predicati DE-9IM: id invariato sotto geo. (11) --------------------
    (3, "predicate_intersects", "geo.predicate_intersects"),
    (3, "predicate_disjoint", "geo.predicate_disjoint"),
    (3, "predicate_contains", "geo.predicate_contains"),
    (3, "predicate_within", "geo.predicate_within"),
    (3, "predicate_equals_topo", "geo.predicate_equals_topo"),
    (3, "predicate_covers", "geo.predicate_covers"),
    (3, "predicate_covered_by", "geo.predicate_covered_by"),
    (
        3,
        "predicate_contains_properly",
        "geo.predicate_contains_properly",
    ),
    (3, "predicate_touches", "geo.predicate_touches"),
    (3, "predicate_crosses", "geo.predicate_crosses"),
    (3, "predicate_overlaps", "geo.predicate_overlaps"),
    // --- Estensioni geo nude: <id> -> geo.<id> (21) ------------------------
    (3, "sjoin", "geo.sjoin"),
    (3, "affine_transform", "geo.affine_transform"),
    (3, "translate", "geo.translate"),
    (3, "scale", "geo.scale"),
    (3, "rotate", "geo.rotate"),
    (3, "concave_hull", "geo.concave_hull"),
    (3, "hausdorff_distance", "geo.hausdorff_distance"),
    (3, "haversine_distance", "geo.haversine_distance"),
    (3, "geodesic_distance", "geo.geodesic_distance"),
    (3, "geodesic_line_length", "geo.geodesic_line_length"),
    (3, "densify", "geo.densify"),
    (3, "snap_to_grid", "geo.snap_to_grid"),
    (3, "delaunay", "geo.delaunay"),
    (3, "polygonize", "geo.polygonize"),
    (3, "line_merge", "geo.line_merge"),
    (3, "split", "geo.split"),
    (3, "line_substring", "geo.line_substring"),
    (3, "line_interpolate_point", "geo.line_interpolate_point"),
    (3, "frechet_distance", "geo.frechet_distance"),
    (3, "bearing", "geo.bearing"),
    (3, "geodesic_area", "geo.geodesic_area"),
    (3, "geometry_diagnostics", "geo.geometry_diagnostics"),
];

/// Risolve un alias legacy per una data `schema_version` verso l'id canonico.
#[must_use]
pub fn resolve_alias(schema_version: u16, alias: &str) -> Option<&'static str> {
    ALIASES
        .iter()
        .find(|(version, a, _)| *version == schema_version && *a == alias)
        .map(|(_, _, canonical)| *canonical)
}

/// Cerca un'operazione per id canonico; accetta anche gli alias legacy
/// (in questo caso la `schema_version` non e' nota al chiamante e si usa
/// la prima voce di tabella corrispondente).
#[must_use]
pub fn find_operation(id: &str) -> Option<&'static OperationDescriptor> {
    CATALOG.iter().find(|op| op.id == id).or_else(|| {
        ALIASES
            .iter()
            .find(|(_, alias, _)| *alias == id)
            .and_then(|(_, _, canonical)| CATALOG.iter().find(|op| op.id == *canonical))
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn catalog_has_146_unique_ids() {
        assert_eq!(CATALOG.len(), 146);
        let ids: HashSet<_> = CATALOG.iter().map(|op| op.id).collect();
        assert_eq!(ids.len(), CATALOG.len());
        assert_eq!(
            CATALOG
                .iter()
                .filter(|op| op.family == Family::Table)
                .count(),
            71
        );
        assert_eq!(
            CATALOG.iter().filter(|op| op.family == Family::Geo).count(),
            75
        );
    }

    #[test]
    fn every_alias_resolves_to_an_existing_catalog_id() {
        assert_eq!(ALIASES.len(), 127);
        for (schema_version, alias, canonical) in ALIASES {
            assert!(
                CATALOG.iter().any(|op| op.id == *canonical),
                "alias {alias} punta a un id assente: {canonical}"
            );
            assert_eq!(
                resolve_alias(*schema_version, alias),
                Some(*canonical),
                "resolve_alias fallisce per {alias}"
            );
        }
    }

    #[test]
    fn no_alias_collides_with_a_canonical_id_of_a_different_family() {
        for (_, alias, canonical) in ALIASES {
            let target = CATALOG
                .iter()
                .find(|op| op.id == *canonical)
                .expect("alias risolto nel test precedente");
            if let Some(conflicting) = CATALOG.iter().find(|op| op.id == *alias) {
                assert_eq!(
                    conflicting.family, target.family,
                    "alias {alias} collide con id canonico di altra famiglia"
                );
            }
        }
    }

    #[test]
    fn crs_requirement_implies_geo_family() {
        for op in CATALOG {
            if op.crs_requirement.is_some() {
                assert_eq!(op.family, Family::Geo, "{} non e' geo", op.id);
            }
            if op.family == Family::Table {
                assert!(op.result_shape.is_none(), "{} tabellare con shape", op.id);
                assert!(
                    op.required_capabilities.is_empty(),
                    "{} tabellare con capability",
                    op.id
                );
            }
        }
    }

    #[test]
    fn find_operation_accepts_canonical_ids_and_aliases() {
        assert_eq!(
            find_operation("table.filter").map(|op| op.id),
            Some("table.filter")
        );
        assert_eq!(
            find_operation("filter").map(|op| op.id),
            Some("table.filter")
        );
        assert_eq!(
            find_operation("geo_buffer").map(|op| op.id),
            Some("geo.buffer")
        );
        assert_eq!(
            find_operation("translate").map(|op| op.id),
            Some("geo.translate")
        );
        assert!(find_operation("nonexistent_op").is_none());
    }

    #[test]
    fn versions_default_to_one_and_expression_versions_are_explicit() {
        // Default: tutte e 4 le componenti a 1 per le op senza incrementi.
        let filter = find_operation("table.filter").expect("table.filter");
        assert_eq!(filter.semantic_version, 1);
        assert_eq!(filter.config_schema_version, 1);
        assert_eq!(filter.contract_analysis_version, 1);
        assert_eq!(filter.kernel_version, 2);
        // Le 4 componenti di table.expression restano esplicite e indipendenti:
        // diagnostics row-scoped cambia semantica e kernel, non schema config
        // né analisi del contratto (piano-v5.md#identita-e-fingerprint).
        let expression = find_operation("table.expression").expect("table.expression");
        assert_eq!(expression.semantic_version, 3);
        assert_eq!(expression.config_schema_version, 2);
        assert_eq!(expression.contract_analysis_version, 2);
        assert_eq!(expression.kernel_version, 4);
        // Nessuna versione puo' essere 0 in tutto il catalogo.
        for op in CATALOG {
            assert!(op.semantic_version >= 1, "{} semantic_version", op.id);
            assert!(
                op.config_schema_version >= 1,
                "{} config_schema_version",
                op.id
            );
            assert!(
                op.contract_analysis_version >= 1,
                "{} contract_analysis_version",
                op.id
            );
            assert!(op.kernel_version >= 1, "{} kernel_version", op.id);
        }
    }

    #[test]
    fn backend_pending_ops_declare_their_capability() {
        for op in CATALOG {
            if op.maturity == Maturity::BackendPending {
                assert!(
                    !op.required_capabilities.is_empty(),
                    "{} backend_pending senza capability",
                    op.id
                );
                assert_eq!(
                    op.cancellation_behavior,
                    CancellationBehavior::NonInterruptible,
                    "{} backend_pending interrompibile",
                    op.id
                );
            }
        }
    }

    #[test]
    fn expansion_constraint_defaults_to_sum_relative() {
        // Retrocompatibilita' comportamentale (errori-e-limiti.md): le op senza
        // dichiarazione esplicita restano sulla base left+right e non sono
        // esenti.
        let filter = find_operation("table.filter").expect("table.filter");
        assert_eq!(
            filter.expansion_constraint,
            ExpansionConstraint::SumRelative
        );
        assert!(!filter.expansion_factor_exempt);
        let reconcile = find_operation("table.reconcile").expect("table.reconcile");
        assert_eq!(
            reconcile.expansion_constraint,
            ExpansionConstraint::SumRelative
        );
    }

    #[test]
    fn row_provenance_audit_is_fail_closed_for_cardinality_and_order_changes() {
        for id in [
            "table.filter",
            "table.sample",
            "table.explode",
            "table.join",
            "table.aggregate",
            "table.sort",
            "table.melt",
            "table.pivot",
            "table.transpose",
            "table.table_diff",
            "table.top_n",
            "table.distinct",
            "table.dedup_advanced",
            "table.window_function",
            "table.concat",
            "table.concat_by_name",
            "table.cross_join",
            "table.fuzzy_join",
            "table.asof_join",
            "table.semi_join",
            "table.anti_join",
            "geo.collect",
            "geo.subdivide",
            "geo.sjoin",
            "geo.generate_grid",
        ] {
            assert_eq!(
                find_operation(id).map(OperationDescriptor::source_row_provenance),
                Some(SourceRowProvenance::Unavailable),
                "{id} non deve dichiarare provenance originale"
            );
        }
        for id in [
            "table.rename",
            "table.reorder_columns",
            "table.flatten_json",
            "table.type_cast",
            "table.bin",
            "table.add_row_number",
            "table.assert_unique",
            "table.assert_foreign_key",
            "geo.from_wkt",
        ] {
            assert_eq!(
                find_operation(id).map(OperationDescriptor::source_row_provenance),
                Some(SourceRowProvenance::Preserved),
                "{id} deve conservare posizione e cardinalita'"
            );
        }
    }

    #[test]
    fn binary_ops_declare_the_binding_constraint() {
        let expected: &[(&str, ExpansionConstraint)] = &[
            ("table.join", ExpansionConstraint::MaxRelative),
            ("table.cross_join", ExpansionConstraint::MaxRelative),
            ("table.fuzzy_join", ExpansionConstraint::MaxRelative),
            ("table.table_diff", ExpansionConstraint::SumRelative),
            ("table.union_distinct", ExpansionConstraint::SumRelative),
            ("table.semi_join", ExpansionConstraint::LeftRelative),
            ("table.anti_join", ExpansionConstraint::LeftRelative),
            ("table.asof_join", ExpansionConstraint::LeftRelative),
            ("table.except", ExpansionConstraint::LeftRelative),
            ("table.intersect", ExpansionConstraint::LeftRelative),
            (
                "table.assert_foreign_key",
                ExpansionConstraint::LeftRelative,
            ),
            ("geo.sjoin", ExpansionConstraint::MaxRelative),
            ("geo.clip", ExpansionConstraint::MaxRelative),
            ("geo.difference", ExpansionConstraint::MaxRelative),
            ("geo.intersection", ExpansionConstraint::MaxRelative),
            ("geo.overlay", ExpansionConstraint::MaxRelative),
            ("geo.symmetric_difference", ExpansionConstraint::MaxRelative),
            ("geo.nearest", ExpansionConstraint::LeftRelative),
            ("geo.within", ExpansionConstraint::LeftRelative),
            (
                "geo.count_points_in_polygons",
                ExpansionConstraint::LeftRelative,
            ),
            ("geo.union", ExpansionConstraint::SumRelative),
        ];
        for (id, constraint) in expected {
            let op = find_operation(id).expect(id);
            assert_eq!(
                op.arity,
                Arity::BinaryOrdered,
                "{id}: vincolo dichiarato su op non binaria"
            );
            assert_eq!(op.expansion_constraint, *constraint, "{id}");
        }
    }

    #[test]
    fn whole_to_many_exemption_is_declared_in_catalog() {
        // errori-e-limiti.md: la classe di esenzione e' dichiarata in catalogo, non
        // riconosciuta a posteriori — esattamente le op WholeToMany
        // generative/diagnostiche.
        let exempt: HashSet<_> = CATALOG
            .iter()
            .filter(|op| op.expansion_factor_exempt)
            .map(|op| op.id)
            .collect();
        assert_eq!(
            exempt,
            HashSet::from([
                "geo.generate_grid",
                "geo.coverage_validate",
                "geo.shared_paths"
            ])
        );
        for op in CATALOG {
            assert_eq!(
                op.expansion_factor_exempt,
                op.result_shape == Some(ResultShape::WholeToMany),
                "{}: esenzione non allineata alla shape WholeToMany",
                op.id
            );
        }
    }

    #[test]
    // Confronti float esatti intenzionali: le metriche sono rapporti di
    // piccoli interi con rappresentazione binaria esatta (es. 6/5, 6/3);
    // il test verifica il valore per costruzione, non un'approssimazione.
    #[allow(clippy::float_cmp)]
    fn join_expansion_binding_metric_selects_the_declared_constraint() {
        let expansion = JoinExpansion::compute(6, 3, 2);
        assert_eq!(expansion.output_over_sum_inputs, 1.2);
        assert_eq!(expansion.output_over_left, 2.0);
        assert_eq!(expansion.output_over_right, 3.0);
        assert_eq!(
            expansion.binding_metric(ExpansionConstraint::SumRelative),
            1.2
        );
        assert_eq!(
            expansion.binding_metric(ExpansionConstraint::LeftRelative),
            2.0
        );
        assert_eq!(
            expansion.binding_metric(ExpansionConstraint::RightRelative),
            3.0
        );
        assert_eq!(
            expansion.binding_metric(ExpansionConstraint::MaxRelative),
            3.0
        );
        // Denominatore nullo: infinito se l'output e' non nullo, zero se
        // anche l'output e' nullo.
        let from_empty = JoinExpansion::compute(1, 0, 0);
        assert!(from_empty.output_over_left.is_infinite());
        assert!(from_empty.output_over_sum_inputs.is_infinite());
        let all_empty = JoinExpansion::compute(0, 0, 0);
        assert_eq!(
            all_empty.binding_metric(ExpansionConstraint::MaxRelative),
            0.0
        );
    }

    #[test]
    fn la_decisione_binaria_e_esatta_anche_dove_le_metriche_arrotondano() {
        // Quarto giro della review. `left = right = 2^53`, `output = 2^53+1`,
        // fattore 1: il rapporto reale e' > 1, ma `output as f64` arrotonda a
        // 2^53 e la metrica diventa esattamente 1.0 — il limite NON scattava.
        const DUE_53: u64 = 1 << 53;
        let output = DUE_53 + 1;
        let metrica = JoinExpansion::compute(output, DUE_53, DUE_53);
        // La metrica osservabile e' ancora (e resta) arrotondata: e' il
        // motivo per cui non decide piu' lei.
        assert!(
            metrica.binding_metric(ExpansionConstraint::MaxRelative) <= 1.0,
            "il rapporto in f64 dovrebbe collassare su 1.0"
        );
        // La decisione, invece, e' esatta.
        assert!(
            ExpansionConstraint::MaxRelative.exceeded(output, DUE_53, DUE_53, 1.0),
            "un'espansione oltre la soglia deve essere rifiutata anche sopra 2^53"
        );
        // E non e' diventata rigida: la stessa cardinalita' pari alla soglia
        // resta accettata, in tutte le basi.
        for constraint in [
            ExpansionConstraint::MaxRelative,
            ExpansionConstraint::LeftRelative,
            ExpansionConstraint::RightRelative,
        ] {
            assert!(
                !constraint.exceeded(DUE_53, DUE_53, DUE_53, 1.0),
                "{constraint:?}: output uguale alla soglia non e' un superamento"
            );
        }
        // Base «somma degli input»: la somma non deve saturare prima del
        // confronto. Con due lati a `u64::MAX` la soglia vale 2^65, che
        // nessun output a 64 bit puo' superare.
        assert!(!ExpansionConstraint::SumRelative.exceeded(u64::MAX, u64::MAX, u64::MAX, 1.0));
        // Denominatore nullo, coerente con le metriche: output non nullo da
        // input vuoti e' espansione infinita, output nullo non lo e'.
        assert!(ExpansionConstraint::MaxRelative.exceeded(1, 0, 0, 1.0));
        assert!(!ExpansionConstraint::MaxRelative.exceeded(0, 0, 0, 1.0));
        // `Custom` sovrascrive la soglia, non la base.
        assert!(ExpansionConstraint::Custom(2.0).exceeded(7, 3, 0, 100.0));
        assert!(!ExpansionConstraint::Custom(2.0).exceeded(6, 3, 0, 100.0));
        // Un fattore frazionario resta esatto: 3 righe da 2 con soglia 1.5
        // e' il limite, 4 lo supera.
        assert!(!ExpansionConstraint::LeftRelative.exceeded(3, 2, 0, 1.5));
        assert!(ExpansionConstraint::LeftRelative.exceeded(4, 2, 0, 1.5));
    }

    #[test]
    // Come sopra: i confronti esatti sui fattori custom verificano
    // l'uguaglianza per bit richiesta dal fingerprint (piano-v5.md#identita-e-fingerprint/6).
    #[allow(clippy::float_cmp)]
    fn custom_constraint_overrides_the_threshold_not_the_metric() {
        // errori-e-limiti.md: `Custom(fattore)` e' la stima a priori per op la cui
        // semantica di output non ha una base fissa. La metrica vincolante
        // resta `output_over_sum_inputs`; il fattore sovrascrive
        // `max_expansion_factor` come soglia per la singola operazione.
        let expansion = JoinExpansion::compute(6, 3, 2);
        assert_eq!(
            expansion.binding_metric(ExpansionConstraint::Custom(2.5)),
            expansion.output_over_sum_inputs
        );
        assert_eq!(ExpansionConstraint::Custom(2.5).binding_threshold(4.0), 2.5);
        assert_eq!(ExpansionConstraint::MaxRelative.binding_threshold(4.0), 4.0);
        // Uguaglianza per bit: nessuna ambiguita' float nel fingerprint.
        assert_eq!(
            ExpansionConstraint::Custom(2.5),
            ExpansionConstraint::Custom(2.5)
        );
        assert_ne!(
            ExpansionConstraint::Custom(2.5),
            ExpansionConstraint::Custom(2.6)
        );
        assert_ne!(
            ExpansionConstraint::Custom(1.2),
            ExpansionConstraint::SumRelative
        );

        // Sintassi della macro `op!`: `expansion_constraint = Custom(f)`
        // coesiste con le altre chiavi in qualunque ordine. Nessuna op del
        // catalogo v1 la usa (riservata a op future guidate da stime).
        let descriptor = op!(
            "table.__custom_test",
            Table,
            Extension,
            BinaryOrdered,
            BinaryBlocking,
            BoundaryOnly,
            None,
            None,
            &[],
            DefinedOrder,
            KernelValidated,
            expansion_constraint = Custom(2.5),
            kernel_version = 2
        );
        assert_eq!(
            descriptor.expansion_constraint,
            ExpansionConstraint::Custom(2.5)
        );
        assert_eq!(descriptor.kernel_version, 2);
    }

    #[test]
    fn geo_fusion_matches_the_adr_0012_perimeter() {
        // architettura.md#geometrie D12.2 + perimetro M1+M3: esattamente le 16 trasformazioni
        // 1:1 revistate (14 di M1 + reproject/make_valid di M3) sono
        // TransformInPlace, le 5 misure terminali sono TerminalMeasure, TUTTO
        // il resto (tabellari incluse) e' NotFusible. Il campo e'
        // dichiarativo: la lista chiusa qui sotto e' il contratto; aggiungere
        // un op fondibile richiede l'oracolo differenziale (gate di M1).
        let transforms: HashSet<_> = CATALOG
            .iter()
            .filter(|op| op.geo_fusion == GeoFusion::TransformInPlace)
            .map(|op| op.id)
            .collect();
        assert_eq!(
            transforms,
            HashSet::from([
                "geo.buffer",
                "geo.simplify",
                "geo.centroid",
                "geo.convex_hull",
                "geo.envelope",
                "geo.boundary",
                "geo.point_on_surface",
                "geo.affine_transform",
                "geo.translate",
                "geo.scale",
                "geo.rotate",
                "geo.concave_hull",
                "geo.densify",
                "geo.snap_to_grid",
                "geo.reproject",
                "geo.make_valid",
            ])
        );
        let terminals: HashSet<_> = CATALOG
            .iter()
            .filter(|op| op.geo_fusion == GeoFusion::TerminalMeasure)
            .map(|op| op.id)
            .collect();
        assert_eq!(
            terminals,
            HashSet::from([
                "geo.area",
                "geo.length",
                "geo.perimeter",
                "geo.vertex_count",
                "geo.to_wkt",
            ])
        );
        for op in CATALOG {
            // Esplicitamente fuori perimetro (M3 incluso): check di tipo
            // per-riga, candidati a una milestone futura.
            if matches!(op.id, "geo.line_substring" | "geo.line_interpolate_point") {
                assert_eq!(op.geo_fusion, GeoFusion::NotFusible, "{}", op.id);
            }
            // La fondibilita' riguarda solo la famiglia geo: ogni tabellare
            // resta NotFusible e nessuna tabellare puo' dichiararsi fondibile.
            if op.family == Family::Table {
                assert_eq!(
                    op.geo_fusion,
                    GeoFusion::NotFusible,
                    "{} tabellare fondibile",
                    op.id
                );
            }
            // Invariante M1: solo op unarie streaming possono fondersi.
            if op.geo_fusion != GeoFusion::NotFusible {
                assert_eq!(op.family, Family::Geo, "{}", op.id);
                assert_eq!(op.arity, Arity::Unary, "{}", op.id);
                assert_eq!(op.execution_class, ExecutionClass::Streaming, "{}", op.id);
            }
        }
        // Default di macro: senza chiave `geo_fusion` il campo e' NotFusible.
        let filter = find_operation("table.filter").expect("table.filter");
        assert_eq!(filter.geo_fusion, GeoFusion::NotFusible);
    }

    #[test]
    fn geo_fusion_names_are_stable_snake_case() {
        // Nomi usati da capabilities JSON e snapshot di catalogo: stabili per
        // contratto (architettura.md#geometrie D12.2), mai derivati dal `Debug` Rust.
        assert_eq!(GeoFusion::NotFusible.as_str(), "not_fusible");
        assert_eq!(GeoFusion::TransformInPlace.as_str(), "transform_in_place");
        assert_eq!(GeoFusion::TerminalMeasure.as_str(), "terminal_measure");
    }

    /// Config di sonda generiche per l'autorita' row-diagnostics: applicate a
    /// TUTTE le op (non sono una lista di op, coprono lo spazio config
    /// sensibile: target di cast e policy null degli hash).
    fn row_diagnostics_probes() -> Vec<serde_json::Value> {
        let mut probes = vec![serde_json::json!({})];
        for target in [
            "int",
            "float",
            "bool",
            "uint64",
            "date",
            "datetime",
            "date32",
            "timestamp_millis",
            "decimal128",
            "str",
        ] {
            probes.push(serde_json::json!({"target_type": target}));
            probes.push(serde_json::json!({"target_type": target, "errors": "coerce"}));
            probes.push(serde_json::json!({"target_type": target, "errors": "raise"}));
        }
        for policy in ["error", "empty", "literal"] {
            probes.push(serde_json::json!({"null_policy": policy}));
        }
        probes
    }

    #[test]
    fn row_diagnostics_authority_locks_config_sensitive_operations() {
        let type_cast = find_operation("table.type_cast").expect("type_cast");
        for target in [
            "int",
            "float",
            "bool",
            "uint64",
            "date",
            "datetime",
            "date32",
            "timestamp_millis",
            "decimal128",
        ] {
            for errors in [None, Some("coerce"), Some("raise")] {
                let config = errors.map_or_else(
                    || serde_json::json!({"target_type": target}),
                    |mode| serde_json::json!({"target_type": target, "errors": mode}),
                );
                assert!(
                    type_cast.emits_row_diagnostics(&config),
                    "type_cast {target}/{errors:?} rifiuta righe: deve emettere"
                );
            }
        }
        // Target senza conversione fallibile row-scoped: nessuna emissione.
        assert!(!type_cast.emits_row_diagnostics(&serde_json::json!({"target_type": "str"})));
        assert!(!type_cast.emits_row_diagnostics(&serde_json::json!({})));

        // P1-3: md5/sha256 rifiutano row-scoped solo con null_policy=error;
        // le altre policy hanno semantica storica dichiarata.
        for id in ["table.md5_hash", "table.sha256_hash"] {
            let hash = find_operation(id).expect(id);
            assert!(hash.emits_row_diagnostics(&serde_json::json!({"null_policy": "error"})));
            assert!(!hash.emits_row_diagnostics(&serde_json::json!({})));
            assert!(!hash.emits_row_diagnostics(&serde_json::json!({"null_policy": "empty"})));
            assert!(!hash.emits_row_diagnostics(&serde_json::json!({"null_policy": "literal"})));
        }

        // P2: hmac_sha256 non emette MAI (le null_policy legacy producono
        // output dichiarato, nessun rifiuto row-scoped).
        let hmac = find_operation("table.hmac_sha256").expect("hmac");
        for config in [
            serde_json::json!({}),
            serde_json::json!({"null_policy": "error"}),
            serde_json::json!({"null_policy": "empty"}),
            serde_json::json!({"null_policy": "null"}),
            serde_json::json!({"null_policy": "skip"}),
        ] {
            assert!(
                !hmac.emits_row_diagnostics(&config),
                "hmac_sha256 non deve mai emettere diagnostica row-scoped"
            );
        }

        // P0 (drift lock): formula ed expression emettono con qualunque
        // configurazione; se il catalogo smettesse di classificarle il gate
        // legacy tornerebbe bypassabile via sort -> formula/expression.
        assert!(find_operation("table.formula")
            .expect("formula")
            .emits_row_diagnostics(&serde_json::json!({})));
        assert!(find_operation("table.expression")
            .expect("expression")
            .emits_row_diagnostics(&serde_json::json!({})));
    }

    #[test]
    fn row_diagnostics_emitting_operations_are_a_closed_catalog_set() {
        // Mutation/anti-drift: il perimetro delle op che emettono diagnostica
        // row-scoped e' chiuso e contato (16 table diag-kernel senza hmac +
        // 24 geo DAG-dispatchate). Cambiarlo richiede un diff esplicito di
        // questo test e del ledger di copertura.
        let probes = row_diagnostics_probes();
        let emitting: Vec<&str> = CATALOG
            .iter()
            .filter(|op| probes.iter().any(|config| op.emits_row_diagnostics(config)))
            .map(|op| op.id)
            .collect();
        assert_eq!(
            emitting.len(),
            40,
            "perimetro row-diagnostics: {emitting:?}"
        );
        for id in &emitting {
            let descriptor = find_operation(id).expect("risolta");
            assert_eq!(
                descriptor.source_row_provenance(),
                SourceRowProvenance::Preserved,
                "{id}: emette diagnostica ma non preserva la provenance sorgente"
            );
        }
    }

    #[test]
    fn row_diagnostics_changes_carry_the_declared_version_bumps() {
        // piano-v5.md#identita-e-fingerprint: ogni op il cui comportamento osservabile, kernel o gate
        // planner e' cambiato con la diagnostica row-scoped (delta 2026-08-03
        // su baseline af812aa) dichiara il bump nelle componenti di versione.
        // La tabella e' hard-coded dal delta e dalla baseline — nessun valore
        // letto dal catalogo stesso (anti-tautologia): (id, semantic,
        // config_schema, contract_analysis, kernel).
        let expected: &[(&str, u32, u32, u32, u32)] = &[
            // diag-kernel table: nuovo reject_rows / comportamento pubblico.
            ("table.date_extract", 2, 1, 1, 3),
            ("table.flatten_json", 2, 1, 1, 3),
            ("table.type_cast", 2, 1, 1, 3),
            ("table.md5_hash", 2, 1, 1, 2),
            ("table.sha256_hash", 2, 1, 1, 2),
            ("table.assert_not_null", 2, 1, 1, 2),
            ("table.assert_range", 2, 1, 1, 2),
            ("table.assert_regex", 2, 1, 1, 2),
            ("table.assert_unique", 2, 1, 1, 3),
            ("table.assert_foreign_key", 2, 1, 1, 3),
            ("table.date_add", 2, 1, 1, 3),
            ("table.date_diff", 2, 1, 1, 3),
            ("table.date_format", 2, 1, 1, 3),
            ("table.timezone_convert", 2, 1, 1, 3),
            ("table.explode", 2, 1, 1, 2),
            ("table.formula", 2, 1, 1, 3),
            ("table.expression", 3, 2, 2, 4),
            // diag-wkt: raccolta nel kernel geo. La successiva dichiarazione
            // pubblica encoding/types del produttore cambia anche semantica e
            // contract analysis (piano-v5.md#identita-e-fingerprint/piano-v5.md#contratti-di-input).
            ("geo.from_wkt", 3, 1, 2, 2),
            // diag-transport / diag-coords: il rifiuto row-scoped ora porta
            // il payload `plenora-row-diagnostics-v1` (comportamento
            // osservabile; kernel invariato -> bump semantico soltanto).
            ("geo.affine_transform", 2, 1, 1, 1),
            ("geo.area", 2, 1, 1, 1),
            ("geo.boundary", 2, 1, 1, 1),
            ("geo.bounds_extractor", 2, 1, 1, 1),
            ("geo.buffer", 2, 1, 1, 1),
            ("geo.centroid", 2, 1, 1, 1),
            ("geo.concave_hull", 2, 1, 1, 1),
            ("geo.convex_hull", 2, 1, 1, 1),
            ("geo.densify", 2, 1, 1, 1),
            ("geo.envelope", 2, 1, 1, 1),
            ("geo.from_coords", 2, 1, 1, 1),
            ("geo.geodesic_area", 2, 1, 1, 1),
            ("geo.geodesic_line_length", 2, 1, 1, 1),
            ("geo.length", 2, 1, 1, 1),
            ("geo.line_interpolate_point", 2, 1, 1, 1),
            ("geo.line_substring", 2, 1, 1, 1),
            ("geo.make_valid", 2, 1, 1, 1),
            ("geo.perimeter", 2, 1, 1, 1),
            ("geo.point_on_surface", 2, 1, 1, 1),
            ("geo.reproject", 2, 1, 1, 1),
            ("geo.rotate", 2, 1, 1, 1),
            ("geo.scale", 2, 1, 1, 1),
            ("geo.simplify", 2, 1, 1, 1),
            ("geo.snap_to_grid", 2, 1, 1, 1),
            ("geo.to_wkt", 2, 1, 1, 1),
            ("geo.translate", 2, 1, 1, 1),
            ("geo.vertex_count", 2, 1, 1, 1),
        ];
        for (id, semantic, config_schema, contract_analysis, kernel) in expected {
            let descriptor = find_operation(id).expect(id);
            assert_eq!(
                (
                    descriptor.semantic_version,
                    descriptor.config_schema_version,
                    descriptor.contract_analysis_version,
                    descriptor.kernel_version,
                ),
                (*semantic, *config_schema, *contract_analysis, *kernel),
                "{id}: versioni non allineate al bump dichiarato (piano-v5.md#identita-e-fingerprint)"
            );
        }
    }
}
