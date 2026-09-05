//! Identita' tipizzata delle operazioni del catalogo.
//!
//! # Perche' un enum e non una stringa
//!
//! Il formato pubblico dei piani usa stringhe — `table.filter`, `geo.buffer` —
//! e continuera' a usarle: sono cio' che gli utenti scrivono e cio' che entra
//! nell'identita' canonica di un piano. Ma una stringa che circola DENTRO il
//! codice non e' un identificatore, e' un'occasione di errore: si puo'
//! confrontare con un letterale sbagliato, si puo' costruire con un typo, e
//! soprattutto **nessun `match` su stringhe puo' essere esaustivo**.
//!
//! Questo enum sposta il confine: la conversione avviene una volta sola, al
//! parsing, e da li' in avanti il compilatore sa quali operazioni esistono. Un
//! descrittore nuovo nel catalogo senza il ramo corrispondente in un `match`
//! diventa un errore di compilazione invece di un fallimento a runtime il
//! giorno in cui qualcuno usa davvero l'operazione nuova.
//!
//! # La bijezione col catalogo e' verificata, non assunta
//!
//! Enum e `CATALOG` sono due elenchi delle stesse 146 operazioni, e due
//! elenchi divergono. Il test `l_enum_e_il_catalogo_sono_in_bijezione` li
//! confronta in entrambe le direzioni: nessuna variante senza descrittore,
//! nessun descrittore senza variante.
//!
//! # Che cosa resta fuori dall'osservabile
//!
//! Questo enum. `as_str` restituisce esattamente l'id del catalogo, quindi
//! la serializzazione canonica, il `plan_hash` e il `catalog_fingerprint`
//! non dipendono da come l'identita' e' rappresentata dentro il codice.
//! Gli alias legacy si risolvono in `find_operation`.

/// Un'operazione del catalogo, per costruzione.
///
/// Le varianti sono in ordine alfabetico di id, lo stesso ordine del catalogo
/// canonico: un diff di questo file si legge accanto a un diff dello snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum OperationId {
    // --- Operazioni geometriche ---
    /// `geo.affine_transform`
    GeoAffineTransform,
    /// `geo.area`
    GeoArea,
    /// `geo.bearing`
    GeoBearing,
    /// `geo.boundary`
    GeoBoundary,
    /// `geo.bounds_extractor`
    GeoBoundsExtractor,
    /// `geo.buffer`
    GeoBuffer,
    /// `geo.centroid`
    GeoCentroid,
    /// `geo.clean_topology`
    GeoCleanTopology,
    /// `geo.clip`
    GeoClip,
    /// `geo.cluster_dbscan`
    GeoClusterDbscan,
    /// `geo.collect`
    GeoCollect,
    /// `geo.concave_hull`
    GeoConcaveHull,
    /// `geo.convex_hull`
    GeoConvexHull,
    /// `geo.count_points_in_polygons`
    GeoCountPointsInPolygons,
    /// `geo.coverage_validate`
    GeoCoverageValidate,
    /// `geo.delaunay`
    GeoDelaunay,
    /// `geo.densify`
    GeoDensify,
    /// `geo.difference`
    GeoDifference,
    /// `geo.dissolve`
    GeoDissolve,
    /// `geo.distance`
    GeoDistance,
    /// `geo.envelope`
    GeoEnvelope,
    /// `geo.explode`
    GeoExplode,
    /// `geo.frechet_distance`
    GeoFrechetDistance,
    /// `geo.from_coords`
    GeoFromCoords,
    /// `geo.from_wkt`
    GeoFromWkt,
    /// `geo.generate_grid`
    GeoGenerateGrid,
    /// `geo.geodesic_area`
    GeoGeodesicArea,
    /// `geo.geodesic_distance`
    GeoGeodesicDistance,
    /// `geo.geodesic_line_length`
    GeoGeodesicLineLength,
    /// `geo.geometry_accessors`
    GeoGeometryAccessors,
    /// `geo.geometry_diagnostics`
    GeoGeometryDiagnostics,
    /// `geo.hausdorff_distance`
    GeoHausdorffDistance,
    /// `geo.haversine_distance`
    GeoHaversineDistance,
    /// `geo.intersection`
    GeoIntersection,
    /// `geo.length`
    GeoLength,
    /// `geo.line_builder`
    GeoLineBuilder,
    /// `geo.line_interpolate_point`
    GeoLineInterpolatePoint,
    /// `geo.line_locate_point`
    GeoLineLocatePoint,
    /// `geo.line_merge`
    GeoLineMerge,
    /// `geo.line_substring`
    GeoLineSubstring,
    /// `geo.make_valid`
    GeoMakeValid,
    /// `geo.nearest`
    GeoNearest,
    /// `geo.overlay`
    GeoOverlay,
    /// `geo.perimeter`
    GeoPerimeter,
    /// `geo.point_on_surface`
    GeoPointOnSurface,
    /// `geo.polygon_builder`
    GeoPolygonBuilder,
    /// `geo.polygonize`
    GeoPolygonize,
    /// `geo.predicate_contains`
    GeoPredicateContains,
    /// `geo.predicate_contains_properly`
    GeoPredicateContainsProperly,
    /// `geo.predicate_covered_by`
    GeoPredicateCoveredBy,
    /// `geo.predicate_covers`
    GeoPredicateCovers,
    /// `geo.predicate_crosses`
    GeoPredicateCrosses,
    /// `geo.predicate_disjoint`
    GeoPredicateDisjoint,
    /// `geo.predicate_equals_topo`
    GeoPredicateEqualsTopo,
    /// `geo.predicate_intersects`
    GeoPredicateIntersects,
    /// `geo.predicate_overlaps`
    GeoPredicateOverlaps,
    /// `geo.predicate_touches`
    GeoPredicateTouches,
    /// `geo.predicate_within`
    GeoPredicateWithin,
    /// `geo.reproject`
    GeoReproject,
    /// `geo.rotate`
    GeoRotate,
    /// `geo.scale`
    GeoScale,
    /// `geo.shared_paths`
    GeoSharedPaths,
    /// `geo.simplify`
    GeoSimplify,
    /// `geo.sjoin`
    GeoSjoin,
    /// `geo.snap`
    GeoSnap,
    /// `geo.snap_to_grid`
    GeoSnapToGrid,
    /// `geo.split`
    GeoSplit,
    /// `geo.subdivide`
    GeoSubdivide,
    /// `geo.symmetric_difference`
    GeoSymmetricDifference,
    /// `geo.to_wkt`
    GeoToWkt,
    /// `geo.translate`
    GeoTranslate,
    /// `geo.union`
    GeoUnion,
    /// `geo.vertex_count`
    GeoVertexCount,
    /// `geo.voronoi`
    GeoVoronoi,
    /// `geo.within`
    GeoWithin,

    // --- Operazioni tabellari ---
    /// `table.add_row_number`
    TableAddRowNumber,
    /// `table.aggregate`
    TableAggregate,
    /// `table.align_schema`
    TableAlignSchema,
    /// `table.anti_join`
    TableAntiJoin,
    /// `table.asof_join`
    TableAsofJoin,
    /// `table.assert_cardinality`
    TableAssertCardinality,
    /// `table.assert_foreign_key`
    TableAssertForeignKey,
    /// `table.assert_metadata`
    TableAssertMetadata,
    /// `table.assert_not_null`
    TableAssertNotNull,
    /// `table.assert_range`
    TableAssertRange,
    /// `table.assert_regex`
    TableAssertRegex,
    /// `table.assert_schema`
    TableAssertSchema,
    /// `table.assert_unique`
    TableAssertUnique,
    /// `table.bin`
    TableBin,
    /// `table.coalesce`
    TableCoalesce,
    /// `table.concat`
    TableConcat,
    /// `table.concat_by_name`
    TableConcatByName,
    /// `table.concat_columns`
    TableConcatColumns,
    /// `table.conditional`
    TableConditional,
    /// `table.cross_join`
    TableCrossJoin,
    /// `table.date_add`
    TableDateAdd,
    /// `table.date_diff`
    TableDateDiff,
    /// `table.date_extract`
    TableDateExtract,
    /// `table.date_format`
    TableDateFormat,
    /// `table.dedup_advanced`
    TableDedupAdvanced,
    /// `table.distinct`
    TableDistinct,
    /// `table.drop_columns`
    TableDropColumns,
    /// `table.except`
    TableExcept,
    /// `table.explode`
    TableExplode,
    /// `table.expression`
    TableExpression,
    /// `table.fill_na`
    TableFillNa,
    /// `table.filter`
    TableFilter,
    /// `table.flatten_json`
    TableFlattenJson,
    /// `table.formula`
    TableFormula,
    /// `table.fuzzy_join`
    TableFuzzyJoin,
    /// `table.hmac_sha256`
    TableHmacSha256,
    /// `table.intersect`
    TableIntersect,
    /// `table.join`
    TableJoin,
    /// `table.limit`
    TableLimit,
    /// `table.lookup`
    TableLookup,
    /// `table.mask_data`
    TableMaskData,
    /// `table.md5_hash`
    TableMd5Hash,
    /// `table.melt`
    TableMelt,
    /// `table.pivot`
    TablePivot,
    /// `table.reconcile`
    TableReconcile,
    /// `table.rename`
    TableRename,
    /// `table.reorder_columns`
    TableReorderColumns,
    /// `table.replace`
    TableReplace,
    /// `table.rolling_window`
    TableRollingWindow,
    /// `table.sample`
    TableSample,
    /// `table.select_columns`
    TableSelectColumns,
    /// `table.semi_join`
    TableSemiJoin,
    /// `table.sha256_hash`
    TableSha256Hash,
    /// `table.sort`
    TableSort,
    /// `table.split_column`
    TableSplitColumn,
    /// `table.stable_fingerprint`
    TableStableFingerprint,
    /// `table.statistics`
    TableStatistics,
    /// `table.string_extract`
    TableStringExtract,
    /// `table.string_length`
    TableStringLength,
    /// `table.string_pad`
    TableStringPad,
    /// `table.table_diff`
    TableTableDiff,
    /// `table.text_normalize`
    TableTextNormalize,
    /// `table.timezone_convert`
    TableTimezoneConvert,
    /// `table.top_n`
    TableTopN,
    /// `table.transpose`
    TableTranspose,
    /// `table.type_cast`
    TableTypeCast,
    /// `table.union_distinct`
    TableUnionDistinct,
    /// `table.unnest`
    TableUnnest,
    /// `table.uuid_generator`
    TableUuidGenerator,
    /// `table.validate_rules`
    TableValidateRules,
    /// `table.window_function`
    TableWindowFunction,
}

impl OperationId {
    /// L'id canonico, identico a quello del descrittore nel catalogo.
    ///
    /// E' la forma che entra nella serializzazione dei piani e quindi nel
    /// `plan_hash`: cambiarla sarebbe una rottura di formato, non una
    /// rinomina.
    #[must_use]
    // 146 rami: la lunghezza e' il NUMERO DELLE OPERAZIONI, non
    // complessita' logica. Spezzare il match in blocchi arbitrari
    // toglierebbe l'unica proprieta' che serve — l'esaustivita'
    // verificata dal compilatore — in cambio di funzioni piu' corte.
    #[allow(clippy::too_many_lines)]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::GeoAffineTransform => "geo.affine_transform",
            Self::GeoArea => "geo.area",
            Self::GeoBearing => "geo.bearing",
            Self::GeoBoundary => "geo.boundary",
            Self::GeoBoundsExtractor => "geo.bounds_extractor",
            Self::GeoBuffer => "geo.buffer",
            Self::GeoCentroid => "geo.centroid",
            Self::GeoCleanTopology => "geo.clean_topology",
            Self::GeoClip => "geo.clip",
            Self::GeoClusterDbscan => "geo.cluster_dbscan",
            Self::GeoCollect => "geo.collect",
            Self::GeoConcaveHull => "geo.concave_hull",
            Self::GeoConvexHull => "geo.convex_hull",
            Self::GeoCountPointsInPolygons => "geo.count_points_in_polygons",
            Self::GeoCoverageValidate => "geo.coverage_validate",
            Self::GeoDelaunay => "geo.delaunay",
            Self::GeoDensify => "geo.densify",
            Self::GeoDifference => "geo.difference",
            Self::GeoDissolve => "geo.dissolve",
            Self::GeoDistance => "geo.distance",
            Self::GeoEnvelope => "geo.envelope",
            Self::GeoExplode => "geo.explode",
            Self::GeoFrechetDistance => "geo.frechet_distance",
            Self::GeoFromCoords => "geo.from_coords",
            Self::GeoFromWkt => "geo.from_wkt",
            Self::GeoGenerateGrid => "geo.generate_grid",
            Self::GeoGeodesicArea => "geo.geodesic_area",
            Self::GeoGeodesicDistance => "geo.geodesic_distance",
            Self::GeoGeodesicLineLength => "geo.geodesic_line_length",
            Self::GeoGeometryAccessors => "geo.geometry_accessors",
            Self::GeoGeometryDiagnostics => "geo.geometry_diagnostics",
            Self::GeoHausdorffDistance => "geo.hausdorff_distance",
            Self::GeoHaversineDistance => "geo.haversine_distance",
            Self::GeoIntersection => "geo.intersection",
            Self::GeoLength => "geo.length",
            Self::GeoLineBuilder => "geo.line_builder",
            Self::GeoLineInterpolatePoint => "geo.line_interpolate_point",
            Self::GeoLineLocatePoint => "geo.line_locate_point",
            Self::GeoLineMerge => "geo.line_merge",
            Self::GeoLineSubstring => "geo.line_substring",
            Self::GeoMakeValid => "geo.make_valid",
            Self::GeoNearest => "geo.nearest",
            Self::GeoOverlay => "geo.overlay",
            Self::GeoPerimeter => "geo.perimeter",
            Self::GeoPointOnSurface => "geo.point_on_surface",
            Self::GeoPolygonBuilder => "geo.polygon_builder",
            Self::GeoPolygonize => "geo.polygonize",
            Self::GeoPredicateContains => "geo.predicate_contains",
            Self::GeoPredicateContainsProperly => "geo.predicate_contains_properly",
            Self::GeoPredicateCoveredBy => "geo.predicate_covered_by",
            Self::GeoPredicateCovers => "geo.predicate_covers",
            Self::GeoPredicateCrosses => "geo.predicate_crosses",
            Self::GeoPredicateDisjoint => "geo.predicate_disjoint",
            Self::GeoPredicateEqualsTopo => "geo.predicate_equals_topo",
            Self::GeoPredicateIntersects => "geo.predicate_intersects",
            Self::GeoPredicateOverlaps => "geo.predicate_overlaps",
            Self::GeoPredicateTouches => "geo.predicate_touches",
            Self::GeoPredicateWithin => "geo.predicate_within",
            Self::GeoReproject => "geo.reproject",
            Self::GeoRotate => "geo.rotate",
            Self::GeoScale => "geo.scale",
            Self::GeoSharedPaths => "geo.shared_paths",
            Self::GeoSimplify => "geo.simplify",
            Self::GeoSjoin => "geo.sjoin",
            Self::GeoSnap => "geo.snap",
            Self::GeoSnapToGrid => "geo.snap_to_grid",
            Self::GeoSplit => "geo.split",
            Self::GeoSubdivide => "geo.subdivide",
            Self::GeoSymmetricDifference => "geo.symmetric_difference",
            Self::GeoToWkt => "geo.to_wkt",
            Self::GeoTranslate => "geo.translate",
            Self::GeoUnion => "geo.union",
            Self::GeoVertexCount => "geo.vertex_count",
            Self::GeoVoronoi => "geo.voronoi",
            Self::GeoWithin => "geo.within",
            Self::TableAddRowNumber => "table.add_row_number",
            Self::TableAggregate => "table.aggregate",
            Self::TableAlignSchema => "table.align_schema",
            Self::TableAntiJoin => "table.anti_join",
            Self::TableAsofJoin => "table.asof_join",
            Self::TableAssertCardinality => "table.assert_cardinality",
            Self::TableAssertForeignKey => "table.assert_foreign_key",
            Self::TableAssertMetadata => "table.assert_metadata",
            Self::TableAssertNotNull => "table.assert_not_null",
            Self::TableAssertRange => "table.assert_range",
            Self::TableAssertRegex => "table.assert_regex",
            Self::TableAssertSchema => "table.assert_schema",
            Self::TableAssertUnique => "table.assert_unique",
            Self::TableBin => "table.bin",
            Self::TableCoalesce => "table.coalesce",
            Self::TableConcat => "table.concat",
            Self::TableConcatByName => "table.concat_by_name",
            Self::TableConcatColumns => "table.concat_columns",
            Self::TableConditional => "table.conditional",
            Self::TableCrossJoin => "table.cross_join",
            Self::TableDateAdd => "table.date_add",
            Self::TableDateDiff => "table.date_diff",
            Self::TableDateExtract => "table.date_extract",
            Self::TableDateFormat => "table.date_format",
            Self::TableDedupAdvanced => "table.dedup_advanced",
            Self::TableDistinct => "table.distinct",
            Self::TableDropColumns => "table.drop_columns",
            Self::TableExcept => "table.except",
            Self::TableExplode => "table.explode",
            Self::TableExpression => "table.expression",
            Self::TableFillNa => "table.fill_na",
            Self::TableFilter => "table.filter",
            Self::TableFlattenJson => "table.flatten_json",
            Self::TableFormula => "table.formula",
            Self::TableFuzzyJoin => "table.fuzzy_join",
            Self::TableHmacSha256 => "table.hmac_sha256",
            Self::TableIntersect => "table.intersect",
            Self::TableJoin => "table.join",
            Self::TableLimit => "table.limit",
            Self::TableLookup => "table.lookup",
            Self::TableMaskData => "table.mask_data",
            Self::TableMd5Hash => "table.md5_hash",
            Self::TableMelt => "table.melt",
            Self::TablePivot => "table.pivot",
            Self::TableReconcile => "table.reconcile",
            Self::TableRename => "table.rename",
            Self::TableReorderColumns => "table.reorder_columns",
            Self::TableReplace => "table.replace",
            Self::TableRollingWindow => "table.rolling_window",
            Self::TableSample => "table.sample",
            Self::TableSelectColumns => "table.select_columns",
            Self::TableSemiJoin => "table.semi_join",
            Self::TableSha256Hash => "table.sha256_hash",
            Self::TableSort => "table.sort",
            Self::TableSplitColumn => "table.split_column",
            Self::TableStableFingerprint => "table.stable_fingerprint",
            Self::TableStatistics => "table.statistics",
            Self::TableStringExtract => "table.string_extract",
            Self::TableStringLength => "table.string_length",
            Self::TableStringPad => "table.string_pad",
            Self::TableTableDiff => "table.table_diff",
            Self::TableTextNormalize => "table.text_normalize",
            Self::TableTimezoneConvert => "table.timezone_convert",
            Self::TableTopN => "table.top_n",
            Self::TableTranspose => "table.transpose",
            Self::TableTypeCast => "table.type_cast",
            Self::TableUnionDistinct => "table.union_distinct",
            Self::TableUnnest => "table.unnest",
            Self::TableUuidGenerator => "table.uuid_generator",
            Self::TableValidateRules => "table.validate_rules",
            Self::TableWindowFunction => "table.window_function",
        }
    }

    /// Riconosce un id CANONICO. Non risolve gli alias legacy: quelli hanno
    /// una tabella versionata propria ([`super::resolve_alias`]), perche' la
    /// stessa stringa puo' significare operazioni diverse a versioni diverse
    /// del formato piano — e un enum non ha un posto dove metterlo.
    #[must_use]
    // 146 rami: la lunghezza e' il NUMERO DELLE OPERAZIONI, non
    // complessita' logica. Spezzare il match in blocchi arbitrari
    // toglierebbe l'unica proprieta' che serve — l'esaustivita'
    // verificata dal compilatore — in cambio di funzioni piu' corte.
    #[allow(clippy::too_many_lines)]
    pub fn from_canonical(id: &str) -> Option<Self> {
        match id {
            "geo.affine_transform" => Some(Self::GeoAffineTransform),
            "geo.area" => Some(Self::GeoArea),
            "geo.bearing" => Some(Self::GeoBearing),
            "geo.boundary" => Some(Self::GeoBoundary),
            "geo.bounds_extractor" => Some(Self::GeoBoundsExtractor),
            "geo.buffer" => Some(Self::GeoBuffer),
            "geo.centroid" => Some(Self::GeoCentroid),
            "geo.clean_topology" => Some(Self::GeoCleanTopology),
            "geo.clip" => Some(Self::GeoClip),
            "geo.cluster_dbscan" => Some(Self::GeoClusterDbscan),
            "geo.collect" => Some(Self::GeoCollect),
            "geo.concave_hull" => Some(Self::GeoConcaveHull),
            "geo.convex_hull" => Some(Self::GeoConvexHull),
            "geo.count_points_in_polygons" => Some(Self::GeoCountPointsInPolygons),
            "geo.coverage_validate" => Some(Self::GeoCoverageValidate),
            "geo.delaunay" => Some(Self::GeoDelaunay),
            "geo.densify" => Some(Self::GeoDensify),
            "geo.difference" => Some(Self::GeoDifference),
            "geo.dissolve" => Some(Self::GeoDissolve),
            "geo.distance" => Some(Self::GeoDistance),
            "geo.envelope" => Some(Self::GeoEnvelope),
            "geo.explode" => Some(Self::GeoExplode),
            "geo.frechet_distance" => Some(Self::GeoFrechetDistance),
            "geo.from_coords" => Some(Self::GeoFromCoords),
            "geo.from_wkt" => Some(Self::GeoFromWkt),
            "geo.generate_grid" => Some(Self::GeoGenerateGrid),
            "geo.geodesic_area" => Some(Self::GeoGeodesicArea),
            "geo.geodesic_distance" => Some(Self::GeoGeodesicDistance),
            "geo.geodesic_line_length" => Some(Self::GeoGeodesicLineLength),
            "geo.geometry_accessors" => Some(Self::GeoGeometryAccessors),
            "geo.geometry_diagnostics" => Some(Self::GeoGeometryDiagnostics),
            "geo.hausdorff_distance" => Some(Self::GeoHausdorffDistance),
            "geo.haversine_distance" => Some(Self::GeoHaversineDistance),
            "geo.intersection" => Some(Self::GeoIntersection),
            "geo.length" => Some(Self::GeoLength),
            "geo.line_builder" => Some(Self::GeoLineBuilder),
            "geo.line_interpolate_point" => Some(Self::GeoLineInterpolatePoint),
            "geo.line_locate_point" => Some(Self::GeoLineLocatePoint),
            "geo.line_merge" => Some(Self::GeoLineMerge),
            "geo.line_substring" => Some(Self::GeoLineSubstring),
            "geo.make_valid" => Some(Self::GeoMakeValid),
            "geo.nearest" => Some(Self::GeoNearest),
            "geo.overlay" => Some(Self::GeoOverlay),
            "geo.perimeter" => Some(Self::GeoPerimeter),
            "geo.point_on_surface" => Some(Self::GeoPointOnSurface),
            "geo.polygon_builder" => Some(Self::GeoPolygonBuilder),
            "geo.polygonize" => Some(Self::GeoPolygonize),
            "geo.predicate_contains" => Some(Self::GeoPredicateContains),
            "geo.predicate_contains_properly" => Some(Self::GeoPredicateContainsProperly),
            "geo.predicate_covered_by" => Some(Self::GeoPredicateCoveredBy),
            "geo.predicate_covers" => Some(Self::GeoPredicateCovers),
            "geo.predicate_crosses" => Some(Self::GeoPredicateCrosses),
            "geo.predicate_disjoint" => Some(Self::GeoPredicateDisjoint),
            "geo.predicate_equals_topo" => Some(Self::GeoPredicateEqualsTopo),
            "geo.predicate_intersects" => Some(Self::GeoPredicateIntersects),
            "geo.predicate_overlaps" => Some(Self::GeoPredicateOverlaps),
            "geo.predicate_touches" => Some(Self::GeoPredicateTouches),
            "geo.predicate_within" => Some(Self::GeoPredicateWithin),
            "geo.reproject" => Some(Self::GeoReproject),
            "geo.rotate" => Some(Self::GeoRotate),
            "geo.scale" => Some(Self::GeoScale),
            "geo.shared_paths" => Some(Self::GeoSharedPaths),
            "geo.simplify" => Some(Self::GeoSimplify),
            "geo.sjoin" => Some(Self::GeoSjoin),
            "geo.snap" => Some(Self::GeoSnap),
            "geo.snap_to_grid" => Some(Self::GeoSnapToGrid),
            "geo.split" => Some(Self::GeoSplit),
            "geo.subdivide" => Some(Self::GeoSubdivide),
            "geo.symmetric_difference" => Some(Self::GeoSymmetricDifference),
            "geo.to_wkt" => Some(Self::GeoToWkt),
            "geo.translate" => Some(Self::GeoTranslate),
            "geo.union" => Some(Self::GeoUnion),
            "geo.vertex_count" => Some(Self::GeoVertexCount),
            "geo.voronoi" => Some(Self::GeoVoronoi),
            "geo.within" => Some(Self::GeoWithin),
            "table.add_row_number" => Some(Self::TableAddRowNumber),
            "table.aggregate" => Some(Self::TableAggregate),
            "table.align_schema" => Some(Self::TableAlignSchema),
            "table.anti_join" => Some(Self::TableAntiJoin),
            "table.asof_join" => Some(Self::TableAsofJoin),
            "table.assert_cardinality" => Some(Self::TableAssertCardinality),
            "table.assert_foreign_key" => Some(Self::TableAssertForeignKey),
            "table.assert_metadata" => Some(Self::TableAssertMetadata),
            "table.assert_not_null" => Some(Self::TableAssertNotNull),
            "table.assert_range" => Some(Self::TableAssertRange),
            "table.assert_regex" => Some(Self::TableAssertRegex),
            "table.assert_schema" => Some(Self::TableAssertSchema),
            "table.assert_unique" => Some(Self::TableAssertUnique),
            "table.bin" => Some(Self::TableBin),
            "table.coalesce" => Some(Self::TableCoalesce),
            "table.concat" => Some(Self::TableConcat),
            "table.concat_by_name" => Some(Self::TableConcatByName),
            "table.concat_columns" => Some(Self::TableConcatColumns),
            "table.conditional" => Some(Self::TableConditional),
            "table.cross_join" => Some(Self::TableCrossJoin),
            "table.date_add" => Some(Self::TableDateAdd),
            "table.date_diff" => Some(Self::TableDateDiff),
            "table.date_extract" => Some(Self::TableDateExtract),
            "table.date_format" => Some(Self::TableDateFormat),
            "table.dedup_advanced" => Some(Self::TableDedupAdvanced),
            "table.distinct" => Some(Self::TableDistinct),
            "table.drop_columns" => Some(Self::TableDropColumns),
            "table.except" => Some(Self::TableExcept),
            "table.explode" => Some(Self::TableExplode),
            "table.expression" => Some(Self::TableExpression),
            "table.fill_na" => Some(Self::TableFillNa),
            "table.filter" => Some(Self::TableFilter),
            "table.flatten_json" => Some(Self::TableFlattenJson),
            "table.formula" => Some(Self::TableFormula),
            "table.fuzzy_join" => Some(Self::TableFuzzyJoin),
            "table.hmac_sha256" => Some(Self::TableHmacSha256),
            "table.intersect" => Some(Self::TableIntersect),
            "table.join" => Some(Self::TableJoin),
            "table.limit" => Some(Self::TableLimit),
            "table.lookup" => Some(Self::TableLookup),
            "table.mask_data" => Some(Self::TableMaskData),
            "table.md5_hash" => Some(Self::TableMd5Hash),
            "table.melt" => Some(Self::TableMelt),
            "table.pivot" => Some(Self::TablePivot),
            "table.reconcile" => Some(Self::TableReconcile),
            "table.rename" => Some(Self::TableRename),
            "table.reorder_columns" => Some(Self::TableReorderColumns),
            "table.replace" => Some(Self::TableReplace),
            "table.rolling_window" => Some(Self::TableRollingWindow),
            "table.sample" => Some(Self::TableSample),
            "table.select_columns" => Some(Self::TableSelectColumns),
            "table.semi_join" => Some(Self::TableSemiJoin),
            "table.sha256_hash" => Some(Self::TableSha256Hash),
            "table.sort" => Some(Self::TableSort),
            "table.split_column" => Some(Self::TableSplitColumn),
            "table.stable_fingerprint" => Some(Self::TableStableFingerprint),
            "table.statistics" => Some(Self::TableStatistics),
            "table.string_extract" => Some(Self::TableStringExtract),
            "table.string_length" => Some(Self::TableStringLength),
            "table.string_pad" => Some(Self::TableStringPad),
            "table.table_diff" => Some(Self::TableTableDiff),
            "table.text_normalize" => Some(Self::TableTextNormalize),
            "table.timezone_convert" => Some(Self::TableTimezoneConvert),
            "table.top_n" => Some(Self::TableTopN),
            "table.transpose" => Some(Self::TableTranspose),
            "table.type_cast" => Some(Self::TableTypeCast),
            "table.union_distinct" => Some(Self::TableUnionDistinct),
            "table.unnest" => Some(Self::TableUnnest),
            "table.uuid_generator" => Some(Self::TableUuidGenerator),
            "table.validate_rules" => Some(Self::TableValidateRules),
            "table.window_function" => Some(Self::TableWindowFunction),
            _ => None,
        }
    }

    /// Tutte le operazioni, in ordine canonico.
    #[must_use]
    pub fn tutte() -> Vec<Self> {
        super::CATALOG
            .iter()
            .filter_map(|descrittore| Self::from_canonical(descrittore.id))
            .collect()
    }
}

impl std::fmt::Display for OperationId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}
