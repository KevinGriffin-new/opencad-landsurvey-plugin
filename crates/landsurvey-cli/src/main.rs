//! Headless Land Survey CLI — run the engine against files and dump a DXF you
//! can open in any CAD viewer, so the math and the geometry can be iterated
//! without launching Open CAD Studio. Layers/colours mirror what the plugin
//! draws in-app, so the DXF preview matches the eventual OCS output.

use std::collections::HashMap;
use std::fs;

use landsurvey::dxf::DxfBuilder;
use landsurvey::surface::{self, Surface};
use landsurvey::transform::{self, Conformal};
use landsurvey::resection;
use landsurvey::{cogo, landxml, pnezd};

const USAGE: &str = "\
landsurvey-cli — headless survey engine + DXF preview

USAGE:
  landsurvey-cli surface <points.csv|landxml> [-o out.dxf]
  landsurvey-cli volume  <top> <bottom> [--grid <step>] [-o out.dxf]
  landsurvey-cli datum   <surface> <elevation> [-o out.dxf]
  landsurvey-cli rts     <points.csv> --base <N,E> [--to <N,E>] [--rot <deg>] [--scale <s>] [--anim out.svg] [--teach] [--csv out.csv] [-o out.dxf]
  landsurvey-cli helmert <pairs.csv> [--apply <points.csv>] [--anim out.svg] [--teach] [--csv out.csv] [-o out.dxf]
  landsurvey-cli resect  <shots.csv> [-o out.dxf]
  landsurvey-cli inverse <N1> <E1> <N2> <E2> [--anim out.svg]

Surface inputs are PNEZD CSV (point, northing, easting, elevation, desc) or
LandXML (auto-detected). rts = Rotate/Translate/Scale about a base point (CCW+);
--to translates the base to a new location. DXF layers: LS-TIN-*, LS-CUTFILL/
LS-DATUM, LS-RTS-SRC/DST, LS-*-LABEL.";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(match run(&args) {
        Ok(msg) => {
            println!("{msg}");
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    });
}

fn run(args: &[String]) -> Result<String, String> {
    match args.first().map(String::as_str).unwrap_or("") {
        "surface" => cmd_surface(&args[1..]),
        "volume" => cmd_volume(&args[1..]),
        "datum" => cmd_datum(&args[1..]),
        "rts" => cmd_rts(&args[1..]),
        "helmert" => cmd_helmert(&args[1..]),
        "resect" => cmd_resect(&args[1..]),
        "inverse" => cmd_inverse(&args[1..]),
        "" | "-h" | "--help" | "help" => Ok(USAGE.to_string()),
        other => Err(format!("unknown subcommand \"{other}\"\n\n{USAGE}")),
    }
}

/// `surface <points.csv> [-o out.dxf]` — build a TIN and write its triangulation
/// (+ the source points) to DXF.
fn cmd_surface(args: &[String]) -> Result<String, String> {
    let (pos, opts) = split_args(args, &["-o", "--out"]);
    let path = pos
        .first()
        .ok_or("usage: surface <points.csv> [-o out.dxf]")?;
    let surf = read_surface(path)?;
    let name = stem(path);
    let tin_layer = format!("LS-TIN-{name}");
    let pts_layer = format!("LS-POINTS-{name}");

    let mut d = DxfBuilder::new();
    d.add_layer(&tin_layer, 3); // green
    d.add_layer(&pts_layer, 7); // white
    add_tin(&mut d, &surf, &tin_layer);
    for n in &surf.nodes {
        d.point(&pts_layer, *n);
    }

    let out = opt(&opts).unwrap_or_else(|| format!("{}.dxf", name.to_lowercase()));
    write_file(&out, &d.build())?;
    Ok(format!(
        "surface \"{name}\": {} pts, {} triangles, {} TIN edges, plan area {:.3} -> {out}",
        surf.nodes.len(),
        surf.triangles.len(),
        surf.edges().len(),
        surf.area_2d()
    ))
}

/// `volume <top.csv> <bottom.csv> [--grid step] [-o out.dxf]` — exact TIN-overlay
/// cut/fill/net (+ optional grid method), and a DXF of both TINs, the cut/fill
/// boundary, and a result label.
fn cmd_volume(args: &[String]) -> Result<String, String> {
    let (pos, opts) = split_args(args, &["-o", "--out", "--grid"]);
    let top_p = pos
        .first()
        .ok_or("usage: volume <top.csv> <bottom.csv> [--grid step] [-o out.dxf]")?;
    let bot_p = pos
        .get(1)
        .ok_or("usage: volume <top.csv> <bottom.csv> [--grid step] [-o out.dxf]")?;
    let top = read_surface(top_p)?;
    let bottom = read_surface(bot_p)?;

    let det = surface::composite_cut_fill_detailed(&top, &bottom);
    let cf = det.cut_fill;
    let mut msg = format!(
        "volume (exact TIN overlay): cut {:.3}, fill {:.3}, net {:.3} [top {} pts, bottom {} pts]",
        cf.cut,
        cf.fill,
        cf.net,
        top.nodes.len(),
        bottom.nodes.len()
    );
    if let Some(g) = opts.get("grid") {
        match g.parse::<f64>() {
            Ok(step) if step > 0.0 => {
                let gv = surface::grid_cut_fill(&top, &bottom, step);
                msg.push_str(&format!(
                    "\nvolume (grid @ {:.3}): cut {:.3}, fill {:.3}, net {:.3}, plan area {:.3}, {} cells",
                    step, gv.cut, gv.fill, gv.net, gv.plan_area, gv.n_cells
                ));
            }
            _ => msg.push_str(&format!("\n(ignoring invalid --grid \"{g}\")")),
        }
    }

    let mut d = DxfBuilder::new();
    d.add_layer("LS-TIN-TOP", 3); // green
    d.add_layer("LS-TIN-BOTTOM", 1); // red
    d.add_layer("LS-CUTFILL", 5); // blue
    d.add_layer("LS-VOLUME-LABEL", 7); // white
    add_tin(&mut d, &top, "LS-TIN-TOP");
    add_tin(&mut d, &bottom, "LS-TIN-BOTTOM");
    for s in &det.cutfill_line {
        d.line("LS-CUTFILL", [s[0][0], s[0][1], 0.0], [s[1][0], s[1][1], 0.0]);
    }
    let (minx, maxx, miny, maxy) = top.extent();
    let height = ((maxx - minx).max(maxy - miny) / 50.0).max(1.0);
    d.text(
        "LS-VOLUME-LABEL",
        [(minx + maxx) / 2.0, (miny + maxy) / 2.0, 0.0],
        height,
        &format!("CUT {:.2}  FILL {:.2}  NET {:.2}", cf.cut, cf.fill, cf.net),
    );

    let out = opt(&opts).unwrap_or_else(|| "volume.dxf".to_string());
    write_file(&out, &d.build())?;
    msg.push_str(&format!(
        "\nDXF -> {out} (cut/fill line: {} seg)",
        det.cutfill_line.len()
    ));
    Ok(msg)
}

/// `datum <surface> <elevation> [-o out.dxf]` — cut/fill of one surface against
/// a horizontal plane, with a DXF of the TIN, the datum contour, and a label.
fn cmd_datum(args: &[String]) -> Result<String, String> {
    let (pos, opts) = split_args(args, &["-o", "--out"]);
    let path = pos
        .first()
        .ok_or("usage: datum <surface> <elevation> [-o out.dxf]")?;
    let elev: f64 = pos
        .get(1)
        .ok_or("usage: datum <surface> <elevation> [-o out.dxf]")?
        .parse()
        .map_err(|_| "elevation must be a number".to_string())?;
    let surf = read_surface(path)?;
    let name = stem(path);

    let (cf, contour) = surf.cut_fill_to_datum_detailed(elev);
    let msg = format!(
        "datum \"{name}\" vs elev {elev}: cut {:.3} (above), fill {:.3} (below), net {:.3} \
         [{} pts, {} triangles]",
        cf.cut,
        cf.fill,
        cf.net,
        surf.nodes.len(),
        surf.triangles.len()
    );

    let tin_layer = format!("LS-TIN-{name}");
    let mut d = DxfBuilder::new();
    d.add_layer(&tin_layer, 3);
    d.add_layer("LS-DATUM", 5);
    d.add_layer("LS-VOLUME-LABEL", 7);
    add_tin(&mut d, &surf, &tin_layer);
    for s in &contour {
        d.line("LS-DATUM", [s[0][0], s[0][1], elev], [s[1][0], s[1][1], elev]);
    }
    let (minx, maxx, miny, maxy) = surf.extent();
    let height = ((maxx - minx).max(maxy - miny) / 50.0).max(1.0);
    d.text(
        "LS-VOLUME-LABEL",
        [(minx + maxx) / 2.0, (miny + maxy) / 2.0, 0.0],
        height,
        &format!("ELEV {elev}  CUT {:.2}  FILL {:.2}  NET {:.2}", cf.cut, cf.fill, cf.net),
    );

    let out = opt(&opts).unwrap_or_else(|| format!("{}_datum.dxf", name.to_lowercase()));
    write_file(&out, &d.build())?;
    Ok(format!("{msg}\nDXF -> {out} (datum contour: {} seg)", contour.len()))
}

/// `rts <points.csv> --base N,E [--to N,E] [--rot deg] [--scale s]` —
/// Rotate/Translate/Scale a PNEZD point set about a base point. Writes a DXF
/// (source vs transformed points) and, with `--csv`, the transformed PNEZD.
fn cmd_rts(args: &[String]) -> Result<String, String> {
    let (pos, opts) = split_args(args, &["-o", "--out", "--csv", "--base", "--to", "--rot", "--scale", "--anim"]);
    let path = pos.first().ok_or("usage: rts <points.csv> --base <N,E> [--to <N,E>] [--rot deg] [--scale s]")?;
    let base = parse_ne(opts.get("base").ok_or("rts needs --base <N,E>")?)?; // (n, e)
    let to = match opts.get("to") {
        Some(s) => parse_ne(s)?,
        None => base,
    };
    let rot: f64 = opts.get("rot").map(|s| s.parse()).transpose().map_err(|_| "rot must be a number")?.unwrap_or(0.0);
    let scale: f64 = opts.get("scale").map(|s| s.parse()).transpose().map_err(|_| "scale must be a number")?.unwrap_or(1.0);

    // Conformal operates on (E, N); base/to are (N, E).
    let t = Conformal::from_base_swing((base.1, base.0), (to.1, to.0), rot, scale);

    let text = fs::read_to_string(path).map_err(|e| format!("cannot read \"{path}\": {e}"))?;
    let parsed = pnezd::parse(&text);
    if parsed.points.is_empty() {
        return Err(format!("\"{path}\" has no usable points"));
    }

    let mut d = DxfBuilder::new();
    d.add_layer("LS-RTS-SRC", 8); // gray
    d.add_layer("LS-RTS-DST", 3); // green
    d.add_layer("LS-RTS-BASE", 1); // red
    let mut csv = String::new();
    let mut first: Option<(f64, f64, f64, f64)> = None;
    for p in &parsed.points {
        let (ep, np_) = t.apply(p.easting, p.northing);
        d.point("LS-RTS-SRC", [p.easting, p.northing, p.elevation]);
        d.point("LS-RTS-DST", [ep, np_, p.elevation]);
        csv.push_str(&format!("{},{:.4},{:.4},{:.4},{}\n", p.number, np_, ep, p.elevation, p.description));
        if first.is_none() {
            first = Some((p.easting, p.northing, ep, np_));
        }
    }
    // Mark the base move with a short connector.
    d.line("LS-RTS-BASE", [base.1, base.0, 0.0], [to.1, to.0, 0.0]);
    d.point("LS-RTS-BASE", [to.1, to.0, 0.0]);

    let name = stem(path);
    let out = opt(&opts).unwrap_or_else(|| format!("{}_rts.dxf", name.to_lowercase()));
    write_file(&out, &d.build())?;
    if let Some(csv_path) = opts.get("csv") {
        write_file(csv_path, &csv)?;
    }
    // Optional animated-SVG explainer (--teach amplifies small rot/scale).
    if let Some(anim_path) = opts.get("anim") {
        let teach = args.iter().any(|a| a == "--teach");
        let pts: Vec<(f64, f64)> = parsed.points.iter().map(|p| (p.easting, p.northing)).collect();
        let svg = landsurvey::viz::rts_anim_svg(&pts, &t, (base.1, base.0), teach);
        write_file(anim_path, &svg)?;
    }

    let (se, sn, de, dn) = first.unwrap();
    Ok(format!(
        "rts \"{name}\": {} pts, scale {:.6}, rotation {:.4}\u{b0} (CCW+), base ({:.3},{:.3})->({:.3},{:.3})\n\
         e.g. (E {:.3}, N {:.3}) -> (E {:.3}, N {:.3})\nDXF -> {out}{}{}",
        parsed.points.len(),
        t.scale(),
        t.rotation_deg(),
        base.0, base.1, to.0, to.1,
        se, sn, de, dn,
        opts.get("csv").map(|c| format!("; CSV -> {c}")).unwrap_or_default(),
        opts.get("anim").map(|a| format!("; anim -> {a}")).unwrap_or_default(),
    ))
}

/// `helmert <pairs.csv> [--apply points.csv] [--csv out] [-o out.dxf]` —
/// least-squares 2-D conformal (Helmert) fit from control pairs. Prints the
/// fit as 7 explicit steps and draws the application *stages* (source -> scaled
/// -> rotated -> final, over the target, with residual vectors). `--apply`
/// transforms a separate point set by the fitted transform.
fn cmd_helmert(args: &[String]) -> Result<String, String> {
    let (pos, opts) = split_args(args, &["-o", "--out", "--csv", "--apply", "--anim"]);
    let path = pos.first().ok_or("usage: helmert <pairs.csv> [--apply points.csv] [--anim out.svg] [-o out.dxf]")?;
    let text = fs::read_to_string(path).map_err(|e| format!("cannot read \"{path}\": {e}"))?;
    let pairs = transform::parse_control_pairs(&text);
    if pairs.len() < 2 {
        return Err(format!("\"{path}\" has {} control pair(s); need at least 2", pairs.len()));
    }
    // Optional animated-SVG explainer of the fit (--teach amplifies a near-grid fit).
    if let Some(anim_path) = opts.get("anim") {
        let teach = args.iter().any(|a| a == "--teach");
        let svg = landsurvey::viz::helmert_anim_svg(&pairs, teach).map_err(|e| e.to_string())?;
        write_file(anim_path, &svg)?;
    }
    let steps = transform::helmert_fit_explained(&pairs).map_err(|e| e.to_string())?;
    let t = steps.transform;
    let (res, rms) = transform::fit_residuals(&t, &pairs);
    let max = res.iter().cloned().fold(0.0_f64, f64::max);

    // ---- the 7 explicit steps -------------------------------------------------
    let mut report = String::from("helmert — 2D 4-parameter conformal (Rotate/Translate/Scale)\n");
    report.push_str(&format!(
        "  step 1  source centroid : E {:.4}, N {:.4}\n",
        steps.src_centroid.0, steps.src_centroid.1
    ));
    report.push_str(&format!(
        "  step 2  target centroid : E {:.4}, N {:.4}\n",
        steps.dst_centroid.0, steps.dst_centroid.1
    ));
    report.push_str(&format!(
        "  step 3  cross-cov sums  : Sxx {:.4}, Sxy {:.4}, S|d|^2 {:.4}\n",
        steps.sxx, steps.sxy, steps.sum_sq
    ));
    report.push_str(&format!(
        "  step 4  scale  s        : {:.8}   (hypot(a,b); a=Sxx/S|d|^2, b=Sxy/S|d|^2)\n",
        steps.scale()
    ));
    report.push_str(&format!(
        "  step 5  rotation theta  : {:.6}\u{b0} CCW+   (atan2(b,a))\n",
        steps.rotation_deg()
    ));
    report.push_str(&format!(
        "  step 6  translation     : E {:.4}, N {:.4}   (centroid_dst - s*R*centroid_src)\n",
        t.c, t.d
    ));
    report.push_str(&format!(
        "  step 7  residuals       : RMS {:.5}, max {:.5}  ({} pairs)",
        rms, max, pairs.len()
    ));
    for (i, r) in res.iter().enumerate() {
        report.push_str(&format!("\n            pair {:>2}: {:.5}", i + 1, r));
    }

    // ---- the staged drawing ---------------------------------------------------
    let mut d = DxfBuilder::new();
    d.add_layer("LS-HMT-0-SRC", 8); // gray   — as given
    d.add_layer("LS-HMT-1-SCALED", 2); // yellow — after scale about src centroid
    d.add_layer("LS-HMT-2-ROTATED", 30); // orange — after rotation
    d.add_layer("LS-HMT-3-FINAL", 3); // green  — after translation to target centroid
    d.add_layer("LS-HMT-TARGET", 1); // red    — actual control targets
    d.add_layer("LS-HMT-RESID", 1); // red    — final -> target (the misfit)
    d.add_layer("LS-HMT-PATH", 8); // gray   — each point's 0->1->2->3 path
    d.add_layer("LS-HMT-CENTROID", 4); // cyan   — the two centroids
    d.add_layer("LS-HMT-LEGEND", 7);

    for &((se, sn), (de, dn)) in &pairs {
        let st = steps.stages(se, sn);
        d.point("LS-HMT-0-SRC", [st[0].0, st[0].1, 0.0]);
        d.point("LS-HMT-1-SCALED", [st[1].0, st[1].1, 0.0]);
        d.point("LS-HMT-2-ROTATED", [st[2].0, st[2].1, 0.0]);
        d.point("LS-HMT-3-FINAL", [st[3].0, st[3].1, 0.0]);
        d.point("LS-HMT-TARGET", [de, dn, 0.0]);
        for w in 0..3 {
            d.line("LS-HMT-PATH", [st[w].0, st[w].1, 0.0], [st[w + 1].0, st[w + 1].1, 0.0]);
        }
        d.line("LS-HMT-RESID", [st[3].0, st[3].1, 0.0], [de, dn, 0.0]);
    }
    d.point("LS-HMT-CENTROID", [steps.src_centroid.0, steps.src_centroid.1, 0.0]);
    d.point("LS-HMT-CENTROID", [steps.dst_centroid.0, steps.dst_centroid.1, 0.0]);

    // Numbered step annotations, placed where each step happens: 1-3 in place at
    // the source centroid, 4 along the move, 5 at the target. Text height is
    // sized from the source control spread (not the big translation distance).
    let (smnx, smxx, smny, smxy) = pairs.iter().fold(
        (f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY),
        |(a, b, c, e), &((se, sn), _)| (a.min(se), b.max(se), c.min(sn), e.max(sn)),
    );
    let h = ((smxx - smnx).max(smxy - smny) / 12.0).max(0.5);
    d.add_layer("LS-HMT-LABEL", 7);
    let (sg, dg) = (steps.src_centroid, steps.dst_centroid);
    let in_place = [
        "1 source".to_string(),
        format!("2 scale x{:.5}", steps.scale()),
        format!("3 rotate {:.4} deg", steps.rotation_deg()),
    ];
    for (i, line) in in_place.iter().enumerate() {
        d.text("LS-HMT-LABEL", [sg.0 + h, sg.1 + h * (3.0 - 1.4 * i as f64), 0.0], h, line);
    }
    d.text(
        "LS-HMT-LABEL",
        [(sg.0 + dg.0) / 2.0, (sg.1 + dg.1) / 2.0 + h * 1.5, 0.0],
        h,
        &format!("4 translate dE {:.2} dN {:.2}", dg.0 - sg.0, dg.1 - sg.1),
    );
    d.text("LS-HMT-LABEL", [dg.0 + h, dg.1 + h * 3.0, 0.0], h, &format!("5 residual max {:.4}", max));

    // ---- optional apply to a separate point set -------------------------------
    if let Some(apply_path) = opts.get("apply") {
        d.add_layer("LS-RTS-SRC", 8);
        d.add_layer("LS-RTS-DST", 3);
        let atext = fs::read_to_string(apply_path)
            .map_err(|e| format!("cannot read \"{apply_path}\": {e}"))?;
        let parsed = pnezd::parse(&atext);
        let mut csv = String::new();
        for p in &parsed.points {
            let (ep, np_) = t.apply(p.easting, p.northing);
            d.point("LS-RTS-SRC", [p.easting, p.northing, p.elevation]);
            d.point("LS-RTS-DST", [ep, np_, p.elevation]);
            csv.push_str(&format!("{},{:.4},{:.4},{:.4},{}\n", p.number, np_, ep, p.elevation, p.description));
        }
        if let Some(csv_path) = opts.get("csv") {
            write_file(csv_path, &csv)?;
        }
        report.push_str(&format!("\napplied to {} points from {apply_path}", parsed.points.len()));
    }

    let name = stem(path);
    let out = opt(&opts).unwrap_or_else(|| format!("{}_helmert.dxf", name.to_lowercase()));
    write_file(&out, &d.build())?;
    if let Some(anim_path) = opts.get("anim") {
        report.push_str(&format!("\nanimation -> {anim_path}"));
    }
    Ok(format!("{report}\nDXF (stages) -> {out}"))
}

/// `resect <shots.csv> [-o out.dxf]` — free-station resection. Solves the
/// occupied (unknown) point from shots to known control: combined (direction +
/// distance, least-squares) when ≥2 shots carry distances, else angle-only
/// three-point (Tienstra) for exactly 3 angle shots. Draws the station, a ray to
/// each known point (labeled with its residual), and a result label.
fn cmd_resect(args: &[String]) -> Result<String, String> {
    let (pos, opts) = split_args(args, &["-o", "--out"]);
    let path = pos.first().ok_or("usage: resect <shots.csv> [-o out.dxf]")?;
    let text = fs::read_to_string(path).map_err(|e| format!("cannot read \"{path}\": {e}"))?;
    let shots = resection::parse_resection_shots(&text);
    if shots.len() < 2 {
        return Err(format!("\"{path}\" has {} usable shot(s); need at least 2", shots.len()));
    }
    let with_dist = shots.iter().filter(|s| s.distance.is_some()).count();

    let mut report = String::from("resect — free-station resection\n");
    let mut d = DxfBuilder::new();
    d.add_layer("LS-RESECT-STATION", 1); // red
    d.add_layer("LS-RESECT-KNOWN", 3); // green
    d.add_layer("LS-RESECT-RAYS", 8); // gray
    d.add_layer("LS-RESECT-LABEL", 7); // white

    // Combined when there are >=2 distance shots; otherwise angle-only 3-point.
    let station: (f64, f64) = if with_dist >= 2 {
        let r = resection::resection_combined(&shots).map_err(|e| e.to_string())?;
        report.push_str(&format!(
            "  method        : combined (direction + distance, least-squares)\n\
             \u{20}\u{20}station (E,N) : {:.4}, {:.4}\n\
             \u{20}\u{20}orientation   : {:.4}\u{b0}  (grid azimuth = reading + orientation)\n\
             \u{20}\u{20}scale check   : {:.8}{}\n\
             \u{20}\u{20}residuals     : RMS {:.5} ({} shot{})",
            r.station.0,
            r.station.1,
            r.orientation_deg,
            r.scale,
            if r.scale_blunder(1e-3) { "   <-- WARNING: strays from 1.0 (distance/EDM blunder?)" } else { "" },
            r.rms,
            r.residuals.len(),
            if r.residuals.len() == 1 { "" } else { "s" },
        ));
        for (name, res) in &r.residuals {
            report.push_str(&format!("\n            {name:>6}: {res:.5}"));
        }
        r.station
    } else if shots.len() == 3 {
        // Angle-only Tienstra. Subtended angles at P from the three readings.
        let a = shots[0].known;
        let b = shots[1].known;
        let c = shots[2].known;
        let sep = |x: f64, y: f64| (x - y).rem_euclid(360.0).min((y - x).rem_euclid(360.0));
        let (ra, rb, rc) = (shots[0].direction_deg, shots[1].direction_deg, shots[2].direction_deg);
        let ang = [sep(rb, rc), sep(rc, ra), sep(ra, rb)]; // [∠BPC, ∠CPA, ∠APB]
        let p = resection::resection_three_point(a, b, c, ang).map_err(|e| e.to_string())?;
        report.push_str(&format!(
            "  method        : angle-only three-point (Tienstra)\n\
             \u{20}\u{20}station (E,N) : {:.4}, {:.4}\n\
             \u{20}\u{20}subtended     : BPC {:.4}\u{b0}, CPA {:.4}\u{b0}, APB {:.4}\u{b0}",
            p.0, p.1, ang[0], ang[1], ang[2]
        ));
        p
    } else {
        return Err(format!(
            "need >=2 distance shots (combined) or exactly 3 angle shots (three-point); \
             got {} shots, {with_dist} with distances",
            shots.len()
        ));
    };

    // Draw: station, known points, rays, and a label.
    d.point("LS-RESECT-STATION", [station.0, station.1, 0.0]);
    for s in &shots {
        d.point("LS-RESECT-KNOWN", [s.known.0, s.known.1, 0.0]);
        d.line("LS-RESECT-RAYS", [station.0, station.1, 0.0], [s.known.0, s.known.1, 0.0]);
    }
    let spread = shots
        .iter()
        .map(|s| (s.known.0 - station.0).hypot(s.known.1 - station.1))
        .fold(0.0_f64, f64::max);
    let h = (spread / 30.0).max(1.0);
    d.text(
        "LS-RESECT-LABEL",
        [station.0 + h, station.1 + h, 0.0],
        h,
        &format!("STA E{:.3} N{:.3}", station.0, station.1),
    );

    let name = stem(path);
    let out = opt(&opts).unwrap_or_else(|| format!("{}_resect.dxf", name.to_lowercase()));
    write_file(&out, &d.build())?;
    Ok(format!("{report}\nDXF -> {out}"))
}

/// Parse an `N,E` pair (northing,easting) into `(n, e)`.
fn parse_ne(s: &str) -> Result<(f64, f64), String> {
    let mut it = s.split(',');
    let n: f64 = it.next().and_then(|v| v.trim().parse().ok()).ok_or_else(|| format!("bad N,E pair \"{s}\""))?;
    let e: f64 = it.next().and_then(|v| v.trim().parse().ok()).ok_or_else(|| format!("bad N,E pair \"{s}\""))?;
    Ok((n, e))
}

/// `inverse <N1> <E1> <N2> <E2> [--anim out.svg]` — distance + azimuth + bearing.
fn cmd_inverse(args: &[String]) -> Result<String, String> {
    let (pos, opts) = split_args(args, &["--anim", "-o", "--out"]);
    let n: Vec<f64> = pos.iter().filter_map(|a| a.parse().ok()).collect();
    if n.len() < 4 {
        return Err("usage: inverse <N1> <E1> <N2> <E2> [--anim out.svg]".to_string());
    }
    let inv = cogo::inverse(n[0], n[1], n[2], n[3]);
    let mut msg = format!(
        "distance {:.4}, azimuth {:.4}\u{b0}, bearing {}",
        inv.distance,
        inv.azimuth_deg,
        cogo::azimuth_to_bearing(inv.azimuth_deg)
    );
    if let Some(anim_path) = opt(&opts).or_else(|| opts.get("anim").cloned()) {
        let svg = landsurvey::viz::inverse_anim_svg(n[0], n[1], n[2], n[3]);
        write_file(&anim_path, &svg)?;
        msg.push_str(&format!("\nanim -> {anim_path}"));
    }
    Ok(msg)
}

// --- helpers -----------------------------------------------------------------

fn add_tin(d: &mut DxfBuilder, s: &Surface, layer: &str) {
    for e in s.edges() {
        d.line(layer, s.nodes[e[0]], s.nodes[e[1]]);
    }
}

/// Read a surface from a file, auto-detecting LandXML (a TIN with explicit
/// faces) vs a PNEZD point file (triangulated here).
fn read_surface(path: &str) -> Result<Surface, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("cannot read \"{path}\": {e}"))?;
    if landxml::looks_like_landxml(&text) {
        let ns = landxml::read_first_surface(&text)
            .ok_or_else(|| format!("\"{path}\": no TIN surface found in LandXML"))?;
        return Ok(ns.surface);
    }
    let out = pnezd::parse(&text);
    if out.points.len() < 3 {
        return Err(format!(
            "\"{path}\" has {} usable point(s); need at least 3 for a surface",
            out.points.len()
        ));
    }
    let nodes: Vec<[f64; 3]> = out
        .points
        .iter()
        .map(|p| [p.easting, p.northing, p.elevation])
        .collect();
    Ok(Surface::from_points(&nodes))
}

/// Split argv into positionals and the values of known `value_flags` (each of
/// which consumes the following token). Flag keys are stored without dashes.
fn split_args(args: &[String], value_flags: &[&str]) -> (Vec<String>, HashMap<String, String>) {
    let mut pos = Vec::new();
    let mut opts = HashMap::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if value_flags.contains(&a.as_str()) {
            let key = a.trim_start_matches('-').to_string();
            if let Some(v) = args.get(i + 1) {
                opts.insert(key, v.clone());
                i += 2;
                continue;
            }
        }
        pos.push(a.clone());
        i += 1;
    }
    (pos, opts)
}

/// The output path from `-o` / `--out`, if present.
fn opt(opts: &HashMap<String, String>) -> Option<String> {
    opts.get("o").or_else(|| opts.get("out")).cloned()
}

fn stem(path: &str) -> String {
    std::path::Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().to_uppercase())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "SURFACE".to_string())
}

fn write_file(path: &str, data: &str) -> Result<(), String> {
    fs::write(path, data).map_err(|e| format!("cannot write \"{path}\": {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> String {
        let mut p = std::env::temp_dir();
        p.push(format!("ls_cli_{}_{name}", std::process::id()));
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn surface_and_volume_emit_dxf() {
        let bot = tmp("bottom.csv");
        let top = tmp("top.csv");
        fs::write(&bot, "1,0,0,100\n2,0,100,100\n3,100,100,100\n4,100,0,100\n").unwrap();
        fs::write(
            &top,
            "1,0,0,105\n2,0,100,105\n3,100,100,105\n4,100,0,105\n5,50,50,105\n",
        )
        .unwrap();

        let sout = tmp("surf.dxf");
        let r = run(&[
            "surface".into(),
            top.clone(),
            "-o".into(),
            sout.clone(),
        ])
        .unwrap();
        assert!(r.contains("triangles"), "{r}");
        let sdxf = fs::read_to_string(&sout).unwrap();
        assert!(sdxf.contains("\nLINE\n"));
        assert!(sdxf.trim_end().ends_with("EOF"));

        let vout = tmp("vol.dxf");
        let r2 = run(&[
            "volume".into(),
            top.clone(),
            bot.clone(),
            "--grid".into(),
            "5".into(),
            "-o".into(),
            vout.clone(),
        ])
        .unwrap();
        assert!(r2.contains("net 50000.000"), "{r2}");
        let vdxf = fs::read_to_string(&vout).unwrap();
        assert!(vdxf.contains("LS-TIN-TOP"));
        assert!(vdxf.contains("LS-VOLUME-LABEL"));

        for f in [bot, top, sout, vout] {
            let _ = fs::remove_file(f);
        }
    }
}
