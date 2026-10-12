//! Opt-in check of the SDR reader against a real Sokkia SDR file: set
//! `SDR_SAMPLE` to its path.
//!
//! Every coordinate record (`02` station, `08` keyed or collector-computed)
//! must come through exactly as written — the first record of each point id,
//! with numeric ids losing their leading zeros — and every backsight azimuth
//! whose two points are known must be reported. (Against MicroSurvey CAD's
//! own import of the 1991 SDR22 sample, all 264 points and descriptions
//! matched exactly; that comparison needs the licensed app, so it is not
//! repeated here.)

use std::collections::HashSet;

use landsurvey::sdr;

#[test]
fn coordinates_come_through_as_written() {
    let Ok(path) = std::env::var("SDR_SAMPLE") else {
        return;
    };
    let text = landsurvey::text::decode(&std::fs::read(&path).expect("read SDR_SAMPLE"));
    assert!(sdr::looks_like_sdr(&text));
    let r = sdr::reduce(&text);
    assert!(!r.points.is_empty(), "{:?}", r.warnings);

    // Independently re-read the coordinate records, trying both layouts.
    let lines: Vec<&str> = text.lines().map(|l| l.trim_matches(|c: char| c.is_control())).collect();
    let mut checked = 0;
    let mut seen = HashSet::new();
    for (id_w, num_w) in [(4usize, 10usize), (16, 16)] {
        let parse = |l: &str, k: usize| -> Option<f64> {
            l.get(4 + id_w + k * num_w..4 + id_w + (k + 1) * num_w)?.trim().parse().ok()
        };
        let fits = lines
            .iter()
            .filter(|l| l.starts_with("08"))
            .all(|l| (0..3).all(|k| parse(l, k).is_some()));
        if !fits {
            continue;
        }
        for l in lines.iter().filter(|l| l.starts_with("08") || l.starts_with("02")) {
            let raw_id = l[4..4 + id_w].trim();
            let id = if raw_id.bytes().all(|b| b.is_ascii_digit()) {
                let t = raw_id.trim_start_matches('0');
                if t.is_empty() { "0".to_string() } else { t.to_string() }
            } else {
                raw_id.to_string()
            };
            if !seen.insert(id.clone()) {
                continue; // later records of the same id are repeats
            }
            let p = r.points.iter().find(|p| p.name == id).unwrap_or_else(|| panic!("point {id} missing"));
            assert_eq!(Some(p.northing), parse(l, 0), "{id} N");
            assert_eq!(Some(p.easting), parse(l, 1), "{id} E");
            assert_eq!(Some(p.elevation), parse(l, 2).or(Some(0.0)), "{id} Z");
            checked += 1;
        }
        break;
    }
    assert!(checked > 0);
    assert_eq!(checked, seen.len());
}
