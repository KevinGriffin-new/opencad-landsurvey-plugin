//! Opt-in check of the RW5 reduction against a real FieldGenius raw file:
//! set `RW5_SAMPLE` to a `.raw` / `.rw5` path.
//!
//! The expected values come from the file itself, written by FieldGenius in
//! the field and never read by the reducer:
//! - each `OC` record's station coordinates, which FieldGenius computed from
//!   the shot to that point, must match our reduction to 0.1 mm;
//! - each backsight's "Distance Error" and "BS Elevation Error" orientation
//!   notes must match our backsight check at the precision written (1 mm).
//!
//! Real field data is not committed; the synthetic cases live in the unit
//! tests in `src/rw5.rs`.

use landsurvey::rw5;

/// The numbers FieldGenius wrote in `--  <label>:  <value>m` comment lines,
/// in file order.
fn notes(text: &str, label: &str) -> Vec<f64> {
    text.lines()
        .filter_map(|l| l.strip_prefix("--"))
        .filter_map(|l| l.trim().strip_prefix(label))
        .filter_map(|l| l.trim_start_matches(':').trim().trim_end_matches('m').parse().ok())
        .collect()
}

#[test]
fn reduction_matches_what_fieldgenius_wrote() {
    let Ok(path) = std::env::var("RW5_SAMPLE") else {
        return;
    };
    let bytes = std::fs::read(&path).expect("read RW5_SAMPLE");
    let text = landsurvey::text::decode(&bytes);
    let r = rw5::reduce(&text);
    assert!(!r.points.is_empty(), "{:?}", r.warnings);

    let mut stored = 0;
    for s in &r.setups {
        if let Some(d) = s.stored_delta {
            assert!(d.iter().all(|v| v.abs() < 1e-4), "setup {}: stored - ours = {d:?}", s.station);
            stored += 1;
        }
    }
    assert!(stored > 0, "no OC record carried coordinates");

    let dist = notes(&text, "Distance Error");
    let elev = notes(&text, "BS Elevation Error");
    let ours: Vec<_> = r
        .setups
        .iter()
        .filter_map(|s| s.bs_distance_error.zip(s.bs_elevation_error))
        .filter(|(h, z)| h.abs() > 0.0 || z.abs() > 0.0) // the BS to a stored check point
        .collect();
    assert_eq!(ours.len(), dist.len(), "one note per checked backsight");
    for ((h, z), (dh, dz)) in ours.iter().zip(dist.iter().zip(&elev)) {
        assert!((h - dh).abs() <= 0.0005 + 1e-9, "distance error {h:.4} vs FieldGenius {dh}");
        assert!((z - dz).abs() <= 0.0005 + 1e-9, "elevation error {z:.4} vs FieldGenius {dz}");
    }
}
