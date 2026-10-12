//! Minimal LandXML reader/writer — `std` only, no XML dependency.
//!
//! Reads the subset a survey drawing needs: TIN `<Surface>`s (the `<Pnts>`/`<P>`
//! point list and `<Faces>`/`<F>` triangle list), `<CgPoints>`, `<Parcels>` and
//! `<PlanFeatures>` boundaries (`<CoordGeom>` lines, arcs and irregular lines),
//! and the `<Units>` block. It is a targeted scanner, not a general XML
//! parser — enough for what Civil 3D, Land Desktop, MicroSurvey, and Softree
//! Terrain export. [`write_document`] writes the same subset as LandXML 1.2.
//!
//! Conventions (matching the LandXML 1.2 spec and the reference reader):
//! * `<P id="k">` text is `northing easting elevation`; we store nodes as
//!   `[easting, northing, elevation]` to match [`crate::surface`] (X=E, Y=N).
//! * `<F>` text is three point **ids** (not positional indices); faces with the
//!   invisible flag `i="1"` are skipped (they pad the convex hull and would
//!   inflate area/volume).

use std::collections::HashMap;

use crate::alignment::{Clothoid, CrossSection, Profile, ProfileData, Pvi, SectionSurface};
use crate::surface::Surface;

/// A named surface read from LandXML.
#[derive(Debug, Clone)]
pub struct NamedSurface {
    pub name: String,
    pub surface: Surface,
}

/// Read every `<Surface>` in a LandXML document.
pub fn read_surfaces(xml: &str) -> Vec<NamedSurface> {
    let mut out = Vec::new();
    let starts = find_all(xml, "<Surface ");
    for (i, &s) in starts.iter().enumerate() {
        let end = starts.get(i + 1).copied().unwrap_or(xml.len());
        if let Some(ns) = parse_surface(&xml[s..end]) {
            out.push(ns);
        }
    }
    out
}

/// Read the first `<Surface>` in a LandXML document, if any.
pub fn read_first_surface(xml: &str) -> Option<NamedSurface> {
    read_surfaces(xml).into_iter().next()
}

/// True if `text` looks like a LandXML document.
pub fn looks_like_landxml(text: &str) -> bool {
    let head = &text[..text.len().min(512)];
    head.contains("<LandXML") || (head.contains("<?xml") && text.contains("<Surface"))
}

fn parse_surface(slice: &str) -> Option<NamedSurface> {
    // Name lives on the opening <Surface ...> tag.
    let open_end = slice.find('>')?;
    let name = attr(&slice[..open_end], "name").unwrap_or("Surface").to_string();

    // Points: scan within <Pnts> ... </Pnts>.
    let pnts = between(slice, "<Pnts", "</Pnts>").unwrap_or("");
    let mut nodes: Vec<[f64; 3]> = Vec::new();
    let mut id_to_index: HashMap<String, usize> = HashMap::new();
    let mut cur = 0;
    while let Some(rel) = pnts[cur..].find("<P") {
        let start = cur + rel;
        let after = pnts[start + 2..].chars().next();
        if after != Some(' ') && after != Some('>') {
            cur = start + 2; // e.g. some other tag starting with "<P..."
            continue;
        }
        let gt = match pnts[start..].find('>') {
            Some(g) => start + g,
            None => break,
        };
        let open = &pnts[start..gt];
        let text_end = match pnts[gt + 1..].find("</P>") {
            Some(e) => gt + 1 + e,
            None => break,
        };
        let text = &pnts[gt + 1..text_end];
        if let (Some(id), Some(node)) = (attr(open, "id"), parse_xyz(text)) {
            id_to_index.insert(id.to_string(), nodes.len());
            nodes.push(node);
        }
        cur = text_end + 4;
    }

    // Faces: scan within <Faces> ... </Faces>.
    let faces = between(slice, "<Faces", "</Faces>").unwrap_or("");
    let mut triangles: Vec<[usize; 3]> = Vec::new();
    let mut cur = 0;
    while let Some(rel) = faces[cur..].find("<F") {
        let start = cur + rel;
        let after = faces[start + 2..].chars().next();
        if after != Some(' ') && after != Some('>') {
            cur = start + 2;
            continue;
        }
        let gt = match faces[start..].find('>') {
            Some(g) => start + g,
            None => break,
        };
        let open = &faces[start..gt];
        let text_end = match faces[gt + 1..].find("</F>") {
            Some(e) => gt + 1 + e,
            None => break,
        };
        let text = &faces[gt + 1..text_end];
        cur = text_end + 4;

        if attr(open, "i") == Some("1") {
            continue; // invisible face — excluded from the surface
        }
        let ids: Vec<&str> = text.split_whitespace().collect();
        if ids.len() >= 3 {
            if let (Some(&a), Some(&b), Some(&c)) = (
                id_to_index.get(ids[0]),
                id_to_index.get(ids[1]),
                id_to_index.get(ids[2]),
            ) {
                triangles.push([a, b, c]);
            }
        }
    }

    if nodes.is_empty() || triangles.is_empty() {
        return None;
    }
    Some(NamedSurface {
        name,
        surface: Surface { nodes, triangles },
    })
}

/// Parse a `<P>` text body `northing easting [elevation]` into `[E, N, Z]`.
fn parse_xyz(text: &str) -> Option<[f64; 3]> {
    let mut it = text.split_whitespace();
    let northing: f64 = it.next()?.parse().ok()?;
    let easting: f64 = it.next()?.parse().ok()?;
    let elev: f64 = it.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
    Some([easting, northing, elev])
}

/// Value of attribute `name` (`name="value"` or `name='value'`) in an
/// opening-tag string. Only a whole attribute name matches, so `name` does
/// not find `surfname="…"`.
fn attr<'a>(open_tag: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("{name}=");
    let mut from = 0;
    while let Some(rel) = open_tag[from..].find(&needle) {
        let at = from + rel;
        from = at + needle.len();
        let whole = at > 0 && open_tag[..at].ends_with(|c: char| c.is_whitespace());
        let quote = open_tag[from..].chars().next();
        if let (true, Some(q @ ('"' | '\''))) = (whole, quote) {
            let rest = &open_tag[from + 1..];
            return rest.find(q).map(|j| &rest[..j]);
        }
    }
    None
}

/// Substring strictly between the end of `open` (its first `>` after the match)
/// and `close`.
fn between<'a>(s: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let o = s.find(open)?;
    let gt = s[o..].find('>')? + o + 1;
    let c = s[gt..].find(close)? + gt;
    Some(&s[gt..c])
}

// ---------------------------------------------------------------------------
// Points, parcels, plan features, units
// ---------------------------------------------------------------------------

/// A COGO point (`<CgPoint>`). LandXML coordinate text is
/// `northing easting [elevation]`.
#[derive(Debug, Clone, PartialEq)]
pub struct CgPoint {
    pub name: String,
    pub northing: f64,
    pub easting: f64,
    pub elevation: Option<f64>,
    /// Raw feature code (`code` attribute).
    pub code: String,
    /// Full description (`desc` attribute).
    pub desc: String,
}

/// One element of a `<CoordGeom>` chain, in plan coordinates `[E, N]`
/// (world X, Y).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Segment {
    Line {
        start: [f64; 2],
        end: [f64; 2],
    },
    /// Circular arc from `start` to `end` about `center`; `ccw` is the
    /// direction of travel seen in plan.
    Arc {
        start: [f64; 2],
        center: [f64; 2],
        end: [f64; 2],
        ccw: bool,
    },
    /// Clothoid transition (`<Spiral spiType="clothoid">`).
    Spiral(Clothoid),
}

impl Segment {
    pub fn start(&self) -> [f64; 2] {
        match *self {
            Segment::Line { start, .. } | Segment::Arc { start, .. } => start,
            Segment::Spiral(c) => c.start,
        }
    }

    pub fn end(&self) -> [f64; 2] {
        match *self {
            Segment::Line { end, .. } | Segment::Arc { end, .. } => end,
            Segment::Spiral(c) => c.end,
        }
    }

    /// Points to draw this segment as straight pieces from its start (end
    /// excluded): just the start for lines and arcs (arcs draw as bulges),
    /// a densified run for a spiral.
    pub fn plan_points(&self) -> Vec<[f64; 2]> {
        match self {
            Segment::Spiral(c) => c.densify(),
            _ => vec![self.start()],
        }
    }

    /// Point and heading (math angle, radians) at distance `s` along the
    /// segment from its start.
    pub fn at(&self, s: f64) -> ([f64; 2], f64) {
        match *self {
            Segment::Line { start, end } => {
                let len = self.length().max(1e-12);
                let t = (s / len).clamp(0.0, 1.0);
                (
                    [start[0] + t * (end[0] - start[0]), start[1] + t * (end[1] - start[1])],
                    (end[1] - start[1]).atan2(end[0] - start[0]),
                )
            }
            Segment::Arc { start, center, .. } => {
                let r = self.radius().max(1e-12);
                let sweep = self.sweep();
                let a = (start[1] - center[1]).atan2(start[0] - center[0])
                    + sweep.signum() * (s / r).clamp(0.0, sweep.abs());
                let heading = a + sweep.signum() * std::f64::consts::FRAC_PI_2;
                ([center[0] + r * a.cos(), center[1] + r * a.sin()], heading)
            }
            Segment::Spiral(c) => c.at(s),
        }
    }

    /// Signed sweep of an arc in radians (positive CCW); 0 for a line or
    /// spiral (neither is drawn with a bulge).
    pub fn sweep(&self) -> f64 {
        match *self {
            Segment::Line { .. } | Segment::Spiral(_) => 0.0,
            Segment::Arc {
                start,
                center,
                end,
                ccw,
            } => {
                let tau = std::f64::consts::TAU;
                let a0 = (start[1] - center[1]).atan2(start[0] - center[0]);
                let a1 = (end[1] - center[1]).atan2(end[0] - center[0]);
                // start == end is a full circle.
                let full = |d: f64| if d < 1e-12 { tau } else { d };
                if ccw {
                    full((a1 - a0).rem_euclid(tau))
                } else {
                    -full((a0 - a1).rem_euclid(tau))
                }
            }
        }
    }

    /// DXF/LWPOLYLINE bulge for this segment: `tan(sweep / 4)`.
    pub fn bulge(&self) -> f64 {
        (self.sweep() / 4.0).tan()
    }

    /// Radius of an arc; 0 for a line or spiral.
    pub fn radius(&self) -> f64 {
        match *self {
            Segment::Line { .. } | Segment::Spiral(_) => 0.0,
            Segment::Arc { start, center, .. } => {
                (start[0] - center[0]).hypot(start[1] - center[1])
            }
        }
    }

    pub fn length(&self) -> f64 {
        match *self {
            Segment::Line { start, end } => (end[0] - start[0]).hypot(end[1] - start[1]),
            Segment::Arc { .. } => self.radius() * self.sweep().abs(),
            Segment::Spiral(c) => c.length,
        }
    }

    /// Build a segment from an LWPOLYLINE edge `p1 → p2` with `bulge`.
    pub fn from_bulge(p1: [f64; 2], p2: [f64; 2], bulge: f64) -> Segment {
        if bulge.abs() < 1e-12 {
            return Segment::Line { start: p1, end: p2 };
        }
        let (dx, dy) = (p2[0] - p1[0], p2[1] - p1[1]);
        let mid = [(p1[0] + p2[0]) / 2.0, (p1[1] + p2[1]) / 2.0];
        // Centre sits left of p1→p2 for a CCW (positive) bulge, at
        // chord·(1 − b²)/(4b) from the chord midpoint.
        let off = (1.0 - bulge * bulge) / (4.0 * bulge);
        Segment::Arc {
            start: p1,
            center: [mid[0] - dy * off, mid[1] + dx * off],
            end: p2,
            ccw: bulge > 0.0,
        }
    }
}

/// A `<Parcel>` (lot) boundary.
#[derive(Debug, Clone, PartialEq)]
pub struct Parcel {
    pub name: String,
    pub desc: String,
    /// The `area` attribute as written by the producing software.
    pub area: Option<f64>,
    /// The `<Center>` label point `[E, N]`, if given.
    pub center: Option<[f64; 2]>,
    pub segments: Vec<Segment>,
}

/// A `<PlanFeature>` (breakline, fence, edge of pavement…).
#[derive(Debug, Clone, PartialEq)]
pub struct PlanFeature {
    pub name: String,
    pub desc: String,
    pub segments: Vec<Segment>,
}

/// Linear unit system declared in `<Units>`.
#[derive(Debug, Clone, PartialEq)]
pub struct Units {
    pub metric: bool,
    /// e.g. `meter`, `foot`, `USSurveyFoot`.
    pub linear_unit: String,
}

/// Everything this module reads from (or writes to) a LandXML document.
#[derive(Debug, Clone, Default)]
pub struct LandXmlDoc {
    pub units: Option<Units>,
    pub points: Vec<CgPoint>,
    pub parcels: Vec<Parcel>,
    pub features: Vec<PlanFeature>,
    pub surfaces: Vec<NamedSurface>,
    pub alignments: Vec<Alignment>,
    /// Elements that were skipped or approximated (unsupported CoordGeom
    /// kinds, inconsistent spirals, unresolved `pntRef`s).
    pub warnings: Vec<String>,
}

/// A horizontal alignment (`<Alignment>`) with its profiles and cross sections.
#[derive(Debug, Clone, PartialEq)]
pub struct Alignment {
    pub name: String,
    pub desc: String,
    /// Station at the start of the alignment.
    pub sta_start: f64,
    pub segments: Vec<Segment>,
    pub profiles: Vec<Profile>,
    pub cross_sections: Vec<CrossSection>,
}

impl Alignment {
    /// Total horizontal length.
    pub fn length(&self) -> f64 {
        self.segments.iter().map(Segment::length).sum()
    }

    /// Point and heading at station `sta` (clamped to the alignment).
    pub fn at_station(&self, sta: f64) -> Option<([f64; 2], f64)> {
        let mut s = (sta - self.sta_start).max(0.0);
        for (i, seg) in self.segments.iter().enumerate() {
            let len = seg.length();
            if s <= len || i + 1 == self.segments.len() {
                return Some(seg.at(s.min(len)));
            }
            s -= len;
        }
        None
    }

    /// Station at the start of each segment, plus the end station.
    pub fn key_stations(&self) -> Vec<f64> {
        let mut out = vec![self.sta_start];
        let mut sta = self.sta_start;
        for seg in &self.segments {
            sta += seg.length();
            out.push(sta);
        }
        out
    }
}

/// Read points, parcels, plan features, surfaces and units from LandXML.
pub fn read_document(xml: &str) -> LandXmlDoc {
    let mut doc = LandXmlDoc {
        units: read_units(xml),
        ..Default::default()
    };

    let mut by_name: HashMap<String, [f64; 3]> = HashMap::new();
    if let Some(body) = block(xml, "CgPoints") {
        let mut refs: Vec<(CgPoint, String)> = Vec::new();
        for el in elements(body, "CgPoint") {
            let mut p = CgPoint {
                name: unescape(attr(el.open, "name").unwrap_or("")),
                northing: 0.0,
                easting: 0.0,
                elevation: None,
                code: unescape(attr(el.open, "code").unwrap_or("")),
                desc: unescape(attr(el.open, "desc").unwrap_or("")),
            };
            match parse_nez(el.inner) {
                Some((n, e, z)) => {
                    p.northing = n;
                    p.easting = e;
                    p.elevation = z;
                    by_name.insert(p.name.clone(), [n, e, z.unwrap_or(0.0)]);
                    doc.points.push(p);
                }
                None => {
                    if let Some(r) = attr(el.open, "pntRef") {
                        refs.push((p, r.to_string()));
                    }
                }
            }
        }
        // Points defined by reference to another point.
        for (mut p, r) in refs {
            match by_name.get(&r) {
                Some(&[n, e, z]) => {
                    p.northing = n;
                    p.easting = e;
                    p.elevation = Some(z);
                    doc.points.push(p);
                }
                None => doc
                    .warnings
                    .push(format!("CgPoint \"{}\": unresolved pntRef \"{r}\"", p.name)),
            }
        }
    }

    // Parcels may nest (a parcel group holding lots): treat each <Parcel> start
    // up to the next one as its own region.
    if let Some(body) = block(xml, "Parcels") {
        let starts = tag_starts(body, "Parcel");
        for (i, &s) in starts.iter().enumerate() {
            let region = &body[s..starts.get(i + 1).copied().unwrap_or(body.len())];
            let Some(gt) = region.find('>') else {
                continue;
            };
            let open = &region[..gt];
            let name = unescape(attr(open, "name").unwrap_or(""));
            let segments = match block(region, "CoordGeom") {
                Some(cg) => read_coord_geom(cg, &by_name, &name, &mut doc.warnings),
                None => Vec::new(),
            };
            if segments.is_empty() {
                continue; // a grouping parcel with no geometry of its own
            }
            // The parcel's own label point precedes its CoordGeom; a <Center>
            // inside the CoordGeom belongs to a curve.
            let head = &region[..tag_starts(region, "CoordGeom")
                .first()
                .copied()
                .unwrap_or(region.len())];
            let center = elements(head, "Center")
                .first()
                .and_then(|c| point_of(c, &by_name));
            doc.parcels.push(Parcel {
                name,
                desc: unescape(attr(open, "desc").unwrap_or("")),
                area: attr(open, "area").and_then(|a| a.parse().ok()),
                center,
                segments,
            });
        }
    }

    if let Some(body) = block(xml, "PlanFeatures") {
        for el in elements(body, "PlanFeature") {
            let name = unescape(attr(el.open, "name").unwrap_or(""));
            if let Some(cg) = block(el.inner, "CoordGeom") {
                let segments = read_coord_geom(cg, &by_name, &name, &mut doc.warnings);
                if !segments.is_empty() {
                    doc.features.push(PlanFeature {
                        name,
                        desc: unescape(attr(el.open, "desc").unwrap_or("")),
                        segments,
                    });
                }
            }
        }
    }

    if let Some(body) = block(xml, "Alignments") {
        for el in elements(body, "Alignment") {
            let name = unescape(attr(el.open, "name").unwrap_or(""));
            let segments = match block(el.inner, "CoordGeom") {
                Some(cg) => read_coord_geom(cg, &by_name, &name, &mut doc.warnings),
                None => Vec::new(),
            };
            let profiles = read_profiles(el.inner, &name, &mut doc.warnings);
            let cross_sections = read_cross_sections(el.inner);
            if segments.is_empty() && profiles.is_empty() && cross_sections.is_empty() {
                continue;
            }
            doc.alignments.push(Alignment {
                name,
                desc: unescape(attr(el.open, "desc").unwrap_or("")),
                sta_start: attr(el.open, "staStart")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0.0),
                segments,
                profiles,
                cross_sections,
            });
        }
    }

    doc.surfaces = read_surfaces(xml);
    doc
}

/// `<Profile>` children of an alignment: design `<ProfAlign>`s (PVIs and
/// vertical curves) and sampled `<ProfSurf>`s.
fn read_profiles(body: &str, owner: &str, warnings: &mut Vec<String>) -> Vec<Profile> {
    let mut out = Vec::new();
    for prof in elements(body, "Profile") {
        for el in elements(prof.inner, "ProfAlign") {
            let mut pvis = Vec::new();
            for p in children(el.inner, &["PVI", "ParaCurve", "UnsymParaCurve", "CircCurve"]) {
                let nums = numbers(p.inner);
                if nums.len() < 2 {
                    continue;
                }
                let num_attr = |a: &str| attr(p.open, a).and_then(|v| v.parse::<f64>().ok());
                let curve_len = match p.tag {
                    "PVI" => 0.0,
                    "ParaCurve" => num_attr("length").unwrap_or(0.0),
                    "UnsymParaCurve" => {
                        warnings.push(format!(
                            "{owner}: unsymmetrical vertical curve at {} drawn symmetric",
                            nums[0]
                        ));
                        num_attr("lengthIn").unwrap_or(0.0) + num_attr("lengthOut").unwrap_or(0.0)
                    }
                    _ => {
                        warnings.push(format!(
                            "{owner}: circular vertical curve at {} drawn as a parabola",
                            nums[0]
                        ));
                        num_attr("length").unwrap_or(0.0)
                    }
                };
                pvis.push(Pvi {
                    sta: nums[0],
                    elev: nums[1],
                    curve_len,
                });
            }
            if !pvis.is_empty() {
                out.push(Profile {
                    name: unescape(attr(el.open, "name").unwrap_or("")),
                    desc: unescape(attr(el.open, "desc").unwrap_or("")),
                    data: ProfileData::Design(pvis),
                });
            }
        }
        for el in elements(prof.inner, "ProfSurf") {
            let pts = elements(el.inner, "PntList2D")
                .first()
                .map(|l| pairs(l.inner))
                .unwrap_or_default();
            if !pts.is_empty() {
                out.push(Profile {
                    name: unescape(attr(el.open, "name").unwrap_or("")),
                    desc: unescape(attr(el.open, "desc").unwrap_or("")),
                    data: ProfileData::Surface(pts),
                });
            }
        }
    }
    out
}

/// `<CrossSects>` of an alignment; sections with no surfaces are skipped.
fn read_cross_sections(body: &str) -> Vec<CrossSection> {
    let mut out = Vec::new();
    let Some(xs) = block(body, "CrossSects") else {
        return out;
    };
    for el in elements(xs, "CrossSect") {
        let Some(sta) = attr(el.open, "sta").and_then(|v| v.parse().ok()) else {
            continue;
        };
        let surfaces: Vec<SectionSurface> = elements(el.inner, "CrossSectSurf")
            .iter()
            .filter_map(|s| {
                let points = pairs(elements(s.inner, "PntList2D").first()?.inner);
                (!points.is_empty()).then(|| SectionSurface {
                    name: unescape(attr(s.open, "name").unwrap_or("")),
                    desc: unescape(attr(s.open, "desc").unwrap_or("")),
                    points,
                })
            })
            .collect();
        if !surfaces.is_empty() {
            out.push(CrossSection {
                sta,
                name: unescape(attr(el.open, "name").unwrap_or("")),
                surfaces,
            });
        }
    }
    out
}

fn numbers(text: &str) -> Vec<f64> {
    text.split_whitespace().filter_map(|t| t.parse().ok()).collect()
}

/// A `PntList2D` body as `[a, b]` pairs, in file order.
fn pairs(text: &str) -> Vec<[f64; 2]> {
    numbers(text).chunks_exact(2).map(|c| [c[0], c[1]]).collect()
}

/// A radius attribute: a number, or `INF` for a tangent.
fn radius_attr(open: &str, name: &str) -> f64 {
    match attr(open, name) {
        Some(v) => v.trim().parse().unwrap_or(f64::INFINITY), // "INF" parses as inf
        None => f64::INFINITY,
    }
}

fn read_units(xml: &str) -> Option<Units> {
    let units = block(xml, "Units")?;
    for (tag, metric) in [("Metric", true), ("Imperial", false)] {
        if let Some(el) = elements(units, tag).first() {
            return Some(Units {
                metric,
                linear_unit: attr(el.open, "linearUnit").unwrap_or("").to_string(),
            });
        }
    }
    None
}

/// Walk a `<CoordGeom>` body in document order.
fn read_coord_geom(
    cg: &str,
    pts: &HashMap<String, [f64; 3]>,
    owner: &str,
    warnings: &mut Vec<String>,
) -> Vec<Segment> {
    let mut out = Vec::new();
    for el in children(cg, &["Line", "Curve", "IrregularLine", "Spiral", "Chain"]) {
        let start = first_point(el.inner, "Start", pts);
        let end = first_point(el.inner, "End", pts);
        match el.tag {
            "Line" => match (start, end) {
                (Some(start), Some(end)) => out.push(Segment::Line { start, end }),
                _ => warnings.push(format!("{owner}: Line without resolvable Start/End")),
            },
            "Curve" => match (start, first_point(el.inner, "Center", pts), end) {
                (Some(start), Some(center), Some(end)) => out.push(Segment::Arc {
                    start,
                    center,
                    end,
                    ccw: attr(el.open, "rot") != Some("cw"),
                }),
                _ => warnings.push(format!("{owner}: Curve without Start/Center/End")),
            },
            "IrregularLine" => {
                let list = elements(el.inner, "PntList2D")
                    .first()
                    .map(|p| (p.inner, 2))
                    .or_else(|| elements(el.inner, "PntList3D").first().map(|p| (p.inner, 3)));
                if let Some((text, dim)) = list {
                    let nums: Vec<f64> =
                        text.split_whitespace().filter_map(|t| t.parse().ok()).collect();
                    let verts: Vec<[f64; 2]> =
                        nums.chunks_exact(dim).map(|c| [c[1], c[0]]).collect();
                    for w in verts.windows(2) {
                        out.push(Segment::Line {
                            start: w[0],
                            end: w[1],
                        });
                    }
                }
            }
            "Spiral" => {
                let length = attr(el.open, "length").and_then(|v| v.parse::<f64>().ok());
                let spi = attr(el.open, "spiType").unwrap_or("clothoid");
                match (start, end, length) {
                    (Some(start), Some(end), Some(length)) if length > 0.0 => {
                        let c = Clothoid {
                            start,
                            end,
                            length,
                            r_start: radius_attr(el.open, "radiusStart"),
                            r_end: radius_attr(el.open, "radiusEnd"),
                            ccw: attr(el.open, "rot") != Some("cw"),
                        };
                        if !spi.eq_ignore_ascii_case("clothoid") {
                            warnings.push(format!("{owner}: {spi} spiral drawn as a clothoid"));
                        }
                        if c.misfit() > 1e-4 {
                            warnings.push(format!(
                                "{owner}: spiral length/radii disagree with its end points by {:.3}%",
                                c.misfit() * 100.0
                            ));
                        }
                        out.push(Segment::Spiral(c));
                    }
                    (Some(start), Some(end), _) => {
                        out.push(Segment::Line { start, end });
                        warnings.push(format!("{owner}: spiral without a length drawn as its chord"));
                    }
                    _ => warnings.push(format!("{owner}: Spiral without Start/End")),
                }
            }
            _ => warnings.push(format!("{owner}: unsupported <{}> skipped", el.tag)),
        }
    }
    out
}

fn first_point(body: &str, tag: &str, pts: &HashMap<String, [f64; 3]>) -> Option<[f64; 2]> {
    elements(body, tag).first().and_then(|el| point_of(el, pts))
}

/// Coordinates of a `<Start>`/`<End>`/`<Center>` element: inline
/// `northing easting` text, or a `pntRef` to a CgPoint. Returns `[E, N]`.
fn point_of(el: &Element, pts: &HashMap<String, [f64; 3]>) -> Option<[f64; 2]> {
    if let Some((n, e, _)) = parse_nez(el.inner) {
        return Some([e, n]);
    }
    let r = attr(el.open, "pntRef")?;
    pts.get(r).map(|p| [p[1], p[0]])
}

/// `northing easting [elevation]`.
fn parse_nez(text: &str) -> Option<(f64, f64, Option<f64>)> {
    let mut it = text.split_whitespace();
    let n: f64 = it.next()?.parse().ok()?;
    let e: f64 = it.next()?.parse().ok()?;
    let z = it.next().and_then(|s| s.parse().ok());
    Some((n, e, z))
}

/// A matched element: its tag name, its opening tag between `<` and `>`, and
/// its inner text (empty for a self-closing tag).
struct Element<'a> {
    tag: &'a str,
    open: &'a str,
    inner: &'a str,
    /// Byte offset just past the element (after `/>` or its closing tag).
    end: usize,
}

/// Byte offsets of every `<tag` that is a whole tag name (followed by
/// whitespace, `>` or `/`), so `<Parcel` does not match `<Parcels`.
fn tag_starts(s: &str, tag: &str) -> Vec<usize> {
    let needle = format!("<{tag}");
    find_all(s, &needle)
        .into_iter()
        .filter(|&i| {
            matches!(
                s[i + needle.len()..].chars().next(),
                Some(' ' | '>' | '/' | '\t' | '\r' | '\n')
            )
        })
        .collect()
}

fn element_at<'a>(s: &'a str, start: usize, tag: &str) -> Option<Element<'a>> {
    let gt = start + s[start..].find('>')?;
    let open = &s[start + 1..gt];
    let name = open.split(|c: char| c.is_whitespace() || c == '/').next()?;
    if open.ends_with('/') {
        return Some(Element {
            tag: name,
            open,
            inner: "",
            end: gt + 1,
        });
    }
    let close = format!("</{tag}>");
    let inner_end = gt + 1 + s[gt + 1..].find(&close)?;
    Some(Element {
        tag: name,
        open,
        inner: &s[gt + 1..inner_end],
        end: inner_end + close.len(),
    })
}

/// The non-nested elements of `s` whose tag is one of `tags`, in document
/// order.
fn children<'a>(s: &'a str, tags: &[&str]) -> Vec<Element<'a>> {
    let mut found: Vec<(usize, &str)> = tags
        .iter()
        .flat_map(|t| tag_starts(s, t).into_iter().map(move |i| (i, *t)))
        .collect();
    found.sort_by_key(|&(i, _)| i);
    let mut out = Vec::new();
    let mut after = 0;
    for (i, t) in found {
        if i < after {
            continue; // inside the previous element
        }
        if let Some(el) = element_at(s, i, t) {
            after = el.end;
            out.push(el);
        }
    }
    out
}

fn elements<'a>(s: &'a str, tag: &str) -> Vec<Element<'a>> {
    children(s, &[tag])
}

/// Inner text of the first `<tag …>…</tag>` in `s`.
fn block<'a>(s: &'a str, tag: &str) -> Option<&'a str> {
    let start = *tag_starts(s, tag).first()?;
    element_at(s, start, tag).map(|e| e.inner)
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Plan area enclosed by a closed chain of segments: shoelace over the chord
/// polygon plus each arc's circular segment. Always non-negative.
pub fn chain_area(segments: &[Segment]) -> f64 {
    // Shoelace over every drawn vertex (a spiral contributes its densified run).
    let mut twice = 0.0;
    for s in segments {
        let mut pts = s.plan_points();
        pts.push(s.end());
        for w in pts.windows(2) {
            twice += w[0][0] * w[1][1] - w[1][0] * w[0][1];
        }
    }
    // A CCW arc bulges left of its chord, adding to a CCW ring's area; the
    // signed sweep carries the side either way.
    let segs: f64 = segments
        .iter()
        .map(|s| {
            let t = s.sweep();
            0.5 * s.radius().powi(2) * (t - t.sin())
        })
        .sum();
    (0.5 * twice + segs).abs()
}

/// A polyline as `[(x, y, bulge)]` vertices plus whether it closes.
pub type Ring = (Vec<(f64, f64, f64)>, bool);

/// Split a segment chain into polylines at any gap. Coordinates are `[E, N]`;
/// each vertex carries the bulge of the segment leaving it.
pub fn chain_polylines(segments: &[Segment]) -> Vec<Ring> {
    const TOL: f64 = 1e-6;
    let near = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).hypot(a[1] - b[1]) <= TOL;
    let mut out: Vec<Ring> = Vec::new();
    let mut cur: Vec<(f64, f64, f64)> = Vec::new();
    let mut first = [0.0; 2];
    let mut last = [0.0; 2];
    for s in segments {
        let p = s.start();
        if cur.is_empty() || !near(p, last) {
            if !cur.is_empty() {
                cur.push((last[0], last[1], 0.0));
                out.push((std::mem::take(&mut cur), false));
            }
            first = p;
        }
        match s {
            Segment::Spiral(_) => cur.extend(s.plan_points().iter().map(|q| (q[0], q[1], 0.0))),
            _ => cur.push((p[0], p[1], s.bulge())),
        }
        last = s.end();
    }
    if !cur.is_empty() {
        if near(last, first) {
            out.push((cur, true));
        } else {
            cur.push((last[0], last[1], 0.0));
            out.push((cur, false));
        }
    }
    out
}

/// Segments of a polyline given as `[(x, y, bulge)]` vertices.
pub fn polyline_segments(verts: &[(f64, f64, f64)], closed: bool) -> Vec<Segment> {
    let n = verts.len();
    let edges = if closed { n } else { n.saturating_sub(1) };
    (0..edges)
        .map(|i| {
            let (a, b) = (verts[i], verts[(i + 1) % n]);
            Segment::from_bulge([a.0, a.1], [b.0, b.1], a.2)
        })
        .collect()
}

/// Write `doc` as a LandXML 1.2 document. `date` / `time` fill the root
/// element's required `YYYY-MM-DD` / `HH:MM:SS` stamps.
pub fn write_document(doc: &LandXmlDoc, date: &str, time: &str) -> String {
    use std::fmt::Write;
    let mut x = String::new();
    let _ = writeln!(x, r#"<?xml version="1.0" encoding="UTF-8"?>"#);
    let _ = writeln!(
        x,
        r#"<LandXML xmlns="http://www.landxml.org/schema/LandXML-1.2" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:schemaLocation="http://www.landxml.org/schema/LandXML-1.2 http://www.landxml.org/schema/LandXML-1.2/LandXML-1.2.xsd" version="1.2" date="{date}" time="{time}" readOnly="false" language="English">"#
    );
    let _ = writeln!(x, "  <Units>");
    match &doc.units {
        Some(u) if !u.metric => {
            let lin = if u.linear_unit.is_empty() {
                "USSurveyFoot"
            } else {
                u.linear_unit.as_str()
            };
            let _ = writeln!(
                x,
                r#"    <Imperial linearUnit="{lin}" areaUnit="squareFoot" volumeUnit="cubicYard" angularUnit="decimal degrees" directionUnit="decimal degrees"/>"#
            );
        }
        _ => {
            let _ = writeln!(
                x,
                r#"    <Metric linearUnit="meter" areaUnit="squareMeter" volumeUnit="cubicMeter" angularUnit="decimal degrees" directionUnit="decimal degrees"/>"#
            );
        }
    }
    let _ = writeln!(x, "  </Units>");
    let _ = writeln!(
        x,
        r#"  <Application name="Open CAD Studio Land Survey" manufacturer="opencad-landsurvey-plugin" version="{}"/>"#,
        env!("CARGO_PKG_VERSION")
    );

    if !doc.points.is_empty() {
        let _ = writeln!(x, "  <CgPoints>");
        for p in &doc.points {
            let z = p.elevation.map(|z| format!(" {}", num(z))).unwrap_or_default();
            let _ = writeln!(
                x,
                r#"    <CgPoint name="{}"{}{}>{} {}{z}</CgPoint>"#,
                escape(&p.name),
                opt_attr("code", &p.code),
                opt_attr("desc", &p.desc),
                num(p.northing),
                num(p.easting)
            );
        }
        let _ = writeln!(x, "  </CgPoints>");
    }

    if !doc.parcels.is_empty() {
        let _ = writeln!(x, "  <Parcels>");
        for p in &doc.parcels {
            let area = p.area.unwrap_or_else(|| chain_area(&p.segments));
            let _ = writeln!(
                x,
                r#"    <Parcel name="{}"{} area="{}">"#,
                escape(&p.name),
                opt_attr("desc", &p.desc),
                num(area)
            );
            if let Some(c) = p.center {
                let _ = writeln!(x, "      <Center>{} {}</Center>", num(c[1]), num(c[0]));
            }
            write_coord_geom(&mut x, &p.segments, "      ");
            let _ = writeln!(x, "    </Parcel>");
        }
        let _ = writeln!(x, "  </Parcels>");
    }

    if !doc.features.is_empty() {
        let _ = writeln!(x, r#"  <PlanFeatures name="Features">"#);
        for f in &doc.features {
            let _ = writeln!(
                x,
                r#"    <PlanFeature name="{}"{}>"#,
                escape(&f.name),
                opt_attr("desc", &f.desc)
            );
            write_coord_geom(&mut x, &f.segments, "      ");
            let _ = writeln!(x, "    </PlanFeature>");
        }
        let _ = writeln!(x, "  </PlanFeatures>");
    }

    if !doc.alignments.is_empty() {
        write_alignments(&mut x, &doc.alignments);
    }

    if !doc.surfaces.is_empty() {
        let _ = writeln!(x, "  <Surfaces>");
        for s in &doc.surfaces {
            let _ = writeln!(x, r#"    <Surface name="{}">"#, escape(&s.name));
            let _ = writeln!(x, r#"      <Definition surfType="TIN">"#);
            let _ = writeln!(x, "        <Pnts>");
            for (i, n) in s.surface.nodes.iter().enumerate() {
                let _ = writeln!(
                    x,
                    r#"          <P id="{}">{} {} {}</P>"#,
                    i + 1,
                    num(n[1]),
                    num(n[0]),
                    num(n[2])
                );
            }
            let _ = writeln!(x, "        </Pnts>");
            let _ = writeln!(x, "        <Faces>");
            for t in &s.surface.triangles {
                let _ = writeln!(x, "          <F>{} {} {}</F>", t[0] + 1, t[1] + 1, t[2] + 1);
            }
            let _ = writeln!(x, "        </Faces>");
            let _ = writeln!(x, "      </Definition>");
            let _ = writeln!(x, "    </Surface>");
        }
        let _ = writeln!(x, "  </Surfaces>");
    }
    let _ = writeln!(x, "</LandXML>");
    x
}

fn opt_attr(name: &str, value: &str) -> String {
    if value.is_empty() {
        String::new()
    } else {
        format!(r#" {name}="{}""#, escape(value))
    }
}

fn write_coord_geom(x: &mut String, segments: &[Segment], ind: &str) {
    use std::fmt::Write;
    let pt = |p: [f64; 2]| format!("{} {}", num(p[1]), num(p[0]));
    let _ = writeln!(x, "{ind}<CoordGeom>");
    for s in segments {
        match *s {
            Segment::Line { start, end } => {
                let _ = writeln!(
                    x,
                    r#"{ind}  <Line length="{}"><Start>{}</Start><End>{}</End></Line>"#,
                    num(s.length()),
                    pt(start),
                    pt(end)
                );
            }
            Segment::Arc {
                start,
                center,
                end,
                ccw,
            } => {
                let _ = writeln!(
                    x,
                    r#"{ind}  <Curve rot="{}" crvType="arc" radius="{}" length="{}" delta="{}"><Start>{}</Start><Center>{}</Center><End>{}</End></Curve>"#,
                    if ccw { "ccw" } else { "cw" },
                    num(s.radius()),
                    num(s.length()),
                    num(s.sweep().abs().to_degrees()),
                    pt(start),
                    pt(center),
                    pt(end)
                );
            }
            Segment::Spiral(c) => {
                let (pi, t0, t1) = c.pi();
                let r = |v: f64| if v.is_finite() { num(v) } else { "INF".to_string() };
                // Tangent lengths only for a spiral off a tangent: for one
                // between two radii, producers quote the parent clothoid's.
                let tangents = if c.r_start.is_finite() && c.r_end.is_finite() {
                    String::new()
                } else {
                    format!(r#" tanLong="{}" tanShort="{}""#, num(t0.max(t1)), num(t0.min(t1)))
                };
                let _ = writeln!(
                    x,
                    r#"{ind}  <Spiral rot="{}" spiType="clothoid" length="{}" radiusStart="{}" radiusEnd="{}"{tangents}><Start>{}</Start><PI>{}</PI><End>{}</End></Spiral>"#,
                    if c.ccw { "ccw" } else { "cw" },
                    num(c.length),
                    r(c.r_start),
                    r(c.r_end),
                    pt(c.start),
                    pt(pi),
                    pt(c.end)
                );
            }
        }
    }
    let _ = writeln!(x, "{ind}</CoordGeom>");
}

fn write_alignments(x: &mut String, alignments: &[Alignment]) {
    use std::fmt::Write;
    let _ = writeln!(x, r#"  <Alignments name="Alignments">"#);
    for a in alignments {
        let _ = writeln!(
            x,
            r#"    <Alignment name="{}"{} staStart="{}" length="{}">"#,
            escape(&a.name),
            opt_attr("desc", &a.desc),
            num(a.sta_start),
            num(a.length())
        );
        if !a.segments.is_empty() {
            write_coord_geom(x, &a.segments, "      ");
        }
        if !a.profiles.is_empty() {
            let _ = writeln!(x, r#"      <Profile name="{}">"#, escape(&a.name));
            for p in &a.profiles {
                let head = format!(r#"name="{}"{}"#, escape(&p.name), opt_attr("desc", &p.desc));
                match &p.data {
                    ProfileData::Design(pvis) => {
                        let _ = writeln!(x, "        <ProfAlign {head}>");
                        for (i, v) in pvis.iter().enumerate() {
                            let interior = i > 0 && i + 1 < pvis.len();
                            if interior && v.curve_len > 0.0 {
                                let _ = writeln!(
                                    x,
                                    r#"          <ParaCurve length="{}">{} {}</ParaCurve>"#,
                                    num(v.curve_len),
                                    num(v.sta),
                                    num(v.elev)
                                );
                            } else {
                                let _ = writeln!(x, "          <PVI>{} {}</PVI>", num(v.sta), num(v.elev));
                            }
                        }
                        let _ = writeln!(x, "        </ProfAlign>");
                    }
                    ProfileData::Surface(pts) => {
                        let _ = writeln!(x, "        <ProfSurf {head}>");
                        let _ = writeln!(x, "          <PntList2D>{}</PntList2D>", pnt_list(pts));
                        let _ = writeln!(x, "        </ProfSurf>");
                    }
                }
            }
            let _ = writeln!(x, "      </Profile>");
        }
        if !a.cross_sections.is_empty() {
            let _ = writeln!(x, r#"      <CrossSects name="{}">"#, escape(&a.name));
            for cs in &a.cross_sections {
                let _ = writeln!(
                    x,
                    r#"        <CrossSect sta="{}"{}>"#,
                    num(cs.sta),
                    opt_attr("name", &cs.name)
                );
                for s in &cs.surfaces {
                    let _ = writeln!(
                        x,
                        r#"          <CrossSectSurf name="{}"{}><PntList2D>{}</PntList2D></CrossSectSurf>"#,
                        escape(&s.name),
                        opt_attr("desc", &s.desc),
                        pnt_list(&s.points)
                    );
                }
                let _ = writeln!(x, "        </CrossSect>");
            }
            let _ = writeln!(x, "      </CrossSects>");
        }
        let _ = writeln!(x, "    </Alignment>");
    }
    let _ = writeln!(x, "  </Alignments>");
}

fn pnt_list(pts: &[[f64; 2]]) -> String {
    pts.iter()
        .map(|p| format!("{} {}", num(p[0]), num(p[1])))
        .collect::<Vec<_>>()
        .join(" ")
}

/// A number to 8 decimals with trailing zeros trimmed.
fn num(v: f64) -> String {
    let s = format!("{v:.8}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" {
        "0".to_string()
    } else {
        s.to_string()
    }
}

fn find_all(s: &str, needle: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut cur = 0;
    while let Some(rel) = s[cur..].find(needle) {
        let at = cur + rel;
        out.push(at);
        cur = at + needle.len();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_inline_tin() {
        // A 10x10 square split into 2 triangles, one invisible face that would
        // double the area if (wrongly) included.
        let xml = r#"<?xml version="1.0"?>
<LandXML><Surfaces>
<Surface name="EG">
  <Definition surfType="TIN">
    <Pnts>
      <P id="0">0 0 5</P>
      <P id="1">0 10 5</P>
      <P id="2">10 10 5</P>
      <P id="3">10 0 5</P>
    </Pnts>
    <Faces>
      <F>0 1 2</F>
      <F>0 2 3</F>
      <F i="1">0 1 3</F>
    </Faces>
  </Definition>
</Surface>
</Surfaces></LandXML>"#;
        let ns = read_first_surface(xml).expect("a surface");
        assert_eq!(ns.name, "EG");
        assert_eq!(ns.surface.nodes.len(), 4);
        // Invisible face excluded -> exactly the 2 real triangles, area 100.
        assert_eq!(ns.surface.triangles.len(), 2);
        assert!((ns.surface.area_2d() - 100.0).abs() < 1e-9);
        // P order is "northing easting elev" -> node [E, N, Z]; P id=1 is N=0,E=10.
        assert_eq!(ns.surface.nodes[1], [10.0, 0.0, 5.0]);
        assert!(looks_like_landxml(xml));
    }

    /// Points (inline + pntRef, escaped attributes), a parcel whose east side
    /// is a half-circle, a plan feature, and units.
    const DOC: &str = r#"<?xml version="1.0"?>
<LandXML version="1.2">
  <Units><Metric linearUnit="meter" areaUnit="squareMeter"/></Units>
  <CgPoints>
    <CgPoint name="1" code="IP" desc="IP &amp; tag">100 200 10.5</CgPoint>
    <CgPoint name="2">100 220</CgPoint>
    <CgPoint name="3" pntRef="1" code="REF"/>
  </CgPoints>
  <Parcels>
    <Parcel name="Group">
      <Parcels>
        <Parcel name="Lot 1" area="557.079633" desc="arc side">
          <Center>110 210</Center>
          <CoordGeom>
            <Line><Start pntRef="1"/><End pntRef="2"/></Line>
            <Curve rot="ccw" radius="10"><Start>100 220</Start><Center>110 220</Center><End>120 220</End></Curve>
            <Line><Start>120 220</Start><End>120 200</End></Line>
            <Line><Start>120 200</Start><End>100 200</End></Line>
          </CoordGeom>
        </Parcel>
      </Parcels>
    </Parcel>
  </Parcels>
  <PlanFeatures>
    <PlanFeature name="FENCE">
      <CoordGeom><IrregularLine><PntList2D>0 0 0 10 5 10</PntList2D></IrregularLine></CoordGeom>
    </PlanFeature>
  </PlanFeatures>
</LandXML>"#;

    #[test]
    fn reads_points_parcels_features_units() {
        let d = read_document(DOC);
        assert_eq!(
            d.units,
            Some(Units {
                metric: true,
                linear_unit: "meter".into()
            })
        );

        assert_eq!(d.points.len(), 3);
        let p1 = &d.points[0];
        assert_eq!((p1.name.as_str(), p1.northing, p1.easting), ("1", 100.0, 200.0));
        assert_eq!(p1.elevation, Some(10.5));
        assert_eq!((p1.code.as_str(), p1.desc.as_str()), ("IP", "IP & tag"));
        assert_eq!(d.points[1].elevation, None);
        // pntRef copies the referenced point's coordinates.
        assert_eq!((d.points[2].northing, d.points[2].easting), (100.0, 200.0));
        assert_eq!(d.points[2].code, "REF");

        // The grouping parcel has no geometry and is skipped.
        assert_eq!(d.parcels.len(), 1);
        let lot = &d.parcels[0];
        assert_eq!(lot.name, "Lot 1");
        assert_eq!(lot.center, Some([210.0, 110.0]));
        assert_eq!(lot.segments.len(), 4);
        // Start/End resolved through pntRef, stored as [E, N].
        assert_eq!(lot.segments[0].start(), [200.0, 100.0]);
        assert_eq!(lot.segments[0].end(), [220.0, 100.0]);
        // The arc runs CCW from E220 N100 about E220 N110 to E220 N120: a
        // half circle bulging east, so the lot is the 20x20 box plus a half
        // disc of radius 10.
        let arc = lot.segments[1];
        assert!((arc.sweep() - std::f64::consts::PI).abs() < 1e-12);
        assert!((arc.radius() - 10.0).abs() < 1e-12);
        let expected = 400.0 + std::f64::consts::PI * 50.0;
        assert!((chain_area(&lot.segments) - expected).abs() < 1e-9);
        assert!((lot.area.unwrap() - 557.079633).abs() < 1e-6);
        assert!(d.warnings.is_empty(), "{:?}", d.warnings);

        assert_eq!(d.features.len(), 1);
        assert_eq!(d.features[0].name, "FENCE");
        assert_eq!(d.features[0].segments.len(), 2);
        assert_eq!(d.features[0].segments[1].end(), [10.0, 5.0]);
    }

    #[test]
    fn bulge_roundtrip() {
        for (ccw, end) in [(true, [10.0, 0.0]), (false, [0.0, 10.0])] {
            let arc = Segment::Arc {
                start: [0.0, 0.0],
                center: [5.0, 5.0],
                end,
                ccw,
            };
            let back = Segment::from_bulge(arc.start(), arc.end(), arc.bulge());
            let Segment::Arc { center, ccw: c2, .. } = back else {
                panic!("expected an arc");
            };
            assert_eq!(c2, ccw);
            assert!((center[0] - 5.0).abs() < 1e-9 && (center[1] - 5.0).abs() < 1e-9);
        }
    }

    #[test]
    fn chain_polylines_close_and_split() {
        let d = read_document(DOC);
        let rings = chain_polylines(&d.parcels[0].segments);
        assert_eq!(rings.len(), 1);
        let (verts, closed) = &rings[0];
        assert!(*closed);
        assert_eq!(verts.len(), 4);
        assert!((verts[1].2 - 1.0).abs() < 1e-12, "half circle: bulge tan(pi/4) = 1");
        // Round trip back to segments preserves the area.
        let segs = polyline_segments(verts, true);
        assert!((chain_area(&segs) - chain_area(&d.parcels[0].segments)).abs() < 1e-9);

        // A gap splits the chain into two open polylines.
        let gap = [
            Segment::Line { start: [0.0, 0.0], end: [1.0, 0.0] },
            Segment::Line { start: [5.0, 0.0], end: [6.0, 0.0] },
        ];
        let rings = chain_polylines(&gap);
        assert_eq!(rings.len(), 2);
        assert!(rings.iter().all(|(v, c)| !c && v.len() == 2));
    }

    #[test]
    fn write_then_read_roundtrip() {
        let mut d = read_document(DOC);
        d.surfaces = read_surfaces(
            r#"<Surface name="EG"><Definition surfType="TIN"><Pnts>
               <P id="a">0 0 1</P><P id="b">0 10 2</P><P id="c">10 0 3</P></Pnts>
               <Faces><F>a b c</F></Faces></Definition></Surface>"#,
        );
        let xml = write_document(&d, "2026-10-12", "09:30:00");
        assert!(xml.contains(r#"date="2026-10-12" time="09:30:00""#));
        assert!(looks_like_landxml(&xml));

        let back = read_document(&xml);
        assert_eq!(back.units, d.units);
        assert_eq!(back.points.len(), 3);
        assert_eq!(back.points[0], d.points[0]); // incl. escaped "IP & tag"
        assert_eq!(back.parcels.len(), 1);
        let (a, b) = (&d.parcels[0], &back.parcels[0]);
        assert_eq!((b.name.as_str(), b.desc.as_str()), ("Lot 1", "arc side"));
        assert_eq!(b.center, a.center);
        assert_eq!(b.segments, a.segments);
        assert_eq!(back.features[0].segments, d.features[0].segments);
        assert_eq!(back.surfaces.len(), 1);
        assert_eq!(back.surfaces[0].surface.nodes, d.surfaces[0].surface.nodes);
        assert_eq!(back.surfaces[0].surface.triangles, d.surfaces[0].surface.triangles);
    }

    #[test]
    fn writes_imperial_units() {
        let d = LandXmlDoc {
            units: Some(Units {
                metric: false,
                linear_unit: "foot".into(),
            }),
            ..Default::default()
        };
        let xml = write_document(&d, "2026-10-12", "00:00:00");
        assert!(xml.contains(r#"<Imperial linearUnit="foot""#));
        assert_eq!(read_document(&xml).units, d.units);
    }

    /// Opt-in check against a real export: set `LANDXML_SAMPLE` to MicroSurvey's
    /// shipped `Sample Data/subdivision-xsect.xml` (Land Desktop, 286 points,
    /// 83 lots, two TINs). Every lot's area computed from its lines and arcs
    /// must match the area Land Desktop wrote.
    #[test]
    fn sample_subdivision_parcel_areas() {
        let Ok(path) = std::env::var("LANDXML_SAMPLE") else {
            return;
        };
        let xml = std::fs::read_to_string(&path).expect("read LANDXML_SAMPLE");
        let d = read_document(&xml);
        assert_eq!(d.units.as_ref().map(|u| u.metric), Some(false));
        assert_eq!(d.points.len(), 286);
        assert_eq!(d.parcels.len(), 83);
        assert_eq!(d.surfaces.len(), 2);
        for p in &d.parcels {
            let declared = p.area.expect("LDT writes area");
            let got = chain_area(&p.segments);
            assert!(
                (got - declared).abs() <= 1e-5 * declared.max(1.0),
                "{}: computed {got:.4} vs declared {declared:.4}",
                p.name
            );
            assert!(chain_polylines(&p.segments).iter().all(|(_, closed)| *closed), "{}", p.name);
        }
    }
}
