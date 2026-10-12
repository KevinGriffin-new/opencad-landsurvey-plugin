//! Road alignment geometry: clothoid spirals, horizontal stationing, vertical
//! (profile) curves and cross sections — `std` only.
//!
//! Plan coordinates are `[E, N]` (world X, Y), angles are math angles in
//! radians (CCW from +X), and curvature is positive for a CCW (left) turn.

/// A clothoid (Euler spiral) whose curvature varies linearly with length,
/// from `1/r_start` to `1/r_end` (`f64::INFINITY` = tangent).
///
/// Only the two end points, the length and the radii are used to place it:
/// the canonical shape is integrated from the origin and then fitted onto
/// `start`→`end` by a rotation (and a scale that is 1 for consistent data).
/// The tangent PI is *derived*, so a file's own PI checks the geometry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Clothoid {
    pub start: [f64; 2],
    pub end: [f64; 2],
    pub length: f64,
    pub r_start: f64,
    pub r_end: f64,
    pub ccw: bool,
}

impl Clothoid {
    /// Signed curvature at the start and end.
    fn curvatures(&self) -> (f64, f64) {
        let sign = if self.ccw { 1.0 } else { -1.0 };
        let k = |r: f64| if r.is_finite() && r != 0.0 { sign / r } else { 0.0 };
        (k(self.r_start), k(self.r_end))
    }

    /// Heading change from the start tangent at arc length `s`.
    fn turn(&self, s: f64) -> f64 {
        let (k0, k1) = self.curvatures();
        k0 * s + (k1 - k0) * s * s / (2.0 * self.length)
    }

    /// Canonical point at arc length `s`: start at the origin heading +X.
    /// Simpson's rule over the heading integral.
    fn local(&self, s: f64) -> [f64; 2] {
        if s <= 0.0 {
            return [0.0, 0.0];
        }
        let n = ((s / self.length.max(1e-12)) * 512.0).ceil().max(16.0) as usize * 2;
        let h = s / n as f64;
        let (mut x, mut y) = (0.0, 0.0);
        for i in 0..=n {
            let w = if i == 0 || i == n {
                1.0
            } else if i % 2 == 1 {
                4.0
            } else {
                2.0
            };
            let t = self.turn(i as f64 * h);
            x += w * t.cos();
            y += w * t.sin();
        }
        [x * h / 3.0, y * h / 3.0]
    }

    /// `(rotation, scale)` mapping the canonical shape onto start→end.
    fn fit(&self) -> (f64, f64) {
        let e = self.local(self.length);
        let (dx, dy) = (self.end[0] - self.start[0], self.end[1] - self.start[1]);
        let rot = dy.atan2(dx) - e[1].atan2(e[0]);
        let scale = dx.hypot(dy) / e[0].hypot(e[1]);
        (rot, scale)
    }

    /// Point and heading (math angle) at arc length `s` from the start.
    pub fn at(&self, s: f64) -> ([f64; 2], f64) {
        let (rot, scale) = self.fit();
        let p = self.local(s.clamp(0.0, self.length));
        let (c, sn) = (rot.cos(), rot.sin());
        (
            [
                self.start[0] + scale * (p[0] * c - p[1] * sn),
                self.start[1] + scale * (p[0] * sn + p[1] * c),
            ],
            rot + self.turn(s.clamp(0.0, self.length)),
        )
    }

    /// Heading at the start and the end.
    pub fn headings(&self) -> (f64, f64) {
        (self.at(0.0).1, self.at(self.length).1)
    }

    /// How far the endpoints are from what the length and radii imply:
    /// `|scale − 1|`. Consistent data is ~1e-9; > 1e-4 means the file's
    /// spiral parameters and coordinates disagree.
    pub fn misfit(&self) -> f64 {
        (self.fit().1 - 1.0).abs()
    }

    /// The tangent intersection (PI) and the two tangent lengths
    /// `(start→PI, PI→end)`.
    pub fn pi(&self) -> ([f64; 2], f64, f64) {
        let (h0, h1) = self.headings();
        let (d0, d1) = ([h0.cos(), h0.sin()], [h1.cos(), h1.sin()]);
        // start + t·d0 = end − u·d1
        let (bx, by) = (self.end[0] - self.start[0], self.end[1] - self.start[1]);
        let det = d0[0] * d1[1] - d0[1] * d1[0];
        if det.abs() < 1e-15 {
            let mid = [(self.start[0] + self.end[0]) / 2.0, (self.start[1] + self.end[1]) / 2.0];
            let half = bx.hypot(by) / 2.0;
            return (mid, half, half);
        }
        let t = (bx * d1[1] - by * d1[0]) / det;
        let u = (d0[0] * by - d0[1] * bx) / det;
        ([self.start[0] + t * d0[0], self.start[1] + t * d0[1]], t, u)
    }

    /// Points along the spiral (start included, end excluded) spaced so the
    /// chord sag stays small: at least 8, about one per 1/32 of the length.
    pub fn densify(&self) -> Vec<[f64; 2]> {
        let n = 32usize;
        (0..n)
            .map(|i| self.at(self.length * i as f64 / n as f64).0)
            .collect()
    }
}

/// Format a station as `k+mmm.mmm` (metric) or `s+ff.ff` (imperial), e.g.
/// `0+049.841` / `12+34.56`.
pub fn format_station(sta: f64, metric: bool) -> String {
    let (unit, digits, dp) = if metric { (1000.0, 3, 3) } else { (100.0, 2, 2) };
    let neg = sta < 0.0;
    let mut a = sta.abs();
    // Round first so 99.999 doesn't print as 0+100.00.
    let scale = 10f64.powi(dp);
    a = (a * scale).round() / scale;
    let major = (a / unit).floor();
    let minor = a - major * unit;
    let width = digits + 1 + dp as usize;
    format!(
        "{}{}+{:0width$.dp$}",
        if neg { "-" } else { "" },
        major as i64,
        minor,
        dp = dp as usize
    )
}

/// A "nice" interval (1, 2, 2.5 or 5 × 10ⁿ) giving about `target` steps
/// across `span`.
pub fn nice_interval(span: f64, target: f64) -> f64 {
    if span <= 0.0 || !span.is_finite() {
        return 1.0;
    }
    let raw = span / target;
    let mag = 10f64.powf(raw.log10().floor());
    for m in [1.0, 2.0, 2.5, 5.0, 10.0] {
        if m * mag >= raw {
            return m * mag;
        }
    }
    10.0 * mag
}

/// A vertical PVI: station, elevation and the length of the symmetric
/// parabolic vertical curve centred on it (0 = no curve).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pvi {
    pub sta: f64,
    pub elev: f64,
    pub curve_len: f64,
}

/// Elevation of a design profile at `sta`: grade lines between PVIs, and
/// the symmetric parabola `y = y_bvc + g1·x + (g2 − g1)·x² / 2L` through each
/// vertical curve. Stations outside the profile extend the end grades.
pub fn profile_elevation(pvis: &[Pvi], sta: f64) -> Option<f64> {
    let n = pvis.len();
    if n == 0 {
        return None;
    }
    if n == 1 {
        return Some(pvis[0].elev);
    }
    let grade = |i: usize| (pvis[i + 1].elev - pvis[i].elev) / (pvis[i + 1].sta - pvis[i].sta);
    for i in 1..n - 1 {
        let l = pvis[i].curve_len;
        if l > 0.0 {
            let bvc = pvis[i].sta - l / 2.0;
            if sta >= bvc && sta <= pvis[i].sta + l / 2.0 {
                let (g1, g2) = (grade(i - 1), grade(i));
                let y_bvc = pvis[i].elev - g1 * l / 2.0;
                let x = sta - bvc;
                return Some(y_bvc + g1 * x + (g2 - g1) * x * x / (2.0 * l));
            }
        }
    }
    let i = (0..n - 1)
        .find(|&i| sta <= pvis[i + 1].sta)
        .unwrap_or(n - 2);
    Some(pvis[i].elev + grade(i) * (sta - pvis[i].sta))
}

/// Points `(sta, elev)` along a design profile: PVIs on tangent, each vertical
/// curve sampled at 16 intervals.
pub fn profile_points(pvis: &[Pvi]) -> Vec<[f64; 2]> {
    let mut out = Vec::new();
    for (i, p) in pvis.iter().enumerate() {
        let interior = i > 0 && i + 1 < pvis.len();
        if interior && p.curve_len > 0.0 {
            let bvc = p.sta - p.curve_len / 2.0;
            for k in 0..=16 {
                let s = bvc + p.curve_len * k as f64 / 16.0;
                if let Some(e) = profile_elevation(pvis, s) {
                    out.push([s, e]);
                }
            }
        } else {
            out.push([p.sta, p.elev]);
        }
    }
    out
}

/// A profile along an alignment: a design profile by PVIs (`<ProfAlign>`) or
/// a sampled surface profile `(sta, elev)` (`<ProfSurf>`).
#[derive(Debug, Clone, PartialEq)]
pub enum ProfileData {
    Design(Vec<Pvi>),
    Surface(Vec<[f64; 2]>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    pub name: String,
    pub desc: String,
    pub data: ProfileData,
}

impl Profile {
    /// `(sta, elev)` points to draw.
    pub fn points(&self) -> Vec<[f64; 2]> {
        match &self.data {
            ProfileData::Design(p) => profile_points(p),
            ProfileData::Surface(p) => p.clone(),
        }
    }
}

/// One surface line in a cross section: `(offset, elev)` points, offset
/// positive right of the alignment.
#[derive(Debug, Clone, PartialEq)]
pub struct SectionSurface {
    pub name: String,
    pub desc: String,
    pub points: Vec<[f64; 2]>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CrossSection {
    pub sta: f64,
    pub name: String,
    pub surfaces: Vec<SectionSurface>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tangent-to-curve clothoid against the textbook series
    /// x = L(1 − θ²/10 + θ⁴/216), y = L(θ/3 − θ³/42 + θ⁵/1320), θ = L/2R.
    #[test]
    fn clothoid_matches_series() {
        let (l, r) = (60.0, 150.0);
        let th: f64 = l / (2.0 * r);
        let x = l * (1.0 - th.powi(2) / 10.0 + th.powi(4) / 216.0);
        let y = l * (th / 3.0 - th.powi(3) / 42.0 + th.powi(5) / 1320.0);
        let c = Clothoid {
            start: [0.0, 0.0],
            end: [x, y],
            length: l,
            r_start: f64::INFINITY,
            r_end: r,
            ccw: true,
        };
        assert!(c.misfit() < 1e-7, "misfit {}", c.misfit());
        let (h0, h1) = c.headings();
        assert!(h0.abs() < 1e-7);
        assert!((h1 - th).abs() < 1e-7);
        // Classic tangent lengths: long = x − y/tanθ, short = y/sinθ.
        let (_, long, short) = c.pi();
        assert!((long - (x - y / th.tan())).abs() < 1e-6);
        assert!((short - y / th.sin()).abs() < 1e-6);
    }

    #[test]
    fn clothoid_cw_mirrors_ccw() {
        let base = Clothoid {
            start: [0.0, 0.0],
            end: [59.93, 3.997],
            length: 60.0,
            r_start: f64::INFINITY,
            r_end: 150.0,
            ccw: true,
        };
        let cw = Clothoid {
            end: [59.93, -3.997],
            ccw: false,
            ..base
        };
        let (a, b) = (base.at(30.0).0, cw.at(30.0).0);
        assert!((a[0] - b[0]).abs() < 1e-9 && (a[1] + b[1]).abs() < 1e-9);
    }

    #[test]
    fn stations_format() {
        assert_eq!(format_station(49.840637, true), "0+049.841");
        assert_eq!(format_station(1234.5678, true), "1+234.568");
        assert_eq!(format_station(1234.567, false), "12+34.57");
        assert_eq!(format_station(99.999, false), "1+00.00");
        assert_eq!(format_station(-12.5, true), "-0+012.500");
    }

    #[test]
    fn nice_intervals() {
        assert_eq!(nice_interval(507.0, 20.0), 50.0);
        assert_eq!(nice_interval(5651.0, 20.0), 500.0);
        assert_eq!(nice_interval(90.0, 10.0), 10.0);
    }

    /// Textbook crest curve: PVI 10+00 @ 100.00, g1 +2 %, g2 −1 %, L = 400.
    #[test]
    fn parabolic_vertical_curve() {
        let pvis = [
            Pvi { sta: 0.0, elev: 80.0, curve_len: 0.0 },
            Pvi { sta: 1000.0, elev: 100.0, curve_len: 400.0 },
            Pvi { sta: 2000.0, elev: 90.0, curve_len: 0.0 },
        ];
        let e = |s| profile_elevation(&pvis, s).unwrap();
        assert!((e(800.0) - 96.0).abs() < 1e-12); // BVC on the +2 % grade
        assert!((e(1200.0) - 98.0).abs() < 1e-12); // EVC on the −1 % grade
        // Mid-curve: y_bvc + g1·200 + (g2−g1)·200²/800 = 96 + 4 − 1.5.
        assert!((e(1000.0) - 98.5).abs() < 1e-12);
        // Tangents outside the curve.
        assert!((e(500.0) - 90.0).abs() < 1e-12);
        assert!((e(1500.0) - 95.0).abs() < 1e-12);
        let pts = profile_points(&pvis);
        assert_eq!(pts.len(), 1 + 17 + 1);
        assert_eq!(pts[1], [800.0, 96.0]);
    }
}
