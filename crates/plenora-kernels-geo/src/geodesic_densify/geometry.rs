//! Superficie Geometry di GOP-007; i ruoli degli anelli vengono da Polygon.

use super::{
    coordinate, edge, emit_part, validation, AtomicBool, Coord, CrsKind, DensifyWork,
    EllissoideGeodetico, GeodesicDensifyError, GeodesicDensifyLimits, LineString, ResolvedCrs,
};
use geo::{Geometry, MultiLineString, MultiPolygon, Polygon};

/// Budget dell'API Geometry, senza sentinelle illimitate.
#[derive(Clone, Copy, Debug)]
pub struct GeodesicGeometryDensifyLimits {
    /// Punti emessi e capacita' viva: output e validazione.
    /// Include vettori di poligoni, buchi, coordinate e indice metrico.
    /// Input in prestito, stack limitato e overhead dell'allocatore esclusi.
    pub output: GeodesicDensifyLimits,
    /// Primitive, letture, visite e raffinamenti effettivi della validazione.
    pub max_validation_work: u64,
}

fn count_line(
    line: &LineString,
    ellipsoid: &EllissoideGeodetico,
    maximum: f64,
    work: &mut DensifyWork<'_>,
) -> Result<(), GeodesicDensifyError> {
    work.checkpoint()?;
    if let Some(&point) = line.0.first() {
        coordinate(point)?;
        work.charge(1, size_of::<Coord>() as u64)?;
    }
    for pair in line.0.windows(2) {
        #[cfg(test)]
        crate::noding::gop028::cycle("geodesic_densify/geometry.rs::count_line:1");
        work.checkpoint()?;
        let divisions = u64::from(edge(pair[0], pair[1], ellipsoid, maximum, work)?.divisions);
        work.charge(divisions, divisions * size_of::<Coord>() as u64)?;
    }
    Ok(())
}

fn count_polygon(
    polygon: &Polygon,
    ellipsoid: &EllissoideGeodetico,
    maximum: f64,
    work: &mut DensifyWork<'_>,
) -> Result<(), GeodesicDensifyError> {
    count_line(polygon.exterior(), ellipsoid, maximum, work)?;
    for hole in polygon.interiors() {
        #[cfg(test)]
        crate::noding::gop028::cycle("geodesic_densify/geometry.rs::count_polygon:1");
        work.checkpoint()?;
        work.charge(0, size_of::<LineString>() as u64)?;
        count_line(hole, ellipsoid, maximum, work)?;
    }
    Ok(())
}

fn emit_polygon(
    polygon: &Polygon,
    ellipsoid: &EllissoideGeodetico,
    maximum: f64,
    work: &DensifyWork<'_>,
) -> Result<Polygon, GeodesicDensifyError> {
    work.checkpoint()?;
    let shell = emit_part(polygon.exterior(), ellipsoid, maximum, work)?;
    let mut holes = Vec::new();
    holes
        .try_reserve_exact(polygon.interiors().len())
        .map_err(|_| GeodesicDensifyError::Allocation)?;
    for hole in polygon.interiors() {
        #[cfg(test)]
        crate::noding::gop028::cycle("geodesic_densify/geometry.rs::emit_polygon:1");
        work.checkpoint()?;
        holes.push(emit_part(hole, ellipsoid, maximum, work)?);
    }
    Ok(Polygon::new(shell, holes))
}

/// Densify `shortest_geodesic` GOP-007 su `LineString`, `MultiLineString`,
/// Polygon e `MultiPolygon` XY; altri tipi sono `UnsupportedGeometry`.
///
/// XY, datum, unita' e regola numerica sono quelli di
/// [`super::geodesic_densify_with_limits`]. Riusa la stessa primitiva per ogni anello.
/// Preserva tipo, parti, anelli, vertici e chiusure esatte, compresi i vuoti.
///
/// GOP-005: guscio e buchi espliciti, semplici e separati; ogni anello
/// rappresenta la regione minore di mezzo ellissoide, indipendentemente
/// dal verso. La serializzazione non esprime un complemento maggiore:
/// tale politica non e' disponibile. Emisferi ambigui sono rifiutati.
/// Seam continuo e calotte polari ammessi; nessuna riproiezione o riparazione.
/// Valida anche contenimento dei buchi, buchi annidati e parti sovrapposte.
/// Separazione proporzionale all'errore posizionale effettivo delle primitive.
/// L'orientamento usa un ventaglio della mappa normale, con errore proporzionale
/// a somma dei cubi dei passi e perimetro; vicino al complemento ambiguo usa
/// il certificato globale delle primitive d'area e della loro accumulazione.
/// Limiti residui e categorie in `docs/geodetica-coordinate.md`.
/// Sui lati oltre 1000 km richiede m12>=1024 m e propaga il margine
/// di posizione dipendente da m12, inferiore a 0.25 mm.
/// Bordi quasi paralleli distanti meno di un metro possono esaurire il
/// budget di validazione: nessuna garanzia di successo per ogni budget finito.
///
/// Certificato geometrico GOP-006: Hausdorff simmetrico assoluto 0.01 m,
/// relativo zero, sul datum risolto, modello `shortest_geodesic`.
/// Nessuna trasformazione o approssimazione di discretizzazione: le divisioni
/// stanno sugli archi originali; il margine numerico delle primitive e'
/// 1 micrometro. La topologia dell'output viene verificata prima del ritorno.
/// Il massimo richiesto limita i lati, non l'errore della forma.
///
/// Conta tutto l'output prima di riservarlo. Il controllo finale addebita
/// insieme output e indice di validazione. La validazione
/// addebita il lavoro effettivo, senza prodotti di cardinalita' all'ammissione.
/// GOP-028: checkpoint all'ingresso, prima del ritorno, per parte, coordinata,
/// visita e raffinamento; al massimo due primitive Karney fra controlli
/// durante l'emissione, una durante la validazione. Primitive sincrone,
/// nessuna garanzia di latenza wall-clock. Deadline prevale su cancellazione;
/// un errore gia' osservato conserva la propria categoria. Risultato atomico.
/// Il costruttore Polygon controlla le chiusure senza callback: al massimo
/// `max_points / 4` buchi fra checkpoint (ogni anello valido ha almeno 4 punti).
/// Una modifica della coda richiede al massimo `2 * usize::BITS` confronti;
/// una riallocazione sincrona copia al massimo `max_reserved_bytes` byte.
///
/// # Errors
/// [`GeodesicDensifyError`] distingue dominio, struttura, accuratezza,
/// risorse e interruzione; i messaggi non contengono dati dell'ingresso.
#[allow(
    clippy::too_many_lines,
    reason = "Sonde O-M3 per ciclo, assenti in produzione"
)]
pub fn geodesic_densify_geometry_with_limits(
    geometry: &Geometry,
    crs: &ResolvedCrs,
    maximum_m: f64,
    limits: GeodesicGeometryDensifyLimits,
    cancelled: Option<&AtomicBool>,
) -> Result<Geometry, GeodesicDensifyError> {
    let mut work = DensifyWork::new(limits.output, cancelled);
    work.checkpoint()?;
    if !matches!(
        geometry,
        Geometry::LineString(_)
            | Geometry::MultiLineString(_)
            | Geometry::Polygon(_)
            | Geometry::MultiPolygon(_)
    ) {
        return Err(GeodesicDensifyError::UnsupportedGeometry);
    }
    if crs.kind() != CrsKind::Geographic {
        return Err(GeodesicDensifyError::GeographicRequired);
    }
    if !(maximum_m.is_finite() && maximum_m > 0.0) {
        return Err(GeodesicDensifyError::InvalidMaximum);
    }
    let ellipsoid =
        EllissoideGeodetico::da_crs(crs).map_err(|_| GeodesicDensifyError::UnsupportedEllipsoid)?;
    let parameters = ellipsoid.parametri();
    if !(6e6..=7e6).contains(&parameters.semi_major_axis_metre)
        || !(250.0..=350.0).contains(&parameters.inverse_flattening)
    {
        return Err(GeodesicDensifyError::UnsupportedEllipsoid);
    }
    match geometry {
        Geometry::LineString(line) => count_line(line, &ellipsoid, maximum_m, &mut work)?,
        Geometry::MultiLineString(lines) => {
            for line in lines {
                #[cfg(test)]
                crate::noding::gop028::cycle(
                    "geodesic_densify/geometry.rs::geodesic_densify_geometry_with_limits:1",
                );
                work.checkpoint()?;
                work.charge(0, size_of::<LineString>() as u64)?;
                count_line(line, &ellipsoid, maximum_m, &mut work)?;
            }
        }
        Geometry::Polygon(polygon) => count_polygon(polygon, &ellipsoid, maximum_m, &mut work)?,
        Geometry::MultiPolygon(polygons) => {
            for polygon in polygons {
                #[cfg(test)]
                crate::noding::gop028::cycle(
                    "geodesic_densify/geometry.rs::geodesic_densify_geometry_with_limits:2",
                );
                work.checkpoint()?;
                work.charge(0, size_of::<Polygon>() as u64)?;
                count_polygon(polygon, &ellipsoid, maximum_m, &mut work)?;
            }
        }
        _ => return Err(GeodesicDensifyError::UnsupportedGeometry),
    }
    let validation_work = validation::validate(geometry, &ellipsoid, limits, cancelled)?;
    let output = match geometry {
        Geometry::LineString(line) => {
            Geometry::LineString(emit_part(line, &ellipsoid, maximum_m, &work)?)
        }
        Geometry::MultiLineString(lines) => {
            let mut output = Vec::new();
            output
                .try_reserve_exact(lines.0.len())
                .map_err(|_| GeodesicDensifyError::Allocation)?;
            for line in lines {
                #[cfg(test)]
                crate::noding::gop028::cycle(
                    "geodesic_densify/geometry.rs::geodesic_densify_geometry_with_limits:3",
                );
                work.checkpoint()?;
                output.push(emit_part(line, &ellipsoid, maximum_m, &work)?);
            }
            Geometry::MultiLineString(MultiLineString(output))
        }
        Geometry::Polygon(polygon) => {
            Geometry::Polygon(emit_polygon(polygon, &ellipsoid, maximum_m, &work)?)
        }
        Geometry::MultiPolygon(polygons) => {
            let mut output = Vec::new();
            output
                .try_reserve_exact(polygons.0.len())
                .map_err(|_| GeodesicDensifyError::Allocation)?;
            for polygon in polygons {
                #[cfg(test)]
                crate::noding::gop028::cycle(
                    "geodesic_densify/geometry.rs::geodesic_densify_geometry_with_limits:4",
                );
                work.checkpoint()?;
                output.push(emit_polygon(polygon, &ellipsoid, maximum_m, &work)?);
            }
            Geometry::MultiPolygon(MultiPolygon(output))
        }
        _ => return Err(GeodesicDensifyError::UnsupportedGeometry),
    };
    work.checkpoint()?;
    let remaining = GeodesicGeometryDensifyLimits {
        output: GeodesicDensifyLimits {
            max_reserved_bytes: limits.output.max_reserved_bytes - work.bytes(),
            ..limits.output
        },
        max_validation_work: limits.max_validation_work - validation_work,
    };
    validation::validate(&output, &ellipsoid, remaining, cancelled).map_err(
        |error| match error {
            GeodesicDensifyError::InvalidGeometry | GeodesicDensifyError::UnsupportedSurface => {
                GeodesicDensifyError::AccuracyUnproven
            }
            other => other,
        },
    )?;
    work.checkpoint()?;
    Ok(output)
}
