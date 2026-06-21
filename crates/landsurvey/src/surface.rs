//! TIN surfaces + earthwork volumes — pure functions, `std` only (no
//! host/CAD/iced/acadrust), so this builds unchanged for CLI/WASM.
//!
//! A [`Surface`] is `nodes` (`[E, N, Z]`) + `triangles` (index triples into
//! `nodes`). Volumes use exact per-facet integration: for a planar TIN triangle
//! the prism volume to a datum is `area_xy * mean_z` (exact, since the facet is
//! linear). Two earthwork paths are provided:
//!
//! * [`Surface::cut_fill_to_datum`] — surface vs a horizontal plane (exact).
//! * [`grid_cut_fill`] — two independent surfaces by the grid (column) method
//!   (approximate, step-sensitive; matches the way MicroSurvey's Earthwork
//!   tutorial samples a TOP/BOTTOM pair that do not share triangulation).
//! * [`exact_composite_cut_fill`] — two independent surfaces by exact TIN
//!   overlay (clip each top facet against each bottom facet; on every convex
//!   overlay cell both surfaces are linear so `dz = top - bottom` is linear and
//!   integrates exactly, split at the `dz = 0` contour for cut/fill).
//!
//! Convention matches [`crate::cogo`]: Easting -> world X, Northing -> world Y.

use serde::Serialize;

/// A node carries `[easting (x), northing (y), elevation (z)]`.
pub type Node = [f64; 3];
/// A triangle is three indices into a surface's `nodes`.
pub type Tri = [usize; 3];

const EPS: f64 = 1e-12;

/// Cut / fill / net earthwork volumes (cubic world units).
///
/// `cut` is material above the reference (top above bottom, or surface above
/// datum); `fill` is below; `net = cut - fill`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct CutFill {
    pub cut: f64,
    pub fill: f64,
    pub net: f64,
}

impl CutFill {
    fn from_cut_fill(cut: f64, fill: f64) -> Self {
        CutFill {
            cut,
            fill,
            net: cut - fill,
        }
    }
}

/// Result of the grid (column) volume between two independent surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct GridVolume {
    pub cut: f64,
    pub fill: f64,
    pub net: f64,
    /// Plan area of cells where both surfaces were defined.
    pub plan_area: f64,
    /// Number of contributing cells.
    pub n_cells: usize,
}

/// A triangulated irregular network.
#[derive(Debug, Clone, PartialEq)]
pub struct Surface {
    pub nodes: Vec<Node>,
    pub triangles: Vec<Tri>,
}

impl Surface {
    /// Build a TIN by Delaunay-triangulating the points' XY.
    pub fn from_points(points: &[Node]) -> Surface {
        let xy: Vec<[f64; 2]> = points.iter().map(|p| [p[0], p[1]]).collect();
        Surface {
            nodes: points.to_vec(),
            triangles: delaunay(&xy),
        }
    }

    fn tri_area_xy(&self, t: Tri) -> f64 {
        let (a, b, c) = (self.nodes[t[0]], self.nodes[t[1]], self.nodes[t[2]]);
        ((b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1])).abs() / 2.0
    }

    /// Total 2-D (plan) area of the triangulation.
    pub fn area_2d(&self) -> f64 {
        self.triangles.iter().map(|&t| self.tri_area_xy(t)).sum()
    }

    /// Unique TIN edges as node-index pairs (each shared edge listed once).
    /// Used to draw the triangulation without doubling shared edges.
    pub fn edges(&self) -> Vec<[usize; 2]> {
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for &t in &self.triangles {
            for e in [[t[0], t[1]], [t[1], t[2]], [t[2], t[0]]] {
                let key = if e[0] < e[1] { (e[0], e[1]) } else { (e[1], e[0]) };
                if seen.insert(key) {
                    out.push([key.0, key.1]);
                }
            }
        }
        out
    }

    /// Plan-view bounding box `(min_x, max_x, min_y, max_y)` of the nodes.
    pub fn extent(&self) -> (f64, f64, f64, f64) {
        let mut minx = f64::INFINITY;
        let mut maxx = f64::NEG_INFINITY;
        let mut miny = f64::INFINITY;
        let mut maxy = f64::NEG_INFINITY;
        for n in &self.nodes {
            minx = minx.min(n[0]);
            maxx = maxx.max(n[0]);
            miny = miny.min(n[1]);
            maxy = maxy.max(n[1]);
        }
        (minx, maxx, miny, maxy)
    }

    /// Signed net volume between the surface and a horizontal `datum` plane
    /// (above datum positive). Exact for planar TIN facets.
    pub fn volume_to_datum(&self, datum: f64) -> f64 {
        self.triangles
            .iter()
            .map(|&t| {
                let zmean =
                    (self.nodes[t[0]][2] + self.nodes[t[1]][2] + self.nodes[t[2]][2]) / 3.0;
                self.tri_area_xy(t) * (zmean - datum)
            })
            .sum()
    }

    /// Exact cut/fill/net volumes vs a horizontal `datum` plane. Triangles that
    /// cross the datum are split exactly at the datum contour.
    pub fn cut_fill_to_datum(&self, datum: f64) -> CutFill {
        self.cut_fill_to_datum_detailed(datum).0
    }

    /// As [`Surface::cut_fill_to_datum`], but also returns the datum contour —
    /// the `z = datum` crossing segments (plan view), i.e. the cut/fill outline.
    pub fn cut_fill_to_datum_detailed(&self, datum: f64) -> (CutFill, Vec<[[f64; 2]; 2]>) {
        let (mut cut, mut fill) = (0.0, 0.0);
        let mut contour = Vec::new();
        for &t in &self.triangles {
            let tri = [
                [self.nodes[t[0]][0], self.nodes[t[0]][1], self.nodes[t[0]][2] - datum],
                [self.nodes[t[1]][0], self.nodes[t[1]][1], self.nodes[t[1]][2] - datum],
                [self.nodes[t[2]][0], self.nodes[t[2]][1], self.nodes[t[2]][2] - datum],
            ];
            let (cu, fi, seg) = tri_cut_fill_seg(&tri);
            cut += cu;
            fill += fi;
            if let Some(s) = seg {
                contour.push(s);
            }
        }
        (CutFill::from_cut_fill(cut, fill), contour)
    }

    /// TIN elevation at `(x, y)` by barycentric interpolation over the
    /// containing triangle, or `None` if `(x, y)` is outside every triangle.
    pub fn interpolate_z(&self, x: f64, y: f64) -> Option<f64> {
        for &t in &self.triangles {
            let (a, b, c) = (self.nodes[t[0]], self.nodes[t[1]], self.nodes[t[2]]);
            let d = (b[1] - c[1]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[1] - c[1]);
            if d.abs() < EPS {
                continue;
            }
            let wa = ((b[1] - c[1]) * (x - c[0]) + (c[0] - b[0]) * (y - c[1])) / d;
            let wb = ((c[1] - a[1]) * (x - c[0]) + (a[0] - c[0]) * (y - c[1])) / d;
            let wc = 1.0 - wa - wb;
            if wa >= -1e-9 && wb >= -1e-9 && wc >= -1e-9 {
                return Some(wa * a[2] + wb * b[2] + wc * c[2]);
            }
        }
        None
    }
}

/// Net volume (top minus bottom) for two surfaces that SHARE XY topology
/// (identical node XY + identical triangles). Exact for planar facets. Returns
/// `Err` if the triangulations differ.
pub fn volume_between_shared(top: &Surface, bottom: &Surface) -> Result<f64, &'static str> {
    if top.triangles != bottom.triangles {
        return Err("volume_between_shared requires identical triangulation/topology");
    }
    let mut v = 0.0;
    for &t in &top.triangles {
        let dz = t
            .iter()
            .map(|&i| top.nodes[i][2] - bottom.nodes[i][2])
            .sum::<f64>()
            / 3.0;
        v += top.tri_area_xy(t) * dz;
    }
    Ok(v)
}

/// Cut/fill/net between two independent surfaces by the grid (column) method.
///
/// A regular grid of `grid_step` spacing covers the shared extent; at each cell
/// centre both surfaces are sampled. Cells where BOTH have a defined elevation
/// contribute `(z_top - z_bottom) * cell_area` — positive as cut, negative as
/// fill. Does not require shared triangulation; accuracy depends on `grid_step`.
pub fn grid_cut_fill(top: &Surface, bottom: &Surface, grid_step: f64) -> GridVolume {
    let (minx0, maxx0, miny0, maxy0) = top.extent();
    let (minx1, maxx1, miny1, maxy1) = bottom.extent();
    let (minx, maxx) = (minx0.min(minx1), maxx0.max(maxx1));
    let (miny, maxy) = (miny0.min(miny1), maxy0.max(maxy1));

    let cell_area = grid_step * grid_step;
    let (mut cut, mut fill, mut plan_area) = (0.0, 0.0, 0.0);
    let mut n_cells = 0usize;

    let mut y = miny + grid_step / 2.0;
    while y <= maxy - grid_step / 2.0 + EPS {
        let mut x = minx + grid_step / 2.0;
        while x <= maxx - grid_step / 2.0 + EPS {
            if let (Some(zt), Some(zb)) = (top.interpolate_z(x, y), bottom.interpolate_z(x, y)) {
                let dz = zt - zb;
                if dz >= 0.0 {
                    cut += dz * cell_area;
                } else {
                    fill += -dz * cell_area;
                }
                plan_area += cell_area;
                n_cells += 1;
            }
            x += grid_step;
        }
        y += grid_step;
    }
    GridVolume {
        cut,
        fill,
        net: cut - fill,
        plan_area,
        n_cells,
    }
}

/// Detailed composite-volume result: the cut/fill volumes plus the cut/fill
/// boundary — the `dz = 0` contour over the overlap, as plan-view segments
/// `[[x1,y1],[x2,y2]]`. The boundary is the line you draw between cut and fill
/// regions; it is empty when one surface is entirely above the other.
#[derive(Debug, Clone, PartialEq)]
pub struct CompositeResult {
    pub cut_fill: CutFill,
    pub cutfill_line: Vec<[[f64; 2]; 2]>,
}

/// Exact cut/fill/net between two independent surfaces by TIN overlay.
///
/// Clips each top facet against each bottom facet in plan; on every convex
/// overlay cell both surfaces are linear, so `dz = top - bottom` is linear and
/// integrates exactly. The cell is split at the `dz = 0` contour to separate cut
/// from fill. Only the region covered by BOTH surfaces contributes. `O(Nt*Nb)`,
/// fine for tutorial-sized surfaces.
pub fn exact_composite_cut_fill(top: &Surface, bottom: &Surface) -> CutFill {
    composite_cut_fill_detailed(top, bottom).cut_fill
}

/// Like [`exact_composite_cut_fill`] but also returns the cut/fill boundary
/// segments (the `dz = 0` contour over the overlap) for drawing.
pub fn composite_cut_fill_detailed(top: &Surface, bottom: &Surface) -> CompositeResult {
    let (mut cut, mut fill) = (0.0, 0.0);
    let mut cutfill_line = Vec::new();
    for &tt in &top.triangles {
        let pt = [top.nodes[tt[0]], top.nodes[tt[1]], top.nodes[tt[2]]];
        let plane_t = match Plane::through(&pt) {
            Some(p) => p,
            None => continue, // vertical/degenerate in plan
        };
        let poly_t = ccw([[pt[0][0], pt[0][1]], [pt[1][0], pt[1][1]], [pt[2][0], pt[2][1]]]);
        let bbt = bbox(&poly_t);

        for &tb in &bottom.triangles {
            let pb = [bottom.nodes[tb[0]], bottom.nodes[tb[1]], bottom.nodes[tb[2]]];
            let poly_b = [[pb[0][0], pb[0][1]], [pb[1][0], pb[1][1]], [pb[2][0], pb[2][1]]];
            if bbox_disjoint(bbt, bbox(&poly_b)) {
                continue;
            }
            let plane_b = match Plane::through(&pb) {
                Some(p) => p,
                None => continue,
            };
            let cell = convex_clip(&poly_t, &ccw(poly_b));
            if cell.len() < 3 {
                continue;
            }
            // Fan-triangulate the convex overlay cell; on each sub-triangle dz is
            // linear, so accumulate exact cut/fill relative to dz = 0 and collect
            // the zero-crossing segment (the cut/fill line).
            for i in 1..cell.len() - 1 {
                let v = [cell[0], cell[i], cell[i + 1]];
                let tri = [
                    [v[0][0], v[0][1], plane_t.z(v[0][0], v[0][1]) - plane_b.z(v[0][0], v[0][1])],
                    [v[1][0], v[1][1], plane_t.z(v[1][0], v[1][1]) - plane_b.z(v[1][0], v[1][1])],
                    [v[2][0], v[2][1], plane_t.z(v[2][0], v[2][1]) - plane_b.z(v[2][0], v[2][1])],
                ];
                let (cu, fi, seg) = tri_cut_fill_seg(&tri);
                cut += cu;
                fill += fi;
                if let Some(s) = seg {
                    cutfill_line.push(s);
                }
            }
        }
    }
    CompositeResult {
        cut_fill: CutFill::from_cut_fill(cut, fill),
        cutfill_line,
    }
}

// --- planar-facet volume primitives (h measured relative to 0) ---------------

/// Signed prism volume between a planar triangle `[(x,y,h);3]` and the `h = 0`
/// plane; positive when the facet is above 0.
fn tri_signed_prism(tri: &[[f64; 3]; 3]) -> f64 {
    let (a, b, c) = (tri[0], tri[1], tri[2]);
    let area = ((b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1])).abs() / 2.0;
    area * (a[2] + b[2] + c[2]) / 3.0
}

/// `(cut, fill)` for one triangle relative to `h = 0`, splitting at the `h = 0`
/// contour when the facet crosses it; also returns the `h = 0` crossing segment
/// (in plan) when the facet straddles the datum — the local cut/fill-line piece.
fn tri_cut_fill_seg(tri: &[[f64; 3]; 3]) -> (f64, f64, Option<[[f64; 2]; 2]>) {
    let hs = [tri[0][2], tri[1][2], tri[2][2]];
    let pos: Vec<usize> = (0..3).filter(|&i| hs[i] > 0.0).collect();
    let neg: Vec<usize> = (0..3).filter(|&i| hs[i] < 0.0).collect();
    if neg.is_empty() {
        return (tri_signed_prism(tri).max(0.0), 0.0, None);
    }
    if pos.is_empty() {
        return (0.0, (-tri_signed_prism(tri)).max(0.0), None);
    }
    // Crosses the datum: the lone vertex is on the minority side.
    let lone = if pos.len() == 1 { pos[0] } else { neg[0] };
    let others: Vec<usize> = (0..3).filter(|&i| i != lone).collect();
    let cross = |p: [f64; 3], q: [f64; 3]| -> [f64; 3] {
        let t = p[2] / (p[2] - q[2]); // h-zero crossing along edge p->q
        [p[0] + t * (q[0] - p[0]), p[1] + t * (q[1] - p[1]), 0.0]
    };
    let (vl, va, vb) = (tri[lone], tri[others[0]], tri[others[1]]);
    let cla = cross(vl, va);
    let clb = cross(vl, vb);
    let (mut cut, mut fill) = (0.0, 0.0);
    for sub in [[vl, cla, clb], [cla, va, vb], [cla, vb, clb]] {
        let v = tri_signed_prism(&sub);
        if v >= 0.0 {
            cut += v;
        } else {
            fill += -v;
        }
    }
    (cut, fill, Some([[cla[0], cla[1]], [clb[0], clb[1]]]))
}

// --- plane through 3 points (z as a linear function of x,y) ------------------

struct Plane {
    a: f64,
    b: f64,
    c: f64,
}

impl Plane {
    /// Plane `z = a*x + b*y + c` through three `[x,y,z]` points, or `None` if
    /// the triangle is degenerate (zero plan area / vertical).
    fn through(p: &[[f64; 3]; 3]) -> Option<Plane> {
        let (p0, p1, p2) = (p[0], p[1], p[2]);
        let ux = p1[0] - p0[0];
        let uy = p1[1] - p0[1];
        let uz = p1[2] - p0[2];
        let vx = p2[0] - p0[0];
        let vy = p2[1] - p0[1];
        let vz = p2[2] - p0[2];
        let nx = uy * vz - uz * vy;
        let ny = uz * vx - ux * vz;
        let nz = ux * vy - uy * vx;
        if nz.abs() < EPS {
            return None;
        }
        // n . (X - p0) = 0  =>  z = z0 - (nx*(x-x0) + ny*(y-y0)) / nz
        let a = -nx / nz;
        let b = -ny / nz;
        let c = p0[2] - a * p0[0] - b * p0[1];
        Some(Plane { a, b, c })
    }

    fn z(&self, x: f64, y: f64) -> f64 {
        self.a * x + self.b * y + self.c
    }
}

// --- convex polygon clipping (Sutherland-Hodgman) ----------------------------

fn bbox(poly: &[[f64; 2]]) -> (f64, f64, f64, f64) {
    let mut minx = f64::INFINITY;
    let mut maxx = f64::NEG_INFINITY;
    let mut miny = f64::INFINITY;
    let mut maxy = f64::NEG_INFINITY;
    for p in poly {
        minx = minx.min(p[0]);
        maxx = maxx.max(p[0]);
        miny = miny.min(p[1]);
        maxy = maxy.max(p[1]);
    }
    (minx, maxx, miny, maxy)
}

fn bbox_disjoint(a: (f64, f64, f64, f64), b: (f64, f64, f64, f64)) -> bool {
    a.1 < b.0 || b.1 < a.0 || a.3 < b.2 || b.3 < a.2
}

/// Return the triangle wound counter-clockwise.
fn ccw(t: [[f64; 2]; 3]) -> Vec<[f64; 2]> {
    let area2 = (t[1][0] - t[0][0]) * (t[2][1] - t[0][1])
        - (t[2][0] - t[0][0]) * (t[1][1] - t[0][1]);
    if area2 < 0.0 {
        vec![t[0], t[2], t[1]]
    } else {
        vec![t[0], t[1], t[2]]
    }
}

/// Clip convex `subject` by convex `clip` (both CCW). Returns the convex
/// intersection polygon (possibly empty).
fn convex_clip(subject: &[[f64; 2]], clip: &[[f64; 2]]) -> Vec<[f64; 2]> {
    let mut output: Vec<[f64; 2]> = subject.to_vec();
    for i in 0..clip.len() {
        if output.is_empty() {
            break;
        }
        let a = clip[i];
        let b = clip[(i + 1) % clip.len()];
        let input = std::mem::take(&mut output);
        // Inside = left of (or on) directed edge a->b, for CCW clip.
        let inside = |p: [f64; 2]| -> f64 {
            (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])
        };
        for j in 0..input.len() {
            let cur = input[j];
            let prev = input[(j + input.len() - 1) % input.len()];
            let (dc, dp) = (inside(cur), inside(prev));
            let cur_in = dc >= -EPS;
            let prev_in = dp >= -EPS;
            if cur_in {
                if !prev_in {
                    output.push(segment_intersect(prev, cur, a, b));
                }
                output.push(cur);
            } else if prev_in {
                output.push(segment_intersect(prev, cur, a, b));
            }
        }
    }
    output
}

/// Intersection of segment `p->q` with the infinite line through `a->b`.
fn segment_intersect(p: [f64; 2], q: [f64; 2], a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    let r = [q[0] - p[0], q[1] - p[1]];
    let s = [b[0] - a[0], b[1] - a[1]];
    let denom = r[0] * s[1] - r[1] * s[0];
    if denom.abs() < EPS {
        return p; // parallel — shouldn't happen for a real crossing
    }
    let t = ((a[0] - p[0]) * s[1] - (a[1] - p[1]) * s[0]) / denom;
    [p[0] + t * r[0], p[1] + t * r[1]]
}

// --- Delaunay triangulation (Bowyer-Watson) ----------------------------------

/// `>0` if `a,b,c` are counter-clockwise, `<0` if clockwise, `0` if collinear.
fn orient2d(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

fn in_circumcircle(a: [f64; 2], mut b: [f64; 2], mut c: [f64; 2], p: [f64; 2]) -> bool {
    if orient2d(a, b, c) < 0.0 {
        std::mem::swap(&mut b, &mut c);
    }
    let (ax, ay) = (a[0] - p[0], a[1] - p[1]);
    let (bx, by) = (b[0] - p[0], b[1] - p[1]);
    let (cx, cy) = (c[0] - p[0], c[1] - p[1]);
    let det = (ax * ax + ay * ay) * (bx * cy - by * cx)
        - (bx * bx + by * by) * (ax * cy - ay * cx)
        + (cx * cx + cy * cy) * (ax * by - ay * bx);
    det > 1e-12
}

/// Delaunay-triangulate 2-D points; returns `(i, j, k)` index triples (CCW)
/// into `points`.
pub fn delaunay(points: &[[f64; 2]]) -> Vec<Tri> {
    let n = points.len();
    if n < 3 {
        return Vec::new();
    }
    let (mut minx, mut maxx) = (f64::INFINITY, f64::NEG_INFINITY);
    let (mut miny, mut maxy) = (f64::INFINITY, f64::NEG_INFINITY);
    for p in points {
        minx = minx.min(p[0]);
        maxx = maxx.max(p[0]);
        miny = miny.min(p[1]);
        maxy = maxy.max(p[1]);
    }
    let dm = (maxx - minx).max(maxy - miny).max(1.0);
    let midx = (minx + maxx) / 2.0;
    let midy = (miny + maxy) / 2.0;

    // points + super-triangle vertices (indices n, n+1, n+2)
    let mut pts = points.to_vec();
    pts.push([midx - 20.0 * dm, midy - dm]);
    pts.push([midx, midy + 20.0 * dm]);
    pts.push([midx + 20.0 * dm, midy - dm]);

    let mut tris: Vec<Tri> = vec![[n, n + 1, n + 2]];

    for ip in 0..n {
        let p = pts[ip];
        let bad: Vec<Tri> = tris
            .iter()
            .copied()
            .filter(|t| in_circumcircle(pts[t[0]], pts[t[1]], pts[t[2]], p))
            .collect();

        // Boundary of the polygonal hole: edges shared by exactly one bad tri.
        let mut edge_count: Vec<((usize, usize), usize)> = Vec::new();
        for t in &bad {
            for e in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                let key = if e.0 < e.1 { e } else { (e.1, e.0) };
                if let Some(entry) = edge_count.iter_mut().find(|(k, _)| *k == key) {
                    entry.1 += 1;
                } else {
                    edge_count.push((key, 1));
                }
            }
        }

        let bad_set = bad;
        tris.retain(|t| !bad_set.contains(t));
        for ((a, b), c) in edge_count.into_iter() {
            if c == 1 {
                tris.push([a, b, ip]);
            }
        }
    }

    // Drop triangles touching the super-triangle; orient CCW.
    let mut out = Vec::new();
    for t in tris {
        if t[0] >= n || t[1] >= n || t[2] >= n {
            continue;
        }
        if orient2d(points[t[0]], points[t[1]], points[t[2]]) < 0.0 {
            out.push([t[0], t[2], t[1]]);
        } else {
            out.push([t[0], t[1], t[2]]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unit-ish square (0..W) at constant elevation `z`, split into 2 tris on
    /// the given diagonal. `diag = false` splits 0-1-2 / 0-2-3; `true` the other
    /// way, so two surfaces can be given *different* topology over the same box.
    fn flat_square(w: f64, z: f64, diag: bool) -> Surface {
        let nodes = vec![[0.0, 0.0, z], [w, 0.0, z], [w, w, z], [0.0, w, z]];
        let triangles = if diag {
            vec![[0, 1, 3], [1, 2, 3]]
        } else {
            vec![[0, 1, 2], [0, 2, 3]]
        };
        Surface { nodes, triangles }
    }

    #[test]
    fn area_and_volume_to_datum_exact() {
        let s = flat_square(10.0, 5.0, false);
        assert!((s.area_2d() - 100.0).abs() < 1e-9);
        // 100 m^2 at z=5 over datum 0 => 500 m^3.
        assert!((s.volume_to_datum(0.0) - 500.0).abs() < 1e-9);
        let cf = s.cut_fill_to_datum(0.0);
        assert!((cf.cut - 500.0).abs() < 1e-9);
        assert!(cf.fill.abs() < 1e-9);
    }

    #[test]
    fn cut_fill_splits_at_datum() {
        // A ramp from z=-10 to z=+10 across a 10x10 square: cut == fill by symmetry.
        let nodes = vec![
            [0.0, 0.0, -10.0],
            [10.0, 0.0, 10.0],
            [10.0, 10.0, 10.0],
            [0.0, 10.0, -10.0],
        ];
        let s = Surface {
            nodes,
            triangles: vec![[0, 1, 2], [0, 2, 3]],
        };
        let cf = s.cut_fill_to_datum(0.0);
        assert!((cf.cut - cf.fill).abs() < 1e-6, "cut {} fill {}", cf.cut, cf.fill);
        assert!(cf.net.abs() < 1e-6);
    }

    #[test]
    fn exact_composite_matches_shared_topology() {
        // Same topology, top 5 above bottom 0 => net = 100 * 5 = 500, all cut.
        let top = flat_square(10.0, 5.0, false);
        let bottom = flat_square(10.0, 0.0, false);
        let shared = volume_between_shared(&top, &bottom).unwrap();
        assert!((shared - 500.0).abs() < 1e-9);
        let cf = exact_composite_cut_fill(&top, &bottom);
        assert!((cf.net - 500.0).abs() < 1e-6, "net {}", cf.net);
        assert!((cf.cut - 500.0).abs() < 1e-6);
        assert!(cf.fill.abs() < 1e-6);
    }

    #[test]
    fn exact_composite_handles_differing_topology() {
        // Top and bottom triangulate the SAME box on OPPOSITE diagonals.
        // Flat planes => result must still be exactly area*offset, all cut.
        let top = flat_square(10.0, 7.0, false);
        let bottom = flat_square(10.0, 2.0, true);
        let cf = exact_composite_cut_fill(&top, &bottom);
        assert!((cf.net - 500.0).abs() < 1e-6, "net {}", cf.net);
        assert!((cf.cut - 500.0).abs() < 1e-6, "cut {}", cf.cut);
        assert!(cf.fill.abs() < 1e-6, "fill {}", cf.fill);
    }

    #[test]
    fn exact_composite_separates_cut_and_fill() {
        // Bottom flat at 0; top tilts from -4 (west) to +4 (east) => equal cut/fill.
        let top = Surface {
            nodes: vec![
                [0.0, 0.0, -4.0],
                [10.0, 0.0, 4.0],
                [10.0, 10.0, 4.0],
                [0.0, 10.0, -4.0],
            ],
            triangles: vec![[0, 1, 2], [0, 2, 3]],
        };
        let bottom = flat_square(10.0, 0.0, true); // opposite diagonal on purpose
        let cf = exact_composite_cut_fill(&top, &bottom);
        assert!(cf.net.abs() < 1e-6, "net {}", cf.net);
        assert!((cf.cut - cf.fill).abs() < 1e-6, "cut {} fill {}", cf.cut, cf.fill);
        // Each side is a wedge: area 50 * avg height 2 = 100.
        assert!((cf.cut - 100.0).abs() < 1e-6, "cut {}", cf.cut);
    }

    #[test]
    fn grid_method_approximates_exact() {
        let top = flat_square(10.0, 7.0, false);
        let bottom = flat_square(10.0, 2.0, true);
        let g = grid_cut_fill(&top, &bottom, 0.5);
        assert!((g.net - 500.0).abs() < 5.0, "grid net {}", g.net);
        assert!(g.n_cells > 0);
    }

    #[test]
    fn datum_detailed_contour_and_volume() {
        // Ramp from z=-10 (west) to z=+10 (east) over 10x10: cut==fill vs datum 0,
        // and the z=0 contour runs up the middle (x=5).
        let s = Surface {
            nodes: vec![
                [0.0, 0.0, -10.0],
                [10.0, 0.0, 10.0],
                [10.0, 10.0, 10.0],
                [0.0, 10.0, -10.0],
            ],
            triangles: vec![[0, 1, 2], [0, 2, 3]],
        };
        let (cf, contour) = s.cut_fill_to_datum_detailed(0.0);
        assert!((cf.cut - cf.fill).abs() < 1e-6);
        assert!(!contour.is_empty());
        for seg in &contour {
            for p in seg {
                assert!((p[0] - 5.0).abs() < 1e-6, "contour x {}", p[0]);
            }
        }
        // A flat pad entirely above the datum: no contour.
        let flat = flat_square(10.0, 5.0, false);
        let (_, c2) = flat.cut_fill_to_datum_detailed(0.0);
        assert!(c2.is_empty());
    }

    #[test]
    fn edges_dedup_shared_diagonal() {
        // A square split into 2 triangles has 5 unique edges (4 sides + 1 diag).
        let s = flat_square(10.0, 0.0, false);
        assert_eq!(s.edges().len(), 5);
    }

    #[test]
    fn detailed_boundary_empty_when_no_crossing_else_present() {
        // Top entirely above bottom: no cut/fill line.
        let top = flat_square(10.0, 7.0, false);
        let bottom = flat_square(10.0, 2.0, true);
        let r = composite_cut_fill_detailed(&top, &bottom);
        assert!(r.cutfill_line.is_empty());
        assert!((r.cut_fill.net - 500.0).abs() < 1e-6);

        // Tilted top crossing a flat bottom: a non-empty boundary near x = 5.
        let tilt = Surface {
            nodes: vec![
                [0.0, 0.0, -4.0],
                [10.0, 0.0, 4.0],
                [10.0, 10.0, 4.0],
                [0.0, 10.0, -4.0],
            ],
            triangles: vec![[0, 1, 2], [0, 2, 3]],
        };
        let flat = flat_square(10.0, 0.0, true);
        let r2 = composite_cut_fill_detailed(&tilt, &flat);
        assert!(!r2.cutfill_line.is_empty());
        for seg in &r2.cutfill_line {
            for p in seg {
                assert!((p[0] - 5.0).abs() < 1e-6, "boundary x {}", p[0]);
            }
        }
    }

    #[test]
    fn delaunay_square_is_two_triangles() {
        let tris = delaunay(&[[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]);
        assert_eq!(tris.len(), 2);
        // Total area must be the full square.
        let pts = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]];
        let s = Surface {
            nodes: pts.to_vec(),
            triangles: tris,
        };
        assert!((s.area_2d() - 1.0).abs() < 1e-9);
    }
}
