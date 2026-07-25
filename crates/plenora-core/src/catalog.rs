//! Catalogo unificato delle operazioni (Architetture.md par. 4.3, ADR 4).
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

/// Comportamento alla cancellazione (ADR 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CancellationBehavior {
    Cooperative,
    BoundaryOnly,
    NonInterruptible,
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

/// Requisito CRS (solo operazioni geo).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CrsRequirement {
    Known,
    Projected,
    Geographic,
    SameProjected,
    Reprojection,
}

/// Politica di determinismo per operazioni con ordine non definito (ADR 1).
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

/// Contratto machine-readable di un'operazione (ADR 4).
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
    pub result_shape: Option<ResultShape>,
    pub crs_requirement: Option<CrsRequirement>,
    /// Backend/feature richiesti (es. `geos`, `proj`).
    pub required_capabilities: &'static [&'static str],
    pub determinism: DeterminismPolicy,
    pub maturity: Maturity,
    // Versioni esplicite per-componente (ADR 4): disciplina di incremento in CI.
    pub semantic_version: u32,
    pub config_schema_version: u32,
    pub contract_analysis_version: u32,
    pub kernel_version: u32,
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
//   capability esterna (`geos`/`proj`, chiamate monolitiche, ADR 3);
// - `result_shape`: `BinaryLineage` del sorgente geo non ha variante
//   equivalente: mappato su `OneToMany` (un left puo' produrre piu' righe);
// - `determinism`: `DefinedOrder` di default; `CanonicalOrder` per le set
//   operation tabellari e le aggregazioni senza ordine; `InputOrder` per
//   `concat` (ordine di arrivo dei rami).
// ---------------------------------------------------------------------------

macro_rules! op {
    // Nessuna versione esplicita: tutte e 4 le componenti a 1 (ADR 4).
    ($id:literal, $family:ident, $origin:ident, $arity:ident, $exec:ident,
     $cancel:ident, $shape:expr, $crs:expr, $caps:expr, $det:ident, $mat:ident) => {
        op!($id, $family, $origin, $arity, $exec, $cancel, $shape, $crs, $caps, $det, $mat,
            kernel_version = 1)
    };
    // Variante con versioni esplicite: `semantic_version`,
    // `config_schema_version`, `contract_analysis_version` e `kernel_version`
    // sono tutte opzionali (default 1) e ammesse in qualsiasi combinazione e
    // ordine; chiave duplicata o sconosciuta -> errore di compilazione.
    ($id:literal, $family:ident, $origin:ident, $arity:ident, $exec:ident,
     $cancel:ident, $shape:expr, $crs:expr, $caps:expr, $det:ident, $mat:ident,
     $($versions:tt)+) => {
        op!(@munch
            ($id, $family, $origin, $arity, $exec, $cancel, $shape, $crs, $caps, $det, $mat)
            (1, 1, 1, 1)
            $($versions)+)
    };
    // Muncher: consuma una chiave per passo aggiornando l'accumulatore
    // (semantic, config_schema, contract_analysis, kernel).
    (@munch ($($base:tt)*) ($s:expr, $c:expr, $a:expr, $k:expr)
        semantic_version = $v:expr) => {
        op!(@build ($($base)*) ($v, $c, $a, $k))
    };
    (@munch ($($base:tt)*) ($s:expr, $c:expr, $a:expr, $k:expr)
        semantic_version = $v:expr, $($rest:tt)+) => {
        op!(@munch ($($base)*) ($v, $c, $a, $k) $($rest)+)
    };
    (@munch ($($base:tt)*) ($s:expr, $c:expr, $a:expr, $k:expr)
        config_schema_version = $v:expr) => {
        op!(@build ($($base)*) ($s, $v, $a, $k))
    };
    (@munch ($($base:tt)*) ($s:expr, $c:expr, $a:expr, $k:expr)
        config_schema_version = $v:expr, $($rest:tt)+) => {
        op!(@munch ($($base)*) ($s, $v, $a, $k) $($rest)+)
    };
    (@munch ($($base:tt)*) ($s:expr, $c:expr, $a:expr, $k:expr)
        contract_analysis_version = $v:expr) => {
        op!(@build ($($base)*) ($s, $c, $v, $k))
    };
    (@munch ($($base:tt)*) ($s:expr, $c:expr, $a:expr, $k:expr)
        contract_analysis_version = $v:expr, $($rest:tt)+) => {
        op!(@munch ($($base)*) ($s, $c, $v, $k) $($rest)+)
    };
    (@munch ($($base:tt)*) ($s:expr, $c:expr, $a:expr, $k:expr)
        kernel_version = $v:expr) => {
        op!(@build ($($base)*) ($s, $c, $a, $v))
    };
    (@munch ($($base:tt)*) ($s:expr, $c:expr, $a:expr, $k:expr)
        kernel_version = $v:expr, $($rest:tt)+) => {
        op!(@munch ($($base)*) ($s, $c, $a, $v) $($rest)+)
    };
    (@build ($id:literal, $family:ident, $origin:ident, $arity:ident, $exec:ident,
     $cancel:ident, $shape:expr, $crs:expr, $caps:expr, $det:ident, $mat:ident)
     ($semantic:expr, $config_schema:expr, $contract_analysis:expr, $kernel:expr)) => {
        OperationDescriptor {
            id: $id,
            family: Family::$family,
            origin: Origin::$origin,
            arity: Arity::$arity,
            execution_class: ExecutionClass::$exec,
            cancellation_behavior: CancellationBehavior::$cancel,
            result_shape: $shape,
            crs_requirement: $crs,
            required_capabilities: $caps,
            determinism: DeterminismPolicy::$det,
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
    op!("table.add_row_number", Table, ManipolaCompat, Unary, Blocking, BoundaryOnly, None, None, &[], DefinedOrder, PublicProtocol),
    op!("table.aggregate", Table, ManipolaCompat, Unary, Blocking, BoundaryOnly, None, None, &[], CanonicalOrder, PublicProtocol, kernel_version = 2),
    op!("table.bin", Table, ManipolaCompat, Unary, Blocking, BoundaryOnly, None, None, &[], DefinedOrder, PublicProtocol),
    op!("table.concat", Table, ManipolaCompat, NAry, Blocking, BoundaryOnly, None, None, &[], InputOrder, PublicProtocol),
    op!("table.concat_columns", Table, ManipolaCompat, Unary, Streaming, Cooperative, None, None, &[], DefinedOrder, PublicProtocol),
    op!("table.conditional", Table, ManipolaCompat, Unary, Streaming, Cooperative, None, None, &[], DefinedOrder, PublicProtocol),
    op!("table.cross_join", Table, ManipolaCompat, BinaryOrdered, BinaryBlocking, BoundaryOnly, None, None, &[], DefinedOrder, PublicProtocol),
    op!("table.date_extract", Table, ManipolaCompat, Unary, Streaming, Cooperative, None, None, &[], DefinedOrder, PublicProtocol, kernel_version = 2),
    op!("table.dedup_advanced", Table, ManipolaCompat, Unary, Blocking, BoundaryOnly, None, None, &[], CanonicalOrder, PublicProtocol, kernel_version = 2),
    op!("table.distinct", Table, ManipolaCompat, Unary, Blocking, BoundaryOnly, None, None, &[], CanonicalOrder, PublicProtocol, kernel_version = 2),
    op!("table.drop_columns", Table, ManipolaCompat, Unary, Streaming, Cooperative, None, None, &[], DefinedOrder, PublicProtocol),
    op!("table.fill_na", Table, ManipolaCompat, Unary, Streaming, Cooperative, None, None, &[], DefinedOrder, PublicProtocol, kernel_version = 2),
    op!("table.filter", Table, ManipolaCompat, Unary, Streaming, Cooperative, None, None, &[], DefinedOrder, PublicProtocol, kernel_version = 2),
    op!("table.flatten_json", Table, ManipolaCompat, Unary, Streaming, Cooperative, None, None, &[], DefinedOrder, PublicProtocol, kernel_version = 2),
    op!("table.formula", Table, ManipolaCompat, Unary, Streaming, Cooperative, None, None, &[], DefinedOrder, PublicProtocol, kernel_version = 2),
    op!("table.join", Table, ManipolaCompat, BinaryOrdered, BinaryBlocking, BoundaryOnly, None, None, &[], DefinedOrder, PublicProtocol, kernel_version = 2),
    op!("table.lookup", Table, ManipolaCompat, Unary, Streaming, Cooperative, None, None, &[], DefinedOrder, PublicProtocol),
    op!("table.melt", Table, ManipolaCompat, Unary, Blocking, BoundaryOnly, None, None, &[], DefinedOrder, PublicProtocol, kernel_version = 2),
    op!("table.pivot", Table, ManipolaCompat, Unary, Blocking, BoundaryOnly, None, None, &[], DefinedOrder, PublicProtocol, kernel_version = 2),
    op!("table.rename", Table, ManipolaCompat, Unary, Streaming, Cooperative, None, None, &[], DefinedOrder, PublicProtocol),
    op!("table.reorder_columns", Table, ManipolaCompat, Unary, Streaming, Cooperative, None, None, &[], DefinedOrder, PublicProtocol),
    op!("table.replace", Table, ManipolaCompat, Unary, Streaming, Cooperative, None, None, &[], DefinedOrder, PublicProtocol),
    op!("table.sample", Table, ManipolaCompat, Unary, Blocking, BoundaryOnly, None, None, &[], DefinedOrder, PublicProtocol),
    op!("table.sort", Table, ManipolaCompat, Unary, Blocking, BoundaryOnly, None, None, &[], DefinedOrder, PublicProtocol, kernel_version = 2),
    op!("table.split_column", Table, ManipolaCompat, Unary, Streaming, Cooperative, None, None, &[], DefinedOrder, PublicProtocol),
    op!("table.statistics", Table, ManipolaCompat, Unary, Blocking, BoundaryOnly, None, None, &[], DefinedOrder, PublicProtocol, kernel_version = 2),
    op!("table.string_extract", Table, ManipolaCompat, Unary, Streaming, Cooperative, None, None, &[], DefinedOrder, PublicProtocol, kernel_version = 2),
    op!("table.string_length", Table, ManipolaCompat, Unary, Streaming, Cooperative, None, None, &[], DefinedOrder, PublicProtocol),
    op!("table.string_pad", Table, ManipolaCompat, Unary, Streaming, Cooperative, None, None, &[], DefinedOrder, PublicProtocol),
    op!("table.table_diff", Table, ManipolaCompat, BinaryOrdered, BinaryBlocking, BoundaryOnly, None, None, &[], DefinedOrder, PublicProtocol, kernel_version = 2),
    op!("table.text_normalize", Table, ManipolaCompat, Unary, Streaming, Cooperative, None, None, &[], DefinedOrder, PublicProtocol, kernel_version = 2),
    op!("table.transpose", Table, ManipolaCompat, Unary, Blocking, BoundaryOnly, None, None, &[], DefinedOrder, PublicProtocol),
    op!("table.type_cast", Table, ManipolaCompat, Unary, Streaming, Cooperative, None, None, &[], DefinedOrder, PublicProtocol, kernel_version = 2),
    op!("table.uuid_generator", Table, ManipolaCompat, Unary, Streaming, Cooperative, None, None, &[], DefinedOrder, PublicProtocol),
    op!("table.window_function", Table, ManipolaCompat, Unary, Blocking, BoundaryOnly, None, None, &[], DefinedOrder, PublicProtocol, kernel_version = 2),
    op!("table.mask_data", Table, ManipolaCompat, Unary, Streaming, Cooperative, None, None, &[], DefinedOrder, PublicProtocol, kernel_version = 2),
    op!("table.md5_hash", Table, ManipolaCompat, Unary, Streaming, Cooperative, None, None, &[], DefinedOrder, PublicProtocol),
    // --- Tabellari estensioni (25) -----------------------------------------
    op!("table.anti_join", Table, Extension, BinaryOrdered, BinaryBlocking, BoundaryOnly, None, None, &[], DefinedOrder, PublicProtocol, kernel_version = 2),
    op!("table.asof_join", Table, Extension, BinaryOrdered, BinaryBlocking, BoundaryOnly, None, None, &[], DefinedOrder, PublicProtocol),
    op!("table.assert_not_null", Table, Extension, Unary, Streaming, Cooperative, None, None, &[], DefinedOrder, PublicProtocol),
    op!("table.assert_range", Table, Extension, Unary, Streaming, Cooperative, None, None, &[], DefinedOrder, PublicProtocol),
    op!("table.assert_regex", Table, Extension, Unary, Streaming, Cooperative, None, None, &[], DefinedOrder, PublicProtocol),
    op!("table.assert_schema", Table, Extension, Unary, Streaming, Cooperative, None, None, &[], DefinedOrder, PublicProtocol),
    op!("table.assert_unique", Table, Extension, Unary, Blocking, BoundaryOnly, None, None, &[], DefinedOrder, PublicProtocol),
    op!("table.coalesce", Table, Extension, Unary, Streaming, Cooperative, None, None, &[], DefinedOrder, PublicProtocol, kernel_version = 2),
    op!("table.date_add", Table, Extension, Unary, Streaming, Cooperative, None, None, &[], DefinedOrder, PublicProtocol, kernel_version = 2),
    op!("table.date_diff", Table, Extension, Unary, Streaming, Cooperative, None, None, &[], DefinedOrder, PublicProtocol, kernel_version = 2),
    op!("table.date_format", Table, Extension, Unary, Streaming, Cooperative, None, None, &[], DefinedOrder, PublicProtocol, kernel_version = 2),
    op!("table.except", Table, Extension, BinaryOrdered, BinaryBlocking, BoundaryOnly, None, None, &[], CanonicalOrder, PublicProtocol, kernel_version = 2),
    op!("table.explode", Table, Extension, Unary, Blocking, BoundaryOnly, None, None, &[], DefinedOrder, PublicProtocol),
    op!("table.intersect", Table, Extension, BinaryOrdered, BinaryBlocking, BoundaryOnly, None, None, &[], CanonicalOrder, PublicProtocol, kernel_version = 2),
    op!("table.rolling_window", Table, Extension, Unary, Blocking, BoundaryOnly, None, None, &[], DefinedOrder, PublicProtocol, kernel_version = 2),
    op!("table.semi_join", Table, Extension, BinaryOrdered, BinaryBlocking, BoundaryOnly, None, None, &[], DefinedOrder, PublicProtocol, kernel_version = 2),
    op!("table.sha256_hash", Table, Extension, Unary, Streaming, Cooperative, None, None, &[], DefinedOrder, PublicProtocol),
    op!("table.timezone_convert", Table, Extension, Unary, Streaming, Cooperative, None, None, &[], DefinedOrder, PublicProtocol, kernel_version = 2),
    op!("table.union_distinct", Table, Extension, BinaryOrdered, BinaryBlocking, BoundaryOnly, None, None, &[], CanonicalOrder, PublicProtocol, kernel_version = 2),
    op!("table.unnest", Table, Extension, Unary, Streaming, Cooperative, None, None, &[], DefinedOrder, PublicProtocol),
    // expression v2 (Fase estensione funzioni/temporali): nuove funzioni
    // (substring, regex_replace, between, in, greatest, least, floor, ceil,
    // power) e date_trunc con output Date32/TimestampMs nativi -> tutte e 4
    // le versioni incrementate (ADR 4).
    op!("table.expression", Table, Extension, Unary, Streaming, Cooperative, None, None, &[], DefinedOrder, PublicProtocol,
        semantic_version = 2, config_schema_version = 2, contract_analysis_version = 2, kernel_version = 3),
    op!("table.assert_cardinality", Table, Extension, Unary, Streaming, Cooperative, None, None, &[], DefinedOrder, PublicProtocol),
    op!("table.assert_metadata", Table, Extension, Unary, Streaming, Cooperative, None, None, &[], DefinedOrder, PublicProtocol),
    op!("table.assert_foreign_key", Table, Extension, BinaryOrdered, BinaryBlocking, BoundaryOnly, None, None, &[], DefinedOrder, PublicProtocol, kernel_version = 2),
    op!("table.reconcile", Table, Extension, BinaryOrdered, BinaryBlocking, BoundaryOnly, None, None, &[], DefinedOrder, PublicProtocol, kernel_version = 2),
    // --- Geografiche Manipola-compat (33) -----------------------------------
    op!("geo.centroid", Geo, ManipolaCompat, Unary, Streaming, Cooperative, Some(ResultShape::OneToOne), Some(CrsRequirement::Projected), &[], DefinedOrder, PublicProtocol),
    op!("geo.convex_hull", Geo, ManipolaCompat, Unary, Streaming, Cooperative, Some(ResultShape::OneToOne), Some(CrsRequirement::Projected), &[], DefinedOrder, PublicProtocol),
    op!("geo.envelope", Geo, ManipolaCompat, Unary, Streaming, Cooperative, Some(ResultShape::OneToOne), Some(CrsRequirement::Projected), &[], DefinedOrder, PublicProtocol),
    op!("geo.sjoin", Geo, ManipolaCompat, BinaryOrdered, BinaryBlocking, BoundaryOnly, Some(ResultShape::OneToMany), Some(CrsRequirement::SameProjected), &[], DefinedOrder, PublicProtocol),
    op!("geo.area", Geo, ManipolaCompat, Unary, Streaming, Cooperative, Some(ResultShape::OneToOne), Some(CrsRequirement::Projected), &[], DefinedOrder, KernelValidated),
    op!("geo.boundary", Geo, ManipolaCompat, Unary, Streaming, Cooperative, Some(ResultShape::OneToOne), Some(CrsRequirement::Projected), &[], DefinedOrder, KernelValidated),
    op!("geo.bounds_extractor", Geo, ManipolaCompat, Unary, Streaming, Cooperative, Some(ResultShape::OneToOne), Some(CrsRequirement::Projected), &[], DefinedOrder, KernelValidated),
    op!("geo.buffer", Geo, ManipolaCompat, Unary, Streaming, Cooperative, Some(ResultShape::OneToOne), Some(CrsRequirement::Projected), &[], DefinedOrder, KernelValidated),
    op!("geo.clean_topology", Geo, ManipolaCompat, Unary, Blocking, BoundaryOnly, Some(ResultShape::OneToMany), Some(CrsRequirement::SameProjected), &[], DefinedOrder, KernelValidated),
    op!("geo.clip", Geo, ManipolaCompat, BinaryOrdered, BinaryBlocking, BoundaryOnly, Some(ResultShape::OneToMany), Some(CrsRequirement::SameProjected), &[], DefinedOrder, KernelValidated),
    op!("geo.count_points_in_polygons", Geo, ManipolaCompat, BinaryOrdered, BinaryBlocking, BoundaryOnly, Some(ResultShape::OneToMany), Some(CrsRequirement::SameProjected), &[], DefinedOrder, KernelValidated),
    op!("geo.difference", Geo, ManipolaCompat, BinaryOrdered, BinaryBlocking, BoundaryOnly, Some(ResultShape::OneToMany), Some(CrsRequirement::SameProjected), &[], DefinedOrder, KernelValidated),
    op!("geo.dissolve", Geo, ManipolaCompat, Unary, Blocking, BoundaryOnly, Some(ResultShape::ManyToOne), Some(CrsRequirement::Projected), &[], CanonicalOrder, KernelValidated),
    op!("geo.distance", Geo, ManipolaCompat, Unary, Streaming, Cooperative, Some(ResultShape::OneToOne), Some(CrsRequirement::SameProjected), &[], DefinedOrder, KernelValidated),
    op!("geo.explode", Geo, ManipolaCompat, Unary, Streaming, Cooperative, Some(ResultShape::OneToMany), Some(CrsRequirement::Known), &[], DefinedOrder, KernelValidated),
    op!("geo.from_coords", Geo, ManipolaCompat, Unary, Streaming, Cooperative, Some(ResultShape::FromCoords), Some(CrsRequirement::Projected), &[], DefinedOrder, KernelValidated),
    op!("geo.intersection", Geo, ManipolaCompat, BinaryOrdered, BinaryBlocking, BoundaryOnly, Some(ResultShape::OneToMany), Some(CrsRequirement::SameProjected), &[], DefinedOrder, KernelValidated),
    op!("geo.length", Geo, ManipolaCompat, Unary, Streaming, Cooperative, Some(ResultShape::OneToOne), Some(CrsRequirement::Projected), &[], DefinedOrder, KernelValidated),
    op!("geo.line_builder", Geo, ManipolaCompat, Unary, Blocking, BoundaryOnly, Some(ResultShape::ManyToOne), Some(CrsRequirement::Projected), &[], DefinedOrder, KernelValidated),
    op!("geo.nearest", Geo, ManipolaCompat, BinaryOrdered, BinaryBlocking, BoundaryOnly, Some(ResultShape::OneToMany), Some(CrsRequirement::SameProjected), &[], DefinedOrder, KernelValidated),
    op!("geo.overlay", Geo, ManipolaCompat, BinaryOrdered, BinaryBlocking, BoundaryOnly, Some(ResultShape::OneToMany), Some(CrsRequirement::SameProjected), &[], DefinedOrder, KernelValidated),
    op!("geo.perimeter", Geo, ManipolaCompat, Unary, Streaming, Cooperative, Some(ResultShape::OneToOne), Some(CrsRequirement::Projected), &[], DefinedOrder, KernelValidated),
    op!("geo.point_on_surface", Geo, ManipolaCompat, Unary, Streaming, Cooperative, Some(ResultShape::OneToOne), Some(CrsRequirement::Projected), &[], DefinedOrder, KernelValidated),
    op!("geo.polygon_builder", Geo, ManipolaCompat, Unary, Blocking, BoundaryOnly, Some(ResultShape::ManyToOne), Some(CrsRequirement::Projected), &[], DefinedOrder, KernelValidated),
    op!("geo.simplify", Geo, ManipolaCompat, Unary, Streaming, Cooperative, Some(ResultShape::OneToOne), Some(CrsRequirement::Projected), &[], DefinedOrder, KernelValidated),
    op!("geo.symmetric_difference", Geo, ManipolaCompat, BinaryOrdered, BinaryBlocking, BoundaryOnly, Some(ResultShape::OneToMany), Some(CrsRequirement::SameProjected), &[], DefinedOrder, KernelValidated),
    op!("geo.to_wkt", Geo, ManipolaCompat, Unary, Streaming, Cooperative, Some(ResultShape::OneToOne), Some(CrsRequirement::Known), &[], DefinedOrder, KernelValidated),
    op!("geo.union", Geo, ManipolaCompat, BinaryOrdered, BinaryBlocking, BoundaryOnly, Some(ResultShape::OneToMany), Some(CrsRequirement::SameProjected), &[], DefinedOrder, KernelValidated),
    op!("geo.vertex_count", Geo, ManipolaCompat, Unary, Streaming, Cooperative, Some(ResultShape::OneToOne), Some(CrsRequirement::Known), &[], DefinedOrder, KernelValidated),
    op!("geo.voronoi", Geo, ManipolaCompat, Unary, Blocking, BoundaryOnly, Some(ResultShape::OneToMany), Some(CrsRequirement::Projected), &[], DefinedOrder, KernelValidated),
    op!("geo.within", Geo, ManipolaCompat, BinaryOrdered, BinaryBlocking, BoundaryOnly, Some(ResultShape::OneToMany), Some(CrsRequirement::SameProjected), &[], DefinedOrder, KernelValidated),
    op!("geo.make_valid", Geo, ManipolaCompat, Unary, Streaming, NonInterruptible, Some(ResultShape::OneToOne), Some(CrsRequirement::Known), &["geos"], DefinedOrder, BackendPending),
    op!("geo.reproject", Geo, ManipolaCompat, Unary, Streaming, NonInterruptible, Some(ResultShape::OneToOne), Some(CrsRequirement::Reprojection), &["proj"], DefinedOrder, BackendPending),
    // --- Predicati DE-9IM, estensioni geo (11) ------------------------------
    op!("geo.predicate_intersects", Geo, Extension, Unary, Streaming, Cooperative, Some(ResultShape::OneToOne), Some(CrsRequirement::SameProjected), &[], DefinedOrder, KernelValidated),
    op!("geo.predicate_disjoint", Geo, Extension, Unary, Streaming, Cooperative, Some(ResultShape::OneToOne), Some(CrsRequirement::SameProjected), &[], DefinedOrder, KernelValidated),
    op!("geo.predicate_contains", Geo, Extension, Unary, Streaming, Cooperative, Some(ResultShape::OneToOne), Some(CrsRequirement::SameProjected), &[], DefinedOrder, KernelValidated),
    op!("geo.predicate_within", Geo, Extension, Unary, Streaming, Cooperative, Some(ResultShape::OneToOne), Some(CrsRequirement::SameProjected), &[], DefinedOrder, KernelValidated),
    op!("geo.predicate_equals_topo", Geo, Extension, Unary, Streaming, Cooperative, Some(ResultShape::OneToOne), Some(CrsRequirement::SameProjected), &[], DefinedOrder, KernelValidated),
    op!("geo.predicate_covers", Geo, Extension, Unary, Streaming, Cooperative, Some(ResultShape::OneToOne), Some(CrsRequirement::SameProjected), &[], DefinedOrder, KernelValidated),
    op!("geo.predicate_covered_by", Geo, Extension, Unary, Streaming, Cooperative, Some(ResultShape::OneToOne), Some(CrsRequirement::SameProjected), &[], DefinedOrder, KernelValidated),
    op!("geo.predicate_contains_properly", Geo, Extension, Unary, Streaming, Cooperative, Some(ResultShape::OneToOne), Some(CrsRequirement::SameProjected), &[], DefinedOrder, KernelValidated),
    op!("geo.predicate_touches", Geo, Extension, Unary, Streaming, Cooperative, Some(ResultShape::OneToOne), Some(CrsRequirement::SameProjected), &[], DefinedOrder, KernelValidated),
    op!("geo.predicate_crosses", Geo, Extension, Unary, Streaming, Cooperative, Some(ResultShape::OneToOne), Some(CrsRequirement::SameProjected), &[], DefinedOrder, KernelValidated),
    op!("geo.predicate_overlaps", Geo, Extension, Unary, Streaming, Cooperative, Some(ResultShape::OneToOne), Some(CrsRequirement::SameProjected), &[], DefinedOrder, KernelValidated),
    // --- Estensioni geo (21) -------------------------------------------------
    op!("geo.affine_transform", Geo, Extension, Unary, Streaming, Cooperative, Some(ResultShape::OneToOne), Some(CrsRequirement::Projected), &[], DefinedOrder, KernelValidated),
    op!("geo.translate", Geo, Extension, Unary, Streaming, Cooperative, Some(ResultShape::OneToOne), Some(CrsRequirement::Projected), &[], DefinedOrder, KernelValidated),
    op!("geo.scale", Geo, Extension, Unary, Streaming, Cooperative, Some(ResultShape::OneToOne), Some(CrsRequirement::Projected), &[], DefinedOrder, KernelValidated),
    op!("geo.rotate", Geo, Extension, Unary, Streaming, Cooperative, Some(ResultShape::OneToOne), Some(CrsRequirement::Projected), &[], DefinedOrder, KernelValidated),
    op!("geo.concave_hull", Geo, Extension, Unary, Streaming, Cooperative, Some(ResultShape::OneToOne), Some(CrsRequirement::Projected), &[], DefinedOrder, KernelValidated),
    op!("geo.hausdorff_distance", Geo, Extension, Unary, Streaming, Cooperative, Some(ResultShape::OneToOne), Some(CrsRequirement::SameProjected), &[], DefinedOrder, KernelValidated),
    op!("geo.haversine_distance", Geo, Extension, Unary, Streaming, Cooperative, Some(ResultShape::OneToOne), Some(CrsRequirement::Geographic), &[], DefinedOrder, KernelValidated),
    op!("geo.geodesic_distance", Geo, Extension, Unary, Streaming, Cooperative, Some(ResultShape::OneToOne), Some(CrsRequirement::Geographic), &[], DefinedOrder, KernelValidated),
    op!("geo.geodesic_line_length", Geo, Extension, Unary, Streaming, Cooperative, Some(ResultShape::OneToOne), Some(CrsRequirement::Geographic), &[], DefinedOrder, KernelValidated),
    op!("geo.densify", Geo, Extension, Unary, Streaming, Cooperative, Some(ResultShape::OneToOne), Some(CrsRequirement::Projected), &[], DefinedOrder, KernelValidated),
    op!("geo.snap_to_grid", Geo, Extension, Unary, Streaming, Cooperative, Some(ResultShape::OneToOne), Some(CrsRequirement::Projected), &[], DefinedOrder, KernelValidated),
    op!("geo.delaunay", Geo, Extension, Unary, Blocking, BoundaryOnly, Some(ResultShape::OneToMany), Some(CrsRequirement::Projected), &[], DefinedOrder, KernelValidated),
    op!("geo.polygonize", Geo, Extension, Unary, Blocking, NonInterruptible, Some(ResultShape::ManyToOne), Some(CrsRequirement::Projected), &["geos"], DefinedOrder, BackendPending),
    op!("geo.line_merge", Geo, Extension, Unary, Blocking, BoundaryOnly, Some(ResultShape::ManyToOne), Some(CrsRequirement::Projected), &[], DefinedOrder, KernelValidated),
    op!("geo.split", Geo, Extension, Unary, Streaming, NonInterruptible, Some(ResultShape::OneToMany), Some(CrsRequirement::SameProjected), &["geos"], DefinedOrder, BackendPending),
    op!("geo.line_substring", Geo, Extension, Unary, Streaming, Cooperative, Some(ResultShape::OneToOne), Some(CrsRequirement::Projected), &[], DefinedOrder, KernelValidated),
    op!("geo.line_interpolate_point", Geo, Extension, Unary, Streaming, Cooperative, Some(ResultShape::OneToOne), Some(CrsRequirement::Projected), &[], DefinedOrder, KernelValidated),
    op!("geo.frechet_distance", Geo, Extension, Unary, Streaming, Cooperative, Some(ResultShape::OneToOne), Some(CrsRequirement::SameProjected), &[], DefinedOrder, KernelValidated),
    op!("geo.bearing", Geo, Extension, Unary, Streaming, Cooperative, Some(ResultShape::OneToOne), Some(CrsRequirement::Geographic), &[], DefinedOrder, KernelValidated),
    op!("geo.geodesic_area", Geo, Extension, Unary, Streaming, Cooperative, Some(ResultShape::OneToOne), Some(CrsRequirement::Geographic), &[], DefinedOrder, KernelValidated),
    op!("geo.geometry_diagnostics", Geo, Extension, Unary, Streaming, Cooperative, Some(ResultShape::Diagnostic), Some(CrsRequirement::Known), &[], DefinedOrder, KernelValidated),
    // --- Estensioni geo v1.1 (4) ---------------------------------------------
    op!("geo.from_wkt", Geo, Extension, Unary, Streaming, Cooperative, Some(ResultShape::FromCoords), Some(CrsRequirement::Known), &[], DefinedOrder, KernelValidated),
    op!("geo.geometry_accessors", Geo, Extension, Unary, Streaming, Cooperative, Some(ResultShape::OneToOne), Some(CrsRequirement::Known), &[], DefinedOrder, KernelValidated),
    op!("geo.collect", Geo, Extension, Unary, Blocking, BoundaryOnly, Some(ResultShape::ManyToOne), Some(CrsRequirement::Known), &[], CanonicalOrder, KernelValidated),
    op!("geo.line_locate_point", Geo, Extension, Unary, Streaming, Cooperative, Some(ResultShape::OneToOne), Some(CrsRequirement::Known), &[], DefinedOrder, KernelValidated),
    // --- Estensioni geo v1.2 (3) ---------------------------------------------
    op!("geo.generate_grid", Geo, Extension, Unary, Blocking, BoundaryOnly, Some(ResultShape::WholeToMany), Some(CrsRequirement::Known), &[], DefinedOrder, KernelValidated),
    op!("geo.subdivide", Geo, Extension, Unary, Streaming, Cooperative, Some(ResultShape::OneToMany), Some(CrsRequirement::Known), &[], DefinedOrder, KernelValidated),
    // `snap`: il riferimento da config (`reference_wkb`) e' assunto nello
    // stesso CRS dell'input (convenzione D16): requisito SameProjected per
    // l'unica colonna, come le distanze "unarie".
    op!("geo.snap", Geo, Extension, Unary, Streaming, Cooperative, Some(ResultShape::OneToOne), Some(CrsRequirement::SameProjected), &[], DefinedOrder, KernelValidated),
    // --- Estensioni geo v1.3 (3) ---------------------------------------------
    // Coperture poligonali (piantine di edifici): entrambe consumano l'intero
    // input (Blocking) e producono una riga per issue/tratto condiviso
    // (WholeToMany, schema nuovo); aree e lunghezze in unita' di mappa,
    // quindi SameProjected.
    op!("geo.coverage_validate", Geo, Extension, Unary, Blocking, BoundaryOnly, Some(ResultShape::WholeToMany), Some(CrsRequirement::SameProjected), &[], DefinedOrder, KernelValidated),
    op!("geo.shared_paths", Geo, Extension, Unary, Blocking, BoundaryOnly, Some(ResultShape::WholeToMany), Some(CrsRequirement::SameProjected), &[], DefinedOrder, KernelValidated),
    // `cluster_dbscan`: clustering globale per densita' (vicinati R-tree
    // sull'intero input) ma output allineato alle righe (un'etichetta UInt64
    // nullable per riga, noise -> null): Blocking con shape OneToOne; eps in
    // unita' di mappa, quindi Projected.
    op!("geo.cluster_dbscan", Geo, Extension, Unary, Blocking, BoundaryOnly, Some(ResultShape::OneToOne), Some(CrsRequirement::Projected), &[], DefinedOrder, KernelValidated),
    // --- Estensioni table v1.1 (4) -------------------------------------------
    op!("table.limit", Table, Extension, Unary, Streaming, Cooperative, None, None, &[], InputOrder, KernelValidated),
    op!("table.select_columns", Table, Extension, Unary, Streaming, Cooperative, None, None, &[], DefinedOrder, KernelValidated),
    op!("table.stable_fingerprint", Table, Extension, Unary, Streaming, Cooperative, None, None, &[], DefinedOrder, KernelValidated),
    op!("table.top_n", Table, Extension, Unary, Blocking, BoundaryOnly, None, None, &[], DefinedOrder, KernelValidated),
    // --- Estensioni table v1.2 (4) -------------------------------------------
    op!("table.align_schema", Table, Extension, Unary, Streaming, Cooperative, None, None, &[], DefinedOrder, KernelValidated),
    op!("table.concat_by_name", Table, Extension, NAry, Blocking, BoundaryOnly, None, None, &[], InputOrder, KernelValidated),
    op!("table.hmac_sha256", Table, Extension, Unary, Streaming, Cooperative, None, None, &[], DefinedOrder, KernelValidated),
    op!("table.validate_rules", Table, Extension, Unary, Streaming, Cooperative, None, None, &[], DefinedOrder, KernelValidated),
    // --- Estensioni table v1.3 (1) -------------------------------------------
    // fuzzy_join: build/probe sui blocchi (prefix/soundex) come i join
    // esatti, ma scoring per coppia candidata -> BinaryBlocking; ordine di
    // output definito (scansione sinistra, indice destro).
    op!("table.fuzzy_join", Table, Extension, BinaryOrdered, BinaryBlocking, BoundaryOnly, None, None, &[], DefinedOrder, KernelValidated),
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
    (3, "geo_count_points_in_polygons", "geo.count_points_in_polygons"),
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
    (3, "predicate_contains_properly", "geo.predicate_contains_properly"),
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
            CATALOG.iter().filter(|op| op.family == Family::Table).count(),
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
        assert_eq!(find_operation("table.filter").map(|op| op.id), Some("table.filter"));
        assert_eq!(find_operation("filter").map(|op| op.id), Some("table.filter"));
        assert_eq!(find_operation("geo_buffer").map(|op| op.id), Some("geo.buffer"));
        assert_eq!(find_operation("translate").map(|op| op.id), Some("geo.translate"));
        assert!(find_operation("nonexistent_op").is_none());
    }

    #[test]
    fn versions_default_to_one_and_expression_is_v2() {
        // Default: tutte e 4 le componenti a 1 per le op senza incrementi.
        let filter = find_operation("table.filter").expect("table.filter");
        assert_eq!(filter.semantic_version, 1);
        assert_eq!(filter.config_schema_version, 1);
        assert_eq!(filter.contract_analysis_version, 1);
        assert_eq!(filter.kernel_version, 2);
        // Macro estesa: le 4 versioni di table.expression sono tutte esplicite.
        let expression = find_operation("table.expression").expect("table.expression");
        assert_eq!(expression.semantic_version, 2);
        assert_eq!(expression.config_schema_version, 2);
        assert_eq!(expression.contract_analysis_version, 2);
        assert_eq!(expression.kernel_version, 3);
        // Nessuna versione puo' essere 0 in tutto il catalogo.
        for op in CATALOG {
            assert!(op.semantic_version >= 1, "{} semantic_version", op.id);
            assert!(op.config_schema_version >= 1, "{} config_schema_version", op.id);
            assert!(op.contract_analysis_version >= 1, "{} contract_analysis_version", op.id);
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
}
