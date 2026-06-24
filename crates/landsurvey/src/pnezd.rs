//! PNEZD / PENZD point parsing — pure, `std` only.
//!
//! Default format, one point per line:
//!
//! ```text
//! point, northing, easting, elevation, description
//! ```
//!
//! Blank lines and lines beginning with `#` are ignored. Real-world survey
//! exports vary in **delimiter** (comma / tab / runs of spaces) and **column
//! order** (PNEZD vs PENZD vs bespoke), so parsing is configurable via
//! [`Format`]; [`parse`] keeps the convenient PNEZD default with auto-detected
//! delimiter. The description is read as the REMAINDER from its column onward,
//! so a trailing free-text description may itself contain the delimiter.

/// A single parsed point record. `number` is kept as a string so alphanumeric
/// point names ("CP1", "TBM") survive — survey numbers are not always integers.
#[derive(Debug, Clone, PartialEq)]
pub struct PnezdPoint {
    pub number: String,
    pub northing: f64,
    pub easting: f64,
    pub elevation: f64,
    pub description: String,
}

impl PnezdPoint {
    /// The feature code — the first whitespace-delimited token of the
    /// description (e.g. `"CB"` from `"CB 0.50 0.30"`). Empty when there is no
    /// description. Used to group/style points by code on import.
    pub fn code(&self) -> &str {
        self.description.split_whitespace().next().unwrap_or("")
    }
}

/// Outcome of parsing a point document.
#[derive(Debug, Default, PartialEq)]
pub struct ParseOutcome {
    pub points: Vec<PnezdPoint>,
    /// Non-blank, non-comment lines that could not be parsed.
    pub skipped: usize,
}

/// Field delimiter for a point file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delimiter {
    /// Comma if the line has one, else tab, else any run of whitespace.
    Auto,
    Comma,
    Tab,
    /// Any run of ASCII whitespace (collapses repeated spaces — the common
    /// fixed-width / space-aligned export case).
    Whitespace,
}

/// Which 0-based column holds each field. `northing` and `easting` are required;
/// the rest are optional. `description`, when it is the last mapped column, is
/// read as the remainder of the line so it may contain the delimiter.
#[derive(Debug, Clone, PartialEq)]
pub struct Format {
    pub number: Option<usize>,
    pub northing: usize,
    pub easting: usize,
    pub elevation: Option<usize>,
    pub description: Option<usize>,
    pub delimiter: Delimiter,
}

impl Format {
    /// `P, N, E, Z, D` — the conventional order; delimiter auto-detected.
    pub fn pnezd() -> Self {
        Self {
            number: Some(0),
            northing: 1,
            easting: 2,
            elevation: Some(3),
            description: Some(4),
            delimiter: Delimiter::Auto,
        }
    }

    /// `P, E, N, Z, D` — easting and northing swapped (Civil 3D PENZD export).
    pub fn penzd() -> Self {
        Self {
            number: Some(0),
            easting: 1,
            northing: 2,
            elevation: Some(3),
            description: Some(4),
            delimiter: Delimiter::Auto,
        }
    }
}

impl Default for Format {
    fn default() -> Self {
        Self::pnezd()
    }
}

fn resolve_delim(line: &str, d: Delimiter) -> Delimiter {
    match d {
        Delimiter::Auto => {
            if line.contains(',') {
                Delimiter::Comma
            } else if line.contains('\t') {
                Delimiter::Tab
            } else {
                Delimiter::Whitespace
            }
        }
        other => other,
    }
}

/// `splitn` over runs of ASCII whitespace: the first `upto - 1` tokens, then the
/// untrimmed remainder as the final element.
fn splitn_whitespace(line: &str, upto: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = line.trim_start();
    while upto == 0 || out.len() + 1 < upto {
        match rest.find(char::is_whitespace) {
            Some(i) => {
                out.push(rest[..i].to_string());
                rest = rest[i..].trim_start();
                if rest.is_empty() {
                    return out;
                }
            }
            None => break,
        }
    }
    if !rest.is_empty() {
        out.push(rest.trim().to_string());
    }
    out
}

/// Split `line` into at most `upto` trimmed fields; the final field keeps the
/// remainder (so a trailing description retains embedded delimiters).
fn split_fields(line: &str, delim: Delimiter, upto: usize) -> Vec<String> {
    match delim {
        Delimiter::Comma => line.splitn(upto, ',').map(|s| s.trim().to_string()).collect(),
        Delimiter::Tab => line.splitn(upto, '\t').map(|s| s.trim().to_string()).collect(),
        Delimiter::Whitespace => splitn_whitespace(line, upto),
        Delimiter::Auto => unreachable!("resolve_delim removes Auto"),
    }
}

/// Parse PNEZD text (comma/tab/space auto-detected, PNEZD order), counting
/// malformed lines rather than failing.
pub fn parse(text: &str) -> ParseOutcome {
    parse_with(text, &Format::pnezd())
}

/// Parse point text under an explicit [`Format`] (column order + delimiter),
/// counting malformed lines rather than failing. Northing and easting are
/// required; a blank or unparseable elevation defaults to `0.0`.
pub fn parse_with(text: &str, fmt: &Format) -> ParseOutcome {
    let mut out = ParseOutcome::default();
    let max_idx = [
        fmt.number,
        Some(fmt.northing),
        Some(fmt.easting),
        fmt.elevation,
        fmt.description,
    ]
    .into_iter()
    .flatten()
    .max()
    .unwrap_or(0);

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let delim = resolve_delim(line, fmt.delimiter);
        let f = split_fields(line, delim, max_idx + 1);
        let get = |i: usize| f.get(i).map(String::as_str).unwrap_or("");

        let (Ok(northing), Ok(easting)) =
            (get(fmt.northing).parse::<f64>(), get(fmt.easting).parse::<f64>())
        else {
            out.skipped += 1;
            continue;
        };

        let elevation = fmt
            .elevation
            .map(|i| get(i).parse::<f64>().unwrap_or(0.0))
            .unwrap_or(0.0);
        let number = fmt.number.map(|i| get(i).to_string()).unwrap_or_default();
        let description = fmt.description.map(|i| get(i).to_string()).unwrap_or_default();

        out.points.push(PnezdPoint {
            number,
            northing,
            easting,
            elevation,
            description,
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
        // Description keeps embedded commas (it is the trailing field).
        assert_eq!(out.points[1].description, "IRON PIN, NE corner");
        // Missing description is empty, not an error.
        assert_eq!(out.points[2].description, "");
    }

    #[test]
    fn feature_code_is_first_description_token() {
        let out = parse("1, 5000, 4000, 10, CB 0.50 0.30\n2, 5001, 4001, 11,\n");
        assert_eq!(out.points[0].code(), "CB");
        assert_eq!(out.points[1].code(), ""); // empty description -> empty code
    }

    #[test]
    fn keeps_alphanumeric_point_names() {
        let out = parse("CP1, 5000, 4000, 10, control\nTBM, 5001, 4001, 11, bench\n");
        assert_eq!(out.points.len(), 2);
        assert_eq!(out.points[0].number, "CP1");
        assert_eq!(out.points[1].number, "TBM");
    }

    #[test]
    fn penzd_swaps_easting_and_northing() {
        // P, E, N, Z, D
        let out = parse_with("1, 5050.25, 5100.50, 101.2, X", &Format::penzd());
        assert_eq!(out.points.len(), 1);
        assert_eq!(out.points[0].easting, 5050.25);
        assert_eq!(out.points[0].northing, 5100.50);
    }

    #[test]
    fn whitespace_delimited_collapses_runs_and_keeps_multiword_desc() {
        // Ragged spacing + a multi-word trailing description.
        let out = parse("1   5000.00    5000.00   100.0   IRON PIN NE corner\n");
        assert_eq!(out.points.len(), 1, "skipped {}", out.skipped);
        let p = &out.points[0];
        assert_eq!(p.easting, 5000.0);
        assert_eq!(p.elevation, 100.0);
        assert_eq!(p.description, "IRON PIN NE corner");
    }

    #[test]
    fn tab_delimited() {
        let fmt = Format {
            delimiter: Delimiter::Tab,
            ..Format::pnezd()
        };
        let out = parse_with("1\t5000\t4000\t100\tCP\n2\t5001\t4001\t101\tIP\n", &fmt);
        assert_eq!(out.points.len(), 2);
        assert_eq!(out.points[1].easting, 4001.0);
    }

    #[test]
    fn auto_detects_whitespace_when_no_commas() {
        let out = parse("1 5000 4000 100 GND\n");
        assert_eq!(out.points.len(), 1);
        assert_eq!(out.points[0].description, "GND");
    }

    #[test]
    fn custom_column_order_without_number_or_elevation() {
        // Columns: Easting, Northing, Description (no number, no elevation).
        let fmt = Format {
            number: None,
            easting: 0,
            northing: 1,
            elevation: None,
            description: Some(2),
            delimiter: Delimiter::Whitespace,
        };
        let out = parse_with("5000.0 5100.0 GND\n", &fmt);
        assert_eq!(out.points.len(), 1);
        let p = &out.points[0];
        assert_eq!(p.easting, 5000.0);
        assert_eq!(p.northing, 5100.0);
        assert_eq!(p.elevation, 0.0);
        assert_eq!(p.number, "");
        assert_eq!(p.description, "GND");
    }
}
