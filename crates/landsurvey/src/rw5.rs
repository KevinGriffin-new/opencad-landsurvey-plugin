//! TDS / Carlson RW5 raw data — the format FieldGenius, SurvCE and TDS data
//! collectors write (`.rw5`, FieldGenius `.raw`) — parsed and reduced to
//! coordinates. `std` only.
//!
//! An RW5 file is one record per line: a two-letter record type, then
//! comma-separated fields keyed by a two-letter prefix (`OP27`, `AR90.15300`),
//! with a free-text description after `--`. Lines starting with `--` are
//! comments. The records this module understands:
//!
//! | record | meaning |
//! |---|---|
//! | `JB` | job name / date |
//! | `MO` | mode: units (`UN`), scale factor (`SF`), EDM offset (`EO`), curvature & refraction (`EC`), angle unit (`AU`), azimuth direction (`AD`) |
//! | `SP`, `NEZ` (FieldGenius) | a stored point with given coordinates |
//! | `OC` | occupy: instrument station `OP`, optionally with the coordinates the collector used |
//! | `LS` | line of sight: instrument height `HI`, rod height `HR` |
//! | `BK` | backsight: station `OP`, backsight point `BP`, back azimuth `BS`, circle `BC` |
//! | `BR` | backsight reading (`AR`, `ZE`, `SD`), used as an orientation check |
//! | `SS`, `FS`, `TR` | sideshot / foresight / traverse to `FP` (`AR` or `AZ`, `ZE`, `SD` or `HD`) |
//! | `OF` | offset details; the observation that follows already carries them |
//!
//! Angles are packed `DDD.MMSSs` degrees. Reduction is plane surveying:
//! horizontal = SD·sin Z, vertical = SD·cos Z, azimuth = BS + (AR − BC),
//! elevation = station Z + HI + vertical − HR. Face-2 shots (Z > 180°) are
//! folded to face 1. Unknown records are counted, not fatal.

use std::collections::HashMap;

/// Linear unit declared by the `MO` record (`UN`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Feet,
    Metre,
    UsSurveyFoot,
}

/// A reduced or given point.
#[derive(Debug, Clone, PartialEq)]
pub struct Point {
    pub name: String,
    pub northing: f64,
    pub easting: f64,
    pub elevation: f64,
    pub description: String,
    /// Station it was shot from; `None` for a given (stored) point.
    pub from: Option<String>,
}

/// One instrument setup, with the collector's own numbers kept for QC.
#[derive(Debug, Clone, PartialEq)]
pub struct Setup {
    pub station: String,
    pub backsight: Option<String>,
    /// Azimuth of the horizontal circle's zero, degrees.
    pub orientation: f64,
    pub hi: f64,
    /// Station coordinates the collector wrote on the `OC` record, if any.
    pub stored: Option<[f64; 3]>,
    /// Stored minus our coordinates for the station, when both exist.
    pub stored_delta: Option<[f64; 3]>,
    /// Backsight check: measured minus computed horizontal distance, and
    /// measured minus known backsight elevation.
    pub bs_distance_error: Option<f64>,
    pub bs_elevation_error: Option<f64>,
    /// Recorded back azimuth minus the azimuth computed from the station and
    /// backsight coordinates, in arc-seconds, when both are available.
    pub bs_azimuth_error: Option<f64>,
}

/// A point name observed more than once; the first value is kept.
#[derive(Debug, Clone, PartialEq)]
pub struct Repeat {
    pub name: String,
    /// Horizontal and vertical difference of the repeat from the kept value.
    pub dh: f64,
    pub dz: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Reduction {
    pub job: String,
    pub unit: Unit,
    pub points: Vec<Point>,
    pub setups: Vec<Setup>,
    pub repeats: Vec<Repeat>,
    pub warnings: Vec<String>,
}

/// A QC difference to 0.1 mm with its sign, printing a value that rounds to
/// zero as `+0.0000` rather than `-0.0000`.
pub fn fmt_delta(v: f64) -> String {
    let v = if v.abs() < 0.00005 { 0.0 } else { v };
    format!("{v:+.4}")
}

/// Packed `DDD.MMSSs` degrees to decimal degrees.
pub fn packed_dms(text: &str) -> Option<f64> {
    let t = text.trim();
    let (neg, t) = match t.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, t),
    };
    let (whole, frac) = t.split_once('.').unwrap_or((t, ""));
    let d: f64 = whole.parse().ok()?;
    let digits: String = frac.chars().chain("0000".chars()).collect();
    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let m: f64 = digits[0..2].parse().ok()?;
    let s: f64 = format!("{}.{}", &digits[2..4], &digits[4..]).parse().ok()?;
    if m >= 60.0 || s >= 60.0 {
        return None;
    }
    let v = d + m / 60.0 + s / 3600.0;
    Some(if neg { -v } else { v })
}

/// One parsed line: record type, keyed fields, description.
struct Record<'a> {
    kind: &'a str,
    fields: Vec<&'a str>,
    desc: String,
}

impl Record<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.fields.iter().find_map(|f| {
            let rest = f.strip_prefix(key)?;
            // `N 200.0` / `E 200.0` carry a space; single-letter keys must not
            // match the start of a longer key (`E` vs `EL`, `N` vs `NM`).
            if key.len() == 1 && !rest.starts_with(' ') {
                return None;
            }
            Some(rest.trim())
        })
    }

    fn num(&self, key: &str) -> Option<f64> {
        self.get(key)?.parse().ok()
    }

    fn angle(&self, key: &str) -> Option<f64> {
        packed_dms(self.get(key)?)
    }
}

fn parse_line(line: &str) -> Option<Record<'_>> {
    let line = line.trim_end_matches(['\r', '\n']);
    if line.trim().is_empty() || line.starts_with("--") {
        return None;
    }
    // The description is everything after the first ",--" (it may hold commas).
    let (body, desc) = match line.find(",--") {
        Some(i) => (&line[..i], line[i + 3..].trim().to_string()),
        None => (line, String::new()),
    };
    let mut parts = body.split(',');
    let kind = parts.next()?.trim();
    Some(Record {
        kind,
        fields: parts.map(str::trim).collect(),
        desc,
    })
}

/// Reduce a data collector raw file, picking the format from its content:
/// Sokkia SDR (an `00NMSDR` header) or RW5. Returns the format name too.
pub fn reduce_any(text: &str) -> (&'static str, Reduction) {
    if crate::sdr::looks_like_sdr(text) {
        ("Sokkia SDR", crate::sdr::reduce(text))
    } else {
        ("RW5", reduce(text))
    }
}

/// Reduce an RW5 file to coordinates.
pub fn reduce(text: &str) -> Reduction {
    let mut out = Reduction {
        job: String::new(),
        unit: Unit::Metre,
        points: Vec::new(),
        setups: Vec::new(),
        repeats: Vec::new(),
        warnings: Vec::new(),
    };
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut unknown: HashMap<String, usize> = HashMap::new();

    let (mut hi, mut hr) = (0.0_f64, 0.0_f64);
    let mut scale = 1.0_f64;
    let mut edm_offset = 0.0_f64;
    let mut curvature = false;
    let mut station: Option<String> = None;
    let mut orientation: Option<f64> = None;

    for (lineno, line) in text.lines().enumerate() {
        let Some(r) = parse_line(line) else { continue };
        let at = lineno + 1;
        match r.kind {
            "JB" => {
                if out.job.is_empty() {
                    out.job = r.get("NM").unwrap_or("").to_string();
                }
            }
            "MO" => {
                out.unit = match r.get("UN") {
                    Some("0") => Unit::Feet,
                    Some("2") => Unit::UsSurveyFoot,
                    _ => Unit::Metre,
                };
                scale = r.num("SF").filter(|v| *v > 0.0).unwrap_or(1.0);
                edm_offset = r.num("EO").unwrap_or(0.0);
                curvature = r.get("EC") == Some("1");
                if r.get("AU").is_some_and(|v| v != "0") {
                    out.warnings.push(format!("line {at}: angle unit AU{} is not degrees; angles read as DMS anyway", r.get("AU").unwrap_or("")));
                }
                if r.get("AD").is_some_and(|v| v != "0") {
                    out.warnings.push(format!("line {at}: south-azimuth mode (AD{}) is not supported", r.get("AD").unwrap_or("")));
                }
            }
            "LS" => {
                if let Some(v) = r.num("HI") {
                    hi = v;
                }
                if let Some(v) = r.num("HR") {
                    hr = v;
                }
            }
            "SP" | "NEZ" => {
                // SP,PN<name>,N <n>,E <e>,EL<z>  /  FieldGenius NEZ,OP<name>,n,e,z
                let given = if r.kind == "SP" {
                    r.get("PN").map(|n| (n, r.num("N"), r.num("E"), r.num("EL")))
                } else {
                    r.get("OP").map(|n| {
                        let v = |i: usize| r.fields.get(i).and_then(|s| s.parse().ok());
                        (n, v(1), v(2), v(3))
                    })
                };
                match given {
                    Some((name, Some(n), Some(e), z)) => add_point(
                        &mut out,
                        &mut index,
                        Point {
                            name: name.to_string(),
                            northing: n,
                            easting: e,
                            elevation: z.unwrap_or(0.0),
                            description: r.desc.clone(),
                            from: None,
                        },
                    ),
                    _ => out.warnings.push(format!("line {at}: {} without a point name and N/E", r.kind)),
                }
            }
            "OC" => {
                let Some(name) = r.get("OP") else {
                    out.warnings.push(format!("line {at}: OC without OP"));
                    continue;
                };
                let stored = match (r.num("N"), r.num("E")) {
                    (Some(n), Some(e)) => Some([n, e, r.num("EL").unwrap_or(0.0)]),
                    _ => None,
                };
                if !index.contains_key(name) {
                    match stored {
                        Some([n, e, z]) => add_point(
                            &mut out,
                            &mut index,
                            Point {
                                name: name.to_string(),
                                northing: n,
                                easting: e,
                                elevation: z,
                                description: r.desc.clone(),
                                from: None,
                            },
                        ),
                        None => out.warnings.push(format!("line {at}: occupied point {name} has no coordinates")),
                    }
                }
                let stored_delta = match (stored, index.get(name)) {
                    (Some(s), Some(&i)) => {
                        let p = &out.points[i];
                        Some([s[0] - p.northing, s[1] - p.easting, s[2] - p.elevation])
                    }
                    _ => None,
                };
                station = Some(name.to_string());
                orientation = None;
                out.setups.push(Setup {
                    station: name.to_string(),
                    backsight: None,
                    orientation: 0.0,
                    hi,
                    stored,
                    stored_delta,
                    bs_distance_error: None,
                    bs_elevation_error: None,
                    bs_azimuth_error: None,
                });
            }
            "BK" => {
                let bp = r.get("BP").map(str::to_string);
                let bc = r.angle("BC").unwrap_or(0.0);
                let az_check = match (r.angle("BS"), station.as_ref(), bp.as_ref()) {
                    (Some(bs), Some(s), Some(b)) => azimuth_between(&out, &index, s, b)
                        .map(|calc| angle_diff_seconds(bs, calc)),
                    _ => None,
                };
                let bs = r.angle("BS").or_else(|| {
                    // No back azimuth written: compute it from coordinates.
                    let (s, b) = (station.as_ref()?, bp.as_ref()?);
                    let (p, q) = (&out.points[*index.get(s)?], &out.points[*index.get(b)?]);
                    Some((q.easting - p.easting).atan2(q.northing - p.northing).to_degrees().rem_euclid(360.0))
                });
                match bs {
                    Some(bs) => {
                        let o = (bs - bc).rem_euclid(360.0);
                        orientation = Some(o);
                        if let Some(s) = out.setups.last_mut() {
                            s.backsight = bp;
                            s.orientation = o;
                            s.hi = hi;
                            s.bs_azimuth_error = az_check;
                        }
                    }
                    None => out.warnings.push(format!("line {at}: backsight without an azimuth or known points")),
                }
            }
            "BR" => {
                // Orientation check against the backsight's known position.
                let (Some(st), Some(bp)) = (station.as_ref(), r.get("BP")) else { continue };
                let (Some(&si), Some(&bi)) = (index.get(st), index.get(bp)) else { continue };
                let Some(obs) = observe(&r, hi, hr, scale, edm_offset, curvature, out.unit) else { continue };
                let (s, b) = (&out.points[si], &out.points[bi]);
                let hd = (b.northing - s.northing).hypot(b.easting - s.easting);
                if let Some(setup) = out.setups.last_mut() {
                    setup.bs_distance_error = Some(obs.hd - hd);
                    setup.bs_elevation_error = Some(s.elevation + obs.dz - b.elevation);
                }
            }
            "SS" | "FS" | "TR" => {
                let Some(fp) = r.get("FP") else {
                    out.warnings.push(format!("line {at}: {} without FP", r.kind));
                    continue;
                };
                let (Some(st), Some(o)) = (station.as_ref(), orientation) else {
                    out.warnings.push(format!("line {at}: shot to {fp} before an oriented setup"));
                    continue;
                };
                let Some(&si) = index.get(st) else { continue };
                let Some(obs) = observe(&r, hi, hr, scale, edm_offset, curvature, out.unit) else {
                    out.warnings.push(format!("line {at}: shot to {fp} needs AR or AZ, ZE and SD or HD"));
                    continue;
                };
                let az = match obs.azimuth {
                    Some(az) => az,
                    None => o + obs.angle_right,
                }
                .to_radians();
                let s = &out.points[si];
                let p = Point {
                    name: fp.to_string(),
                    northing: s.northing + obs.hd * az.cos(),
                    easting: s.easting + obs.hd * az.sin(),
                    elevation: s.elevation + obs.dz,
                    description: r.desc.clone(),
                    from: Some(st.clone()),
                };
                add_point(&mut out, &mut index, p);
            }
            "OF" => {} // offsets are already applied to the observation that follows
            other => *unknown.entry(other.to_string()).or_default() += 1,
        }
    }
    let mut unknown: Vec<_> = unknown.into_iter().collect();
    unknown.sort();
    for (kind, n) in unknown {
        out.warnings.push(format!("{n} {kind} record(s) not supported, skipped"));
    }
    out
}

/// A reduced observation: horizontal distance, elevation change from the
/// station mark to the target point, and the horizontal direction.
struct Observation {
    hd: f64,
    dz: f64,
    angle_right: f64,
    azimuth: Option<f64>,
}

fn observe(
    r: &Record,
    hi: f64,
    hr: f64,
    scale: f64,
    edm_offset: f64,
    curvature: bool,
    unit: Unit,
) -> Option<Observation> {
    let mut angle_right = r.angle("AR");
    let mut azimuth = r.angle("AZ");
    if angle_right.is_none() && azimuth.is_none() {
        return None;
    }
    let mut zenith = r.angle("ZE").unwrap_or(90.0);
    if zenith > 180.0 {
        // Face 2: fold to face 1.
        zenith = 360.0 - zenith;
        angle_right = angle_right.map(|a| a + 180.0);
        azimuth = azimuth.map(|a| a + 180.0);
    }
    let z = zenith.to_radians();
    let (hd, vd) = match (r.num("SD"), r.num("HD")) {
        (Some(sd), _) => {
            let sd = sd + edm_offset;
            (sd * z.sin(), sd * z.cos())
        }
        (None, Some(hd)) => (hd, r.num("VD").unwrap_or(0.0)),
        _ => return None,
    };
    let mut dz = hi + vd - hr;
    if curvature {
        // (1 − k)·HD² / 2R, k = 0.14, in the job's linear unit.
        let radius = match unit {
            Unit::Metre => 6_371_000.0,
            Unit::Feet => 6_371_000.0 / 0.3048,
            Unit::UsSurveyFoot => 6_371_000.0 * 3937.0 / 1200.0,
        };
        dz += (1.0 - 0.14) * hd * hd / (2.0 * radius);
    }
    Some(Observation {
        hd: hd * scale,
        dz,
        angle_right: angle_right.unwrap_or(0.0),
        azimuth,
    })
}

/// Grid azimuth (degrees) from point `a` to point `b`, when both are known.
pub(crate) fn azimuth_between(
    out: &Reduction,
    index: &HashMap<String, usize>,
    a: &str,
    b: &str,
) -> Option<f64> {
    let (p, q) = (&out.points[*index.get(a)?], &out.points[*index.get(b)?]);
    if p.northing == q.northing && p.easting == q.easting {
        return None;
    }
    Some((q.easting - p.easting).atan2(q.northing - p.northing).to_degrees().rem_euclid(360.0))
}

/// `a − b` in arc-seconds, wrapped to ±180°.
pub(crate) fn angle_diff_seconds(a: f64, b: f64) -> f64 {
    ((a - b + 180.0).rem_euclid(360.0) - 180.0) * 3600.0
}

/// Add a point, keeping the first value of a repeated name and recording the
/// repeat's difference for QC.
pub(crate) fn add_point(out: &mut Reduction, index: &mut HashMap<String, usize>, p: Point) {
    match index.get(&p.name) {
        Some(&i) => {
            let q = &out.points[i];
            let dh = (p.northing - q.northing).hypot(p.easting - q.easting);
            let dz = p.elevation - q.elevation;
            // An identical repeat (a station re-occupied with the same
            // coordinates) is not a check; only report real differences.
            if dh > 1e-9 || dz.abs() > 1e-9 {
                out.repeats.push(Repeat { name: p.name.clone(), dh, dz });
            }
        }
        None => {
            index.insert(p.name.clone(), out.points.len());
            out.points.push(p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dms(d: f64, m: f64, s: f64) -> f64 {
        d + m / 60.0 + s / 3600.0
    }

    #[test]
    fn packed_angles() {
        assert_eq!(packed_dms("202.41320"), Some(dms(202.0, 41.0, 32.0)));
        assert_eq!(packed_dms("93.46252"), Some(dms(93.0, 46.0, 25.2)));
        assert_eq!(packed_dms("180"), Some(180.0));
        assert_eq!(packed_dms("0.00000"), Some(0.0));
        assert_eq!(packed_dms("12.6000"), None); // 60 minutes
        assert_eq!(packed_dms("x"), None);
    }

    /// A setup at 27 (200, 200, 50) backsighting south, then occupying the
    /// shot point 33 and checking back to 27. The coordinates FieldGenius
    /// wrote for 33 (N 241.7529, E 217.4590, EL 51.3981) are reproduced by
    /// hand: az = 180° + 202°41'32" = 22°41'32", HD = 45.2564·sin 90°11'05",
    /// EL = 50 + 1.651 + 45.2564·cos 90°11'05" − 0.107.
    const JOB: &str = "\
--FieldGenius v4.0.4
JB,NMDEMO,DT10-23-2010,TM23:14:07
MO,AD0,UN1,SF1.000000,EC0,EO0.0,AU0
LS,HI1.651,HR0.107
NEZ,OP27,200.000,200.000,50.000
OC,OP27,N 200.0000,E 200.0000,EL50.0000,--TH CN
BK,OP27,BP99,BS180.00000,BC0.00000
SS,OP27,FP33,AR202.41320,ZE90.11050,SD45.2564,--TH CN
SS,OP27,FP34,AR22.41320,ZE269.48550,SD45.2564,--FACE 2 OF 33
OF,AR205.18030,ZE93.54350,SD4.9894
OF,HD0.1800,--Horizontal Distance Offset
SS,OP27,FP35,AR90.00000,ZE90.00000,HD10.0000,--PIN, FOUND
GPS,PN9,LA49.0,LN-123.0,EL0
LS,HI1.655,HR0.107
OC,OP33,N 241.7529,E 217.4590,EL51.3981,--TH CN
BK,OP33,BP27,BS202.41320,BC0.00000
BR,OP33,BP27,AR0.00000,ZE93.43130,SD45.3524
SS,OP33,FP27,AR0.00000,ZE93.43130,SD45.3524,--CHK 27
";

    #[test]
    fn reduces_and_reports_qc() {
        let r = reduce(JOB);
        assert_eq!(r.job, "DEMO");
        assert_eq!(r.unit, Unit::Metre);
        let p = |n: &str| r.points.iter().find(|p| p.name == n).unwrap();

        let p33 = p("33");
        assert!((p33.northing - 241.7529).abs() < 5e-5, "{}", p33.northing);
        assert!((p33.easting - 217.4590).abs() < 5e-5, "{}", p33.easting);
        assert!((p33.elevation - 51.3981).abs() < 5e-5, "{}", p33.elevation);
        assert_eq!(p33.from.as_deref(), Some("27"));

        // Face 2 of the same shot lands on the same point.
        let p34 = p("34");
        assert!((p34.northing - p33.northing).abs() < 1e-9);
        assert!((p34.easting - p33.easting).abs() < 1e-9);
        assert!((p34.elevation - p33.elevation).abs() < 1e-9);

        // HD shot, description with a comma kept whole.
        let p35 = p("35");
        assert!((p35.northing - 200.0).abs() < 1e-9 && (p35.easting - 190.0).abs() < 1e-9);
        assert_eq!(p35.description, "PIN, FOUND");

        // Setup 33: FieldGenius' stored coordinates agree with ours.
        let s33 = r.setups.iter().find(|s| s.station == "33").unwrap();
        assert_eq!(s33.backsight.as_deref(), Some("27"));
        let d = s33.stored_delta.unwrap();
        assert!(d.iter().all(|v| v.abs() < 5e-5), "{d:?}");
        // Backsight check: about a millimetre, as FieldGenius reported.
        assert!(s33.bs_distance_error.unwrap().abs() < 0.002);
        assert!(s33.bs_elevation_error.unwrap().abs() < 0.005);

        // The check shot back to 27 is a repeat of a given point, measured.
        let rep = r.repeats.iter().find(|x| x.name == "27").unwrap();
        assert!(rep.dh < 0.002 && rep.dz.abs() < 0.005, "{rep:?}");

        // GPS records are reported, not fatal.
        assert!(r.warnings.iter().any(|w| w.contains("GPS")), "{:?}", r.warnings);
    }

    #[test]
    fn scale_factor_and_curvature() {
        let base = "MO,AD0,UN1,SF1.000000,EC0,EO0.0,AU0\nNEZ,OP1,0,0,0\nOC,OP1\nBK,OP1,BS0.00000,BC0.00000\nLS,HI0,HR0\nSS,OP1,FP2,AR90.00000,ZE90.00000,SD1000.0000\n";
        let r = reduce(base);
        assert!((r.points[1].easting - 1000.0).abs() < 1e-9);
        let scaled = reduce(&base.replace("SF1.000000", "SF0.999600"));
        assert!((scaled.points[1].easting - 999.6).abs() < 1e-9);
        let curved = reduce(&base.replace("EC0", "EC1"));
        // (1 − 0.14)·1000² / (2·6371000) = 0.0675 m.
        assert!((curved.points[1].elevation - 0.0675).abs() < 1e-4);
    }

    #[test]
    fn computed_back_azimuth_and_unoriented_shots() {
        let text = "NEZ,OP1,0,0,0\nNEZ,OP2,0,100,0\nOC,OP1\nBK,OP1,BP2,BC0.00000\nLS,HI0,HR0\nSS,OP1,FP3,AR90.00000,ZE90.00000,SD10\n";
        let r = reduce(text);
        // Backsight due east (az 90°) + 90° right = due south.
        let p3 = r.points.iter().find(|p| p.name == "3").unwrap();
        assert!((p3.northing + 10.0).abs() < 1e-9 && p3.easting.abs() < 1e-9);

        let r = reduce("NEZ,OP1,0,0,0\nOC,OP1\nSS,OP1,FP3,AR90.00000,ZE90.00000,SD10\n");
        assert!(r.points.iter().all(|p| p.name != "3"));
        assert!(r.warnings.iter().any(|w| w.contains("before an oriented setup")));
    }
}
