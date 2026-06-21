//! Resection / free-station — solve an **unknown** occupied point from shots to
//! **known** control. Pure functions, `std` only (see `docs/resection-design.md`).
//!
//! Two flavors:
//! * **Combined** (modern total station): direction **+ distance** to ≥2 known
//!   points → a least-squares 2-D similarity fit. Each shot, in the instrument
//!   frame, is a local coordinate `local = (d·sin r, d·cos r)` (`r` = horizontal
//!   circle reading, `d` = horizontal distance), using the same clockwise-from-
//!   north convention as [`crate::cogo::inverse`]. Mapping instrument-frame →
//!   world is a rotation (orientation) + translation (station) with scale ≈ 1, so
//!   it *is* a Helmert fit — we reuse [`crate::transform::helmert_fit`] rather
//!   than re-derive the math.
//! * **Angle-only** (classic three-point): directions only to exactly 3 known
//!   points → Tienstra's barycentric formula.
//!
//! ## Orientation sign (the subtle bit)
//! [`Conformal::rotation_deg`] is CCW-positive (math convention). Working through
//! `world_rel = s·R_ccw(θ)·local_rel` with `local = (sin r, cos r)` gives
//! `azimuth = reading − θ`. So the **orientation** to add to a circle reading to
//! get a grid azimuth is `−θ`. A unit test pins this against a synthesized setup.

use crate::transform::{self, Conformal};

/// How a resection was solved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResectionMethod {
    /// Combined direction + distance, least-squares similarity (Helmert) fit.
    Combined,
    /// Angle-only three-point (Tienstra).
    AngleOnly,
}

/// One shot from the occupied (unknown) station to a known control point.
#[derive(Debug, Clone)]
pub struct ResectionShot {
    /// `(E, N)` of the known control point.
    pub known: (f64, f64),
    /// Horizontal circle reading in degrees (instrument frame).
    pub direction_deg: f64,
    /// Horizontal distance to the point; `None` = angle-only shot.
    pub distance: Option<f64>,
    /// Point label (for the residual report).
    pub name: String,
}

/// The solved station plus quality metrics.
#[derive(Debug, Clone)]
pub struct ResectionResult {
    /// Solved occupied-point coordinate `(E, N)`.
    pub station: (f64, f64),
    /// Orientation to **add** to a circle reading to get a grid azimuth, in
    /// degrees, normalized to `[0, 360)`: `azimuth = (reading + orientation) % 360`.
    pub orientation_deg: f64,
    /// EDM scale check (`hypot(a, b)` of the fit). Should sit near `1.0`; a
    /// deviation flags a distance/EDM blunder. `1.0` for angle-only.
    pub scale: f64,
    /// Per-shot planimetric residual `(name, distance)` in coordinate units.
    pub residuals: Vec<(String, f64)>,
    /// RMS of the residuals (`0.0` for an exact / angle-only solution).
    pub rms: f64,
    /// Which solver produced this result.
    pub method: ResectionMethod,
}

impl ResectionResult {
    /// Grid azimuth (degrees, CW-from-N, `[0,360)`) for a circle reading.
    pub fn azimuth_of(&self, reading_deg: f64) -> f64 {
        (reading_deg + self.orientation_deg).rem_euclid(360.0)
    }

    /// True if the EDM scale strays from 1 by more than `tol` (a blunder flag).
    pub fn scale_blunder(&self, tol: f64) -> bool {
        (self.scale - 1.0).abs() > tol
    }
}

/// Combined resection: ≥2 shots **with distances** → reuse the Helmert engine.
///
/// Builds instrument-frame locals `(d·sin r, d·cos r)` paired with the known
/// world coordinates and fits a 2-D conformal. The station is the transform
/// applied to the local origin; orientation is `−rotation`; scale is the EDM
/// check. Returns `Err` if fewer than two shots carry a distance, or the control
/// geometry is degenerate.
pub fn resection_combined(shots: &[ResectionShot]) -> Result<ResectionResult, &'static str> {
    let mut pairs: Vec<((f64, f64), (f64, f64))> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    for s in shots {
        if let Some(d) = s.distance {
            let r = s.direction_deg.to_radians();
            let local = (d * r.sin(), d * r.cos());
            pairs.push((local, s.known));
            names.push(s.name.clone());
        }
    }
    if pairs.len() < 2 {
        return Err("combined resection needs at least 2 shots with distances");
    }
    let t: Conformal = transform::helmert_fit(&pairs)?;
    let station = t.apply(0.0, 0.0); // local origin (instrument) -> world
    let orientation_deg = (-t.rotation_deg()).rem_euclid(360.0);
    let (res, rms) = transform::fit_residuals(&t, &pairs);
    let residuals = names.into_iter().zip(res).collect();
    Ok(ResectionResult {
        station,
        orientation_deg,
        scale: t.scale(),
        residuals,
        rms,
        method: ResectionMethod::Combined,
    })
}

/// Angle-only three-point resection (Tienstra). Exactly three known points
/// `a`, `b`, `c` (`(E, N)` each) and the three angles **subtended at the
/// unknown point P**, in degrees:
/// `ang_at_p = [∠BPC, ∠CPA, ∠APB]` (each paired with the opposite vertex).
///
/// ```text
///   W_A = 1/(cot A − cot α),  …    A = interior ∠BAC,  α = ∠BPC
///   P   = (W_A·A + W_B·B + W_C·C) / (W_A + W_B + W_C)
/// ```
///
/// Returns `Err` on the **danger circle** (P on the circumcircle of A/B/C → the
/// solution is indeterminate and the Tienstra weights blow up) or on degenerate
/// (collinear / coincident) control.
pub fn resection_three_point(
    a: (f64, f64),
    b: (f64, f64),
    c: (f64, f64),
    ang_at_p: [f64; 3],
) -> Result<(f64, f64), &'static str> {
    // Interior triangle angles at each vertex.
    let int_a = angle_at(a, b, c);
    let int_b = angle_at(b, c, a);
    let int_c = angle_at(c, a, b);
    if int_a < 1e-9 || int_b < 1e-9 || int_c < 1e-9 {
        return Err("degenerate control (collinear or coincident known points)");
    }
    let [alpha, beta, gamma] = ang_at_p;

    // Tienstra weights; a near-zero denominator is the danger circle.
    let w = |int_deg: f64, sub_deg: f64| -> Result<f64, &'static str> {
        let den = cot(int_deg)? - cot(sub_deg)?;
        if den.abs() < 1e-9 {
            return Err("danger circle: P near the circumcircle of the control — indeterminate");
        }
        Ok(1.0 / den)
    };
    let wa = w(int_a, alpha)?;
    let wb = w(int_b, beta)?;
    let wc = w(int_c, gamma)?;
    let sum = wa + wb + wc;
    if sum.abs() < 1e-12 {
        return Err("danger circle: degenerate weight sum — indeterminate");
    }
    Ok((
        (wa * a.0 + wb * b.0 + wc * c.0) / sum,
        (wa * a.1 + wb * b.1 + wc * c.1) / sum,
    ))
}

/// Unsigned angle (degrees, `[0,180]`) at vertex `v` of the wedge `p1–v–p2`.
fn angle_at(v: (f64, f64), p1: (f64, f64), p2: (f64, f64)) -> f64 {
    let (ux, uy) = (p1.0 - v.0, p1.1 - v.1);
    let (wx, wy) = (p2.0 - v.0, p2.1 - v.1);
    let cross = ux * wy - uy * wx;
    let dot = ux * wx + uy * wy;
    cross.atan2(dot).abs().to_degrees()
}

/// Cotangent of an angle given in degrees; `Err` near a multiple of 180° where
/// the sine vanishes (cot is unbounded).
fn cot(deg: f64) -> Result<f64, &'static str> {
    let r = deg.to_radians();
    let s = r.sin();
    if s.abs() < 1e-12 {
        return Err("angle too close to 0/180° (cotangent unbounded)");
    }
    Ok(r.cos() / s)
}

/// Parse a resection shot file. Each non-blank, non-`#` line is
/// `knownN, knownE, direction_deg[, distance][, name]` (comma / whitespace
/// separated). A missing or blank distance field makes the shot angle-only.
/// Stored `known` is `(E, N)` to match the engine's coordinate order.
pub fn parse_resection_shots(text: &str) -> Vec<ResectionShot> {
    let mut out = Vec::new();
    for line in text.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        // Split into raw fields so a blank distance (e.g. `1000,2000,45,,CP1`) is
        // preserved positionally; the name may contain spaces only if there are
        // no commas after it, so we treat the field after distance as the name.
        let fields: Vec<&str> = l.split(',').map(str::trim).collect();
        let toks: Vec<&str> = if fields.len() >= 3 {
            fields
        } else {
            l.split_whitespace().collect()
        };
        if toks.len() < 3 {
            continue;
        }
        let n: Option<f64> = toks[0].parse().ok();
        let e: Option<f64> = toks[1].parse().ok();
        let dir: Option<f64> = toks[2].parse().ok();
        let (n, e, dir) = match (n, e, dir) {
            (Some(n), Some(e), Some(d)) => (n, e, d),
            _ => continue,
        };
        let distance = toks.get(3).and_then(|s| {
            let s = s.trim();
            if s.is_empty() {
                None
            } else {
                s.parse::<f64>().ok()
            }
        });
        let name = toks
            .get(4)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("PT{}", out.len() + 1));
        out.push(ResectionShot {
            known: (e, n),
            direction_deg: dir,
            distance,
            name,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() < tol
    }

    /// Azimuth (CW from N) and distance from a station to a target — the same
    /// convention the engine uses to build the instrument-frame locals.
    fn az_dist(station: (f64, f64), target: (f64, f64)) -> (f64, f64) {
        let de = target.0 - station.0;
        let dn = target.1 - station.1;
        ((de.atan2(dn).to_degrees()).rem_euclid(360.0), de.hypot(dn))
    }

    #[test]
    fn combined_recovers_station_and_orientation() {
        // Truth: station, orientation (instrument zero offset from grid north).
        let station = (1000.0, 2000.0);
        let orientation = 37.0; // grid azimuth = reading + orientation
        let known = [
            ("CP1", (1300.0, 2400.0)),
            ("CP2", (700.0, 2350.0)),
            ("CP3", (1100.0, 1650.0)),
        ];
        let shots: Vec<ResectionShot> = known
            .iter()
            .map(|&(name, k)| {
                let (az, d) = az_dist(station, k);
                ResectionShot {
                    known: k,
                    direction_deg: (az - orientation).rem_euclid(360.0),
                    distance: Some(d),
                    name: name.to_string(),
                }
            })
            .collect();

        let r = resection_combined(&shots).unwrap();
        assert!(close(r.station.0, station.0, 1e-6) && close(r.station.1, station.1, 1e-6),
            "station {:?}", r.station);
        assert!(close(r.orientation_deg, orientation, 1e-6), "orientation {}", r.orientation_deg);
        assert!(close(r.scale, 1.0, 1e-9), "scale {}", r.scale);
        assert!(r.rms < 1e-6, "rms {}", r.rms);
        // Round-trip a reading back to a grid azimuth.
        let (az1, _) = az_dist(station, known[0].1);
        let reading1 = (az1 - orientation).rem_euclid(360.0);
        assert!(close(r.azimuth_of(reading1), az1, 1e-6));
    }

    #[test]
    fn combined_needs_two_distances() {
        let shots = vec![ResectionShot {
            known: (100.0, 100.0),
            direction_deg: 10.0,
            distance: Some(50.0),
            name: "A".into(),
        }];
        assert!(resection_combined(&shots).is_err());
    }

    #[test]
    fn combined_scale_flags_distance_blunder() {
        // All distances inflated 1% -> scale ~1.01, station still ~recovered.
        let station = (500.0, 500.0);
        let known = [(800.0, 700.0), (300.0, 750.0), (520.0, 200.0)];
        let shots: Vec<ResectionShot> = known
            .iter()
            .map(|&k| {
                let (az, d) = az_dist(station, k);
                ResectionShot { known: k, direction_deg: az, distance: Some(d * 1.01), name: "X".into() }
            })
            .collect();
        let r = resection_combined(&shots).unwrap();
        // Instrument distances inflated 1% -> fit maps inflated-local to true
        // world at scale 1/1.01 (world/instrument ratio).
        assert!(close(r.scale, 1.0 / 1.01, 1e-6), "scale {}", r.scale);
        assert!(r.scale_blunder(0.001));
    }

    #[test]
    fn three_point_recovers_station() {
        // Known control and a station inside the triangle.
        let a = (0.0, 0.0);
        let b = (1000.0, 0.0);
        let c = (500.0, 900.0);
        let p = (500.0, 300.0);
        // Subtended angles at P: ∠BPC, ∠CPA, ∠APB.
        let ang = [angle_at(p, b, c), angle_at(p, c, a), angle_at(p, a, b)];
        // Sanity: interior station -> the three angles close 360°.
        assert!(close(ang[0] + ang[1] + ang[2], 360.0, 1e-6));
        let solved = resection_three_point(a, b, c, ang).unwrap();
        assert!(close(solved.0, p.0, 1e-6) && close(solved.1, p.1, 1e-6), "got {:?}", solved);
    }

    #[test]
    fn three_point_detects_danger_circle() {
        // Put P on the circumcircle of A,B,C: the three angles satisfy the
        // inscribed-angle relation, so the Tienstra weights blow up.
        let a: (f64, f64) = (0.0, 0.0);
        let b: (f64, f64) = (1000.0, 0.0);
        let c: (f64, f64) = (500.0, 866.025_403_8); // ~equilateral
        let cen: (f64, f64) = (500.0, 288.675_134_6); // circumcircle centre
        let rad: f64 = (a.0 - cen.0).hypot(a.1 - cen.1);
        // A point on the same circumcircle, at 200° from the centre.
        let th: f64 = 200f64.to_radians();
        let p: (f64, f64) = (cen.0 + rad * th.cos(), cen.1 + rad * th.sin());
        let ang = [angle_at(p, b, c), angle_at(p, c, a), angle_at(p, a, b)];
        assert!(resection_three_point(a, b, c, ang).is_err(), "should flag danger circle");
    }

    #[test]
    fn parse_handles_blank_distance_and_names() {
        let text = "# N, E, dir, dist, name\n\
                    2400, 1300, 45.0, 500.0, CP1\n\
                    2350, 700, 312.5, , CP2\n\
                    1650 1100 178.0 640.0 CP3\n";
        let shots = parse_resection_shots(text);
        assert_eq!(shots.len(), 3);
        assert_eq!(shots[0].known, (1300.0, 2400.0)); // (E, N)
        assert_eq!(shots[0].distance, Some(500.0));
        assert_eq!(shots[1].distance, None); // blank -> angle-only
        assert_eq!(shots[1].name, "CP2");
        assert_eq!(shots[2].distance, Some(640.0));
    }
}
