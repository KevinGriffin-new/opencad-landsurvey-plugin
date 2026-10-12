//! Spiral, stationing and profile checks against real alignment exports from
//! Aplitop MDT, 12d Model and Bentley OpenRoads (see
//! `fixtures/landxml-alignments/README.md` for sources and licences).
//!
//! The engine places each clothoid from its two end points, length and radii
//! only. Each vendor also wrote the spiral's PI (and Aplitop its tangent
//! lengths), which the engine never reads — so reproducing them is an
//! independent check of the clothoid maths, rotation sense and coordinate
//! order. Heading continuity at every element joint, and the cumulative
//! stations against each element's `staStart`, check the chain as a whole.

use landsurvey::alignment::ProfileData;
use landsurvey::landxml::{self, Segment};

const SAMPLES: [(&str, &str); 5] = [
    ("aplitop-1", include_str!("fixtures/landxml-alignments/aplitop-1.xml")),
    ("aplitop-2", include_str!("fixtures/landxml-alignments/aplitop-2.xml")),
    ("indot-openroads", include_str!("fixtures/landxml-alignments/indot-openroads.xml")),
    ("12d-example", include_str!("fixtures/landxml-alignments/12d-example.xml")),
    (
        "openroads-spiral-double-radius",
        include_str!("fixtures/landxml-alignments/openroads-spiral-double-radius.xml"),
    ),
];

/// Text of the first `<tag>…</tag>` after `from`, with its end offset.
fn next_text<'a>(xml: &'a str, from: usize, tag: &str) -> Option<(&'a str, usize)> {
    let open = format!("<{tag}>");
    let i = from + xml[from..].find(&open)? + open.len();
    let j = i + xml[i..].find(&format!("</{tag}>"))?;
    Some((&xml[i..j], j))
}

fn attr_f(open_tag: &str, name: &str) -> Option<f64> {
    let k = open_tag.find(&format!(" {name}=\""))? + name.len() + 3;
    open_tag[k..k + open_tag[k..].find('"')?].parse().ok()
}

/// Each `<Spiral …>` in file order: (PI as [E, N], tanLong, tanShort).
fn file_spirals(xml: &str) -> Vec<([f64; 2], Option<f64>, Option<f64>)> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(rel) = xml[at..].find("<Spiral") {
        let s = at + rel;
        let gt = s + xml[s..].find('>').unwrap();
        let open = &xml[s..gt];
        let (pi, end) = next_text(xml, gt, "PI").expect("spiral PI");
        let v: Vec<f64> = pi.split_whitespace().map(|t| t.parse().unwrap()).collect();
        out.push(([v[1], v[0]], attr_f(open, "tanLong"), attr_f(open, "tanShort")));
        at = end;
    }
    out
}

/// `staStart` of each CoordGeom element (Line / Curve / Spiral) in file order.
fn file_element_stations(xml: &str) -> Vec<Option<f64>> {
    let cg = &xml[xml.find("<CoordGeom").unwrap()..xml.find("</CoordGeom>").unwrap()];
    let mut out = Vec::new();
    for (i, _) in cg.match_indices('<') {
        let tag: String = cg[i + 1..].chars().take_while(|c| c.is_alphabetic()).collect();
        if matches!(tag.as_str(), "Line" | "Curve" | "Spiral") {
            let gt = i + cg[i..].find('>').unwrap();
            out.push(attr_f(&cg[i..gt], "staStart"));
        }
    }
    out
}

fn spirals(doc: &landxml::LandXmlDoc) -> Vec<landsurvey::alignment::Clothoid> {
    doc.alignments
        .iter()
        .flat_map(|a| &a.segments)
        .filter_map(|s| match s {
            Segment::Spiral(c) => Some(*c),
            _ => None,
        })
        .collect()
}

#[test]
fn spiral_pi_and_tangents_match_each_vendor() {
    let mut checked = 0;
    for (name, xml) in SAMPLES {
        let doc = landxml::read_document(xml);
        let ours = spirals(&doc);
        let theirs = file_spirals(xml);
        assert_eq!(ours.len(), theirs.len(), "{name}: spiral count");
        for (i, (c, (pi, long, short))) in ours.iter().zip(&theirs).enumerate() {
            assert!(c.misfit() < 1e-5, "{name} spiral {i}: misfit {}", c.misfit());
            let (got, t0, t1) = c.pi();
            let d = (got[0] - pi[0]).hypot(got[1] - pi[1]);
            // Files round lengths/radii to 3-6 decimals, which moves the
            // derived PI by a few ppm of the spiral length.
            let tol = (1e-5 * c.length).max(0.002);
            assert!(d < tol, "{name} spiral {i}: PI off by {d:.4} (tol {tol:.4})");
            // For a spiral between two radii, vendors quote the tangents of the
            // parent clothoid (from its zero-curvature origin), not of this
            // piece, so tangent lengths are compared only when one end is a
            // tangent. The PI above still pins the geometry.
            let simple = !c.r_start.is_finite() || !c.r_end.is_finite();
            if let (true, Some(long), Some(short)) = (simple, long, short) {
                assert!((t0.max(t1) - long).abs() < 0.002, "{name} spiral {i}: tanLong");
                assert!((t0.min(t1) - short).abs() < 0.002, "{name} spiral {i}: tanShort");
            }
            checked += 1;
        }
        assert!(
            doc.warnings.iter().all(|w| !w.contains("spiral")),
            "{name}: {:?}",
            doc.warnings
        );
    }
    assert_eq!(checked, 34);
}

#[test]
fn headings_are_continuous_and_stations_match() {
    for (name, xml) in SAMPLES {
        let doc = landxml::read_document(xml);
        assert!(!doc.alignments.is_empty(), "{name}");
        for a in &doc.alignments {
            for (i, w) in a.segments.windows(2).enumerate() {
                let out = w[0].at(w[0].length()).1;
                let inn = w[1].at(0.0).1;
                let diff = (out - inn + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU)
                    - std::f64::consts::PI;
                assert!(diff.abs() < 2e-5, "{name}/{} joint {i}: {diff:e} rad", a.name);
                let gap = (w[0].end()[0] - w[1].start()[0]).hypot(w[0].end()[1] - w[1].start()[1]);
                // Coordinates are written to 6 decimals.
                assert!(gap < 1e-5, "{name}/{} joint {i}: gap {gap}", a.name);
            }
        }
        // Only single-alignment files carry per-element staStart worth checking.
        if doc.alignments.len() == 1 {
            let ours = doc.alignments[0].key_stations();
            for (i, sta) in file_element_stations(xml).into_iter().enumerate() {
                if let Some(sta) = sta {
                    assert!((ours[i] - sta).abs() < 2e-3, "{name} element {i}: {} vs {sta}", ours[i]);
                }
            }
        }
    }
}

#[test]
fn profiles_read() {
    let doc = landxml::read_document(SAMPLES[0].1);
    let a = &doc.alignments[0];
    assert_eq!(a.name, "Horizontal");
    assert!((a.length() - 507.067).abs() < 1e-3, "{}", a.length());
    let ProfileData::Design(pvis) = &a.profiles[0].data else {
        panic!("design profile")
    };
    assert_eq!(pvis.len(), 4);
    assert_eq!((pvis[1].sta, pvis[1].elev, pvis[1].curve_len), (79.0, 372.0, 129.487));
    // PVI on the curve: the parabola passes below a crest PVI by L·A/8.
    let e = landsurvey::alignment::profile_elevation(pvis, 79.0).unwrap();
    let g1 = (372.0 - 365.8) / 79.0;
    let g2 = (346.0 - 372.0) / (467.0 - 79.0);
    assert!((e - (372.0 - 129.487 * (g1 - g2) / 8.0)).abs() < 1e-9);

    // OpenRoads writes 3-value coordinates and its own staStart.
    let indot = landxml::read_document(SAMPLES[2].1);
    let a = &indot.alignments[0];
    assert!((a.sta_start - 2103.72056).abs() < 1e-5);
    assert!((a.length() - 2796.6790253265699).abs() < 1e-6);
    let ProfileData::Design(pvis) = &a.profiles[0].data else {
        panic!("design profile")
    };
    assert_eq!(pvis.iter().filter(|p| p.curve_len > 0.0).count(), 4);
}

#[test]
fn alignments_roundtrip_through_writer() {
    for (name, xml) in SAMPLES {
        let doc = landxml::read_document(xml);
        let back = landxml::read_document(&landxml::write_document(&doc, "2026-10-12", "00:00:00"));
        assert_eq!(back.alignments.len(), doc.alignments.len(), "{name}");
        for (a, b) in doc.alignments.iter().zip(&back.alignments) {
            assert_eq!(a.name, b.name);
            assert!((a.sta_start - b.sta_start).abs() < 1e-7, "{name}");
            assert_eq!(a.profiles.len(), b.profiles.len(), "{name}");
            for (p, q) in a.profiles.iter().zip(&b.profiles) {
                // The writer keeps 8 decimals.
                let close = |x: &[[f64; 2]], y: &[[f64; 2]]| {
                    x.len() == y.len()
                        && x.iter().zip(y).all(|(u, v)| (u[0] - v[0]).abs() < 1e-7 && (u[1] - v[1]).abs() < 1e-7)
                };
                assert!(close(&p.points(), &q.points()), "{name}/{}", p.name);
            }
            assert_eq!(a.segments.len(), b.segments.len(), "{name}");
            for (x, y) in a.segments.iter().zip(&b.segments) {
                assert!((x.length() - y.length()).abs() < 1e-6, "{name}");
                let (p, q) = (x.at(x.length() / 2.0).0, y.at(y.length() / 2.0).0);
                assert!((p[0] - q[0]).hypot(p[1] - q[1]) < 1e-6, "{name}");
            }
        }
    }
}
