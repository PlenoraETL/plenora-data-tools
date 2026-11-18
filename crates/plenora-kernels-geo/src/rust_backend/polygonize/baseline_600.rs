// Snapshot letterale delle funzioni pubblicate in v6.0.0.
// Blob sorgente: 768a11b454fc839fd964aa313aaba5729976ff09.
// Gli helper privati sono quelli del modulo padre, invariati dal tag.
// Corpus e provenienza: compatibility_600.rs nello stesso percorso.
use super::*;

pub fn polygonize_linework_rust(
    linework: &Geometry<f64>,
    options: PolygonizeOptions,
) -> Result<PolygonizeResult, PolygonizeError> {
    polygonize_linework_rust_with_rounded(linework, options).map(|(result, _)| result)
}

pub fn polygonize_linework_rust_with_rounded(
    linework: &Geometry<f64>,
    options: PolygonizeOptions,
) -> Result<(PolygonizeResult, Vec<Coord<f64>>), PolygonizeError> {
    if !(options.precision.is_finite() && options.precision > 0.0) {
        return Err(PolygonizeError::InvalidPrecision);
    }
    checked_preflight(linework, options)?;
    let mut lines = Vec::new();
    collect_lines(linework, &mut lines)?;
    ValidazioneOgc::valida_ogc_rapida(linework)
        .map_err(|error| PolygonizeError::InvalidInput(error.to_string()))?;
    // Coordinate troppo rade per la precisione: nessun punto calcolato
    // (noding) potrebbe restarvi entro.
    if !super::precision::coordinate_abbastanza_fitte(
        super::precision::modulo_massimo(linework.coords_iter()),
        options.precision,
    ) {
        return Err(PolygonizeError::PrecisionInsufficient);
    }
    let count = segment_count(&lines)?;
    let mut segments = collect_segments(&lines, count)?;
    if options.node_input {
        segments = deduplicate_segments(segments);
    }
    let mut output_budget = OutputBudget::new(options.limits);
    let (excluded_edges, mut duplicate_cut_lines) =
        duplicate_lines_without_noding(&lines, options.node_input, &mut output_budget)?;
    let mut rounded = BTreeSet::new();
    let mut edges = if options.node_input {
        node_edges_iteratively(
            &segments,
            options.limits.max_noding_work,
            options.precision,
            &mut rounded,
        )?
    } else {
        build_unnoded_edges(&segments)
    };
    edges.retain(|edge, _| !excluded_edges.contains(edge));
    drop(excluded_edges);
    drop(segments);
    drop(lines);
    let mut coordinates = BTreeMap::new();
    for (edge, (start, end)) in &edges {
        coordinates.entry(edge.0).or_insert(*start);
        coordinates.entry(edge.1).or_insert(*end);
    }
    let rounded = rounded
        .into_iter()
        .filter_map(|key| coordinates.get(&key).copied())
        .collect::<Vec<_>>();
    let mut active = edges.keys().copied().collect::<BTreeSet<_>>();
    drop(edges);
    let dangle_edges = remove_dangles(&mut active);
    let Faces {
        polygons,
        polygon_edges,
        invalid_rings,
    } = extract_faces(&active, &coordinates, options.limits, &mut output_budget)?;
    let cut_edges = active
        .difference(&polygon_edges)
        .copied()
        .collect::<BTreeSet<_>>();
    drop(active);
    drop(polygon_edges);
    let cut_lines = edge_chains(&cut_edges, &coordinates, &mut output_budget)?;
    drop(cut_edges);
    duplicate_cut_lines.extend(cut_lines);
    let dangles = edge_chains(&dangle_edges, &coordinates, &mut output_budget)?;
    drop(dangle_edges);
    drop(coordinates);
    let result = PolygonizeResult {
        polygons,
        cut_edges: duplicate_cut_lines,
        dangles,
        invalid_ring_lines: invalid_rings,
    };
    checked_output(&result, options)?;
    let residuals = result.residual_count()?;
    if options.require_complete && residuals != 0 {
        return Err(PolygonizeError::Incomplete { residuals });
    }
    Ok((result, rounded))
}

pub fn polygonize_linework_rust_bounded(
    linework: &Geometry<f64>,
    options: PolygonizeOptions,
) -> Result<PolygonizeResult, PolygonizeError> {
    if !options.limits.is_fully_bounded() {
        return Err(PolygonizeError::UnboundedLimitConfiguration);
    }
    polygonize_linework_rust(linework, options)
}
