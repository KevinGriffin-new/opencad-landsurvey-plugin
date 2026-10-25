//! TIN contour slicing and topological chaining, independent of the CAD host.
//!
//! Endpoints are identified by their **global TIN edge**, rather than comparing
//! floating-point XY positions. Exact vertex levels are sampled one representable
//! elevation above the nominal level to avoid ambiguous zero-height vertices.

use std::collections::BTreeMap;

use crate::surface::{tri_cut_fill_seg_with_edges, Surface};

type EdgeKey = (usize, usize);

fn edge_key(a: usize, b: usize) -> EdgeKey {
    (a.min(b), a.max(b))
}

/// One continuous isoline. Z is the requested (nominal) elevation.
#[derive(Debug, Clone, PartialEq)]
pub struct Contour {
    pub elevation: f64,
    pub points: Vec<[f64; 3]>,
    pub closed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContourError {
    InvalidLevel,
    NonManifoldEdge,
    BranchingTopology,
}

/// Slice a TIN at one nominal level, connecting pieces by their shared edge IDs.
///
/// For a level coincident with a TIN vertex, slicing occurs at `level.next_up()`;
/// the resulting entities retain `level` as their Z. This is a topological
/// convention, not an exact-coordinate contour at the vertex.
pub fn contours_at_level(surface: &Surface, level: f64) -> Result<Vec<Contour>, ContourError> {
    if !level.is_finite() {
        return Err(ContourError::InvalidLevel);
    }
    let sample = if surface.nodes.iter().any(|n| n[2] == level) {
        level.next_up()
    } else {
        level
    };
    if !sample.is_finite() {
        return Err(ContourError::InvalidLevel);
    }

    // Each TIN edge is incident on at most two triangles in a manifold TIN.
    let mut edge_incidence = BTreeMap::<EdgeKey, usize>::new();
    let mut pieces = Vec::<([EdgeKey; 2], [[f64; 2]; 2])>::new();
    for &tri in &surface.triangles {
        if tri.iter().any(|&i| i >= surface.nodes.len())
            || tri[0] == tri[1] || tri[1] == tri[2] || tri[0] == tri[2]
        {
            continue;
        }
        let nodes = tri.map(|i| surface.nodes[i]);
        if nodes.iter().flatten().any(|x| !x.is_finite()) {
            continue;
        }
        let area2 = (nodes[1][0] - nodes[0][0]) * (nodes[2][1] - nodes[0][1])
            - (nodes[1][1] - nodes[0][1]) * (nodes[2][0] - nodes[0][0]);
        if !area2.is_finite() || area2.abs() <= 1e-12 {
            continue;
        }
        for (a,b) in [(0,1),(1,2),(2,0)] {
            let count = edge_incidence.entry(edge_key(tri[a], tri[b])).or_default();
            *count += 1;
            if *count > 2 {
                return Err(ContourError::NonManifoldEdge);
            }
        }
        let shifted = nodes.map(|n| [n[0], n[1], n[2] - sample]);
        if let Some((xy, local_edges)) = tri_cut_fill_seg_with_edges(&shifted) {
            let ids = local_edges.map(|(a,b)| edge_key(tri[a], tri[b]));
            if ids[0] != ids[1] {
                pieces.push((ids, xy));
            }
        }
    }

    // Build an edge-key graph. Each crossing TIN edge is one graph vertex.
    let mut key_to_vertex = BTreeMap::<EdgeKey, usize>::new();
    let mut coords = Vec::<[f64; 2]>::new();
    let mut links = Vec::<(usize, usize)>::new();
    for (keys, xy) in pieces {
        let mut ids = [0usize; 2];
        for end in 0..2 {
            ids[end] = match key_to_vertex.get(&keys[end]) {
                Some(&id) => id,
                None => {
                    let id = coords.len();
                    key_to_vertex.insert(keys[end], id);
                    coords.push(xy[end]);
                    id
                }
            };
        }
        links.push((ids[0], ids[1]));
    }
    let mut incident = vec![Vec::<usize>::new(); coords.len()];
    for (id, &(a,b)) in links.iter().enumerate() {
        incident[a].push(id);
        incident[b].push(id);
    }
    if incident.iter().any(|v| v.len() > 2) {
        return Err(ContourError::BranchingTopology);
    }

    let mut visited = vec![false; links.len()];
    let mut result = Vec::new();
    let seeds = (0..coords.len()).filter(|&i| incident[i].len() == 1)
        .chain((0..coords.len()).filter(|&i| incident[i].len() == 2));
    for start in seeds {
        if incident[start].iter().all(|&e| visited[e]) {
            continue;
        }
        let mut current = start;
        let mut points = vec![[coords[start][0],coords[start][1],level]];
        let mut closed = false;
        loop {
            let next_edge = incident[current].iter().copied().find(|&e| !visited[e]);
            let Some(e) = next_edge else { break };
            visited[e] = true;
            let (a,b) = links[e];
            current = if a == current { b } else { a };
            if current == start {
                closed = true;
                break;
            }
            points.push([coords[current][0],coords[current][1],level]);
        }
        if points.len() >= 2 {
            result.push(Contour { elevation: level, points, closed });
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plane_two_triangles_stitches_by_shared_edge() {
        let s = Surface {
            nodes: vec![[0.,0.,0.],[2.,0.,2.],[0.,2.,0.],[2.,2.,2.]],
            triangles: vec![[0,1,2],[1,3,2]],
        };
        for z in [0.5,1.,1.5] {
            let curves = contours_at_level(&s,z).unwrap();
            assert_eq!(curves.len(),1);
            let c = &curves[0];
            assert!(!c.closed);
            assert!(c.points.iter().all(|p| (p[0]-z).abs()<1e-10 && p[2]==z));
            let length: f64 = c.points.windows(2)
                .map(|pair| (pair[1][0]-pair[0][0]).hypot(pair[1][1]-pair[0][1]))
                .sum();
            assert!((length-2.).abs()<1e-10);
        }
    }

    #[test]
    fn closed_peak_is_one_ring() {
        let s = Surface {
            nodes: vec![[0.,0.,2.],[-2.,-2.,0.],[2.,-2.,0.],[2.,2.,0.],[-2.,2.,0.]],
            triangles: vec![[0,1,2],[0,2,3],[0,3,4],[0,4,1]],
        };
        let curves = contours_at_level(&s,1.).unwrap();
        assert_eq!(curves.len(),1);
        assert!(curves[0].closed);
        assert_eq!(curves[0].points.len(),4);
    }

    #[test]
    fn exact_vertex_nudge_keeps_nominal_z() {
        let s = Surface {
            nodes: vec![[0.,0.,0.],[2.,0.,2.],[0.,2.,0.],[2.,2.,2.]],
            triangles: vec![[0,1,2],[1,3,2]],
        };
        let curves = contours_at_level(&s,0.).unwrap();
        assert!(curves.iter().all(|c| c.points.iter().all(|p| p[2]==0.)));
    }

    #[test]
    fn utm_translation_does_not_change_contour_length() {
        let s = Surface {
            nodes: vec![[500000.,900000.,20.],[500002.,900000.,22.],
                        [500000.,900002.,20.],[500002.,900002.,22.]],
            triangles: vec![[0,1,2],[1,3,2]],
        };
        let c = contours_at_level(&s,21.).unwrap();
        assert_eq!(c.len(),1);
        assert!(c[0].points.iter().all(|p| (p[0]-500001.).abs()<1e-8));
    }

    #[test]
    fn invalid_and_degenerate_triangles_are_ignored() {
        let s = Surface {
            nodes: vec![[0.,0.,0.],[1.,0.,1.],[2.,0.,2.]],
            triangles: vec![[0,1,2],[0,1,999]],
        };
        assert!(contours_at_level(&s,0.5).unwrap().is_empty());
        assert_eq!(contours_at_level(&s,f64::NAN),Err(ContourError::InvalidLevel));
    }
}

#[cfg(test)]
mod topology_regression_tests {
    use super::*;

    fn saddle() -> Surface {
        Surface {
            nodes: vec![
                [0., 0., 0.],
                [1., 0., 1.],
                [0., 1., -1.],
                [-1., 0., 1.],
                [0., -1., -1.],
            ],
            triangles: vec![
                [0, 1, 2],
                [0, 2, 3],
                [0, 3, 4],
                [0, 4, 1],
            ],
        }
    }

    #[test]
    fn exact_saddle_never_cross_connects_branches() {
        let contours = contours_at_level(&saddle(), 0.).unwrap();

        assert_eq!(
            contours.len(), 2,
            "exact saddle must produce two distinct branches"
        );

        for contour in contours {
            assert!(!contour.closed);
            assert_eq!(contour.elevation, 0.);
            assert!(contour.points.len() >= 2);
        }
    }

    #[test]
    fn off_saddle_levels_remain_disconnected() {
        for z in [-0.25, 0.25] {
            let contours = contours_at_level(&saddle(), z).unwrap();

            assert_eq!(
                contours.len(), 2,
                "saddle branches must remain separate at z={z}"
            );

            assert!(contours.iter().all(|c| !c.closed));
        }
    }

    #[test]
    fn topology_is_independent_of_triangle_order() {
        let original = saddle();
        let mut reordered = original.clone();
        reordered.triangles.reverse();

        for z in [-0.25, 0., 0.25] {
            let first = contours_at_level(&original, z).unwrap();
            let second = contours_at_level(&reordered, z).unwrap();

            assert_eq!(first.len(), second.len());
            assert_eq!(
                first.iter().filter(|c| c.closed).count(),
                second.iter().filter(|c| c.closed).count()
            );
        }
    }
}
