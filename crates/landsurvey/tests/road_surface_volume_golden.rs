//! Golden regression test for LandXML surface import + surface-to-datum volume,
//! validated against **Civil 3D 2026** (the licensed ground truth).
//!
//! Fixture: `fixtures/road_surface.landxml` — the Softree Terrain "Road Surface"
//! sample (Topo1) exported to LandXML 1.2, 1,770 points / 3,234 TIN faces,
//! Imperial (feet). Source: Softree's Terrain → LandXML export KB article.
//!
//! The expected values were captured from Civil 3D 2026 by importing this exact
//! LandXML surface (`LANDXMLIN`, faces preserved) and building a TIN volume
//! surface against a flat datum plane via the in-process Civil 3D .NET API
//! (`TinVolumeSurface` → `GetVolumeProperties`). Our engine reproduces them to
//! the cubic foot, and an independent triangle-split computation agrees too —
//! so a drift here is a real regression, not a tolerance artifact.
//!
//! Civil 3D sign convention: volume elevation = comparison − base. With
//! base = flat datum and comparison = the real surface, C3D's *FillVolume* is
//! the material ABOVE the datum (our `CutFill::cut`) and *CutVolume* is BELOW
//! (our `CutFill::fill`).

use landsurvey::landxml;

const XML: &str = include_str!("fixtures/road_surface.landxml");

fn road_surface() -> landsurvey::surface::Surface {
    landxml::read_first_surface(XML)
        .expect("road_surface.landxml should contain a TIN surface")
        .surface
}

#[test]
fn topology_and_area_match_civil3d() {
    let s = road_surface();
    assert_eq!(s.nodes.len(), 1770, "point count");
    assert_eq!(s.triangles.len(), 3234, "triangle count");
    // Civil 3D Statistics.Area2d = 197939.005436743
    let a = s.area_2d();
    assert!((a - 197_939.005_436_743).abs() < 0.01, "area2d {a} != C3D 197939.0054");
}

#[test]
fn datum_volume_below_surface_all_cut() {
    // Datum 1160 sits below the whole surface (min Z = 1160.001): all "cut"
    // (material above the datum), zero below. C3D: above = 6231957.544445779.
    let s = road_surface();
    let (cf, contour) = s.cut_fill_to_datum_detailed(1160.0);
    assert!((cf.cut - 6_231_957.544_445_779).abs() < 0.1, "cut {} != C3D golden", cf.cut);
    assert!(cf.fill.abs() < 1e-6, "fill {} should be 0", cf.fill);
    assert!(contour.is_empty(), "no datum crossing expected at 1160");
}

#[test]
fn datum_volume_crossing_matches_civil3d() {
    // Datum 1190 cuts through the surface. C3D authoritative:
    //   above (our cut)  = 1561780.7790662292
    //   below (our fill) = 1267993.3977226885
    //   net              =  293787.38134354
    let s = road_surface();
    let (cf, contour) = s.cut_fill_to_datum_detailed(1190.0);
    assert!((cf.cut - 1_561_780.779_066_229).abs() < 0.1, "cut {} != C3D golden", cf.cut);
    assert!((cf.fill - 1_267_993.397_722_688).abs() < 0.1, "fill {} != C3D golden", cf.fill);
    assert!((cf.net - 293_787.381_343_54).abs() < 0.1, "net {} != C3D golden", cf.net);
    assert!(!contour.is_empty(), "datum 1190 should cross the surface");
}
