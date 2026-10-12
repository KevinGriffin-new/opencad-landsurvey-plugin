//! PNEZD / PENZD point parsing — pure, `std` only.
//!
//! Default format, one point per line:
//!
//! ```text
//! point, northing, easting, elevation, description
//! ```
//!
//! Blank lines and lines beginning with `#` are ignored. Real-world survey
//! exports vary in **delimiter** (comma / tab / semicolon / runs of spaces),
//! **column order** (PNEZD vs PENZD vs bespoke) and **line ending** (LF, CRLF,
//! or the bare CR of classic Mac exports), so parsing is configurable via
//! [`Format`]; [`parse`] keeps the convenient PNEZD default with auto-detected
//! delimiter. The description is read as the REMAINDER from its column onward,
//! so a trailing free-text description may itself contain the delimiter.
//!
//! Fields may be double-quoted, CSV style (`"IRON PIN, FND"`, `""` for a
//! literal quote). In semicolon-delimited files a decimal comma (`5000,123`)
//! is accepted, as European locales export them. Coordinates must be finite:
//! `NaN` and `inf` lines are skipped rather than imported.

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
    /// Per line: comma, tab and semicolon in the order they first appear,
    /// then whitespace; the first that yields a numeric northing and easting.
    Auto,
    Comma,
    Tab,
    /// `;` — the CSV separator in locales that write a decimal comma.
    Semicolon,
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

/// The delimiters [`Delimiter::Auto`] tries on `line`: comma, tab and
/// semicolon in the order they first appear (the one after the point number
/// comes first, so commas in a semicolon file's description don't win), then
/// whitespace, which also separates fields padded around a real delimiter.
fn auto_order(line: &str) -> Vec<Delimiter> {
    let mut found: Vec<(usize, Delimiter)> = [
        (',', Delimiter::Comma),
        ('\t', Delimiter::Tab),
        (';', Delimiter::Semicolon),
    ]
    .into_iter()
    .filter_map(|(c, d)| line.find(c).map(|i| (i, d)))
    .collect();
    found.sort_by_key(|&(i, _)| i);
    found.into_iter().map(|(_, d)| d).chain([Delimiter::Whitespace]).collect()
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

/// `splitn` on `sep`, ignoring separators inside double quotes: the first
/// `upto - 1` fields, then the remainder as the final element.
fn splitn_quoted(line: &str, sep: char, upto: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    for (i, c) in line.char_indices() {
        if out.len() + 1 >= upto {
            break;
        }
        if c == '"' {
            quoted = !quoted;
        } else if c == sep && !quoted {
            out.push(line[start..i].to_string());
            start = i + c.len_utf8();
        }
    }
    out.push(line[start..].to_string());
    out
}

/// Trim a field and, when the whole field is double-quoted, unwrap it and
/// turn each `""` back into `"`.
fn unquote(field: &str) -> String {
    let f = field.trim();
    match f.strip_prefix('"').and_then(|f| f.strip_suffix('"')) {
        Some(inner) => inner.replace("\"\"", "\""),
        None => f.to_string(),
    }
}

/// Split `line` into at most `upto` unquoted fields; the final field keeps the
/// remainder (so a trailing description retains embedded delimiters).
fn split_fields(line: &str, delim: Delimiter, upto: usize) -> Vec<String> {
    let raw = match delim {
        Delimiter::Comma => splitn_quoted(line, ',', upto),
        Delimiter::Tab => splitn_quoted(line, '\t', upto),
        Delimiter::Semicolon => splitn_quoted(line, ';', upto),
        Delimiter::Whitespace => splitn_whitespace(line, upto),
        Delimiter::Auto => unreachable!("parse_line picks a concrete delimiter"),
    };
    raw.iter().map(|f| unquote(f)).collect()
}

/// A finite number, or `None`. Under a semicolon delimiter the decimal-comma
/// notation of those locales is read too (see [`decimal_comma`]).
fn number(field: &str, delim: Delimiter) -> Option<f64> {
    let v = field
        .parse::<f64>()
        .ok()
        .or_else(|| (delim == Delimiter::Semicolon).then(|| decimal_comma(field)).flatten())?;
    v.is_finite().then_some(v)
}

/// Decimal-comma notation, as Excel writes it in pt-BR / de-DE locales: one
/// decimal comma (`5000,123`), optionally with dot thousands separators in
/// groups of three (`7.395.000,000`, `1.234,5`). Dots alone count as
/// thousands separators only when there are at least two groups
/// (`7.395.000`): a lone `395.000` already parsed as 395.0 above, and stays
/// that way. Only used for semicolon files, where a comma cannot be a
/// delimiter.
fn decimal_comma(field: &str) -> Option<f64> {
    let (int, frac) = match field.split_once(',') {
        Some((_, f)) if f.contains([',', '.']) => return None,
        Some((i, f)) => (i, Some(f)),
        None => (field, None),
    };
    let (sign, digits) = int.strip_prefix('-').map_or(("", int), |d| ("-", d));
    let groups: Vec<&str> = digits.split('.').collect();
    let all_digits = |t: &str| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit());
    if !groups.iter().all(|g| all_digits(g)) {
        return None;
    }
    if groups.len() > 1 {
        let well_grouped = groups[0].len() <= 3 && groups[1..].iter().all(|g| g.len() == 3);
        if !well_grouped || (frac.is_none() && groups.len() < 3) {
            return None;
        }
    } else if frac.is_none() {
        return None;
    }
    let mut text = format!("{sign}{}", groups.concat());
    if let Some(f) = frac {
        if !all_digits(f) {
            return None;
        }
        text = format!("{text}.{f}");
    }
    text.parse().ok()
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

    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    // LF, CRLF and bare CR all end a line; CRLF's empty middle is a blank line.
    for raw in text.split(['\n', '\r']) {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        match parse_line(line, fmt, max_idx + 1) {
            Some(p) => out.points.push(p),
            None => out.skipped += 1,
        }
    }
    out
}

/// One record under `fmt`, or `None` when no candidate delimiter gives a
/// finite northing and easting.
fn parse_line(line: &str, fmt: &Format, upto: usize) -> Option<PnezdPoint> {
    let candidates = match fmt.delimiter {
        Delimiter::Auto => auto_order(line),
        d => vec![d],
    };
    candidates.into_iter().find_map(|delim| {
        let f = split_fields(line, delim, upto);
        let get = |i: usize| f.get(i).map(String::as_str).unwrap_or("");
        let northing = number(get(fmt.northing), delim)?;
        let easting = number(get(fmt.easting), delim)?;
        // A blank or unparseable elevation defaults to 0.0.
        let elevation = fmt.elevation.and_then(|i| number(get(i), delim)).unwrap_or(0.0);
        Some(PnezdPoint {
            number: fmt.number.map(|i| get(i).to_string()).unwrap_or_default(),
            northing,
            easting,
            elevation,
            description: fmt.description.map(|i| get(i).to_string()).unwrap_or_default(),
        })
    })
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

    #[test]
    fn bare_cr_and_crlf_line_endings() {
        let rows = ["1,5000,1000,100,IP", "2,5100,1020,101,IP", "3,5050,1100,99,CP"];
        for sep in ["\r", "\r\n", "\n"] {
            let out = parse(&rows.join(sep));
            assert_eq!((out.points.len(), out.skipped), (3, 0), "separator {sep:?}");
        }
    }

    #[test]
    fn non_finite_coordinates_are_skipped() {
        let out = parse("1,NaN,1000,1,A\n2,5000,inf,1,B\n3,-infinity,1,1,C\n4,5000,1000,100,OK\n");
        assert_eq!((out.points.len(), out.skipped), (1, 3));
        assert_eq!(out.points[0].number, "4");
        // A non-finite elevation is unparseable, so it defaults like a blank one.
        assert_eq!(parse("1,5000,1000,NaN,X").points[0].elevation, 0.0);
    }

    #[test]
    fn semicolon_delimited_with_decimal_commas() {
        let out = parse("1;5000,125;1000,5;100,25;IP gefunden\n2;5100;1020;101;CP\n");
        assert_eq!((out.points.len(), out.skipped), (2, 0));
        let p = &out.points[0];
        assert_eq!((p.northing, p.easting, p.elevation), (5000.125, 1000.5, 100.25));
        assert_eq!(p.description, "IP gefunden");
        // Commas in a semicolon file's description don't hijack the split.
        let out = parse("7;5000;1000;100;FENCE 12,5 m, gate");
        assert_eq!(out.points[0].northing, 5000.0);
        assert_eq!(out.points[0].description, "FENCE 12,5 m, gate");
    }

    #[test]
    fn semicolon_thousands_separators() {
        // Excel pt-BR "CSV (separado por ponto e virgula)" with dotted thousands.
        let text = "1;7.395.000,000;333.000,000;752,000;MARCO\n\
                    2;7.395.010,5;333.010;752,1;MARCO\n\
                    3;-1.234,5;7.395.000;0;NEG\n";
        let out = parse(text);
        assert_eq!((out.points.len(), out.skipped), (3, 0));
        let p = &out.points[0];
        assert_eq!((p.northing, p.easting, p.elevation), (7_395_000.0, 333_000.0, 752.0));
        assert_eq!(out.points[0].description, "MARCO");
        // A lone dot group stays a decimal point, as in every other file.
        assert_eq!((out.points[1].northing, out.points[1].easting), (7_395_010.5, 333.01));
        // Dots alone count as thousands with two or more groups.
        assert_eq!((out.points[2].northing, out.points[2].easting), (-1_234.5, 7_395_000.0));
    }

    #[test]
    fn malformed_grouping_is_skipped_not_misread() {
        for line in [
            "1;7.39.000,0;333000;0;BAD",  // a group of two
            "1;7.3950.000,0;333000;0;BAD", // a group of four
            "1;7.395,000,0;333000;0;BAD",  // two commas
            "1;7,395.000;333000;0;BAD",    // dot after the comma
        ] {
            assert_eq!(parse(line).points.len(), 0, "{line}");
        }
    }

    #[test]
    fn decimal_commas_stay_off_outside_semicolon_files() {
        // A space- or tab-separated file with decimal commas must not be read
        // as a comma file with the spaces squeezed out (7395000,123 becoming
        // northing 123345000): the commas are not accepted there at all.
        for line in ["1 7395000,123 345000,456 100,5 MON", "1\t7395000,123\t345000,456\t100,5\tMON"] {
            let out = parse(line);
            assert!(
                out.points.iter().all(|p| p.northing < 10_000_000.0 && p.northing != 123_345_000.0),
                "{line}: {:?}",
                out.points
            );
        }
    }

    #[test]
    fn decimal_commas_only_under_semicolons() {
        // In a comma file, "5000,125" is two fields, not a decimal.
        let out = parse("1,5000,125,1000,IP");
        assert_eq!(out.points[0].northing, 5000.0);
        assert_eq!(out.points[0].easting, 125.0);
    }

    #[test]
    fn quoted_fields_are_unwrapped() {
        let text = "\"1\",\"5000.25\",\"1020.5\",\"101.25\",\"IP\"\n\
                    2,5100,1020,101,\"IRON PIN, FND\"\n\
                    \"A,3\",5050,1100,99,\"6\"\" \"\"PVC\"\"\"\n";
        let out = parse(text);
        assert_eq!((out.points.len(), out.skipped), (3, 0));
        assert_eq!(out.points[0].northing, 5000.25);
        assert_eq!(out.points[0].description, "IP");
        assert_eq!(out.points[1].description, "IRON PIN, FND");
        assert_eq!(out.points[2].number, "A,3");
        assert_eq!(out.points[2].description, "6\" \"PVC\"");
    }

    #[test]
    fn delimiter_in_a_tab_or_space_files_description() {
        let out = parse("1\t5000\t1000\t100\tIP, found\n2 5100 1020 101 FENCE, wood\n");
        assert_eq!((out.points.len(), out.skipped), (2, 0));
        assert_eq!(out.points[0].description, "IP, found");
        assert_eq!(out.points[1].description, "FENCE, wood");
    }

    #[test]
    fn leading_bom_does_not_stick_to_the_first_number() {
        assert_eq!(parse("\u{feff}1,5000,1000,100,IP").points[0].number, "1");
    }
}
