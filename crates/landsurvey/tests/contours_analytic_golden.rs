//! Independent analytical contour golden for the public first-pass contribution.
//!
//! These exact geometric expectations come from elementary plane and
//! interpolation geometry, NOT from the Rust contour algorithm. They do not
//! claim to be a Civil 3D export or to reproduce the private Gualaca survey.
//!
//! A separate, externally sourced Civil 3D golden is still required for
//! independent software-to-software validation.

use landsurvey::contours::contours_at_level;
use landsurvey::surface::Surface;

fn length_xy(points: &[[f64; 3]], closed: bool) -> f64 {
    let segments = points
        .windows(2)
        .map(|pair| (pair[1][0] - pair[0][0]).hypot(pair[1][1] - pair[0][1]))
        .sum::<f64>();
    if closed && points.len() > 1 {
        let a = points[0];
        let b = points[points.len() - 1];
        segments + (b[0] - a[0]).hypot(b[1] - a[1])
    } else {
        segments
    }
}

#[test]
fn analytic_plane_contours_match_absolute_datum_golden() {
    // Two triangles form a rectangular TIN, 0 <= E,N <= 2, with Z = 100 + E.
    // For 100 < h < 102, the exact level set is E = h-100, 0 <= N <= 2.
    // One open 2 m chain must exist at each of the three requested levels.
    let surface = Surface {
        nodes: vec![
            [0., 0., 100.],
            [2., 0., 102.],
            [0., 2., 100.],
            [2., 2., 102.],
        ],
        triangles: vec![[0, 1, 2], [1, 3, 2]],
    };

    for (elevation, easting) in [(100.5, 0.5), (101., 1.), (101.5, 1.5)] {
        let curves = contours_at_level(&surface, elevation).expect("valid manifold plane");
        assert_eq!(curves.len(), 1, "chain count at {elevation}");
        let contour = &curves[0];
        assert!(!contour.closed, "plane section must be open");
        assert_eq!(contour.elevation, elevation);
        assert!(contour.points.len() >= 2);
        for p in &contour.points {
            assert!((p[0] - easting).abs() < 1e-9, "E at {elevation}: {}", p[0]);
            assert!(p[1] >= -1e-9 && p[1] <= 2. + 1e-9);
            assert_eq!(p[2], elevation);
        }
        let ys: Vec<f64> = contour.points.iter().map(|p| p[1]).collect();
        let min_y = ys.iter().copied().fold(f64::INFINITY, f64::min);
        let max_y = ys.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        assert!((min_y - 0.).abs() < 1e-9, "south boundary at {elevation}");
        assert!((max_y - 2.).abs() < 1e-9, "north boundary at {elevation}");
        assert!((length_xy(&contour.points, contour.closed) - 2.).abs() < 1e-9);
    }
}

#[test]
fn analytic_closed_peak_matches_square_perimeter_golden() {
    // Center at Z=2, corners at Z=0. At level Z=1 the section is the
    // diamond/square with vertices (+/-1, +/-1) and perimeter 8 m.
    let surface = Surface {
        nodes: vec![
            [0., 0., 2.],
            [-2., -2., 0.],
            [2., -2., 0.],
            [2., 2., 0.],
            [-2., 2., 0.],
        ],
        triangles: vec![[0, 1, 2], [0, 2, 3], [0, 3, 4], [0, 4, 1]],
    };
    let curves = contours_at_level(&surface, 1.).expect("valid peak TIN");
    assert_eq!(curves.len(), 1);
    let contour = &curves[0];
    assert!(contour.closed);
    assert_eq!(contour.points.len(), 4);
    for point in &contour.points {
        assert!((point[0].abs() - 1.).abs() < 1e-9);
        assert!((point[1].abs() - 1.).abs() < 1e-9);
        assert_eq!(point[2], 1.);
    }
    assert!((length_xy(&contour.points, true) - 8.).abs() < 1e-9);
}
