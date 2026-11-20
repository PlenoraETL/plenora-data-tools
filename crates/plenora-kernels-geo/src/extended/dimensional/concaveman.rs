//! Percorso contato del concaveman vendorizzato (geo 0.33.1, MIT/Apache-2.0).
//! Conserva albero, heap, parita' e ordine degli inserimenti del percorso legacy.
//! I contatori coprono le visite e i buffer propri; `RTree` e quickhull sono
//! ancora primitive opache con checkpoint ai confini, non una qualifica GOP-027.

use crate::noding::{CountedVec, Work};
use crate::predicates::PredicateError;
use geo::concave_hull::ConcaveHullOptions;
use geo::{
    coord, point, Contains, Coord, Distance, Euclidean, Intersects, Length, Line, LineString,
    Polygon, Triangle,
};
use rstar::{Envelope, ParentNode, RTree, RTreeNode, AABB};
use std::{cmp::Ordering, collections::BinaryHeap};

fn partial_min(a: f64, b: f64) -> f64 {
    if a < b {
        a
    } else {
        b
    }
}

// Il buffer precede la ricevuta nel drop; durante realloc entrambe sono vive.
struct CountedHeap<T: Ord> {
    values: BinaryHeap<T>,
    reservation: Option<crate::noding::BufferReservation>,
}
impl<T: Ord> CountedHeap<T> {
    const fn new() -> Self {
        Self {
            values: BinaryHeap::new(),
            reservation: None,
        }
    }
    fn push(&mut self, item: T, work: &mut Work<'_>) -> Result<(), PredicateError> {
        work.step()?;
        if self.values.len() == self.values.capacity() {
            let capacity = self
                .values
                .capacity()
                .max(1)
                .checked_mul(2)
                .ok_or(PredicateError::ReservationLimit)?;
            let bytes = capacity
                .checked_mul(std::mem::size_of::<T>())
                .and_then(|n| u64::try_from(n).ok())
                .ok_or(PredicateError::ReservationLimit)?;
            let reservation = work.reserve_buffer(bytes)?;
            self.values
                .try_reserve_exact(capacity - self.values.len())
                .map_err(|_| PredicateError::AllocationFailed)?;
            if self.values.capacity() != capacity {
                self.values = BinaryHeap::new();
                self.reservation = None;
                return Err(PredicateError::CalcoloNonConcluso(
                    "capacita' della prenotazione",
                ));
            }
            self.reservation = Some(reservation);
        }
        self.values.push(item);
        Ok(())
    }
    fn pop(&mut self) -> Option<T> {
        self.values.pop()
    }
}

enum RTreeNodeRef<'a> {
    Parent(&'a ParentNode<Coord>),
    Leaf(&'a Coord),
}

struct NodeQueueItem<'a> {
    tree_node: RTreeNodeRef<'a>,
    distance: f64,
}

impl Ord for NodeQueueItem<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        other.distance.total_cmp(&self.distance)
    }
}
impl PartialOrd for NodeQueueItem<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl PartialEq for NodeQueueItem<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.distance.total_cmp(&other.distance) == Ordering::Equal
    }
}
impl Eq for NodeQueueItem<'_> {}

#[derive(Clone)]
struct CurrentHullEdge {
    line: Line,
    i: usize,
    prev_i: usize,
    next_i: usize,
}

fn line_to_bbox_distance(line: &Line, bbox: &AABB<Coord>) -> f64 {
    // Calculate the euclidean distance from a line to a bounding box.
    // This function is equivalent to `Euclidean.distance` between a `Rect` and a `Line`,
    // but is optimized for the R-tree depth-first search used here.
    // Since lines are likely to be intersecting or contained within the bounding box, resulting
    // in a distance of zero, calculate each seperately and return early if zero is found.

    // If either line endpoint is contained within the bbox, distance is zero
    if bbox.contains_point(&line.start) || bbox.contains_point(&line.end) {
        return 0.0;
    }

    // If any distances are zero, then return as no further distance calculations needed
    let c1 = coord! {x: bbox.lower().x, y: bbox.lower().y};
    let c2 = coord! {x: bbox.lower().x, y: bbox.upper().y};
    let c3 = coord! {x: bbox.upper().x, y: bbox.upper().y};
    let c4 = coord! {x: bbox.upper().x, y: bbox.lower().y};
    let d1 = Euclidean.distance(line, &Line::new(c1, c4));
    if d1 == 0.0 {
        return d1;
    }
    let d2 = Euclidean.distance(line, &Line::new(c1, c2));
    if d2 == 0.0 {
        return d2;
    }
    let d3 = Euclidean.distance(line, &Line::new(c4, c3));
    if d3 == 0.0 {
        return d3;
    }
    let d4 = Euclidean.distance(line, &Line::new(c2, c3));
    if d4 == 0.0 {
        return d4;
    }

    // If the line is contained within or intersecting the bounding box, return the minimum distance
    partial_min(partial_min(d1, d2), partial_min(d3, d4))
}

fn no_hull_intersections(
    line: &Line,
    current_hull_tree: &RTree<Line>,
    work: &mut Work<'_>,
) -> Result<bool, PredicateError> {
    // Check if the line intersects with any existing hull line
    // Hull lines which share an endpoint with the line are skipped

    // Create bounding box for the line
    let min_x = f64::min(line.start.x, line.end.x);
    let max_x = f64::max(line.start.x, line.end.x);
    let min_y = f64::min(line.start.y, line.end.y);
    let max_y = f64::max(line.start.y, line.end.y);
    let bbox = AABB::from_corners(point!([min_x, min_y]), point!([max_x, max_y]));

    // Iterate over all lines in the hull which intersect with the bounding box
    let hull_lines = current_hull_tree.locate_in_envelope_intersecting(&bbox);
    for hull_line in hull_lines {
        work.pair()?;
        // Skip lines which share an endpoint
        if [line.start, line.end].contains(&hull_line.start)
            || [line.start, line.end].contains(&hull_line.end)
        {
            continue;
        }
        if line.intersects(hull_line) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn find_excluded_point(
    start_line: Line,
    end_line: Line,
    previously_checked_points: &[Coord],
    work: &mut Work<'_>,
) -> Result<Option<Coord>, PredicateError> {
    // Check previously checked interior points to see if any would lie outside the hull if the new lines were added.
    // If so, return that point as the candidate to ensure all interior points remain within the hull.

    // Create a triangle which represents the area which would be excluded from the hull
    let triangle = Triangle::new(start_line.start, start_line.end, end_line.end);
    for point in previously_checked_points {
        work.pair()?;
        // If the point is contained within the triangle, it would be outside the hull and so return it as the candidate
        if triangle.contains(point) {
            return Ok(Some(*point));
        }
    }
    Ok(None)
}

fn find_candidate(
    hull_edge: &CurrentHullEdge,
    max_length: f64,
    current_hull_edges: &[CurrentHullEdge],
    interior_points_tree: &RTree<Coord>,
    current_hull_tree: &RTree<Line>,
    work: &mut Work<'_>,
) -> Result<Option<Coord>, PredicateError> {
    let line = hull_edge.line;

    // Initialize priority queue with R-tree root node
    let mut queue = CountedHeap::new();
    queue.push(
        NodeQueueItem {
            tree_node: RTreeNodeRef::Parent(interior_points_tree.root()),
            distance: 0.0,
        },
        work,
    )?;

    // Keep track of nearest interior points which failed checks
    let mut previously_checked_points = CountedVec::new();

    // Perform depth-first search through the R-tree
    while let Some(node) = queue.pop() {
        work.step()?;
        match node.tree_node {
            RTreeNodeRef::Parent(parent) => {
                // Add the children of a parent node to the queue if they are within the max distance
                for child in parent.children() {
                    work.pair()?;
                    match child {
                        RTreeNode::Parent(p) => {
                            let distance = line_to_bbox_distance(&line, &p.envelope());
                            if distance <= max_length {
                                queue.push(
                                    NodeQueueItem {
                                        tree_node: RTreeNodeRef::Parent(p),
                                        distance,
                                    },
                                    work,
                                )?;
                            }
                        }
                        RTreeNode::Leaf(l) => {
                            let distance = Euclidean.distance(*l, &line);
                            if distance <= max_length {
                                queue.push(
                                    NodeQueueItem {
                                        tree_node: RTreeNodeRef::Leaf(l),
                                        distance,
                                    },
                                    work,
                                )?;
                            }
                        }
                    }
                }
            }
            RTreeNodeRef::Leaf(leaf) => {
                // Check if candidate point is further from adjacent hull lines
                if node.distance
                    >= Euclidean.distance(*leaf, &current_hull_edges[hull_edge.prev_i].line)
                    || node.distance
                        >= Euclidean.distance(*leaf, &current_hull_edges[hull_edge.next_i].line)
                {
                    work.push(&mut previously_checked_points, *leaf)?;
                    continue;
                }

                let start_line = Line::new(line.start, *leaf);
                let end_line = Line::new(*leaf, line.end);

                // Check if using candidate point would cause intersections with hull lines
                if no_hull_intersections(&start_line, current_hull_tree, work)?
                    && no_hull_intersections(&end_line, current_hull_tree, work)?
                {
                    // Check if any of the previously checked interior points would lie outside the hull if the new lines
                    // were added and use that point as the candidate if so
                    if let Some(point) =
                        find_excluded_point(start_line, end_line, &previously_checked_points, work)?
                    {
                        return Ok(Some(point));
                    }
                    return Ok(Some(*leaf));
                }
                work.push(&mut previously_checked_points, *leaf)?;
            }
        }
    }
    Ok(None)
}

fn order_concave_hull(
    current_hull_edges: &[CurrentHullEdge],
    work: &mut Work<'_>,
) -> Result<LineString, PredicateError> {
    // Order the constituent concave hull edges and return as a `LineString`
    let mut ordered_coords = CountedVec::new();
    let mut current_i = 0;
    work.push(
        &mut ordered_coords,
        current_hull_edges[current_i].line.start,
    )?;

    for _ in 0..current_hull_edges.len() {
        let next_i = current_hull_edges[current_i].next_i;
        let line = current_hull_edges[current_i].line;
        work.push(&mut ordered_coords, line.end)?;
        current_i = next_i;
    }
    Ok(LineString(ordered_coords.export(work)?))
}

fn remove_interior_point(
    coord: &Coord,
    tree: &mut RTree<Coord>,
    work: &mut Work<'_>,
) -> Result<(), PredicateError> {
    // Remove all instances of the point from the R-tree
    work.step()?;
    let n = tree.nearest_neighbors(coord).len();
    work.step()?;
    for _ in 0..n {
        work.step()?;
        tree.remove(coord);
        work.step()?;
    }
    Ok(())
}

pub(super) fn run(
    coords: &mut [Coord],
    concave_hull_options: &ConcaveHullOptions<f64>,
    work: &mut Work<'_>,
) -> Result<Polygon, PredicateError> {
    // Ensure concavity is non-negative
    let concavity: f64 = f64::max(0.0, concave_hull_options.concavity);

    // Compute initial convex hull
    work.step()?;
    let convex_hull = geo::convex_hull::qhull::quick_hull(coords);
    work.step()?;

    // Return convex hull if less than 4 points
    if coords.len() < 4 {
        return Ok(Polygon::new(convex_hull, vec![]));
    }

    let (mut interior_points_tree, mut current_hull_tree) =
        build_trees(coords, &convex_hull, work)?;

    let mut edge_queue = CountedVec::new();
    let mut queue_head = 0;
    let mut current_hull_edges = CountedVec::new();

    // Populate edge queue and current hull edges with convex hull edges
    let hull_length = convex_hull.lines().len();
    for (i, line) in convex_hull.lines().enumerate() {
        work.step()?;
        let edge = CurrentHullEdge {
            line,
            i,
            prev_i: if i == 0 { hull_length - 1 } else { i - 1 },
            next_i: if i == hull_length - 1 { 0 } else { i + 1 },
        };
        work.push(&mut current_hull_edges, edge.clone())?;
        work.push(&mut edge_queue, i)?;

        // Remove hull points from interior points
        if i == 0 {
            remove_interior_point(&line.start, &mut interior_points_tree, work)?;
        }
        remove_interior_point(&line.end, &mut interior_points_tree, work)?;
    }

    while let Some(&edge_i) = edge_queue.get(queue_head) {
        work.step()?;
        queue_head += 1;
        // Queue identities, not snapshots: adjacent edges may have been split.
        let hull_edge = current_hull_edges[edge_i].clone();
        let line = hull_edge.line;
        let length = Euclidean.length(&line);

        // Only consider drilling down if line length exceeds threshold
        if length > concave_hull_options.length_threshold {
            // Calculate maximum length for new hull edges
            let max_length = length / concavity;

            if let Some(candidate_point) = find_candidate(
                &hull_edge,
                max_length,
                &current_hull_edges,
                &interior_points_tree,
                &current_hull_tree,
                work,
            )? {
                // Create new hull lines from start→candidate and candidate→end
                let start_line = Line::new(line.start, candidate_point);
                let end_line = Line::new(candidate_point, line.end);

                // Verify at least one of the new edges is less than max_length
                if partial_min(Euclidean.length(&start_line), Euclidean.length(&end_line))
                    < max_length
                {
                    // Remove candidate point from interior points
                    remove_interior_point(&candidate_point, &mut interior_points_tree, work)?;

                    // Update current hull tree
                    work.step()?;
                    current_hull_tree.remove(&line);
                    work.step()?;
                    current_hull_tree.insert(start_line);
                    work.step()?;
                    current_hull_tree.insert(end_line);
                    work.step()?;

                    // Set end edges' index as the length of current hull
                    let end_edge_i = current_hull_edges.len();

                    // Set new hull edges with indexes of adjacent edges
                    let start_hull_edge = CurrentHullEdge {
                        line: start_line,
                        i: hull_edge.i,
                        prev_i: hull_edge.prev_i,
                        next_i: end_edge_i,
                    };
                    let end_hull_edge = CurrentHullEdge {
                        line: end_line,
                        i: end_edge_i,
                        prev_i: hull_edge.i,
                        next_i: hull_edge.next_i,
                    };

                    // The start edge retains its index; only the successor changes predecessor.
                    current_hull_edges[hull_edge.next_i].prev_i = end_edge_i;

                    // Replace the current edge with the new start edge
                    current_hull_edges[hull_edge.i] = start_hull_edge.clone();

                    // Push new end edge to current hull edges
                    work.push(&mut current_hull_edges, end_hull_edge.clone())?;

                    // Push both new edges to queue
                    work.push(&mut edge_queue, start_hull_edge.i)?;
                    work.push(&mut edge_queue, end_hull_edge.i)?;
                }
            }
        }
    }
    Ok(Polygon::new(
        order_concave_hull(&current_hull_edges, work)?,
        vec![],
    ))
}

fn build_trees(
    coords: &[Coord],
    convex_hull: &LineString,
    work: &mut Work<'_>,
) -> Result<(RTree<Coord>, RTree<Line>), PredicateError> {
    // Build R-trees for interior points and hull lines
    let mut points = CountedVec::new();
    for &coordinate in coords {
        work.push(&mut points, coordinate)?;
    }
    let interior_points_tree: RTree<Coord> = RTree::bulk_load(points.export(work)?);
    work.step()?;
    let mut lines = CountedVec::new();
    for line in convex_hull.lines() {
        work.push(&mut lines, line)?;
    }
    let current_hull_tree: RTree<Line> = RTree::bulk_load(lines.export(work)?);
    work.step()?;

    Ok((interior_points_tree, current_hull_tree))
}
