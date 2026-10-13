//! Field-to-finish linework: turn coded point descriptions into figures
//! (connected strings of points), as MicroSurvey AutoMAP, Carlson
//! field-to-finish and Civil 3D figures do. See
//! `docs/field-to-finish-design.md`.
//!
//! Two coding conventions are supported, one at a time ([`Mode`]):
//!
//! - **Strings** — string-numbered codes. The first word of the description is
//!   a feature code plus a string number (`ER1`, `GL2`); every point with the
//!   same first word joins one figure. Control words after the code steer it:
//!   `B` begins a new figure, `E` ends it, `CLS` closes it back to its first
//!   point, and `PC` … `PT` mark a curve.
//! - **Xyz** — MicroSurvey's prefix coding ("Connect Points by XYZ Coding"):
//!   `Z` line, `X` curve, `Y` sideshot spur, `.` close; the first point with the
//!   bare code ends the line.
//!
//! In both, anything after the first space is a note, and `A*B` (or `A.B`)
//! puts one point on two figures. Curve points are flagged on their vertices;
//! they are drawn straight until curve fitting is built.

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};

use crate::library::Library;

/// A survey point with its field description.
#[derive(Debug, Clone, PartialEq)]
pub struct CodedPoint {
    pub name: String,
    pub northing: f64,
    pub easting: f64,
    pub elevation: f64,
    pub description: String,
}

/// Which line-coding convention the descriptions use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Strings,
    Xyz,
}

/// The order points are connected in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Order {
    /// The order the points were shot (file / import order).
    #[default]
    Shot,
    /// Increasing point number, as MicroSurvey's XYZ coding specifies.
    Number,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Options {
    pub mode: Mode,
    pub order: Order,
}

/// One vertex of a figure: an index into the input points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Vertex {
    pub point: usize,
    /// The point was shot on a curve (`X` prefix, or between `PC` and `PT`).
    pub curve: bool,
}

/// A connected string of points.
#[derive(Debug, Clone, PartialEq)]
pub struct Figure {
    /// Feature code, without prefix or string number (`ER`, `CURB`).
    pub code: String,
    /// The string's key as coded (`ER1`, `CURB`).
    pub string: String,
    pub vertices: Vec<Vertex>,
    pub closed: bool,
    /// The library row that styles this figure, when a library is in use.
    pub row: Option<usize>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Linework {
    pub figures: Vec<Figure>,
    pub warnings: Vec<String>,
}

impl Linework {
    /// Number of input points with a curve flag on some figure.
    pub fn curve_vertices(&self) -> usize {
        self.figures
            .iter()
            .flat_map(|f| &f.vertices)
            .filter(|v| v.curve)
            .count()
    }
}

/// Build the figures for `points` (given in shot order), with no library:
/// any string-numbered code connects.
pub fn build(points: &[CodedPoint], opts: &Options) -> Linework {
    build_with(points, opts, None)
}

/// Build the figures for `points` (given in shot order). With a `library`,
/// string coding connects only the codes whose row has `CONNECT` set, with or
/// without a string number, and reports codes the library lacks; XYZ coding
/// treats a whole word that is itself a library code as a plain code, never
/// as a prefix (`ZL` stays `ZL`). Each figure records its styling row.
pub fn build_with(points: &[CodedPoint], opts: &Options, library: Option<&Library>) -> Linework {
    let order = ordered(points, opts.order);
    let mut b = Builder {
        points,
        library,
        open: HashMap::new(),
        figures: Vec::new(),
        warnings: Vec::new(),
        unknown: BTreeMap::new(),
    };
    match opts.mode {
        Mode::Strings => b.strings(&order),
        Mode::Xyz => b.xyz(&order),
    }
    b.finish()
}

/// Point indices in connection order.
fn ordered(points: &[CodedPoint], order: Order) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..points.len()).collect();
    if order == Order::Number {
        // Stable, so equal numbers keep their shot order.
        idx.sort_by(|&a, &b| compare_names(&points[a].name, &points[b].name));
    }
    idx
}

/// Numeric point names compare as numbers, others as text; numbers first.
fn compare_names(a: &str, b: &str) -> Ordering {
    match (a.trim().parse::<f64>(), b.trim().parse::<f64>()) {
        (Ok(x), Ok(y)) => x.partial_cmp(&y).unwrap_or(Ordering::Equal),
        (Ok(_), Err(_)) => Ordering::Less,
        (Err(_), Ok(_)) => Ordering::Greater,
        (Err(_), Err(_)) => a.cmp(b),
    }
}

/// The coded part of a description: its first word.
fn code_word(description: &str) -> &str {
    description.split_whitespace().next().unwrap_or("")
}

/// Split a code word into the codes it joins: `A*B` and `A.B` put one point
/// on two figures. A leading `.` is kept (it is the XYZ close prefix).
fn join_parts(word: &str) -> Vec<&str> {
    let (lead, rest) = match word.strip_prefix('.') {
        Some(r) => (1, r),
        None => (0, word),
    };
    let mut parts: Vec<&str> = Vec::new();
    let mut start = 0;
    for (i, c) in rest.char_indices() {
        if c == '*' || c == '.' {
            parts.push(&rest[start..i]);
            start = i + 1;
        }
    }
    parts.push(&rest[start..]);
    if lead == 1 {
        // Re-attach the close prefix to the first code.
        parts[0] = &word[..1 + parts[0].len()];
    }
    parts.into_iter().filter(|p| !p.is_empty() && *p != ".").collect()
}

/// `ER12` → `("ER", true)`: a string-numbered code is letters (and other
/// non-digits) followed by a string number.
fn split_string_number(code: &str) -> (&str, bool) {
    let base = code.trim_end_matches(|c: char| c.is_ascii_digit());
    let numbered = base.len() < code.len() && base.chars().any(|c| c.is_alphabetic());
    (base, numbered)
}

struct Builder<'a> {
    points: &'a [CodedPoint],
    library: Option<&'a Library>,
    /// Open figure per string key.
    open: HashMap<String, usize>,
    figures: Vec<Figure>,
    warnings: Vec<String>,
    /// Codes missing from the library, with how many points use them.
    unknown: BTreeMap<String, usize>,
}

impl Builder<'_> {
    fn row(&self, code: &str, string: &str) -> Option<usize> {
        self.library.and_then(|l| l.lookup_word(string).or_else(|| l.lookup_word(code)))
    }

    fn start(&mut self, code: &str, string: &str, v: Vertex) -> usize {
        let row = self.row(code, string);
        self.figures.push(Figure {
            code: code.to_string(),
            string: string.to_string(),
            vertices: vec![v],
            closed: false,
            row,
        });
        let f = self.figures.len() - 1;
        self.open.insert(string.to_string(), f);
        f
    }

    fn name(&self, i: usize) -> &str {
        &self.points[i].name
    }

    fn strings(&mut self, order: &[usize]) {
        // Strings inside a PC … PT curve.
        let mut in_curve: HashMap<String, bool> = HashMap::new();
        for &i in order {
            let desc = &self.points[i].description;
            let controls: Vec<String> = desc
                .split_whitespace()
                .skip(1)
                .map(|w| w.to_ascii_uppercase())
                .collect();
            let has = |w: &str| controls.iter().any(|c| c == w);
            let parts = join_parts(code_word(desc));
            let single = parts.len() == 1;
            for part in parts {
                let key = part.to_ascii_uppercase();
                let (base, numbered) = split_string_number(&key);
                let connects = match self.library {
                    Some(lib) => {
                        // A lone code may match the whole description
                        // (AutoMAP rows such as `TREE 40FIR`).
                        let row = if single { lib.lookup(desc) } else { lib.lookup_word(&key) };
                        match row {
                            Some(r) => lib.codes[r].connect,
                            None => {
                                let code = if numbered { base } else { key.as_str() };
                                *self.unknown.entry(code.to_string()).or_insert(0) += 1;
                                false
                            }
                        }
                    }
                    None => numbered,
                };
                if !connects {
                    continue;
                }
                let base = if numbered { base } else { key.as_str() };
                if has("B") {
                    self.open.remove(&key);
                }
                let curving = in_curve.get(&key).copied().unwrap_or(false);
                let curve = curving || has("PC") || has("PT");
                if has("PC") {
                    in_curve.insert(key.clone(), true);
                }
                if has("PT") {
                    in_curve.insert(key.clone(), false);
                }
                let v = Vertex { point: i, curve };
                let f = match self.open.get(&key) {
                    Some(&f) => {
                        self.figures[f].vertices.push(v);
                        f
                    }
                    None => self.start(base, &key, v),
                };
                if has("CLS") {
                    self.figures[f].closed = true;
                }
                if has("CLS") || has("E") {
                    self.open.remove(&key);
                    in_curve.remove(&key);
                }
            }
        }
    }

    fn xyz(&mut self, order: &[usize]) {
        for &i in order {
            let desc = &self.points[i].description;
            for part in join_parts(code_word(desc)) {
                let upper = part.to_ascii_uppercase();
                let mut chars = upper.chars();
                let prefix = chars.next().unwrap_or(' ');
                let rest = chars.as_str();
                let is_code = self.library.is_some_and(|l| l.has_exact(&upper));
                let (kind, base) = match prefix {
                    'Z' | 'X' | 'Y' | '.' if !rest.is_empty() && !is_code => (prefix, rest.to_string()),
                    _ => (' ', upper.clone()),
                };
                match kind {
                    'Z' | 'X' => {
                        let v = Vertex { point: i, curve: kind == 'X' };
                        match self.open.get(&base) {
                            Some(&f) => self.figures[f].vertices.push(v),
                            None => {
                                self.start(&base, &base, v);
                            }
                        }
                    }
                    'Y' => match self.open.get(&base) {
                        Some(&f) => {
                            let from = *self.figures[f].vertices.last().expect("open figure has a vertex");
                            let row = self.row(&base, &base);
                            self.figures.push(Figure {
                                code: base.clone(),
                                string: base.clone(),
                                vertices: vec![Vertex { point: from.point, curve: false }, Vertex { point: i, curve: false }],
                                closed: false,
                                row,
                            });
                        }
                        None => self.warnings.push(format!(
                            "point {}: sideshot {part} has no open {base} line to attach to",
                            self.name(i)
                        )),
                    },
                    '.' => match self.open.remove(&base) {
                        Some(f) => {
                            self.figures[f].vertices.push(Vertex { point: i, curve: false });
                            self.figures[f].closed = true;
                        }
                        None => self.warnings.push(format!(
                            "point {}: {part} closes a {base} line, but none is open",
                            self.name(i)
                        )),
                    },
                    _ => {
                        // A bare code ends an open line, including this point.
                        if let Some(f) = self.open.remove(&base) {
                            self.figures[f].vertices.push(Vertex { point: i, curve: false });
                        }
                    }
                }
            }
        }
    }

    fn finish(mut self) -> Linework {
        let mut figures = Vec::with_capacity(self.figures.len());
        for mut f in std::mem::take(&mut self.figures) {
            // A figure that ends on its own start point closes.
            if f.vertices.len() >= 4 {
                let (a, z) = (f.vertices[0].point, f.vertices[f.vertices.len() - 1].point);
                let (p, q) = (&self.points[a], &self.points[z]);
                if a == z || ((p.northing - q.northing).hypot(p.easting - q.easting) < 1e-4) {
                    f.vertices.pop();
                    f.closed = true;
                }
            }
            if f.vertices.len() < 2 {
                let at = f.vertices.first().map(|v| self.name(v.point)).unwrap_or("?");
                self.warnings.push(format!("{}: only one point ({at}), no line drawn", f.string));
                continue;
            }
            figures.push(f);
        }
        if !self.unknown.is_empty() {
            let mut codes: Vec<(&String, &usize)> = self.unknown.iter().collect();
            codes.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
            let shown: Vec<String> = codes.iter().take(12).map(|(c, n)| format!("{c} ({n})")).collect();
            let more = codes.len().saturating_sub(12);
            self.warnings.push(format!(
                "not in the library, so not connected: {}{}",
                shown.join(", "),
                if more > 0 { format!(" and {more} more") } else { String::new() }
            ));
        }
        Linework { figures, warnings: self.warnings }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pts(rows: &[(&str, &str)]) -> Vec<CodedPoint> {
        rows.iter()
            .enumerate()
            .map(|(i, (name, desc))| CodedPoint {
                name: name.to_string(),
                northing: i as f64,
                easting: (i * i) as f64,
                elevation: 0.0,
                description: desc.to_string(),
            })
            .collect()
    }

    /// Each figure as its point names, joined with `-` (closed ones end `-*`).
    fn chains(points: &[CodedPoint], lw: &Linework) -> Vec<String> {
        lw.figures
            .iter()
            .map(|f| {
                let mut s: Vec<&str> = f.vertices.iter().map(|v| points[v.point].name.as_str()).collect();
                if f.closed {
                    s.push("*");
                }
                s.join("-")
            })
            .collect()
    }

    fn xyz(points: &[CodedPoint]) -> Linework {
        build(points, &Options { mode: Mode::Xyz, order: Order::Number })
    }

    /// MicroSurvey help, "Z Prefix (Line)": lines 1-2-4-6 and 3-5-7-8-9.
    #[test]
    fn xyz_z_prefix_help_example() {
        let p = pts(&[
            ("1", "ZSIB"), ("2", "ZSIB"), ("3", "ZCURB"), ("4", "ZSIB"), ("5", "ZCURB"),
            ("6", "SIB"), ("7", "ZCURB"), ("8", "ZCURB"), ("9", "CURB"),
        ]);
        let lw = xyz(&p);
        assert_eq!(chains(&p, &lw), ["1-2-4-6", "3-5-7-8-9"]);
        assert!(lw.warnings.is_empty(), "{:?}", lw.warnings);
    }

    /// "X Prefix (Curve)": all seven connected, 3-5 on curves.
    #[test]
    fn xyz_x_prefix_help_example() {
        let p = pts(&[
            ("1", "ZCURB"), ("2", "ZCURB"), ("3", "XCURB"), ("4", "XCURB"), ("5", "XCURB"),
            ("6", "ZCURB"), ("7", "CURB"),
        ]);
        let lw = xyz(&p);
        assert_eq!(chains(&p, &lw), ["1-2-3-4-5-6-7"]);
        let curved: Vec<bool> = lw.figures[0].vertices.iter().map(|v| v.curve).collect();
        assert_eq!(curved, [false, false, true, true, true, false, false]);
        assert_eq!(lw.curve_vertices(), 3);
    }

    /// "Y Prefix (Sideshot)": 1-2-4-5 and 2-3.
    #[test]
    fn xyz_y_prefix_help_example() {
        let p = pts(&[("1", "ZFENCE"), ("2", "ZFENCE"), ("3", "YFENCE"), ("4", "ZFENCE"), ("5", "FENCE")]);
        let mut got = chains(&p, &xyz(&p));
        got.sort();
        assert_eq!(got, ["1-2-4-5", "2-3"]);
    }

    /// ". Prefix (Close Figure)": 25-26-32-33 back to 25; BUILDING then
    /// starts a new figure.
    #[test]
    fn xyz_close_help_example() {
        let p = pts(&[
            ("25", "ZBUILDING"), ("26", "ZBUILDING"), ("32", "ZBUILDING"), ("33", ".BUILDING"),
            ("40", "ZBUILDING"), ("41", "BUILDING"),
        ]);
        assert_eq!(chains(&p, &xyz(&p)), ["25-26-32-33-*", "40-41"]);
    }

    /// Notes after a space are ignored; `*` and `.` join two lines at a point.
    #[test]
    fn xyz_notes_and_joins() {
        let p = pts(&[
            ("1", "ZCL 1+100"), ("2", "ZCURB1"), ("3", "ZCL 1+150"), ("4", "ZCURB1*ZSIDEW1"),
            ("5", "ZSIDEW1"), ("6", "CURB1.SIDEW1"), ("7", "CL"),
        ]);
        assert_eq!(chains(&p, &xyz(&p)), ["1-3-7", "2-4-6", "4-5-6"]);
    }

    /// XYZ joins in point-number order when asked, whatever the shot order.
    #[test]
    fn xyz_number_order() {
        let p = pts(&[("3", "ZSIB"), ("1", "ZSIB"), ("10", "SIB"), ("2", "ZSIB")]);
        assert_eq!(chains(&p, &xyz(&p)), ["1-2-3-10"]);
        let shot = build(&p, &Options { mode: Mode::Xyz, order: Order::Shot });
        assert_eq!(chains(&p, &shot), ["3-1-10"]);
    }

    #[test]
    fn xyz_orphans_warn() {
        let p = pts(&[("1", "YFENCE"), ("2", ".SHED"), ("3", "ZPOLE")]);
        let lw = xyz(&p);
        assert!(lw.figures.is_empty());
        assert_eq!(lw.warnings.len(), 3, "{:?}", lw.warnings);
        assert!(lw.warnings[2].contains("ZPOLE") || lw.warnings[2].contains("POLE"), "{:?}", lw.warnings);
    }

    /// The Lab 8 FieldGenius job's coding: string-numbered codes with notes,
    /// interleaved with point-only codes.
    #[test]
    fn strings_lab8_style() {
        let p = pts(&[
            ("1", "TH CN"), ("2", "ER1"), ("3", "G"), ("4", "ER1"), ("5", "GL1"), ("6", "ER2"),
            ("7", "BLDG1 STUCCO"), ("8", "ER1"), ("9", "OIP LOT 28"), ("10", "GL1"), ("11", "ER2"),
            ("12", "BLDG1 STUCCO"), ("13", "CHK 27"), ("14", "VLTC1"),
        ]);
        let lw = build(&p, &Options::default());
        assert_eq!(chains(&p, &lw), ["2-4-8", "5-10", "6-11", "7-12"]);
        let codes: Vec<&str> = lw.figures.iter().map(|f| f.code.as_str()).collect();
        assert_eq!(codes, ["ER", "GL", "ER", "BLDG"]);
        assert_eq!(lw.warnings, ["VLTC1: only one point (14), no line drawn"]);
    }

    #[test]
    fn strings_control_words() {
        let p = pts(&[
            ("1", "FE1"), ("2", "FE1"), ("3", "FE1 E"), ("4", "FE1"), ("5", "FE1"),
            ("6", "FE1 B"), ("7", "FE1"), ("8", "BG1"), ("9", "BG1"), ("10", "BG1 cls"),
            ("11", "CB1"), ("12", "CB1 PC"), ("13", "CB1"), ("14", "CB1 PT"), ("15", "CB1"),
        ]);
        let lw = build(&p, &Options::default());
        assert_eq!(chains(&p, &lw), ["1-2-3", "4-5", "6-7", "8-9-10-*", "11-12-13-14-15"]);
        let curved: Vec<bool> = lw.figures[4].vertices.iter().map(|v| v.curve).collect();
        assert_eq!(curved, [false, true, true, true, false]);
    }

    /// A figure shot back onto its first point closes without a duplicate.
    #[test]
    fn strings_auto_close() {
        let mut p = pts(&[("1", "BG1"), ("2", "BG1"), ("3", "BG1"), ("4", "BG1")]);
        p[3].northing = p[0].northing;
        p[3].easting = p[0].easting + 0.00001;
        let lw = build(&p, &Options::default());
        assert_eq!(chains(&p, &lw), ["1-2-3-*"]);
    }

    #[test]
    fn strings_join_and_unnumbered() {
        let p = pts(&[("1", "EP1"), ("2", "EP1*CU1"), ("3", "CU1"), ("4", "EP1"), ("5", "EP"), ("6", "EP")]);
        let lw = build(&p, &Options::default());
        assert_eq!(chains(&p, &lw), ["1-2-4", "2-3"]);
    }

    fn lib(rows: &str) -> Library {
        Library::parse(&format!("KEY,CONNECT,LINE LAYER
{rows}")).unwrap()
    }

    /// With a library, AutoMAP CONNECT joins every point with the code, string
    /// number or not; string numbers still split figures; codes the library
    /// lacks, or that do not connect, stay points and are reported.
    #[test]
    fn strings_with_library() {
        let l = lib("TC,1,CURB TOP
OIP,0,
FE*,1,FENCE
");
        let p = pts(&[
            ("1", "TC"), ("2", "OIP"), ("3", "TC east"), ("4", "G"), ("5", "TC1"), ("6", "TC"),
            ("7", "TC1"), ("8", "FENCE2"), ("9", "G"), ("10", "FENCE2"), ("11", "ER1"),
        ]);
        let lw = build_with(&p, &Options::default(), Some(&l));
        assert_eq!(chains(&p, &lw), ["1-3-6", "5-7", "8-10"]);
        let rows: Vec<Option<&str>> = lw.figures.iter().map(|f| f.row.map(|r| l.codes[r].key.as_str())).collect();
        assert_eq!(rows, [Some("TC"), Some("TC"), Some("FE*")]);
        assert_eq!(lw.warnings, ["not in the library, so not connected: G (2), ER (1)"]);
    }

    /// MoTI codes such as `ZL` (miscellaneous line) begin with an XYZ prefix
    /// letter; when the library knows the whole word it is a plain code.
    #[test]
    fn xyz_library_code_is_not_a_prefix() {
        let l = lib("ZL,0,
CURB,1,CURB
");
        let p = pts(&[("1", "ZCURB"), ("2", "ZL"), ("3", "ZCURB"), ("4", "ZL"), ("5", "CURB")]);
        let lw = build_with(&p, &Options { mode: Mode::Xyz, order: Order::Number }, Some(&l));
        assert_eq!(chains(&p, &lw), ["1-3-5"]);
        assert_eq!(lw.figures[0].row, Some(1));
        // Without the library, ZL reads as "line, code L".
        let bare = build(&p, &Options { mode: Mode::Xyz, order: Order::Number });
        assert_eq!(chains(&p, &bare), ["1-3-5", "2-4"]);
    }

    #[test]
    fn split_helpers() {
        assert_eq!(split_string_number("ER12"), ("ER", true));
        assert_eq!(split_string_number("RT1M"), ("RT1M", false));
        assert_eq!(split_string_number("27"), ("", false));
        assert_eq!(join_parts(".BUILDING"), [".BUILDING"]);
        assert_eq!(join_parts("A*B.C"), ["A", "B", "C"]);
        assert_eq!(join_parts(".A*B"), [".A", "B"]);
    }
}
