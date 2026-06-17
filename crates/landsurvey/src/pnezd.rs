//! PNEZD comma-delimited point parsing — pure, `std` only.
//!
//! Format, one point per line:
//!
//! ```text
//! point, northing, easting, elevation, description
//! ```
//!
//! Blank lines and lines beginning with `#` are ignored. The description is
//! optional and may itself contain commas (only the first four fields are
//! split).

/// A single parsed PNEZD record.
#[derive(Debug, Clone, PartialEq)]
pub struct PnezdPoint {
    pub number: String,
    pub northing: f64,
    pub easting: f64,
    pub elevation: f64,
    pub description: String,
}

/// Outcome of parsing a PNEZD document.
#[derive(Debug, Default, PartialEq)]
pub struct ParseOutcome {
    pub points: Vec<PnezdPoint>,
    /// Non-blank, non-comment lines that could not be parsed.
    pub skipped: usize,
}

/// Parse PNEZD text into points, counting malformed lines rather than failing.
pub fn parse(text: &str) -> ParseOutcome {
    let mut out = ParseOutcome::default();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = line.splitn(5, ',').map(str::trim).collect();
        if f.len() < 4 {
            out.skipped += 1;
            continue;
        }
        let (Ok(northing), Ok(easting), Ok(elevation)) = (
            f[1].parse::<f64>(),
            f[2].parse::<f64>(),
            f[3].parse::<f64>(),
        ) else {
            out.skipped += 1;
            continue;
        };
        out.points.push(PnezdPoint {
            number: f[0].to_string(),
            northing,
            easting,
            elevation,
            description: f.get(4).copied().unwrap_or("").to_string(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_records_and_skips_junk() {
        // Comment + blank line are ignored; one unparseable line is skipped.
        let text = "\
# control points
1, 5000.00, 5000.00, 100.0, CP1
2, 5100.50, 5050.25, 101.2, IRON PIN, NE corner

garbage line
3, 5200, 5100, 102
";
        let out = parse(text);
        assert_eq!(out.points.len(), 3);
        assert_eq!(out.skipped, 1);
        assert_eq!(out.points[0].number, "1");
        assert_eq!(out.points[0].northing, 5000.0);
        // Description keeps embedded commas.
        assert_eq!(out.points[1].description, "IRON PIN, NE corner");
        // Missing description is empty, not an error.
        assert_eq!(out.points[2].description, "");
    }
}
