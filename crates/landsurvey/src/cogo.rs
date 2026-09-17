//! COGO geometry — pure functions, `std` only (no host/CAD/iced/acadrust).
//!
//! Convention: Northing maps to world Y, Easting maps to world X — matching
//! Open CAD Studio's X=Easting / Y=Northing screen convention.

/// Result of a coordinate inverse: the straight-line distance and the
/// azimuth measured clockwise from North (grid north), in degrees.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Inverse {
    pub distance: f64,
    pub azimuth_deg: f64,
}

/// Distance and azimuth from point 1 to point 2.
pub fn inverse(n1: f64, e1: f64, n2: f64, e2: f64) -> Inverse {
    let dn = n2 - n1;
    let de = e2 - e1;
    let distance = (dn * dn + de * de).sqrt();
    // atan2(ΔEasting, ΔNorthing) yields the bearing clockwise from North.
    let mut azimuth_deg = de.atan2(dn).to_degrees();
    if azimuth_deg < 0.0 {
        azimuth_deg += 360.0;
    }
    Inverse {
        distance,
        azimuth_deg,
    }
}

/// Format an azimuth (degrees clockwise from North) as a quadrant bearing,
/// e.g. `N 45°30'00.00" E`.
pub fn azimuth_to_bearing(azimuth_deg: f64) -> String {
    let a = azimuth_deg.rem_euclid(360.0);
    let (ns, angle, ew) = if a <= 90.0 {
        ("N", a, "E")
    } else if a <= 180.0 {
        ("S", 180.0 - a, "E")
    } else if a <= 270.0 {
        ("S", a - 180.0, "W")
    } else {
        ("N", 360.0 - a, "W")
    };
    let (d, m, s) = to_dms(angle);
    format!("{ns} {d}\u{b0}{m:02}'{s:05.2}\" {ew}")
}

/// Split decimal degrees into (degrees, minutes, seconds).
fn to_dms(deg: f64) -> (i64, i64, f64) {
    // Round in integer centiseconds before splitting the fields.  Splitting
    // first lets a value such as 29°59'59.999" format as 29°59'60.00".
    let total_centiseconds = (deg * 360_000.0).round() as i64;
    let d = total_centiseconds / 360_000;
    let rem = total_centiseconds % 360_000;
    let m = rem / 6_000;
    let seconds_centiseconds = rem % 6_000;
    (d, m, seconds_centiseconds as f64 / 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inverse_due_east_is_azimuth_90() {
        let inv = inverse(0.0, 0.0, 0.0, 100.0);
        assert!((inv.distance - 100.0).abs() < 1e-9);
        assert!((inv.azimuth_deg - 90.0).abs() < 1e-9);
    }

    #[test]
    fn inverse_diagonal_ne() {
        let inv = inverse(0.0, 0.0, 100.0, 100.0);
        assert!((inv.distance - (2.0f64).sqrt() * 100.0).abs() < 1e-9);
        assert!((inv.azimuth_deg - 45.0).abs() < 1e-9);
        assert_eq!(azimuth_to_bearing(inv.azimuth_deg), "N 45\u{b0}00'00.00\" E");
    }

    #[test]
    fn bearing_quadrants() {
        assert!(azimuth_to_bearing(135.0).starts_with("S 45"));
        assert!(azimuth_to_bearing(135.0).ends_with("E"));
        assert!(azimuth_to_bearing(225.0).starts_with("S 45"));
        assert!(azimuth_to_bearing(225.0).ends_with("W"));
        assert!(azimuth_to_bearing(315.0).starts_with("N 45"));
        assert!(azimuth_to_bearing(315.0).ends_with("W"));
    }

    #[test]
    fn dms_rounding_carries_seconds_into_minutes() {
        let almost_thirty = 29.0 + 59.0 / 60.0 + 59.999 / 3_600.0;
        assert_eq!(azimuth_to_bearing(almost_thirty), "N 30\u{b0}00'00.00\" E");
    }
}
