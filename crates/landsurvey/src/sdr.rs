//! Sokkia SDR raw data (SDR2x and SDR33 data collectors, `.sdr` / `.raw`),
//! parsed and reduced into the same [`Reduction`](crate::rw5::Reduction) as
//! RW5. `std` only.
//!
//! SDR is fixed-width: a two-digit record type and a two-letter derivation
//! code (`02TP`, `08KI`, `09F1`), then point ids and numbers in columns.
//! SDR2x writes 4-character ids and 10-character numbers; SDR33 can write
//! 16 and 16. The widths are detected per file from the coordinate records.
//!
//! | record | meaning |
//! |---|---|
//! | `00` | header: format/version, then unit flags (first digit: angle unit, 1 = degrees, 2 = gons) |
//! | `10` | job name |
//! | `02` | station: id, N, E, Z, instrument height, description |
//! | `03` | target (rod) height |
//! | `06` | scale factor |
//! | `07` | backsight: station, backsight, azimuth, horizontal circle reading |
//! | `08` | coordinates: id, N, E, Z, description (`KI` keyed, `TP` computed by the collector, …) |
//! | `09` | observation: from, to, slope distance (blank for angle-only), zenith angle, horizontal angle, description |
//! | `13` | notes; `13CP` records which corrections the collector applied |
//!
//! Coordinate records are taken as written: they are the collector's own
//! reductions. Observations are reduced like RW5 (plane survey, zenith
//! angles); curvature and refraction are applied when the file's notes say
//! the collector applied them, while sea-level and atmospheric corrections
//! are not (they depend on the instrument and are reported instead).

use std::collections::HashMap;

use crate::rw5::{add_point, angle_diff_seconds, azimuth_between, Point, Reduction, Setup, Unit};

/// True if `text` looks like an SDR file (an `00NMSDR` header).
pub fn looks_like_sdr(text: &str) -> bool {
    text.lines()
        .take(5)
        .any(|l| l.trim_start_matches(|c: char| c.is_control()).starts_with("00NMSDR"))
}

/// An SDR point id: numeric ids lose their leading zeros (`0008` → `8`), as
/// MicroSurvey and the collector display them.
fn point_id(field: &str) -> String {
    let t = field.trim();
    if !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()) {
        let s = t.trim_start_matches('0');
        if s.is_empty() { "0".to_string() } else { s.to_string() }
    } else {
        t.to_string()
    }
}

/// Fixed-width columns of one SDR line, after the 4-character record code.
struct Cols<'a> {
    body: &'a str,
    id: usize,
    num: usize,
}

impl<'a> Cols<'a> {
    fn slice(&self, start: usize, len: usize) -> &'a str {
        let b = self.body.as_bytes();
        let s = start.min(b.len());
        let e = (start + len).min(b.len());
        // Fields are ASCII; fall back to empty on a broken boundary.
        self.body.get(s..e).unwrap_or("").trim()
    }
    /// The `i`-th id field.
    fn id(&self, i: usize) -> String {
        point_id(self.slice(i * self.id, self.id))
    }
    /// The `i`-th number after `ids` id fields; `None` when blank or invalid.
    fn num(&self, ids: usize, i: usize) -> Option<f64> {
        self.slice(ids * self.id + i * self.num, self.num).parse().ok()
    }
    /// Text after `ids` ids and `nums` numbers.
    fn rest(&self, ids: usize, nums: usize) -> String {
        let start = ids * self.id + nums * self.num;
        self.body.get(start.min(self.body.len())..).unwrap_or("").trim().to_string()
    }
}

/// The (id width, number width) that parses every `08` coordinate record.
fn detect_widths(lines: &[&str]) -> Option<(usize, usize)> {
    let coords: Vec<&str> = lines.iter().copied().filter(|l| l.starts_with("08")).collect();
    [(4, 10), (16, 16), (4, 16), (16, 10)].into_iter().find(|&(id, num)| {
        !coords.is_empty()
            && coords.iter().all(|l| {
                let c = Cols { body: l.get(4..).unwrap_or(""), id, num };
                (0..3).all(|i| c.num(1, i).is_some())
            })
    })
}

/// Reduce an SDR file.
pub fn reduce(text: &str) -> Reduction {
    let mut out = Reduction {
        job: String::new(),
        unit: Unit::Metre,
        points: Vec::new(),
        setups: Vec::new(),
        repeats: Vec::new(),
        warnings: Vec::new(),
    };
    // Strip control characters (SDR files end in ^C…^Z) and line breaks.
    let lines: Vec<&str> = text
        .lines()
        .map(|l| l.trim_matches(|c: char| c.is_control()))
        .collect();
    let Some((id_w, num_w)) = detect_widths(&lines) else {
        out.warnings.push("no SDR coordinate (08) records with a recognised field layout".into());
        return out;
    };

    let mut index: HashMap<String, usize> = HashMap::new();
    let mut gons = false;
    let mut curvature = false;
    let mut scale = 1.0_f64;
    let (mut hi, mut hr) = (0.0_f64, 0.0_f64);
    let mut station: Option<String> = None;
    let mut backsight: Option<String> = None;
    let mut orientation: Option<f64> = None;
    let mut collector_notes: Vec<String> = Vec::new();
    let mut angle_only = 0usize;
    let mut unknown: HashMap<String, usize> = HashMap::new();

    for (lineno, line) in lines.iter().enumerate() {
        let at = lineno + 1;
        if line.len() < 4 || !line.as_bytes()[..2].iter().all(u8::is_ascii_digit) {
            continue;
        }
        let c = Cols { body: &line[4..], id: id_w, num: num_w };
        let angle = |v: f64| if gons { v * 0.9 } else { v };
        match &line[..2] {
            "00" => {
                // The unit flags are the trailing digits; the first is the angle unit.
                let flags: String = line.chars().rev().take_while(char::is_ascii_digit).collect();
                let flags: String = flags.chars().rev().collect();
                match flags.chars().next() {
                    Some('1') | None => {}
                    Some('2') => gons = true,
                    Some(other) => out.warnings.push(format!(
                        "line {at}: angle unit flag {other} not recognised; angles read as degrees"
                    )),
                }
            }
            "10" => {
                if out.job.is_empty() {
                    out.job = c.slice(0, 16).to_string();
                }
            }
            "13" => {
                let note = c.slice(0, line.len()).to_string();
                if &line[2..4] == "CP" && note.ends_with(": Y") {
                    if note.starts_with("C & R") {
                        curvature = true;
                    } else {
                        let what = note.trim_end_matches(": Y").to_ascii_lowercase();
                        collector_notes.push(match what.as_str() {
                            "sea level cn" => "the sea-level correction".to_string(),
                            "atmos crn" => "the atmospheric correction".to_string(),
                            _ => what,
                        });
                    }
                }
            }
            "06" => scale = c.num(0, 0).filter(|v| *v > 0.0).unwrap_or(1.0),
            "03" => hr = c.num(0, 0).unwrap_or(hr),
            "02" => {
                let id = c.id(0);
                match (c.num(1, 0), c.num(1, 1)) {
                    (Some(n), Some(e)) => add_point(
                        &mut out,
                        &mut index,
                        Point {
                            name: id.clone(),
                            northing: n,
                            easting: e,
                            elevation: c.num(1, 2).unwrap_or(0.0),
                            description: c.rest(1, 4),
                            from: None,
                        },
                    ),
                    _ => out.warnings.push(format!("line {at}: station {id} without coordinates")),
                }
                hi = c.num(1, 3).unwrap_or(0.0);
                start_setup(&mut out, &id, hi);
                station = Some(id);
                backsight = None;
                orientation = None;
            }
            "07" => {
                let (st, bs) = (c.id(0), c.id(1));
                let (Some(az), Some(ha)) = (c.num(2, 0), c.num(2, 1)) else {
                    out.warnings.push(format!("line {at}: backsight {st}->{bs} without azimuth and reading"));
                    continue;
                };
                let (az, ha) = (angle(az), angle(ha));
                // A backsight from a new station starts a setup (no 02 record).
                if station.as_deref() != Some(st.as_str()) {
                    start_setup(&mut out, &st, hi);
                    station = Some(st.clone());
                }
                let o = (az - ha).rem_euclid(360.0);
                orientation = Some(o);
                let az_check = azimuth_between(&out, &index, &st, &bs).map(|calc| angle_diff_seconds(az, calc));
                if let Some(s) = out.setups.last_mut() {
                    s.backsight = Some(bs.clone());
                    s.orientation = o;
                    s.bs_azimuth_error = az_check;
                }
                backsight = Some(bs);
            }
            "08" => {
                let id = c.id(0);
                match (c.num(1, 0), c.num(1, 1), c.num(1, 2)) {
                    (Some(n), Some(e), z) => add_point(
                        &mut out,
                        &mut index,
                        Point {
                            name: id,
                            northing: n,
                            easting: e,
                            elevation: z.unwrap_or(0.0),
                            description: c.rest(1, 3),
                            from: None,
                        },
                    ),
                    _ => out.warnings.push(format!("line {at}: coordinate record {id} without N/E")),
                }
            }
            "09" => {
                let (from, to) = (c.id(0), c.id(1));
                let Some(sd) = c.num(2, 0) else {
                    angle_only += 1;
                    continue;
                };
                let (Some(za), Some(ha)) = (c.num(2, 1), c.num(2, 2)) else {
                    out.warnings.push(format!("line {at}: observation {from}->{to} without angles"));
                    continue;
                };
                let (Some(o), Some(&si)) = (orientation, index.get(&from)) else {
                    out.warnings.push(format!("line {at}: observation {from}->{to} before an oriented setup"));
                    continue;
                };
                let (mut za, mut ha) = (angle(za), angle(ha));
                if za > 180.0 {
                    za = 360.0 - za;
                    ha += 180.0;
                }
                let z = za.to_radians();
                let hd = sd * z.sin();
                let mut dz = hi + sd * z.cos() - hr;
                if curvature {
                    dz += (1.0 - 0.14) * hd * hd / (2.0 * 6_371_000.0);
                }
                let hd = hd * scale;
                let az = (o + ha).to_radians();
                let s = &out.points[si];
                let p = Point {
                    name: to.clone(),
                    northing: s.northing + hd * az.cos(),
                    easting: s.easting + hd * az.sin(),
                    elevation: s.elevation + dz,
                    description: c.rest(2, 3),
                    from: Some(from),
                };
                // A measured backsight is the setup's distance/elevation check.
                if backsight.as_deref() == Some(to.as_str()) {
                    if let Some(&bi) = index.get(&to) {
                        let b = &out.points[bi];
                        let (dh, dzz) = (
                            ((p.northing - out.points[si].northing).hypot(p.easting - out.points[si].easting))
                                - (b.northing - out.points[si].northing).hypot(b.easting - out.points[si].easting),
                            p.elevation - b.elevation,
                        );
                        if let Some(setup) = out.setups.last_mut() {
                            setup.bs_distance_error = Some(dh);
                            setup.bs_elevation_error = Some(dzz);
                        }
                        continue;
                    }
                }
                add_point(&mut out, &mut index, p);
            }
            "01" | "05" => {} // instrument, atmosphere: informational
            other => *unknown.entry(other.to_string()).or_default() += 1,
        }
    }
    if angle_only > 0 {
        out.warnings.push(format!("{angle_only} angle-only observation(s) (no distance) not reduced"));
    }
    if !collector_notes.is_empty() {
        out.warnings.push(format!(
            "the collector applied {} to its coordinates; observations reduced here do not",
            collector_notes.join(" and ")
        ));
    }
    let mut unknown: Vec<_> = unknown.into_iter().collect();
    unknown.sort();
    for (kind, n) in unknown {
        out.warnings.push(format!("{n} record(s) of type {kind} not supported, skipped"));
    }
    out
}

fn start_setup(out: &mut Reduction, station: &str, hi: f64) {
    out.setups.push(Setup {
        station: station.to_string(),
        backsight: None,
        orientation: 0.0,
        hi,
        stored: None,
        stored_delta: None,
        bs_distance_error: None,
        bs_elevation_error: None,
        bs_azimuth_error: None,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic SDR2x job (4-character ids, 10-character numbers): a keyed
    /// monument, a station with a backsight to it, a measured backsight, a
    /// computed topo point, a new shot, an angle-only check, and a backsight
    /// whose recorded azimuth disagrees with the coordinates.
    fn job() -> String {
        let id = |s: &str| format!("{s:<4}");
        let n = |v: &str| format!("{v:<10}");
        let mut t = String::from("\r\n00NMSDR22  V03-03.7 000003-Jun-91 13:00 121111\r\n");
        t += "10NMDEMOJOB         \r\n13CPC & R crn: Y\r\n13CPAtmos crn: Y\r\n";
        t += &format!("08KI{}{}{}{}MON\r\n", id("0008"), n("1000.000"), n("2000.000"), n("100.000"));
        t += &format!("02TP{}{}{}{}{}BOLT\r\n", id("0010"), n("900.000"), n("2000.000"), n("95.000"), n("1.500"));
        t += &format!("03NM{}\r\n", n("1.500"));
        t += &format!("07TP{}{}{}{}\r\n", id("0010"), id("0008"), n("0.00000"), n("0.00000"));
        t += &format!("09F1{}{}{}{}{}MON\r\n", id("0010"), id("0008"), n("100.000"), n("90.00000"), n("0.00000"));
        t += &format!("08TP{}{}{}{}TP\r\n", id("0011"), n("950.000"), n("2050.000"), n("96.000"));
        t += &format!("09F1{}{}{}{}{}NEW\r\n", id("0010"), id("0012"), n("50.000"), n("90.00000"), n("90.00000"));
        t += &format!("09F1{}{}{}{}{}MON\r\n", id("0010"), id("0008"), n(""), n("90.00000"), n("0.00000"));
        t += &format!("07TP{}{}{}{}\r\n", id("0010"), id("0011"), n("10.00000"), n("0.00000"));
        t += "\u{3}38\u{1a}";
        t
    }

    #[test]
    fn detects_sdr2x_and_reduces() {
        let text = job();
        assert!(looks_like_sdr(&text));
        let r = reduce(&text);
        assert_eq!(r.job, "DEMOJOB");
        let p = |n: &str| r.points.iter().find(|p| p.name == n).unwrap_or_else(|| panic!("{n}"));
        // Leading zeros dropped; keyed and computed coordinates taken as written.
        assert_eq!((p("8").northing, p("8").easting, p("8").elevation), (1000.0, 2000.0, 100.0));
        assert_eq!(p("8").description, "MON");
        assert_eq!(p("11").description, "TP");
        // A new observed point: due east (orientation 0 + 90°) at 50 m, level.
        let n = p("12");
        assert!((n.northing - 900.0).abs() < 1e-9 && (n.easting - 2050.0).abs() < 1e-9);
        // Elevation: 95 + HI 1.5 − HR 1.5 + curvature/refraction on 50 m.
        assert!((n.elevation - (95.0 + 0.86 * 2500.0 / 12_742_000.0)).abs() < 1e-9);
        assert_eq!(n.from.as_deref(), Some("10"));

        assert_eq!(r.setups.len(), 1);
        let s = &r.setups[0];
        assert_eq!(s.backsight.as_deref(), Some("11"));
        // The measured backsight to 8 checks distance (exact) …
        assert!(s.bs_distance_error.unwrap().abs() < 1e-9);
        // … and the second backsight's recorded 10° azimuth vs 45° computed.
        assert!((s.bs_azimuth_error.unwrap() - (10.0 - 45.0) * 3600.0).abs() < 1e-6);

        assert!(r.warnings.iter().any(|w| w.contains("1 angle-only")), "{:?}", r.warnings);
        assert!(r.warnings.iter().any(|w| w.contains("atmospheric correction")), "{:?}", r.warnings);
    }

    #[test]
    fn sdr33_wide_fields() {
        let line = |code: &str, id: &str, n: &str, e: &str, z: &str, d: &str| {
            format!("{code}{id:>16}{n:>16}{e:>16}{z:>16}{d}\n")
        };
        let text = format!(
            "00NMSDR33 V04-04.02    01-Jan-00 00:00 113111\n{}{}",
            line("08KI", "STN-A", "5000.000000000", "1000.000000000", "100.0000000000", "CTRL"),
            line("08TP", "0042", "5010.000000000", "1005.000000000", "101.0000000000", "FENCE")
        );
        let r = reduce(&text);
        assert_eq!(r.points.len(), 2, "{:?}", r.warnings);
        assert_eq!(r.points[0].name, "STN-A");
        assert_eq!(r.points[1].name, "42");
        assert_eq!(r.points[1].description, "FENCE");
        assert_eq!(r.points[1].elevation, 101.0);
    }

    #[test]
    fn rejects_unrecognised_layout() {
        let r = reduce("00NMSDR22\n08KIxx garbage\n");
        assert!(r.points.is_empty());
        assert!(!r.warnings.is_empty());
    }
}
