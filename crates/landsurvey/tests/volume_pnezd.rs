//! End-to-end check of the LS_VOLUME data path: PNEZD text -> TIN -> composite
//! volume, exactly as `dispatch::volume` runs it in the cdylib (minus the host
//! console). Also serves as a golden-capture harness: feed the same two point
//! sets to MicroSurvey / Civil 3D and compare the reported cut/fill/net.

use landsurvey::{pnezd, surface};

fn surface_from_pnezd(text: &str) -> surface::Surface {
    let out = pnezd::parse(text);
    let nodes: Vec<[f64; 3]> = out
        .points
        .iter()
        .map(|p| [p.easting, p.northing, p.elevation])
        .collect();
    surface::Surface::from_points(&nodes)
}

#[test]
fn flat_pad_lift_is_area_times_offset() {
    // 100 x 100 pad. Bottom flat at elev 100, top flat at elev 105.
    // Net fill (top above bottom) = 100 * 100 * 5 = 50_000.
    // PNEZD columns: point, northing, easting, elevation, desc
    let bottom = "\
1, 0,   0,   100, EG
2, 0,   100, 100, EG
3, 100, 100, 100, EG
4, 100, 0,   100, EG
";
    let top = "\
1, 0,   0,   105, FG
2, 0,   100, 105, FG
3, 100, 100, 105, FG
4, 100, 0,   105, FG
5, 50,  50,  105, FG
";
    let bot = surface_from_pnezd(bottom);
    let tp = surface_from_pnezd(top);
    assert!(bot.triangles.len() >= 1 && tp.triangles.len() >= 1);

    let cf = surface::exact_composite_cut_fill(&tp, &bot);
    assert!((cf.cut - 50_000.0).abs() < 1e-3, "cut {}", cf.cut);
    assert!(cf.fill.abs() < 1e-3, "fill {}", cf.fill);
    assert!((cf.net - 50_000.0).abs() < 1e-3, "net {}", cf.net);

    // Grid method should land close to the exact answer over the shared extent.
    let g = surface::grid_cut_fill(&tp, &bot, 2.0);
    assert!((g.net - 50_000.0).abs() < 50_000.0 * 0.02, "grid net {}", g.net);
}
