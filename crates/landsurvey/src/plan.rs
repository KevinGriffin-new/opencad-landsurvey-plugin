//! Recognized survey-plan geometry — the JSON the `plan2cad` vector-PDF
//! pipeline emits. Parsing only (no CAD types), so a headless CLI can read the
//! same file. See `PLUGIN.md` for the import contract.
//!
//! All coordinates are world Easting/Northing. Elements are positional tuples,
//! matching the pipeline's current output verbatim (consume-as-is; a named v2
//! can be added as an additive deserialize path later).

use serde::Deserialize;

/// A parsed plan. Unknown keys (e.g. `plines`, until its shape is confirmed)
/// are ignored; missing keys default to empty.
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
        // Only `lines` present; `plines`/`arcs`/etc. absent → empty.
        let p = parse(r#"{"lines": [[0.0, 0.0, 1.0, 0.0, "L"]], "extra": 1}"#).unwrap();
        assert_eq!(p.lines.len(), 1);
        assert!(p.arcs.is_empty() && p.circles.is_empty() && p.texts.is_empty());
    }
}
