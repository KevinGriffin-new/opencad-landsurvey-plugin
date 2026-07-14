//! Recognized survey-plan geometry — the JSON the `plan2cad` vector-PDF
//! pipeline emits. Parsing only (no CAD types), so a headless CLI can read the
//! same file. See `PLUGIN.md` for the import contract.
//!
//! All coordinates are world Easting/Northing. Elements are positional tuples,
//! matching the pipeline's current output verbatim (consume-as-is; a named v2
//! can be added as an additive deserialize path later).

use serde::Deserialize;

/// A parsed plan. Unknown keys are ignored; missing keys default to empty.
#[derive(Debug, Default, Deserialize)]
pub struct Plan {
    /// `[x1, y1, x2, y2, layer]`
    #[serde(default)]
    pub lines: Vec<(f64, f64, f64, f64, String)>,
    /// `[cx, cy, radius, start_deg, end_deg, layer]` — angles in degrees.
    #[serde(default)]
    pub arcs: Vec<(f64, f64, f64, f64, f64, String)>,
    /// `[cx, cy, radius, layer]`
    #[serde(default)]
    pub circles: Vec<(f64, f64, f64, String)>,
    /// `[x, y, value, style]`
    #[serde(default)]
    pub texts: Vec<(f64, f64, String, String)>,
    /// `[[x, y], [x, y], …, layer]` — ordered chain, layer name last.
    /// plat2json `--vectorize trace` emits these as `polylines`; `plines`
    /// is accepted as an alias.
    #[serde(default, alias = "plines")]
    pub polylines: Vec<PlanPolyline>,
}

/// One ordered point chain from the plan: `[[x, y], [x, y], …, layer]`.
/// The heterogeneous tail (points, then a layer string) needs a hand-rolled
/// deserialize — serde tuples can't express "N pairs then a string".
#[derive(Debug, PartialEq)]
pub struct PlanPolyline {
    pub points: Vec<(f64, f64)>,
    pub layer: String,
}

impl<'de> Deserialize<'de> for PlanPolyline {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let items = Vec::<serde_json::Value>::deserialize(d)?;
        let (layer_v, pts_v) = items
            .split_last()
            .ok_or_else(|| D::Error::custom("polyline: empty element"))?;
        let layer = layer_v
            .as_str()
            .ok_or_else(|| D::Error::custom("polyline: last element must be the layer name"))?
            .to_string();
        let mut points = Vec::with_capacity(pts_v.len());
        for p in pts_v {
            let xy = p
                .as_array()
                .filter(|a| a.len() == 2)
                .ok_or_else(|| D::Error::custom("polyline: point must be [x, y]"))?;
            match (xy[0].as_f64(), xy[1].as_f64()) {
                (Some(x), Some(y)) => points.push((x, y)),
                _ => return Err(D::Error::custom("polyline: point coordinates must be numbers")),
            }
        }
        Ok(PlanPolyline { points, layer })
    }
}

impl PlanPolyline {
    /// The vertices to draw and whether the chain closes: a chain whose last
    /// point repeats the first (with at least 3 distinct vertices) is a closed
    /// ring — the duplicate closing vertex is dropped and `closed` is true.
    pub fn ring(&self) -> (&[(f64, f64)], bool) {
        let pts = &self.points;
        if pts.len() >= 4 && pts.first() == pts.last() {
            (&pts[..pts.len() - 1], true)
        } else {
            (pts, false)
        }
    }

    /// Direction-independent keys of every edge in the chain (a ring's closing
    /// edge included — the source keeps the duplicate closing vertex).
    pub fn edge_keys(&self) -> impl Iterator<Item = SegKey> + '_ {
        self.points
            .windows(2)
            .map(|w| seg_key(w[0].0, w[0].1, w[1].0, w[1].1))
    }
}

/// Direction-independent identity of a line segment, by exact f64 bits.
/// plat2json writes the polyline vertices and the flattened `lines` from the
/// same rounded values, so bit-equality is the right duplicate test — no
/// tolerance needed.
pub type SegKey = [u64; 4];

/// Key for the segment (x1, y1)–(x2, y2), the same in either direction.
pub fn seg_key(x1: f64, y1: f64, x2: f64, y2: f64) -> SegKey {
    let a = [x1.to_bits(), y1.to_bits()];
    let b = [x2.to_bits(), y2.to_bits()];
    if a <= b {
        [a[0], a[1], b[0], b[1]]
    } else {
        [b[0], b[1], a[0], a[1]]
    }
}

/// Parse plan JSON. Errors on malformed JSON or a wrong element shape.
pub fn parse(json: &str) -> Result<Plan, serde_json::Error> {
    serde_json::from_str(json)
}

/// Translate AutoCAD/MicroSurvey inline control codes to Unicode for display
/// (`%%d`→°, `%%p`→±, `%%c`→⌀).
pub fn decode_text(s: &str) -> String {
    s.replace("%%d", "\u{b0}")
        .replace("%%p", "\u{b1}")
        .replace("%%c", "\u{2300}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_each_element_kind() {
        let j = r#"{
            "lines":   [[0.0, 0.0, 1.0, 1.0, "PROPERTY_LINE"]],
            "arcs":    [[2.0, 2.0, 5.0, 0.0, 90.0, "PROPERTY_LINE"]],
            "circles": [[3.0, 3.0, 0.3, "MSPOINT"]],
            "plines":  [],
            "texts":   [[4.0, 4.0, "N88%%d16'15.00\"E", "BEARING4"]]
        }"#;
        let p = parse(j).unwrap();
        assert_eq!(p.lines.len(), 1);
        assert_eq!(p.lines[0].4, "PROPERTY_LINE");
        assert_eq!(p.arcs[0].4, 90.0); // end angle (degrees)
        assert_eq!(p.circles[0].2, 0.3);
        assert_eq!(p.texts[0].3, "BEARING4");
        assert_eq!(decode_text(&p.texts[0].2), "N88\u{b0}16'15.00\"E");
    }

    #[test]
    fn missing_keys_default_empty_and_unknown_ignored() {
        // Only `lines` present; `polylines`/`arcs`/etc. absent → empty.
        let p = parse(r#"{"lines": [[0.0, 0.0, 1.0, 0.0, "L"]], "extra": 1}"#).unwrap();
        assert_eq!(p.lines.len(), 1);
        assert!(p.arcs.is_empty() && p.circles.is_empty() && p.texts.is_empty());
        assert!(p.polylines.is_empty());
    }

    #[test]
    fn parses_polylines_and_plines_alias() {
        // plat2json `--vectorize trace` shape: point pairs then the layer.
        let p = parse(
            r#"{"polylines": [[[0.0, 0.0], [10.5, 0.0], [10.5, 7.25], "PROPERTY_LINE"]]}"#,
        )
        .unwrap();
        assert_eq!(p.polylines.len(), 1);
        assert_eq!(p.polylines[0].points.len(), 3);
        assert_eq!(p.polylines[0].points[2], (10.5, 7.25));
        assert_eq!(p.polylines[0].layer, "PROPERTY_LINE");

        // The key name plan.rs originally anticipated still works.
        let p = parse(r#"{"plines": [[[0.0, 0.0], [1.0, 1.0], "L"]]}"#).unwrap();
        assert_eq!(p.polylines.len(), 1);
        assert_eq!(p.polylines[0].points.len(), 2);
    }

    #[test]
    fn polyline_malformed_elements_error() {
        // Layer missing (all points) and non-numeric coordinates both fail
        // loudly rather than importing garbage.
        assert!(parse(r#"{"polylines": [[[0.0, 0.0], [1.0, 1.0]]]}"#).is_err());
        assert!(parse(r#"{"polylines": [[[0.0, "x"], "L"]]}"#).is_err());
        assert!(parse(r#"{"polylines": [[]]}"#).is_err());
    }

    #[test]
    fn ring_detects_closed_chains() {
        let closed = PlanPolyline {
            points: vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 0.0)],
            layer: "L".into(),
        };
        let (pts, is_closed) = closed.ring();
        assert!(is_closed);
        assert_eq!(pts.len(), 3); // duplicate closing vertex dropped

        let open = PlanPolyline {
            points: vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)],
            layer: "L".into(),
        };
        let (pts, is_closed) = open.ring();
        assert!(!is_closed);
        assert_eq!(pts.len(), 3);

        // A degenerate 2-point "ring" (A, A) stays open — not enough vertices.
        let degenerate = PlanPolyline { points: vec![(1.0, 2.0), (1.0, 2.0)], layer: "L".into() };
        assert!(!degenerate.ring().1);
    }

    #[test]
    fn seg_key_is_direction_independent() {
        assert_eq!(seg_key(1.0, 2.0, 3.0, 4.0), seg_key(3.0, 4.0, 1.0, 2.0));
        assert_ne!(seg_key(1.0, 2.0, 3.0, 4.0), seg_key(1.0, 2.0, 3.0, 4.5));
        // Every flattened edge of a chain matches the chain's own edge keys —
        // the dedupe contract LS_IMPORTPLAN relies on.
        let pl = PlanPolyline {
            points: vec![(0.0, 0.0), (10.5, 0.0), (10.5, 7.25), (0.0, 0.0)],
            layer: "L".into(),
        };
        let keys: std::collections::HashSet<SegKey> = pl.edge_keys().collect();
        assert_eq!(keys.len(), 3);
        assert!(keys.contains(&seg_key(10.5, 0.0, 0.0, 0.0))); // reversed line
        assert!(keys.contains(&seg_key(10.5, 7.25, 0.0, 0.0))); // closing edge
        assert!(!keys.contains(&seg_key(1.0, 1.0, 2.0, 2.0))); // unrelated segment
    }
}
