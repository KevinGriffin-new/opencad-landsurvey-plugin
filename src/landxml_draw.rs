//! Drawing LandXML alignments, profiles, cross sections and lot labels into
//! the document — and reading them back out of it for `LS_LANDXMLOUT`.
//!
//! The drawing is the source of truth for export: every entity that carries
//! data is tagged with XDATA holding what the geometry alone can't (names,
//! stations, spiral parameters, the profile/section view transform), so an
//! edited drawing exports what is now drawn.
//!
//! Layout: profile views stack below the plan, 10× vertical exaggeration;
//! cross-section views follow below them in rows, 5× vertical exaggeration.

use codec::xdata::{ExtendedDataRecord, XDataValue};
use codec::{EntityType, Line, LwPolyline, Text, Vector2, Vector3};
use ocs_plugin_api::host::HostApi;

use landsurvey::alignment::{
    format_station, nice_interval, Clothoid, CrossSection, Profile, ProfileData, Pvi,
    SectionSurface,
};
use landsurvey::landxml::{self, Alignment, Segment};

/// Alignment centreline LWPolyline: `[name, desc, staStart, spiral…]`, one
/// `SPI <first vertex> <vertex count> <length> <r_start> <r_end> <ccw>` value
/// per spiral (radii `inf` for a tangent).
pub const XDATA_ALIGN: &str = "LANDSURVEY_ALIGN";
/// Profile-view polyline: `[alignment, kind, name, desc, view, VC…]` where
/// `kind` is `PVI` (vertices are the PVIs; `VC <sta> <length>` values give
/// vertical curves), `SURFACE` (vertices are sampled points) or `FG`
/// (finished grade, drawn only). `view` is `x0 y0 sta0 datum vexag`.
pub const XDATA_PROFILE: &str = "LANDSURVEY_PROFILE";
/// Cross-section polyline: `[alignment, sta, section name, surface name,
/// surface desc, view]`, `view` = `x0 y0 datum vexag` (x0 = centreline).
pub const XDATA_XSECT: &str = "LANDSURVEY_XSECT";

/// Profile vertical exaggeration.
const PROFILE_VEXAG: f64 = 10.0;
/// Cross-section vertical exaggeration.
const SECTION_VEXAG: f64 = 5.0;

/// Plan extents `[minx, miny, maxx, maxy]`.
pub type Bbox = [f64; 4];

pub fn bbox_add(b: &mut Option<Bbox>, p: [f64; 2]) {
    let n = b.get_or_insert([p[0], p[1], p[0], p[1]]);
    *n = [n[0].min(p[0]), n[1].min(p[1]), n[2].max(p[0]), n[3].max(p[1])];
}

fn add(host: &mut dyn HostApi, mut ent: EntityType, layer: &str) {
    ent.common_mut().layer = layer.to_string();
    host.add_entity(ent);
}

fn add_tagged(host: &mut dyn HostApi, mut ent: EntityType, layer: &str, app: &str, values: &[String]) {
    ent.common_mut().layer = layer.to_string();
    let handle = host.add_entity(ent);
    let mut rec = ExtendedDataRecord::new(app);
    for v in values {
        rec.add_value(XDataValue::String(v.clone()));
    }
    host.write_record(handle, rec);
}

/// All string values of an `app` record on an entity.
pub fn tag_values(e: &EntityType, app: &str) -> Option<Vec<String>> {
    let rec = e.common().extended_data.get_record(app)?;
    Some(
        rec.values
            .iter()
            .filter_map(|v| match v {
                XDataValue::String(s) => Some(s.clone()),
                _ => None,
            })
            .collect(),
    )
}

fn polyline(pts: &[[f64; 2]]) -> EntityType {
    let mut lw = LwPolyline::new();
    for p in pts {
        lw.add_point_with_bulge(Vector2::new(p[0], p[1]), 0.0);
    }
    EntityType::LwPolyline(lw)
}

fn line(a: [f64; 2], b: [f64; 2]) -> EntityType {
    EntityType::Line(Line::from_points(
        Vector3::new(a[0], a[1], 0.0),
        Vector3::new(b[0], b[1], 0.0),
    ))
}

fn text(s: &str, at: [f64; 2], h: f64, rot: f64) -> EntityType {
    EntityType::Text(
        Text::with_value(s.to_string(), Vector3::new(at[0], at[1], 0.0))
            .with_height(h)
            .with_rotation(rot),
    )
}

/// Approximate rendered width of `s` at height `h`.
fn text_width(s: &str, h: f64) -> f64 {
    s.chars().count() as f64 * h * 0.6
}

fn nums(s: &str) -> Vec<f64> {
    s.split_whitespace().filter_map(|t| t.parse().ok()).collect()
}

// ---------------------------------------------------------------------------
// Lot labels
// ---------------------------------------------------------------------------

/// Lot area text in the file's units.
pub fn area_text(area: f64, units: Option<&landxml::Units>) -> String {
    match units {
        Some(u) if u.metric => format!("{area:.1} m\u{b2}"),
        Some(_) => format!("{area:.0} sq ft ({:.3} ac)", area / 43_560.0),
        None => format!("{area:.2}"),
    }
}

/// Draw a lot label centred on `center`: the name, and the area beneath it.
/// The name text is tagged `LANDSURVEY_PARCEL [name, desc, LABEL]` so
/// [`lot_label_center`] can recover the label point.
pub fn draw_lot_label(
    host: &mut dyn HostApi,
    app: &str,
    name: &str,
    desc: &str,
    area: &str,
    center: [f64; 2],
    h: f64,
) {
    let name_at = [center[0] - text_width(name, h) / 2.0, center[1] + 0.15 * h];
    add_tagged(
        host,
        text(name, name_at, h, 0.0),
        "LS-PARCEL-LABEL",
        app,
        &[name.to_string(), desc.to_string(), "LABEL".to_string()],
    );
    let ha = 0.7 * h;
    let area_at = [center[0] - text_width(area, ha) / 2.0, center[1] - 0.15 * h - ha];
    add(host, text(area, area_at, ha, 0.0), "LS-PARCEL-LABEL");
}

/// The label point of a lot label drawn by [`draw_lot_label`], from its name
/// text (so a moved label exports where it now sits).
pub fn lot_label_center(t: &Text) -> [f64; 2] {
    let h = t.height;
    [
        t.insertion_point.x + text_width(&t.value, h) / 2.0,
        t.insertion_point.y - 0.15 * h,
    ]
}

/// Area centroid of a closed chain (its drawn vertices), `[E, N]`.
pub fn chain_centroid(segments: &[Segment]) -> Option<[f64; 2]> {
    let mut pts: Vec<[f64; 2]> = segments.iter().flat_map(|s| s.plan_points()).collect();
    if pts.len() < 3 {
        return None;
    }
    pts.push(pts[0]);
    let (mut a, mut cx, mut cy) = (0.0, 0.0, 0.0);
    for w in pts.windows(2) {
        let c = w[0][0] * w[1][1] - w[1][0] * w[0][1];
        a += c;
        cx += (w[0][0] + w[1][0]) * c;
        cy += (w[0][1] + w[1][1]) * c;
    }
    (a.abs() > 1e-12).then(|| [cx / (3.0 * a), cy / (3.0 * a)])
}

// ---------------------------------------------------------------------------
// Alignments
// ---------------------------------------------------------------------------

/// Centreline vertices `(x, y, bulge)` plus the spiral specs for the tag.
fn centreline(a: &Alignment) -> (Vec<(f64, f64, f64)>, Vec<String>, bool) {
    let mut verts: Vec<(f64, f64, f64)> = Vec::new();
    let mut specs = Vec::new();
    for s in &a.segments {
        match s {
            Segment::Spiral(c) => {
                let pts = c.densify();
                specs.push(format!(
                    "SPI {} {} {} {} {} {}",
                    verts.len(),
                    pts.len(),
                    c.length,
                    c.r_start,
                    c.r_end,
                    u8::from(c.ccw)
                ));
                verts.extend(pts.iter().map(|p| (p[0], p[1], 0.0)));
            }
            _ => {
                let p = s.start();
                verts.push((p[0], p[1], s.bulge()));
            }
        }
    }
    let closed = match (a.segments.first(), a.segments.last()) {
        (Some(f), Some(l)) => {
            let (p, q) = (f.start(), l.end());
            (p[0] - q[0]).hypot(p[1] - q[1]) < 1e-6
        }
        _ => false,
    };
    if !closed {
        if let Some(l) = a.segments.last() {
            verts.push((l.end()[0], l.end()[1], 0.0));
        }
    }
    (verts, specs, closed)
}

/// Rebuild alignment segments from a tagged centreline polyline.
fn segments_from_centreline(verts: &[(f64, f64, f64)], closed: bool, specs: &[String]) -> Vec<Segment> {
    // first vertex -> (count, clothoid parameters)
    let spirals: Vec<(usize, usize, f64, f64, f64, bool)> = specs
        .iter()
        .filter(|s| s.starts_with("SPI "))
        .filter_map(|s| {
            let v = nums(&s[4..]);
            (v.len() == 6).then(|| (v[0] as usize, v[1] as usize, v[2], v[3], v[4], v[5] != 0.0))
        })
        .collect();
    let n = verts.len();
    let at = |i: usize| {
        let v = verts[i % n];
        [v.0, v.1]
    };
    let edges = if closed { n } else { n.saturating_sub(1) };
    let mut out = Vec::new();
    let mut i = 0;
    while i < edges {
        if let Some(&(_, count, length, r0, r1, ccw)) = spirals.iter().find(|s| s.0 == i) {
            out.push(Segment::Spiral(Clothoid {
                start: at(i),
                end: at(i + count),
                length,
                r_start: r0,
                r_end: r1,
                ccw,
            }));
            i += count;
        } else {
            out.push(Segment::from_bulge(at(i), at(i + 1), verts[i].2));
            i += 1;
        }
    }
    out
}

/// Draw alignment centrelines (`LS-ALIGN`) with station ticks and labels
/// (`LS-ALIGN-LABEL`), then their profile and cross-section views below
/// `plan`. Returns `(profiles, sections)` drawn.
pub fn draw_alignments(
    host: &mut dyn HostApi,
    alignments: &[Alignment],
    metric: bool,
    plan: Option<Bbox>,
) -> (usize, usize) {
    let mut plan = plan;
    for a in alignments {
        for s in &a.segments {
            for p in s.plan_points().into_iter().chain([s.end()]) {
                bbox_add(&mut plan, p);
            }
        }
    }
    let plan = plan.unwrap_or([0.0, 0.0, 1000.0, 1000.0]);
    let extent = (plan[2] - plan[0]).max(plan[3] - plan[1]).max(1.0);
    let h = extent / 150.0;

    for a in alignments {
        if a.segments.is_empty() {
            continue;
        }
        let (verts, specs, closed) = centreline(a);
        let mut lw = LwPolyline::new();
        for &(x, y, b) in &verts {
            lw.add_point_with_bulge(Vector2::new(x, y), b);
        }
        lw.is_closed = closed;
        let mut values = vec![a.name.clone(), a.desc.clone(), a.sta_start.to_string()];
        values.extend(specs);
        add_tagged(host, EntityType::LwPolyline(lw), "LS-ALIGN", XDATA_ALIGN, &values);
        draw_stations(host, a, metric, h);
    }

    // Views below the plan: profiles first, then cross sections.
    let gap = extent * 0.1;
    let mut top = plan[1] - gap;
    let (mut profiles, mut sections) = (0, 0);
    for a in alignments {
        if !a.profiles.is_empty() {
            top = draw_profile_view(host, a, metric, [plan[0], top], h) - gap;
            profiles += a.profiles.len();
        }
    }
    for a in alignments {
        if !a.cross_sections.is_empty() {
            top = draw_section_views(host, a, metric, [plan[0], top], h) - gap;
            sections += a.cross_sections.len();
        }
    }
    (profiles, sections)
}

/// Ticks across the centreline at every geometry point and at a round
/// interval, each labelled with its station.
fn draw_stations(host: &mut dyn HostApi, a: &Alignment, metric: bool, h: f64) {
    let len = a.length();
    let step = nice_interval(len, 20.0);
    let mut stations: Vec<(f64, bool)> = a.key_stations().into_iter().map(|s| (s, true)).collect();
    let mut s = (a.sta_start / step).ceil() * step;
    while s < a.sta_start + len {
        if !stations.iter().any(|(k, _)| (k - s).abs() < step * 0.05) {
            stations.push((s, false));
        }
        s += step;
    }
    for (sta, key) in stations {
        let Some((p, heading)) = a.at_station(sta) else {
            continue;
        };
        let n = [-heading.sin(), heading.cos()]; // left normal
        let half = if key { 1.5 * h } else { h };
        add(
            host,
            line([p[0] - n[0] * half, p[1] - n[1] * half], [p[0] + n[0] * half, p[1] + n[1] * half]),
            "LS-ALIGN-LABEL",
        );
        // The label runs along the left normal beyond the tick, flipped to
        // stay readable: reading outward normally, inward (ending at the
        // tick) when outward would read upside-down.
        let label = format_station(sta, metric);
        let th = 0.8 * h;
        let gap = half + 0.3 * h;
        let mut rot = heading + std::f64::consts::FRAC_PI_2;
        let mut dist = gap;
        let deg = rot.to_degrees().rem_euclid(360.0);
        if deg > 90.0 && deg <= 270.0 {
            rot += std::f64::consts::PI;
            dist = gap + text_width(&label, th);
        }
        // Centre the glyph band on the tick line (baseline is th/2 below it).
        let up = [-rot.sin(), rot.cos()];
        let at = [
            p[0] + n[0] * dist - up[0] * th / 2.0,
            p[1] + n[1] * dist - up[1] * th / 2.0,
        ];
        add(host, text(&label, at, th, rot), "LS-ALIGN-LABEL");
    }
}

/// A profile view with its top-left at `corner`; returns its bottom edge.
fn draw_profile_view(host: &mut dyn HostApi, a: &Alignment, metric: bool, corner: [f64; 2], h: f64) -> f64 {
    // Extents cover the drawn profiles and the PVIs (a crest PVI sits above
    // its curve).
    let mut pts_all: Vec<[f64; 2]> = a.profiles.iter().flat_map(Profile::points).collect();
    for p in &a.profiles {
        if let ProfileData::Design(pvis) = &p.data {
            pts_all.extend(pvis.iter().map(|v| [v.sta, v.elev]));
        }
    }
    pts_all.retain(|p| p[0].is_finite() && p[1].is_finite());
    if pts_all.is_empty() {
        return corner[1];
    }
    let (mut s0, mut s1, mut e0, mut e1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for p in &pts_all {
        s0 = s0.min(p[0]);
        s1 = s1.max(p[0]);
        e0 = e0.min(p[1]);
        e1 = e1.max(p[1]);
    }
    let estep = nice_interval((e1 - e0).max(1.0), 5.0);
    let datum = (e0 / estep).floor() * estep;
    let etop = (e1 / estep).ceil() * estep;
    let vex = PROFILE_VEXAG;
    let height = (etop - datum) * vex;
    let (x0, y0) = (corner[0], corner[1] - height);
    let map = |sta: f64, el: f64| [x0 + (sta - s0), y0 + (el - datum) * vex];
    let view = format!("{x0} {y0} {s0} {datum} {vex}");

    // Frame, grid and axis labels.
    let grid = "LS-PROF-GRID";
    let (bl, tr) = (map(s0, datum), map(s1, etop));
    add(host, polyline(&[bl, [tr[0], bl[1]], tr, [bl[0], tr[1]], bl]), grid);
    let sstep = nice_interval((s1 - s0).max(1.0), 10.0);
    let mut s = (s0 / sstep).ceil() * sstep;
    while s <= s1 + 1e-9 {
        let (b, t) = (map(s, datum), map(s, etop));
        add(host, line(b, t), grid);
        let lab = format_station(s, metric);
        add(host, text(&lab, [b[0] - text_width(&lab, 0.8 * h) / 2.0, b[1] - 1.5 * h], 0.8 * h, 0.0), grid);
        s += sstep;
    }
    let mut e = datum;
    while e <= etop + 1e-9 {
        let (l, r) = (map(s0, e), map(s1, e));
        add(host, line(l, r), grid);
        let lab = format!("{e:.1}");
        add(host, text(&lab, [l[0] - text_width(&lab, 0.8 * h) - 0.5 * h, l[1] - 0.4 * h], 0.8 * h, 0.0), grid);
        e += estep;
    }
    let title = format!("PROFILE {}  (V {vex}:1)", a.name);
    add(host, text(&title, [bl[0], tr[1] + h], 1.2 * h, 0.0), grid);

    for p in &a.profiles {
        match &p.data {
            ProfileData::Design(pvis) => {
                let pvi_pts: Vec<[f64; 2]> = pvis.iter().map(|v| map(v.sta, v.elev)).collect();
                let mut values = vec![
                    a.name.clone(),
                    "PVI".to_string(),
                    p.name.clone(),
                    p.desc.clone(),
                    view.clone(),
                ];
                for (i, v) in pvis.iter().enumerate() {
                    if v.curve_len > 0.0 && i > 0 && i + 1 < pvis.len() {
                        values.push(format!("VC {} {}", v.sta, v.curve_len));
                    }
                }
                add_tagged(host, polyline(&pvi_pts), "LS-PROF-PVI", XDATA_PROFILE, &values);
                let fg: Vec<[f64; 2]> = p.points().iter().map(|q| map(q[0], q[1])).collect();
                add_tagged(
                    host,
                    polyline(&fg),
                    "LS-PROF-FG",
                    XDATA_PROFILE,
                    &[a.name.clone(), "FG".to_string(), p.name.clone()],
                );
                // Label PVIs that carry a vertical curve.
                for v in pvis.iter().filter(|v| v.curve_len > 0.0) {
                    let at = map(v.sta, v.elev);
                    let lab = format!("PVI {} EL {:.3} L={:.2}", format_station(v.sta, metric), v.elev, v.curve_len);
                    add(host, text(&lab, [at[0], at[1] + 0.8 * h], 0.7 * h, 0.0), "LS-PROF-PVI");
                }
            }
            ProfileData::Surface(points) => {
                let pts: Vec<[f64; 2]> = points.iter().map(|q| map(q[0], q[1])).collect();
                add_tagged(
                    host,
                    polyline(&pts),
                    "LS-PROF-EG",
                    XDATA_PROFILE,
                    &[a.name.clone(), "SURFACE".to_string(), p.name.clone(), p.desc.clone(), view.clone()],
                );
            }
        }
    }
    y0 - 3.0 * h
}

/// Cross-section views in rows of up to 8, top-left at `corner`; returns the
/// bottom edge.
fn draw_section_views(host: &mut dyn HostApi, a: &Alignment, metric: bool, corner: [f64; 2], h: f64) -> f64 {
    let (mut o0, mut o1, mut span) = (0.0f64, 0.0f64, 1.0f64);
    for cs in &a.cross_sections {
        let (mut lo, mut hi) = (f64::MAX, f64::MIN);
        for p in cs.surfaces.iter().flat_map(|s| &s.points) {
            o0 = o0.min(p[0]);
            o1 = o1.max(p[0]);
            lo = lo.min(p[1]);
            hi = hi.max(p[1]);
        }
        if hi >= lo {
            span = span.max(hi - lo);
        }
    }
    let cell_w = (o1 - o0) * 1.15 + 4.0 * h;
    let vex = SECTION_VEXAG;
    let cell_h = span * vex * 1.3 + 6.0 * h;
    let cols = 8usize;
    let title = format!("CROSS SECTIONS {}  (V {vex}:1)", a.name);
    add(host, text(&title, [corner[0], corner[1] + h], 1.2 * h, 0.0), "LS-XSEC-LABEL");
    let mut bottom = corner[1];
    for (i, cs) in a.cross_sections.iter().enumerate() {
        let (row, col) = (i / cols, i % cols);
        let cx = corner[0] + col as f64 * cell_w - o0 * 1.075 + 2.0 * h;
        let cell_top = corner[1] - row as f64 * cell_h;
        let datum = cs
            .surfaces
            .iter()
            .flat_map(|s| &s.points)
            .map(|p| p[1])
            .fold(f64::MAX, f64::min);
        let y0 = cell_top - cell_h + 3.0 * h;
        let view = format!("{cx} {y0} {datum} {vex}");
        for s in &cs.surfaces {
            let pts: Vec<[f64; 2]> = s.points.iter().map(|p| [cx + p[0], y0 + (p[1] - datum) * vex]).collect();
            let layer = if s.name.to_ascii_lowercase().contains("exist") { "LS-XSEC-EG" } else { "LS-XSEC" };
            add_tagged(
                host,
                polyline(&pts),
                layer,
                XDATA_XSECT,
                &[a.name.clone(), cs.sta.to_string(), cs.name.clone(), s.name.clone(), s.desc.clone(), view.clone()],
            );
        }
        // Centreline tick and station label.
        add(host, line([cx, y0 - 0.5 * h], [cx, y0 + span * vex * 1.1]), "LS-XSEC-LABEL");
        let lab = format!("STA {}", format_station(cs.sta, metric));
        add(host, text(&lab, [cx - text_width(&lab, 0.8 * h) / 2.0, y0 - 2.0 * h], 0.8 * h, 0.0), "LS-XSEC-LABEL");
        bottom = bottom.min(cell_top - cell_h);
    }
    bottom
}

// ---------------------------------------------------------------------------
// Export: rebuild alignments from the drawing
// ---------------------------------------------------------------------------

/// Alignments (centrelines, profiles, cross sections) recovered from tagged
/// drawing entities, in drawing order.
pub fn collect_alignments(doc: &codec::CadDocument) -> Vec<Alignment> {
    let mut out: Vec<Alignment> = Vec::new();
    fn slot<'a>(out: &'a mut Vec<Alignment>, name: &str) -> &'a mut Alignment {
        if let Some(i) = out.iter().position(|a| a.name == name) {
            return &mut out[i];
        }
        out.push(Alignment {
            name: name.to_string(),
            desc: String::new(),
            sta_start: 0.0,
            segments: Vec::new(),
            profiles: Vec::new(),
            cross_sections: Vec::new(),
        });
        out.last_mut().unwrap()
    }

    for e in doc.entities() {
        let EntityType::LwPolyline(pl) = e else { continue };
        let verts: Vec<(f64, f64, f64)> =
            pl.vertices.iter().map(|v| (v.location.x, v.location.y, v.bulge)).collect();

        if let Some(v) = tag_values(e, XDATA_ALIGN) {
            let Some(name) = v.first() else { continue };
            let a = slot(&mut out, name);
            a.desc = v.get(1).cloned().unwrap_or_default();
            a.sta_start = v.get(2).and_then(|s| s.parse().ok()).unwrap_or(0.0);
            a.segments = segments_from_centreline(&verts, pl.is_closed, v.get(3..).unwrap_or(&[]));
        } else if let Some(v) = tag_values(e, XDATA_PROFILE) {
            if v.len() < 5 {
                continue; // FG: drawn only
            }
            let view = nums(&v[4]);
            if view.len() != 5 {
                continue;
            }
            let (x0, y0, s0, datum, vex) = (view[0], view[1], view[2], view[3], view[4]);
            let pts: Vec<[f64; 2]> = verts
                .iter()
                .map(|q| [q.0 - x0 + s0, (q.1 - y0) / vex + datum])
                .collect();
            let data = match v[1].as_str() {
                "PVI" => {
                    let curves: Vec<(f64, f64)> = v[5..]
                        .iter()
                        .filter_map(|s| s.strip_prefix("VC "))
                        .map(nums)
                        .filter(|n| n.len() == 2)
                        .map(|n| (n[0], n[1]))
                        .collect();
                    ProfileData::Design(
                        pts.iter()
                            .map(|p| Pvi {
                                sta: p[0],
                                elev: p[1],
                                curve_len: curves
                                    .iter()
                                    .find(|c| (c.0 - p[0]).abs() < 1e-6)
                                    .map_or(0.0, |c| c.1),
                            })
                            .collect(),
                    )
                }
                "SURFACE" => ProfileData::Surface(pts),
                _ => continue,
            };
            let a = slot(&mut out, &v[0]);
            a.profiles.push(Profile {
                name: v[2].clone(),
                desc: v[3].clone(),
                data,
            });
        } else if let Some(v) = tag_values(e, XDATA_XSECT) {
            if v.len() < 6 {
                continue;
            }
            let (Ok(sta), view) = (v[1].parse::<f64>(), nums(&v[5])) else { continue };
            if view.len() != 4 {
                continue;
            }
            let (cx, y0, datum, vex) = (view[0], view[1], view[2], view[3]);
            let points: Vec<[f64; 2]> =
                verts.iter().map(|q| [q.0 - cx, (q.1 - y0) / vex + datum]).collect();
            let a = slot(&mut out, &v[0]);
            let surface = SectionSurface {
                name: v[3].clone(),
                desc: v[4].clone(),
                points,
            };
            match a.cross_sections.iter_mut().find(|c| (c.sta - sta).abs() < 1e-9) {
                Some(cs) => cs.surfaces.push(surface),
                None => a.cross_sections.push(CrossSection {
                    sta,
                    name: v[2].clone(),
                    surfaces: vec![surface],
                }),
            }
        }
    }
    for a in &mut out {
        a.cross_sections.sort_by(|x, y| x.sta.total_cmp(&y.sta));
    }
    out
}
