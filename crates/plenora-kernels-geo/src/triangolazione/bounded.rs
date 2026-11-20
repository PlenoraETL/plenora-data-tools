//! Costruzione con il contesto comune; Spade resta un confine esterno.
use super::{chiave, Sito, Triangolazione};
use crate::extended_algorithms::delaunay_limited::DelaunayBuildError;
use crate::noding::{sorted, CountedVec, Work};
use geo::{Coord, GeoNum, Kernel, Orientation};
use spade::{DelaunayTriangulation, Triangulation as _};

fn distinct(
    coordinates: &[Coord<f64>],
    work: &mut Work<'_>,
) -> Result<CountedVec<Coord<f64>>, DelaunayBuildError> {
    let mut order = CountedVec::new();
    for index in 0..coordinates.len() {
        work.push(&mut order, index)?;
    }
    sorted(
        &mut order,
        |&a, &b| Ok((chiave(coordinates[a]), a).cmp(&(chiave(coordinates[b]), b))),
        work,
    )?;
    let mut groups = CountedVec::new();
    for _ in coordinates {
        work.push(&mut groups, 0_usize)?;
    }
    let mut previous = None;
    for &index in &order {
        work.step()?;
        let key = chiave(coordinates[index]);
        let first = match previous {
            Some((same, first)) if same == key => first,
            _ => index,
        };
        previous = Some((key, first));
        groups[index] = first;
    }
    drop(order);
    let mut sites = CountedVec::new();
    let mut collinear = true;
    for (index, &coordinate) in coordinates.iter().enumerate() {
        work.step()?;
        let first = groups[index];
        if first == index {
            if collinear && sites.len() >= 2 {
                collinear = <f64 as GeoNum>::Ker::orient2d(sites[0], sites[1], coordinate)
                    == Orientation::Collinear;
            }
            groups[index] = sites.len();
            work.nodes(1)?;
            work.push(&mut sites, coordinate)?;
        } else if sites.len() == 1 || !collinear {
            sites[groups[first]] = coordinate;
        }
    }
    Ok(sites)
}

pub fn build(
    coordinates: &[Coord<f64>],
    work: &mut Work<'_>,
) -> Result<Triangolazione, DelaunayBuildError> {
    work.step()?;
    for &coordinate in coordinates {
        work.step()?;
        spade::validate_vertex(&coordinate).map_err(|error| {
            DelaunayBuildError::Triangulation(super::ErroreTriangolazione::Inserimento(error))
        })?;
    }
    let sites = distinct(coordinates, work)?;
    let mut vertices = CountedVec::new();
    for (rango, &coordinata) in sites.iter().enumerate() {
        work.push(&mut vertices, Sito { coordinata, rango })?;
    }
    let vertices = vertices.export(work)?;
    work.step()?;
    // Opzione A: nessuna promessa di prenotazioni o checkpoint DENTRO Spade.
    // L'ordine dei vertici e la chiamata bulk_load sono quelli della baseline.
    let result = crate::calcolo_protetto(|| DelaunayTriangulation::<Sito>::bulk_load(vertices));
    work.step()?;
    let triangulation = result
        .map_err(DelaunayBuildError::Calculation)?
        .map_err(|error| {
            DelaunayBuildError::Triangulation(super::ErroreTriangolazione::Inserimento(error))
        })?;
    let mut present = CountedVec::new();
    for _ in &sites {
        work.push(&mut present, false)?;
    }
    let mut complete = triangulation.num_vertices() == sites.len();
    for vertex in triangulation.vertices() {
        work.step()?;
        match present.get_mut(vertex.data().rango) {
            Some(mark) if !*mark => *mark = true,
            _ => complete = false,
        }
    }
    if !complete {
        return Err(DelaunayBuildError::Triangulation(
            super::ErroreTriangolazione::VerticiInattesi,
        ));
    }
    Ok(Triangolazione {
        triangolazione: triangulation,
        siti: sites.export(work)?,
    })
}
