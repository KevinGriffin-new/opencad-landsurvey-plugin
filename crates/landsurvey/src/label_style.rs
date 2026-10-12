//! MicroSurvey CAD (MsAnnotate) line-label styles: bearing + distance text
//! formatting and text sizing, so labels drawn by this plugin read the same as
//! labels drawn by MicroSurvey.
//!
//! The style fields mirror the `bearingstyles` / `distancestyles` arrays that
//! MicroSurvey CAD 2027 persists per job in `<job>.MSJ/config.json`, so a real
//! job file can be loaded with [`LabelStyles::from_msj_config`]. Formatting
//! behaviour was recovered from MSCAD 2027's `FormatBearing` (mscad.dll) and
//! `FormatDistanceString` (config.dll); see `docs/msannotate-line-labels.md`
//! for the evidence and for which parts are inferred rather than observed.

use serde::Deserialize;

/// One MsAnnotate bearing style (`bearingstyles[i]` in a job's config.json).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct BearingStyle {
    pub name: String,
    pub layername: String,
    /// AutoCAD colour index.
    pub color: i16,
    pub fontname: String,
    /// Plotted text height in thousandths of an inch (60 ≈ 1.5 mm).
    pub leroy: u32,
    /// Label clearance from the line, in text heights.
    pub textoffset: f64,
    /// Round the angle to a multiple of this many seconds; 60 drops seconds.
    pub rounding: u32,
    /// Spaces between the quadrant letters and the angle.
    pub quadrantspacing: u32,
    /// Spaces after the degree and minute symbols.
    pub spacesbetweendms: u32,
    /// Zero-pad single-digit degrees (`05°`) instead of space-padding (` 5°`).
    pub useleadingzero: bool,
    pub usereferencenorth: bool,
    pub width: f64,
    pub obliqueangle: f64,
}

impl Default for BearingStyle {
    /// MicroSurvey's shipped `BEARING1`.
    fn default() -> Self {
        Self {
            name: "BEARING1".into(),
            layername: "BEARING1".into(),
            color: 1,
            fontname: "MSURVEY.SHX".into(),
            leroy: 60,
            textoffset: 0.5,
            rounding: 1,
            quadrantspacing: 0,
            spacesbetweendms: 0,
            useleadingzero: false,
            usereferencenorth: false,
            width: 1.0,
            obliqueangle: 0.0,
        }
    }
}

/// One MsAnnotate distance style (`distancestyles[i]` in a job's config.json).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct DistanceStyle {
    pub name: String,
    pub layername: String,
    pub color: i16,
    pub fontname: String,
    pub leroy: u32,
    pub textoffset: f64,
    pub decimalplaces: usize,
    pub distanceprefix: String,
    pub distancesuffix: String,
    /// Multiplier applied to the labelled distance.
    pub scalefactor: f64,
    /// Append a foot mark (`'`) to the distance.
    pub usefootsymbol: bool,
    pub width: f64,
    pub obliqueangle: f64,
}

impl Default for DistanceStyle {
    /// MicroSurvey's shipped `DISTANCE1`.
    fn default() -> Self {
        Self {
            name: "DISTANCE1".into(),
            layername: "DISTANCE1".into(),
            color: 3,
            fontname: "MSURVEY.SHX".into(),
            leroy: 60,
            textoffset: 0.5,
            decimalplaces: 2,
            distanceprefix: String::new(),
            distancesuffix: String::new(),
            scalefactor: 1.0,
            usefootsymbol: false,
            width: 1.0,
            obliqueangle: 0.0,
        }
    }
}

/// The bearing + distance style sets of a MicroSurvey job and which entry of
/// each is current.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct LabelStyles {
    pub bearingstyleindex: usize,
    pub bearingstyles: Vec<BearingStyle>,
    pub distancestyleindex: usize,
    pub distancestyles: Vec<DistanceStyle>,
}

impl Default for LabelStyles {
    fn default() -> Self {
        Self {
            bearingstyleindex: 0,
            bearingstyles: vec![BearingStyle::default()],
            distancestyleindex: 0,
            distancestyles: vec![DistanceStyle::default()],
        }
    }
}

impl LabelStyles {
    /// Parse the label styles out of a MicroSurvey job's `config.json`. Keys
    /// this module does not model (curve styles, point protection, …) are
    /// ignored.
    pub fn from_msj_config(json: &str) -> Result<Self, String> {
        let mut s: Self = serde_json::from_str(json).map_err(|e| e.to_string())?;
        if s.bearingstyles.is_empty() {
            s.bearingstyles.push(BearingStyle::default());
        }
        if s.distancestyles.is_empty() {
            s.distancestyles.push(DistanceStyle::default());
        }
        s.bearingstyleindex = s.bearingstyleindex.min(s.bearingstyles.len() - 1);
        s.distancestyleindex = s.distancestyleindex.min(s.distancestyles.len() - 1);
        Ok(s)
    }

    pub fn bearing(&self) -> &BearingStyle {
        &self.bearingstyles[self.bearingstyleindex]
    }

    pub fn distance(&self) -> &DistanceStyle {
        &self.distancestyles[self.distancestyleindex]
    }
}

/// Format an azimuth (degrees clockwise from North) as a MicroSurvey quadrant
/// bearing, e.g. `N88°16'15"E` with the default style.
pub fn format_bearing(azimuth_deg: f64, style: &BearingStyle) -> String {
    let a = azimuth_deg.rem_euclid(360.0);
    // Quadrant 1..4 = NE, SE, SW, NW, and the angle measured from N or S.
    let (mut quadrant, angle) = if a < 90.0 {
        (1, a)
    } else if a < 180.0 {
        (2, 180.0 - a)
    } else if a < 270.0 {
        (3, a - 180.0)
    } else {
        (4, 360.0 - a)
    };

    // MSCAD shows minutes only when rounding is exactly 60"; a rounding above
    // 60" falls back to whole seconds.
    let show_seconds = style.rounding != 60;
    // `f64::round` rounds halves away from zero, as MSCAD's modf/copysign does.
    let step = match style.rounding {
        0 => 1.0,
        r if r > 60 => 1.0,
        r => r as f64,
    };
    let total_sec = (angle * 3600.0 / step).round() * step;
    let total_sec = if show_seconds {
        total_sec
    } else {
        (total_sec / 60.0).round() * 60.0
    };
    let deg = (total_sec / 3600.0).floor();
    let min = ((total_sec - deg * 3600.0) / 60.0).floor();
    let sec = total_sec - deg * 3600.0 - min * 60.0;

    // A bearing that rounds to exactly 90° reads N…E / N…W, never S90°.
    if deg == 90.0 {
        quadrant = match quadrant {
            2 => 1,
            3 => 4,
            q => q,
        };
    }

    let gap = " ".repeat(style.spacesbetweendms as usize);
    let mut dms = if style.useleadingzero {
        format!("{deg:02.0}\u{b0}")
    } else {
        format!("{deg:2.0}\u{b0}")
    };
    dms.push_str(&gap);
    dms.push_str(&format!("{min:02.0}'"));
    if show_seconds {
        dms.push_str(&gap);
        dms.push_str(&format!("{sec:02.0}\""));
    }

    let q = " ".repeat(style.quadrantspacing as usize);
    let (ns, ew) = match quadrant {
        1 => ("N", "E"),
        2 => ("S", "E"),
        3 => ("S", "W"),
        _ => ("N", "W"),
    };
    format!("{ns}{q}{dms}{q}{ew}")
}

/// Format a horizontal distance with a MicroSurvey distance style: the value
/// times the style's scale factor at its decimal places, then the foot mark,
/// wrapped in the prefix and suffix.
pub fn format_distance(distance: f64, style: &DistanceStyle) -> String {
    let d = distance * style.scalefactor;
    let foot = if style.usefootsymbol { "'" } else { "" };
    format!(
        "{}{:.*}{}{}",
        style.distanceprefix, style.decimalplaces, d, foot, style.distancesuffix
    )
}

/// World-unit text height for a leroy size at a plot scale of `1:scale`.
/// Leroy is thousandths of an inch on paper, so a metric drawing converts to
/// metres (leroy 60 at 1:500 → 0.762 m) and an imperial one to feet.
pub fn leroy_height(leroy: u32, scale: f64, metric: bool) -> f64 {
    let inches = leroy as f64 / 1000.0;
    if metric {
        inches * 0.0254 * scale
    } else {
        inches * scale / 12.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dms(d: f64, m: f64, s: f64) -> f64 {
        d + m / 60.0 + s / 3600.0
    }

    #[test]
    fn default_style_matches_msannotate() {
        let st = BearingStyle::default();
        assert_eq!(format_bearing(dms(88.0, 16.0, 15.0), &st), "N88\u{b0}16'15\"E");
        assert_eq!(
            format_bearing(180.0 - dms(1.0, 43.0, 45.0), &st),
            "S 1\u{b0}43'45\"E"
        );
        assert_eq!(
            format_bearing(180.0 + dms(45.0, 0.0, 0.0), &st),
            "S45\u{b0}00'00\"W"
        );
        assert_eq!(
            format_bearing(360.0 - dms(12.0, 5.0, 9.0), &st),
            "N12\u{b0}05'09\"W"
        );
    }

    #[test]
    fn seconds_round_and_carry() {
        let st = BearingStyle::default();
        // 29°59'59.6" → 30°00'00".
        assert_eq!(
            format_bearing(dms(29.0, 59.0, 59.6), &st),
            "N30\u{b0}00'00\"E"
        );
        let five = BearingStyle {
            rounding: 5,
            ..BearingStyle::default()
        };
        assert_eq!(
            format_bearing(dms(10.0, 20.0, 33.0), &five),
            "N10\u{b0}20'35\"E"
        );
    }

    #[test]
    fn rounding_60_drops_seconds() {
        let st = BearingStyle {
            rounding: 60,
            ..BearingStyle::default()
        };
        assert_eq!(format_bearing(dms(10.0, 20.0, 31.0), &st), "N10\u{b0}21'E");
    }

    #[test]
    fn due_east_and_west_read_north() {
        let st = BearingStyle::default();
        assert_eq!(format_bearing(90.0, &st), "N90\u{b0}00'00\"E");
        assert_eq!(format_bearing(270.0, &st), "N90\u{b0}00'00\"W");
        // Rounds up to 90° from the SE quadrant.
        assert_eq!(
            format_bearing(90.0 + 0.2 / 3600.0, &st),
            "N90\u{b0}00'00\"E"
        );
    }

    #[test]
    fn spacing_and_leading_zero() {
        let st = BearingStyle {
            quadrantspacing: 1,
            spacesbetweendms: 1,
            useleadingzero: true,
            ..BearingStyle::default()
        };
        assert_eq!(
            format_bearing(dms(5.0, 6.0, 7.0), &st),
            "N 05\u{b0} 06' 07\" E"
        );
    }

    #[test]
    fn distance_formats() {
        let st = DistanceStyle::default();
        assert_eq!(format_distance(36.0, &st), "36.00");
        assert_eq!(format_distance(20.004999, &st), "20.00");
        let ft = DistanceStyle {
            decimalplaces: 3,
            usefootsymbol: true,
            distanceprefix: "~".into(),
            distancesuffix: " m".into(),
            ..DistanceStyle::default()
        };
        assert_eq!(format_distance(12.3456, &ft), "~12.346' m");
    }

    #[test]
    fn leroy_to_world_height() {
        assert!((leroy_height(60, 500.0, true) - 0.762).abs() < 1e-9);
        assert!((leroy_height(80, 600.0, false) - 4.0).abs() < 1e-9);
    }

    #[test]
    fn parses_msj_config() {
        let json = r#"{
            "bearingstyleindex": 1,
            "bearingstyles": [
                {"name":"BEARING1","index":0,"leroy":60,"rounding":1},
                {"name":"BEARING2","index":1,"leroy":80,"rounding":5,"layername":"BRG2","color":2}
            ],
            "distancestyleindex": 0,
            "distancestyles": [{"name":"DISTANCE1","decimalplaces":3}],
            "curvestyles": [],
            "pointprotectionsummary": {"enabled": true}
        }"#;
        let s = LabelStyles::from_msj_config(json).unwrap();
        assert_eq!(s.bearing().name, "BEARING2");
        assert_eq!(s.bearing().layername, "BRG2");
        assert_eq!(s.bearing().rounding, 5);
        assert_eq!(s.distance().decimalplaces, 3);
        // Unset keys keep MicroSurvey's defaults.
        assert_eq!(s.distance().layername, "DISTANCE1");
        assert_eq!(s.distance().color, 3);
    }
}
