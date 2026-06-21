//! Animated-SVG explainers for survey operations — pure string building, `std`
//! only (no host/CAD deps), so they generate identically from the plugin, the
//! CLI, or WASM. Each generator is *data-driven*: it animates the user's actual
//! inputs/outputs, so the clip shows the real result (scale, rotation,
//! residuals, distance, bearing, …).
//!
//! Two modes where it helps:
//! * **faithful** (default) — exact geometry; what a surveyor wants for QA.
//! * **teach** — small rotations/scales are amplified (with an on-screen note)
//!   so the *operation* reads even when the real numbers are tiny — the version
//!   for a classroom or a careers page.
//!
//! Output is a self-contained, theme-aware SVG that loops in any browser. Shared
//! mechanics: a world→SVG viewport (survey N-up / SVG y-down flip), a convex
//! hull for the morphing shape, and a fixed keyframe timeline.

use crate::cogo;
use crate::transform::{fit_residuals, helmert_fit_explained, Conformal};

const W: f64 = 680.0;
const H: f64 = 380.0;
const MARGIN: f64 = 56.0;
const DUR_S: f64 = 7.0;

/// Loop keyframe fractions: source, scaled, rotated, final, hold, back-to-source.
const KEY_TIMES: [f64; 6] = [0.0, 0.16, 0.40, 0.66, 0.85, 1.0];
const FRAME_STAGE: [usize; 6] = [0, 1, 2, 3, 3, 0];

struct Caption {
    color: String,
    text: String,
    op: [f64; 6],
}

// =====================================================================
// Helmert
// =====================================================================

/// Animated SVG of a 2-D Helmert fit. `teach` amplifies a near-grid transform
/// so the rotation/scale are visible. Needs ≥ 2 control pairs.
pub fn helmert_anim_svg(
    pairs: &[((f64, f64), (f64, f64))],
    teach: bool,
) -> Result<String, &'static str> {
    let steps = helmert_fit_explained(pairs)?;
    let (_, rms) = fit_residuals(&steps.transform, pairs);
    let src: Vec<(f64, f64)> = pairs.iter().map(|&(s, _)| s).collect();
    let dst: Vec<(f64, f64)> = pairs.iter().map(|&(_, d)| d).collect();
    let pivot = steps.src_centroid;
    let sub = format!("2-D Helmert ({} control points): scale · rotate · translate", pairs.len());

    if teach {
        let (tt, rk, sk) = exaggerate(&steps.transform, pivot);
        let caps = vec![
            cap("#caa000", format!("scale ×{:.5} (shown ×{:.0})", steps.scale(), sk.max(1.0)), [1., 1., 0., 0., 0., 1.]),
            cap("#e8772e", format!("rotate {:.3}° (shown ×{:.0})", steps.rotation_deg(), rk.max(1.0)), [0., 1., 1., 0., 0., 0.]),
            cap("var(--muted)", "translate ΔE, ΔN".into(), [0., 0., 1., 1., 0., 0.]),
            cap("#2ea043", format!("fit ✓  RMS {:.3}", rms), [0., 0., 0., 1., 1., 0.]),
        ];
        Ok(morph_svg(&src, &tt, pivot, None, &caps, &sub, Some("rotation & scale exaggerated for clarity")))
    } else {
        let caps = vec![
            cap("#caa000", format!("scale ×{:.5}", steps.scale()), [1., 1., 0., 0., 0., 1.]),
            cap("#e8772e", format!("rotate {:.3}°", steps.rotation_deg()), [0., 1., 1., 0., 0., 0.]),
            cap("var(--muted)", "translate ΔE, ΔN".into(), [0., 0., 1., 1., 0., 0.]),
            cap("#2ea043", format!("fit ✓  RMS {:.3}", rms), [0., 0., 0., 1., 1., 0.]),
        ];
        Ok(morph_svg(&src, &steps.transform, pivot, Some(&dst), &caps, &sub, None))
    }
}

// =====================================================================
// RTS (rotate / translate / scale)
// =====================================================================

/// Animated SVG of an explicit Rotate/Translate/Scale applied to `points`
/// about `pivot` (the base point). `teach` amplifies small rotation/scale.
pub fn rts_anim_svg(points: &[(f64, f64)], t: &Conformal, pivot: (f64, f64), teach: bool) -> String {
    let (tt, rk, sk) = if teach { exaggerate(t, pivot) } else { (*t, 1.0, 1.0) };
    let note = if teach { Some("rotation & scale exaggerated for clarity") } else { None };
    let sfx = |k: f64| if teach && k > 1.5 { format!(" (shown ×{k:.0})") } else { String::new() };
    let caps = vec![
        cap("#caa000", format!("scale ×{:.5}{}", t.scale(), sfx(sk)), [1., 1., 0., 0., 0., 1.]),
        cap("#e8772e", format!("rotate {:.3}°{}", t.rotation_deg(), sfx(rk)), [0., 1., 1., 0., 0., 0.]),
        cap("var(--muted)", "translate".into(), [0., 0., 1., 1., 0., 0.]),
        cap("#2ea043", "result".into(), [0., 0., 0., 1., 1., 0.]),
    ];
    morph_svg(points, &tt, pivot, None, &caps, "Rotate · Translate · Scale about a base point", note)
}

// =====================================================================
// Inverse (distance + bearing between two points)
// =====================================================================

/// Animated SVG of a COGO inverse: point A and B appear, the line draws on, then
/// the distance and quadrant bearing reveal.
pub fn inverse_anim_svg(n1: f64, e1: f64, n2: f64, e2: f64) -> String {
    let inv = cogo::inverse(n1, e1, n2, e2);
    let bearing = cogo::azimuth_to_bearing(inv.azimuth_deg);
    let vp = Viewport::fit(&[(e1, n1), (e2, n2)]);
    let (ax, ay) = vp.map((e1, n1));
    let (bx, by) = vp.map((e2, n2));
    let len = ((bx - ax).powi(2) + (by - ay).powi(2)).sqrt().max(1.0);
    let (mx, my) = ((ax + bx) / 2.0, (ay + by) / 2.0);
    let kt = key_times_str();

    let mut svg = svg_head(
        "Animated COGO inverse",
        "Two points appear, the line between them draws on, then the distance and bearing reveal.",
    );
    // line draws on via dashoffset, then holds
    svg.push_str(&format!(
        "<line x1=\"{ax:.1}\" y1=\"{ay:.1}\" x2=\"{bx:.1}\" y2=\"{by:.1}\" stroke=\"var(--ink)\" \
         stroke-width=\"2\" stroke-dasharray=\"{len:.1}\">\
         <animate attributeName=\"stroke-dashoffset\" dur=\"{DUR_S}s\" repeatCount=\"indefinite\" \
         keyTimes=\"{kt}\" values=\"{len:.1};{len:.1};0;0;0;{len:.1}\"/></line>\n"
    ));
    // endpoints
    for (px, py, label, t0) in [(ax, ay, "A", 0usize), (bx, by, "B", 1usize)] {
        let op = if t0 == 0 { "1;1;1;1;1;1" } else { "0;1;1;1;1;0" };
        svg.push_str(&format!(
            "<circle cx=\"{px:.1}\" cy=\"{py:.1}\" r=\"4\" fill=\"#2ea043\" opacity=\"{}\">\
             <animate attributeName=\"opacity\" dur=\"{DUR_S}s\" repeatCount=\"indefinite\" keyTimes=\"{kt}\" values=\"{op}\"/></circle>\
             <text x=\"{:.1}\" y=\"{:.1}\" font-size=\"12\" fill=\"var(--muted)\">{label}</text>\n",
            if t0 == 0 { 1 } else { 0 }, px + 7.0, py - 7.0
        ));
    }
    // distance + bearing labels reveal after the line is drawn
    svg.push_str(&format!(
        "<text x=\"{mx:.1}\" y=\"{:.1}\" text-anchor=\"middle\" font-size=\"14\" font-weight=\"600\" \
         fill=\"var(--ink)\" opacity=\"0\">dist {:.3}\
         <animate attributeName=\"opacity\" dur=\"{DUR_S}s\" repeatCount=\"indefinite\" keyTimes=\"{kt}\" values=\"0;0;1;1;1;0\"/></text>\n",
        my - 8.0, inv.distance
    ));
    svg.push_str(&format!(
        "<text x=\"{:.0}\" y=\"28\" text-anchor=\"middle\" font-size=\"14\" font-weight=\"600\" fill=\"#1f6feb\" opacity=\"0\">\
         azimuth {:.4}°   bearing {}\
         <animate attributeName=\"opacity\" dur=\"{DUR_S}s\" repeatCount=\"indefinite\" keyTimes=\"{kt}\" values=\"0;0;0;1;1;0\"/></text>\n",
        W / 2.0, inv.azimuth_deg, xml_escape(&bearing)
    ));
    svg.push_str(&subtitle("COGO inverse: straight-line distance + azimuth/bearing between two points"));
    svg.push_str("</svg>\n");
    svg
}

// =====================================================================
// shared morph engine
// =====================================================================

fn morph_svg(
    src: &[(f64, f64)],
    t: &Conformal,
    pivot: (f64, f64),
    targets: Option<&[(f64, f64)]>,
    captions: &[Caption],
    sub: &str,
    teach_note: Option<&str>,
) -> String {
    let stage_pos: Vec<[(f64, f64); 4]> =
        src.iter().map(|&(e, n)| t.stages_about(e, n, pivot)).collect();

    let mut all: Vec<(f64, f64)> = Vec::new();
    for s in &stage_pos {
        all.extend_from_slice(s);
    }
    if let Some(tg) = targets {
        all.extend_from_slice(tg);
    }
    all.push(pivot);
    all.push(t.apply(pivot.0, pivot.1));
    let vp = Viewport::fit(&all);

    let src0: Vec<(f64, f64)> = stage_pos.iter().map(|s| s[0]).collect();
    let hull = convex_hull(&src0);
    let kt = key_times_str();

    let mut svg = svg_head("Animated transform", "A control shape scales, rotates, then translates.");

    // translation path
    let (pgx, pgy) = vp.map(pivot);
    let (tgx, tgy) = vp.map(t.apply(pivot.0, pivot.1));
    svg.push_str(&format!(
        "<line x1=\"{pgx:.1}\" y1=\"{pgy:.1}\" x2=\"{tgx:.1}\" y2=\"{tgy:.1}\" stroke=\"var(--muted)\" \
         stroke-width=\"1.2\" stroke-dasharray=\"4 5\" opacity=\"0.6\"/>\n"
    ));

    // targets + residual vectors (faithful mode only)
    if let Some(tg) = targets {
        if hull.len() >= 3 {
            let tp: String = hull.iter().map(|&i| fmt_pt(vp.map(tg[i]))).collect::<Vec<_>>().join(" ");
            svg.push_str(&format!(
                "<polygon points=\"{tp}\" fill=\"none\" stroke=\"#d83b3b\" stroke-width=\"1.5\" stroke-dasharray=\"5 3\"/>\n"
            ));
        }
        for &p in tg {
            let (x, y) = vp.map(p);
            svg.push_str(&format!("<circle cx=\"{x:.1}\" cy=\"{y:.1}\" r=\"3\" fill=\"#d83b3b\"/>\n"));
        }
        for (s, &p) in stage_pos.iter().zip(tg) {
            let (fx, fy) = vp.map(s[3]);
            let (px, py) = vp.map(p);
            svg.push_str(&format!(
                "<line x1=\"{fx:.1}\" y1=\"{fy:.1}\" x2=\"{px:.1}\" y2=\"{py:.1}\" stroke=\"#d83b3b\" stroke-width=\"1.5\" opacity=\"0\">\
                 <animate attributeName=\"opacity\" dur=\"{DUR_S}s\" repeatCount=\"indefinite\" keyTimes=\"{kt}\" values=\"0;0;0;1;1;0\"/></line>\n"
            ));
        }
    }

    // morphing hull
    if hull.len() >= 3 {
        let frames: Vec<String> = FRAME_STAGE
            .iter()
            .map(|&k| hull.iter().map(|&i| fmt_pt(vp.map(stage_pos[i][k]))).collect::<Vec<_>>().join(" "))
            .collect();
        svg.push_str(&format!(
            "<polygon stroke=\"var(--ink)\" stroke-width=\"2\" fill-opacity=\"0.5\">\
             <animate attributeName=\"points\" dur=\"{DUR_S}s\" repeatCount=\"indefinite\" keyTimes=\"{kt}\" values=\"{}\"/>\
             <animate attributeName=\"fill\" dur=\"{DUR_S}s\" repeatCount=\"indefinite\" keyTimes=\"{kt}\" \
             values=\"#9aa0a6;#e0a800;#e8772e;#2ea043;#2ea043;#9aa0a6\"/></polygon>\n",
            frames.join(" ; ")
        ));
    }

    // moving dots
    for s in &stage_pos {
        let cx: Vec<String> = FRAME_STAGE.iter().map(|&k| format!("{:.1}", vp.map(s[k]).0)).collect();
        let cy: Vec<String> = FRAME_STAGE.iter().map(|&k| format!("{:.1}", vp.map(s[k]).1)).collect();
        svg.push_str(&format!(
            "<circle r=\"3.2\" fill=\"#3a3f45\">\
             <animate attributeName=\"cx\" dur=\"{DUR_S}s\" repeatCount=\"indefinite\" keyTimes=\"{kt}\" values=\"{}\"/>\
             <animate attributeName=\"cy\" dur=\"{DUR_S}s\" repeatCount=\"indefinite\" keyTimes=\"{kt}\" values=\"{}\"/></circle>\n",
            cx.join(";"), cy.join(";")
        ));
    }

    // captions
    svg.push_str("<g text-anchor=\"middle\" font-size=\"14\" font-weight=\"600\">\n");
    for c in captions {
        let vals = c.op.iter().map(|v| format!("{v}")).collect::<Vec<_>>().join(";");
        svg.push_str(&format!(
            "<text x=\"{:.0}\" y=\"28\" fill=\"{}\">{}\
             <animate attributeName=\"opacity\" dur=\"{DUR_S}s\" repeatCount=\"indefinite\" keyTimes=\"{kt}\" values=\"{vals}\"/></text>\n",
            W / 2.0, c.color, xml_escape(&c.text)
        ));
    }
    svg.push_str("</g>\n");
    if let Some(note) = teach_note {
        svg.push_str(&format!(
            "<text x=\"{:.0}\" y=\"46\" text-anchor=\"middle\" font-size=\"10\" fill=\"var(--muted)\">{}</text>\n",
            W / 2.0, xml_escape(note)
        ));
    }
    svg.push_str(&subtitle(sub));
    svg.push_str("</svg>\n");
    svg
}

/// Amplify a near-identity transform's rotation and scale (keeping the pivot's
/// translation faithful) so the operation is visible. Returns the exaggerated
/// transform and the rotation / scale amplification factors actually used.
fn exaggerate(t: &Conformal, pivot: (f64, f64)) -> (Conformal, f64, f64) {
    let th = t.rotation_deg();
    let (th_show, rk) = if th.abs() < 1e-9 {
        (0.0, 1.0)
    } else {
        let k = (22.0 / th.abs()).clamp(1.0, 80.0);
        let shown = (th * k).clamp(-28.0, 28.0);
        (shown, (shown / th).abs())
    };
    let dev = t.scale() - 1.0;
    let (s_show, sk) = if dev.abs() < 1e-12 {
        (1.0, 1.0)
    } else {
        let k = (0.30 / dev.abs()).clamp(1.0, 500.0);
        let shown = (1.0 + dev * k).clamp(0.6, 1.5);
        (shown, ((shown - 1.0) / dev).abs())
    };
    (Conformal::from_base_swing(pivot, t.apply(pivot.0, pivot.1), th_show, s_show), rk, sk)
}

// --- small helpers -----------------------------------------------------------

fn cap(color: &str, text: String, op: [f64; 6]) -> Caption {
    Caption { color: color.to_string(), text, op }
}

fn svg_head(title: &str, desc: &str) -> String {
    format!(
        "<svg viewBox=\"0 0 {W} {H}\" xmlns=\"http://www.w3.org/2000/svg\" \
         font-family=\"ui-sans-serif, system-ui, sans-serif\" role=\"img\">\n\
         <title>{}</title><desc>{}</desc>\n\
         <style>:root{{--ink:var(--text-primary,#1f2328);--muted:var(--text-secondary,#57606a);}}</style>\n",
        xml_escape(title), xml_escape(desc)
    )
}

fn subtitle(s: &str) -> String {
    format!(
        "<text x=\"{:.0}\" y=\"{:.0}\" text-anchor=\"middle\" font-size=\"11\" fill=\"var(--muted)\">{}</text>\n",
        W / 2.0, H - 10.0, xml_escape(s)
    )
}

struct Viewport {
    minx: f64,
    miny: f64,
    s: f64,
    offx: f64,
    offy: f64,
}

impl Viewport {
    fn fit(pts: &[(f64, f64)]) -> Viewport {
        let (minx, maxx, miny, maxy) = bbox(pts);
        let (dx, dy) = ((maxx - minx).max(1e-9), (maxy - miny).max(1e-9));
        let s = ((W - 2.0 * MARGIN) / dx).min((H - 2.0 * MARGIN) / dy);
        let offx = MARGIN + ((W - 2.0 * MARGIN) - dx * s) / 2.0;
        let offy = MARGIN + ((H - 2.0 * MARGIN) - dy * s) / 2.0;
        Viewport { minx, miny, s, offx, offy }
    }
    fn map(&self, (x, y): (f64, f64)) -> (f64, f64) {
        (self.offx + (x - self.minx) * self.s, H - (self.offy + (y - self.miny) * self.s))
    }
}

fn bbox(pts: &[(f64, f64)]) -> (f64, f64, f64, f64) {
    let mut r = (f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY);
    for &(x, y) in pts {
        r.0 = r.0.min(x);
        r.1 = r.1.max(x);
        r.2 = r.2.min(y);
        r.3 = r.3.max(y);
    }
    r
}

fn fmt_pt((x, y): (f64, f64)) -> String {
    format!("{x:.1},{y:.1}")
}

fn key_times_str() -> String {
    KEY_TIMES.iter().map(|t| format!("{t}")).collect::<Vec<_>>().join(";")
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Convex hull (Andrew's monotone chain); returns vertex indices CCW.
fn convex_hull(pts: &[(f64, f64)]) -> Vec<usize> {
    let n = pts.len();
    if n < 3 {
        return (0..n).collect();
    }
    let mut idx: Vec<usize> = (0..n).collect();
    idx.sort_by(|&a, &b| {
        pts[a].0.partial_cmp(&pts[b].0).unwrap().then(pts[a].1.partial_cmp(&pts[b].1).unwrap())
    });
    let cross = |o: usize, a: usize, b: usize| {
        (pts[a].0 - pts[o].0) * (pts[b].1 - pts[o].1) - (pts[a].1 - pts[o].1) * (pts[b].0 - pts[o].0)
    };
    let mut lower: Vec<usize> = Vec::new();
    for &i in &idx {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], i) <= 0.0 {
            lower.pop();
        }
        lower.push(i);
    }
    let mut upper: Vec<usize> = Vec::new();
    for &i in idx.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], i) <= 0.0 {
            upper.pop();
        }
        upper.push(i);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helmert_anim_faithful_and_teach() {
        let truth = Conformal::from_base_swing((0.0, 0.0), (300.0, 200.0), 0.6, 1.0004);
        let src = [(0.0, 0.0), (40.0, 0.0), (40.0, 30.0), (0.0, 30.0), (20.0, 15.0)];
        let pairs: Vec<_> = src.iter().map(|&(e, n)| ((e, n), truth.apply(e, n))).collect();
        let faithful = helmert_anim_svg(&pairs, false).unwrap();
        let teach = helmert_anim_svg(&pairs, true).unwrap();
        assert!(faithful.starts_with("<svg") && faithful.trim_end().ends_with("</svg>"));
        assert!(teach.contains("exaggerated"));
        assert!(faithful.contains("rotate 0.600"));
        assert!(teach.contains("shown ×")); // amplified
    }

    #[test]
    fn rts_and_inverse_wellformed() {
        let t = Conformal::from_base_swing((0.0, 0.0), (50.0, 20.0), 30.0, 1.2);
        let pts = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        let rts = rts_anim_svg(&pts, &t, (0.0, 0.0), false);
        assert!(rts.contains("Rotate") && rts.contains("<animate"));
        let inv = inverse_anim_svg(0.0, 0.0, 100.0, 100.0);
        assert!(inv.contains("bearing") && inv.contains("stroke-dashoffset"));
    }

    #[test]
    fn convex_hull_of_square_is_four() {
        let sq = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0), (0.5, 0.5)];
        assert_eq!(convex_hull(&sq).len(), 4);
    }
}
