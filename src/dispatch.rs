//! Command routing for the Land Survey add-on. All `LS_` commands land here via
//! `BuiltinPlugin::dispatch`. Geometry/COGO math lives in the host-free
//! `landsurvey` engine crate; this file is the glue that turns engine output
//! into `acadrust` entities + XDATA on the active document.

use std::fs;

use codec::xdata::{ExtendedDataRecord, XDataValue};
use codec::entities::Mesh;
use codec::{
    Arc as CadArc, Circle, Color, EntityType, Line, LwPolyline, Point as CadPoint, Text, Vector2, Vector3,
};

use ocs_plugin_api::host::HostApi;


use landsurvey::label_style::{self, LabelStyles};
use landsurvey::surface::{self, Surface};

use crate::batch::Batch;
use crate::landxml_draw;
use landsurvey::{cogo, landxml, plan, pnezd, resection, rw5, transform, viz};

/// XDATA application carrying survey metadata on a `Point` entity.
/// Record values: `[String(point_number), String(description)]`.
pub const XDATA_POINT: &str = "LANDSURVEY_POINT";

/// XDATA application tagging entities imported from a recognized plan.
/// Record values: `[String(source_filename)]`.
pub const XDATA_PLAN: &str = "LANDSURVEY_PLAN";

/// XDATA application tagging entities drawn for a surface / earthwork result.
/// Record values: `[String(surface_name), String(kind)]` where `kind` is one of
/// `TIN`, `CUTFILL`, `LABEL`.
pub const XDATA_SURFACE: &str = "LANDSURVEY_SURFACE";

/// XDATA application tagging a parcel boundary (LandXML `<Parcel>`).
/// Record values: `[String(name), String(description)]`.
pub const XDATA_PARCEL: &str = "LANDSURVEY_PARCEL";

/// XDATA application tagging a plan feature (LandXML `<PlanFeature>`).
/// Record values: `[String(name), String(description)]`.
pub const XDATA_FEATURE: &str = "LANDSURVEY_FEATURE";

/// Default world-unit height for imported plan labels (the source JSON carries
/// no text height).
const PLAN_TEXT_HEIGHT: f64 = 2.0;

pub fn handle(host: &mut dyn HostApi, cmd: &str) -> bool {
    // Route on the first whitespace-delimited token; keep the original `cmd`
    // for argument parsing (paths/coords are case- and content-sensitive).
    let verb = cmd.split_whitespace().next().unwrap_or("").to_uppercase();
    if !(verb.starts_with("LS_") || verb == "LANDXMLIMPORT") {
        return false;
    }
    // XDATA records round-trip natively since acadrust e88a9a6 / OCS #249
    // (records encode to EED on save and decode back on read), so commands
    // read and write tags through the host API with no extra codec pass.
    // Layer-table registration for novel LS-* layers is still host-side work
    // (OCS #252) — nothing the plugin can do out-of-process.
    dispatch_verb(host, &verb, cmd)
}

fn dispatch_verb(host: &mut dyn HostApi, verb: &str, cmd: &str) -> bool {
    match verb {
        "LS_HELLO" => {
            host.push_info(
                "Land Survey plugin ready. Points: LS_PNEZD, LS_RW5, LS_LIST, LS_AUTOLABEL. \
                 Import / export: LS_IMPORTPLAN, LS_LANDXML, LS_LANDXMLOUT. \
                 Surfaces: LS_SURFACE, LS_VOLUME, LS_DATUM. \
                 COGO / transforms: LS_INVERSE, LS_RTS, LS_HELMERT, LS_RESECT. \
                 Labels: LS_LABELSTYLE. \
                 See PLUGIN.md for usage.",
            );
            true
        }
        "LS_PNEZD" => {
            import_pnezd(host, cmd);
            true
        }
        "LS_RW5" => {
            import_rw5(host, cmd);
            true
        }
        "LS_AUTOLABEL" => {
            auto_label_toggle(host, cmd);
            true
        }
        "LS_IMPORTPLAN" => {
            import_plan(host, cmd);
            true
        }
        "LS_INVERSE" => {
            inverse(host, cmd);
            true
        }
        "LS_LABELSTYLE" => {
            label_style_cmd(host, cmd);
            true
        }
        "LS_VOLUME" => {
            volume(host, cmd);
            true
        }
        "LS_SURFACE" => {
            build_surface(host, cmd);
            true
        }
        // `LS_LANDXML` is our own ribbon command; `LANDXMLIMPORT` is the host's
        // Insert-tab button verb — we handle it so the host can dispatch LandXML
        // import to this plugin (per OpenCADStudio issue #157).
        "LS_LANDXML" | "LANDXMLIMPORT" => {
            import_landxml(host, cmd);
            true
        }
        "LS_LANDXMLOUT" => {
            export_landxml(host, cmd);
            true
        }
        "LS_DATUM" => {
            datum_volume(host, cmd);
            true
        }
        "LS_RTS" => {
            rts(host, cmd);
            true
        }
        "LS_HELMERT" => {
            helmert(host, cmd);
            true
        }
        "LS_RESECT" => {
            resect(host, cmd);
            true
        }
        "LS_LIST" => {
            list_points(host);
            true
        }
        _ => false,
    }
}

/// First whitespace-delimited argument after the command verb, trimmed.
fn first_arg(cmd: &str) -> &str {
    cmd.splitn(2, char::is_whitespace)
        .nth(1)
        .map(str::trim)
        .unwrap_or("")
}

/// `LS_PNEZD <path> [pnezd|penzd] [preview]` — import a point file as labeled
/// `Point` entities, grouped onto a per-feature-code layer (`LS-PT-<CODE>`) and
/// tagged with `LANDSURVEY_POINT` XDATA (number + description). `preview` parses
/// and reports without drawing. Delimiter (comma/tab/space) is auto-detected.
fn import_pnezd(host: &mut dyn HostApi, cmd: &str) {
    let rest = first_arg(cmd);
    if rest.is_empty() {
        host.push_info(
            "Usage: LS_PNEZD <path-to-points> [pnezd|penzd] [detail] [preview]  \
             (labels show the point number; add 'detail' for elevation + \
             description; delimiter auto-detected)",
        );
        return;
    }

    // Peel optional trailing keywords (column order + preview) off the tail in
    // any order; the remainder is the path (which may contain spaces).
    let mut path = rest;
    let mut fmt = pnezd::Format::pnezd();
    let mut preview = false;
    let mut detail = false;
    loop {
        let trimmed = path.trim_end();
        let Some(idx) = trimmed.rfind(char::is_whitespace) else {
            break;
        };
        match trimmed[idx + 1..].to_ascii_lowercase().as_str() {
            "penzd" => {
                fmt = pnezd::Format::penzd();
                path = &trimmed[..idx];
            }
            "pnezd" => {
                fmt = pnezd::Format::pnezd();
                path = &trimmed[..idx];
            }
            "preview" => {
                preview = true;
                path = &trimmed[..idx];
            }
            "detail" => {
                detail = true;
                path = &trimmed[..idx];
            }
            _ => break,
        }
    }
    let path = path.trim();

    let text = match read_text(path) {
        Ok(t) => t,
        Err(e) => {
            host.push_error(&format!("LS_PNEZD: cannot read \"{path}\": {e}"));
            return;
        }
    };

    let outcome = pnezd::parse_with(&text, &fmt);
    if outcome.points.is_empty() {
        host.push_error(&format!(
            "LS_PNEZD: no valid points in \"{path}\" ({} line(s) skipped).",
            outcome.skipped
        ));
        return;
    }

    // Preview: report what WOULD import — no changes to the drawing.
    if preview {
        let order = if fmt.easting < fmt.northing { "PENZD" } else { "PNEZD" };
        host.push_output(&format!(
            "LS_PNEZD preview \"{path}\" — order {order}: {} point(s), {} skipped. First rows:",
            outcome.points.len(),
            outcome.skipped
        ));
        for p in outcome.points.iter().take(5) {
            host.push_output(&format!(
                "  {:<8} E {:.3}  N {:.3}  Z {:.3}  {}",
                p.number, p.easting, p.northing, p.elevation, p.description
            ));
        }
        if outcome.points.len() > 5 {
            host.push_output(&format!("  … and {} more", outcome.points.len() - 5));
        }
        return;
    }

    host.push_undo("LS_PNEZD import");
    let (added, total) = draw_survey_points(host, &outcome.points, detail);

    host.bump_geometry();
    host.set_dirty();
    let skipped = if outcome.skipped > 0 {
        format!(", {} line(s) skipped", outcome.skipped)
    } else {
        String::new()
    };
    host.push_output(&format!(
        "LS_PNEZD: imported {added} labeled point(s){skipped}, grouped by code. \
         {total} this session."
    ));
}

/// Draw survey points as `Point` entities on per-code layers (`LS-PT-<CODE>`),
/// tagged with `LANDSURVEY_POINT` XDATA (number + description) and labelled
/// unless `LS_AUTOLABEL OFF`. Returns `(added, total imported this session)`.
fn draw_survey_points(
    host: &mut dyn HostApi,
    points: &[pnezd::PnezdPoint],
    detail: bool,
) -> (usize, usize) {
    // Label/marker sizing follows MicroSurvey MSannotate proportions: a SMALL
    // precise marker with the point number hugging it (~half a text-height
    // away). The number — not the marker — is the prominent element. Text
    // height scales to the survey extent so it stays legible at zoom-extents.
    let h = label_height(points);
    let marker = {
        let header = &mut host.document_mut().header;
        if header.point_display_mode == 0 {
            header.point_display_mode = 3; // '×' glyph
        }
        if header.point_display_size == 0.0 {
            // A modest tick (~0.4×H), not a giant ×; the label carries the eye.
            header.point_display_size = h * 0.4;
        }
        header.point_display_size
    };
    // Hug the marker: clear its arm (~half the display size) plus a small gap,
    // matching MSannotate's ~0.47×H point-number offset for a small marker.
    let off = marker * 0.5 + h * 0.3;
    let auto_label = !crate::state::state().suppress_labels;

    // One bulk add: per-point round trips made large files quadratic (#38).
    let mut batch = Batch::new();
    let mut starts = Vec::with_capacity(points.len());
    for p in points {
        let layer = code_layer(p.code());
        // World mapping: X = Easting, Y = Northing, Z = Elevation.
        let pt = EntityType::Point(CadPoint::at(Vector3::new(p.easting, p.northing, p.elevation)));
        let mut rec = ExtendedDataRecord::new(XDATA_POINT);
        rec.add_value(XDataValue::String(p.number.clone()));
        rec.add_value(XDataValue::String(p.description.clone()));
        starts.push(batch.len());
        batch.add_tagged(pt, &layer, rec);

        if auto_label {
            draw_point_label(&mut batch, p, h, off, &layer, detail);
        }
    }
    let accepted = batch.commit(host);
    let added = starts.iter().filter(|&&i| i < accepted).count();

    let mut st = crate::state::state();
    st.imported += added;
    (added, st.imported)
}

/// `LS_AUTOLABEL [ON|OFF]` — toggle whether `LS_PNEZD` draws point-number
/// labels on import. With no argument, reports the current state.
fn auto_label_toggle(host: &mut dyn HostApi, cmd: &str) {
    let arg = first_arg(cmd).trim().to_ascii_uppercase();
    if !matches!(arg.as_str(), "" | "ON" | "OFF") {
        host.push_info("Usage: LS_AUTOLABEL [ON|OFF]");
        return;
    }
    let on = {
        let mut st = crate::state::state();
        match arg.as_str() {
            "OFF" => st.suppress_labels = true,
            "ON" => st.suppress_labels = false,
            _ => {}
        }
        !st.suppress_labels
    };
    host.push_output(&format!(
        "LS_AUTOLABEL: point labels {} on import.",
        if on { "ON" } else { "OFF" }
    ));
}

/// Per-feature-code layer name: `LS-PT-<CODE>` (code upper-cased, DWG-illegal
/// characters replaced with `_`), or `LS-POINTS` when the point has no code.
fn code_layer(code: &str) -> String {
    if code.is_empty() {
        return "LS-POINTS".to_string();
    }
    let safe: String = code
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    format!("LS-PT-{safe}")
}

/// Read a text input file in whatever encoding it was exported in (UTF-8,
/// UTF-16 with a BOM, or Windows-1252); see [`landsurvey::text::decode`].
fn read_text(path: impl AsRef<std::path::Path>) -> std::io::Result<String> {
    fs::read(path).map(|bytes| landsurvey::text::decode(&bytes))
}

/// Point-number text height scaled to the point set's 2-D extent, so labels
/// stay legible at zoom-extents whether the survey spans metres or kilometres
/// (MicroSurvey sizes labels to the drawing rather than to a fixed value).
fn label_height(points: &[pnezd::PnezdPoint]) -> f64 {
    let (mut minx, mut maxx, mut miny, mut maxy) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for p in points {
        minx = minx.min(p.easting);
        maxx = maxx.max(p.easting);
        miny = miny.min(p.northing);
        maxy = maxy.max(p.northing);
    }
    ((maxx - minx).max(maxy - miny) / 40.0).max(1.0)
}

/// Draw a point's label up-right of the mark: the point number always, plus an
/// elevation + description detail line below it when `show_detail` is set. Both
/// go on `layer`.
fn draw_point_label(
    batch: &mut Batch,
    p: &pnezd::PnezdPoint,
    h: f64,
    off: f64,
    layer: &str,
    show_detail: bool,
) {
    let x = p.easting + off;
    let y = p.northing + off;
    batch.add(
        EntityType::Text(Text::with_value(p.number.clone(), Vector3::new(x, y, 0.0)).with_height(h)),
        layer,
    );

    if !show_detail {
        return;
    }

    let mut detail = String::new();
    if p.elevation != 0.0 {
        detail.push_str(&format!("{:.2}", p.elevation));
    }
    if !p.description.is_empty() {
        if !detail.is_empty() {
            detail.push(' ');
        }
        detail.push_str(&p.description);
    }
    if !detail.is_empty() {
        batch.add(
            EntityType::Text(
                Text::with_value(detail, Vector3::new(x, y - h * 1.3, 0.0)).with_height(h * 0.75),
            ),
            layer,
        );
    }
}

/// `LS_INVERSE <N1> <E1> <N2> <E2>` — distance + bearing between two coords.
fn inverse(host: &mut dyn HostApi, cmd: &str) {
    const USAGE: &str = "Usage: LS_INVERSE <N1> <E1> <N2> <E2> [draw] [anim]";
    let toks: Vec<&str> = cmd.split_whitespace().skip(1).collect();
    if toks.is_empty() {
        host.push_info(USAGE);
        return;
    }
    // Strict: exactly 4 finite coordinates plus known keywords — a lenient
    // parse would let a typo shift the coordinates one slot left.
    let mut nums: Vec<f64> = Vec::with_capacity(4);
    for t in &toks {
        if t.eq_ignore_ascii_case("draw") || t.eq_ignore_ascii_case("anim") {
            continue;
        }
        match t.parse::<f64>() {
            Ok(v) if v.is_finite() => nums.push(v),
            _ => {
                host.push_error(&format!("LS_INVERSE: \"{t}\" is not a finite number. {USAGE}"));
                return;
            }
        }
    }
    if nums.len() != 4 {
        host.push_error(&format!(
            "LS_INVERSE: expected 4 coordinates, got {}. {USAGE}",
            nums.len()
        ));
        return;
    }
    let inv = cogo::inverse(nums[0], nums[1], nums[2], nums[3]);
    host.push_output(&format!(
        "LS_INVERSE: distance {:.4}, azimuth {:.4}\u{b0}, bearing {}",
        inv.distance,
        inv.azimuth_deg,
        cogo::azimuth_to_bearing(inv.azimuth_deg)
    ));
    // `LS_INVERSE <N1> <E1> <N2> <E2> draw` → draw the line with bearing +
    // distance labels positioned by the MSannotate convention.
    if cmd.split_whitespace().any(|t| t.eq_ignore_ascii_case("draw")) {
        // world mapping X = Easting, Y = Northing.
        draw_inverse_line(host, nums[1], nums[0], nums[3], nums[2], &inv);
    }
    // `LS_INVERSE <N1> <E1> <N2> <E2> anim` → export an animated-SVG explainer.
    if cmd.split_whitespace().any(|t| t.eq_ignore_ascii_case("anim")) {
        let svg = viz::inverse_anim_svg(nums[0], nums[1], nums[2], nums[3]);
        write_anim_file(host, "inverse", &svg);
    }
}

/// Draw a line with bearing + distance labels using the MicroSurvey MSannotate
/// convention (see vault note "Survey label positioning"): both labels at the
/// segment **midpoint**, offset perpendicular to opposite sides (bearing +perp,
/// distance −perp) by ~1 text-height, each rotated to the line and flipped 180°
/// when the line would otherwise read upside-down. For a standalone inverse
/// there is no parcel, so sides are simply left/right (the bearing-outside /
/// distance-inside refinement needs a parcel centroid).
fn draw_inverse_line(host: &mut dyn HostApi, e1: f64, n1: f64, e2: f64, n2: f64, inv: &cogo::Inverse) {
    const LAYER: &str = "LS-INVERSE";

    host.push_undo("LS_INVERSE draw");
    add_on_layer(
        host,
        EntityType::Line(Line::from_points(
            Vector3::new(e1, n1, 0.0),
            Vector3::new(e2, n2, 0.0),
        )),
        LAYER,
    );

    let (dx, dy) = (e2 - e1, n2 - n1);
    let (mx, my) = ((e1 + e2) / 2.0, (n1 + n2) / 2.0);
    let line_ang = dy.atan2(dx);

    // Rotation aligned to the line, flipped if it would read upside-down.
    let mut deg = line_ang.to_degrees().rem_euclid(360.0);
    if deg > 90.0 && deg <= 270.0 {
        deg = (deg + 180.0).rem_euclid(360.0);
    }
    let rot = deg.to_radians();

    // Text, layer, colour and size come from the current MicroSurvey
    // bearing/distance styles (LS_LABELSTYLE).
    let (styles, scale, imperial) = {
        let st = crate::state::state();
        (st.label_styles.clone(), st.label_scale, st.label_imperial)
    };
    let (bs, ds) = (styles.bearing(), styles.distance());
    // With a plot scale, size from leroy like MSCAD; without one, ~5% of the
    // line at leroy 60, scaled by each style's leroy.
    let height = |leroy: u32| match scale {
        Some(s) => label_style::leroy_height(leroy, s, !imperial),
        None => (inv.distance / 20.0).max(1.0) * leroy as f64 / 60.0,
    };

    // Work in the TEXT's own frame so spacing is symmetric regardless of the
    // readable-flip. Text is baseline-anchored; glyphs rise from the baseline
    // toward `up` and read along `read`. Put each label's CENTRE at ±(gap+h/2)
    // along `up` (so its body clears the line by `gap` on its side), then step
    // back to the baseline (−h/2 along `up`) and centre it along the line
    // (−half-width along `read`).
    let read = (rot.cos(), rot.sin());
    let up = (-rot.sin(), rot.cos());

    let labels = [
        // bearing, +up side
        (
            label_style::format_bearing(inv.azimuth_deg, bs),
            1.0_f64,
            height(bs.leroy),
            bs.textoffset,
            bs.layername.as_str(),
            bs.color,
        ),
        // distance, −up side
        (
            label_style::format_distance(inv.distance, ds),
            -1.0,
            height(ds.leroy),
            ds.textoffset,
            ds.layername.as_str(),
            ds.color,
        ),
    ];
    for (text, sign, h, offset, layer, color) in labels {
        // MsAnnotate's text offset is the clearance from the line in text heights.
        let d = offset * h + 0.5 * h;
        let half_w = 0.5 * text.chars().count() as f64 * h * 0.6;
        let bx = mx + (sign * d - 0.5 * h) * up.0 - half_w * read.0;
        let by = my + (sign * d - 0.5 * h) * up.1 - half_w * read.1;
        let mut ent = EntityType::Text(
            Text::with_value(text, Vector3::new(bx, by, 0.0))
                .with_height(h)
                .with_rotation(rot),
        );
        ent.common_mut().color = Color::from_index(color);
        add_on_layer(host, ent, layer);
    }

    host.bump_geometry();
    host.set_dirty();
}

/// `LS_LABELSTYLE` — show or change the MicroSurvey label styles used for
/// bearing/distance labels:
/// `LOAD <config.json | job.MSJ folder>`, `BEARING <n>` / `DISTANCE <n>`
/// (1-based style number), `SCALE <n>` / `SCALE OFF` (plot scale 1:n for
/// leroy sizing), `METRIC` / `IMPERIAL`, `RESET`.
fn label_style_cmd(host: &mut dyn HostApi, cmd: &str) {
    const USAGE: &str = "Usage: LS_LABELSTYLE [LOAD <config.json|job.MSJ>] [BEARING <n>] \
                         [DISTANCE <n>] [SCALE <n>|OFF] [METRIC|IMPERIAL] [RESET]";
    let toks: Vec<&str> = cmd.split_whitespace().skip(1).collect();
    let mut i = 0;
    while i < toks.len() {
        let key = toks[i].to_ascii_uppercase();
        let arg = toks.get(i + 1).copied();
        match key.as_str() {
            "LOAD" => {
                // Paths may contain spaces: LOAD takes the rest of the line.
                let rest = toks[i + 1..].join(" ");
                if rest.is_empty() {
                    host.push_error(USAGE);
                    return;
                }
                let mut path = std::path::PathBuf::from(rest.trim_matches('"'));
                if path.is_dir() {
                    path.push("config.json");
                }
                let loaded = fs::read_to_string(&path)
                    .map_err(|e| e.to_string())
                    .and_then(|s| LabelStyles::from_msj_config(&s));
                match loaded {
                    Ok(styles) => crate::state::state().label_styles = styles,
                    Err(e) => {
                        host.push_error(&format!(
                            "LS_LABELSTYLE: cannot load {}: {e}",
                            path.display()
                        ));
                        return;
                    }
                }
                i = toks.len();
                continue;
            }
            "BEARING" | "DISTANCE" => {
                let mut st = crate::state::state();
                let count = if key == "BEARING" {
                    st.label_styles.bearingstyles.len()
                } else {
                    st.label_styles.distancestyles.len()
                };
                let Some(n) = arg
                    .and_then(|a| a.parse::<usize>().ok())
                    .filter(|n| (1..=count).contains(n))
                else {
                    drop(st);
                    host.push_error(&format!(
                        "LS_LABELSTYLE: {key} takes a style number 1-{count}."
                    ));
                    return;
                };
                if key == "BEARING" {
                    st.label_styles.bearingstyleindex = n - 1;
                } else {
                    st.label_styles.distancestyleindex = n - 1;
                }
                i += 2;
                continue;
            }
            "SCALE" => {
                let scale = match arg {
                    Some(a) if a.eq_ignore_ascii_case("OFF") => None,
                    Some(a) => match a.trim_start_matches("1:").parse::<f64>() {
                        Ok(v) if v.is_finite() && v > 0.0 => Some(v),
                        _ => {
                            host.push_error("LS_LABELSTYLE: SCALE takes a positive number or OFF.");
                            return;
                        }
                    },
                    None => {
                        host.push_error(USAGE);
                        return;
                    }
                };
                crate::state::state().label_scale = scale;
                i += 2;
                continue;
            }
            "METRIC" => crate::state::state().label_imperial = false,
            "IMPERIAL" => crate::state::state().label_imperial = true,
            "RESET" => {
                let mut st = crate::state::state();
                st.label_styles = LabelStyles::default();
                st.label_scale = None;
                st.label_imperial = false;
            }
            _ => {
                host.push_error(&format!("LS_LABELSTYLE: unknown option \"{}\". {USAGE}", toks[i]));
                return;
            }
        }
        i += 1;
    }

    let st = crate::state::state();
    let (b, d) = (st.label_styles.bearing(), st.label_styles.distance());
    let size = match st.label_scale {
        Some(s) => format!(
            "plot scale 1:{s} ({})",
            if st.label_imperial { "imperial" } else { "metric" }
        ),
        None => "sized to the line (no plot scale)".to_string(),
    };
    let msg = format!(
        "LS_LABELSTYLE: bearing {} (layer {}, leroy {}, rounding {}\"), \
         distance {} (layer {}, leroy {}, {} dp); {size}. Example: {}  {}",
        b.name,
        b.layername,
        b.leroy,
        b.rounding,
        d.name,
        d.layername,
        d.leroy,
        d.decimalplaces,
        label_style::format_bearing(88.0 + 16.0 / 60.0 + 15.0 / 3600.0, b),
        label_style::format_distance(36.0, d),
    );
    drop(st);
    host.push_output(&msg);
}

/// Write an animated-SVG explainer to a temp folder and report the path so the
/// user can open it in a browser (OCS can't play animation in the canvas).
fn write_anim_file(host: &mut dyn HostApi, op: &str, svg: &str) {
    let mut path = std::env::temp_dir();
    path.push("landsurvey-anim");
    let _ = std::fs::create_dir_all(&path);
    path.push(format!("{op}.svg"));
    match std::fs::write(&path, svg) {
        Ok(_) => host.push_output(&format!(
            "LS animation -> {} (open in a browser)",
            path.display()
        )),
        Err(e) => host.push_error(&format!("LS animation: cannot write SVG: {e}")),
    }
}

/// `LS_SURFACE <points.csv>` — build a TIN from a PNEZD file and draw its
/// triangulation as `Line` entities on layer `LS-TIN-<NAME>` (tagged with
/// `LANDSURVEY_SURFACE` XDATA). Reports point/triangle counts and plan area.
/// Wired to the ribbon via a native file picker.
fn build_surface(host: &mut dyn HostApi, cmd: &str) {
    let arg = first_arg(cmd);
    if arg.is_empty() {
        host.push_info("Usage: LS_SURFACE <points.csv>  (PNEZD point file)");
        return;
    }
    let surf = match read_surface(host, arg) {
        Some(s) => s,
        None => return,
    };
    let name = layer_stem(arg);
    let layer = format!("LS-TIN-{name}");

    host.push_undo("LS_SURFACE");
    let edges = draw_tin(host, &surf, &layer, &name);
    host.bump_geometry();
    host.set_dirty();

    let (npts, ntri, area) = (surf.nodes.len(), surf.triangles.len(), surf.area_2d());
    // Retain the surface so LS_VOLUME / LS_DATUM can use it by name later.
    {
        let mut st = crate::state::state();
        st.put_surface(&name, surf);
    }
    host.push_output(&format!(
        "LS_SURFACE: stored \"{name}\" — {npts} pts, {ntri} triangles, {edges} TIN edges on \
         layer {layer}; plan area {area:.3}. (Use it in LS_VOLUME / LS_DATUM by name.)"
    ));
}

/// LandXML content kinds, for the `LS_LANDXML` / `LS_LANDXMLOUT` filters.
/// `alignments` covers their profiles and cross sections.
const LANDXML_KINDS: [&str; 5] = ["points", "parcels", "features", "surfaces", "alignments"];

/// Split trailing keywords (case-insensitive, any order) off a command's
/// argument string; the remainder is a path, which may contain spaces.
fn peel_keywords<'a>(rest: &'a str, keywords: &[&str]) -> (&'a str, Vec<String>) {
    let mut path = rest.trim();
    let mut found = Vec::new();
    while let Some(idx) = path.rfind(char::is_whitespace) {
        let word = path[idx + 1..].to_ascii_lowercase();
        if !keywords.contains(&word.as_str()) {
            break;
        }
        found.push(word);
        path = path[..idx].trim_end();
    }
    (path.trim_matches('"'), found)
}

/// `LS_LANDXML <file.xml> [points] [parcels] [features] [surfaces]
/// [alignments] [preview]` (and the host's `LANDXMLIMPORT` verb, OpenCADStudio
/// #157) — import LandXML COGO points (as Land Survey points, like
/// `LS_PNEZD`), parcels (closed LWPolylines on `LS-PARCEL`, labelled with name
/// and area on `LS-PARCEL-LABEL`), plan features (LWPolylines on
/// `LS-FEATURE`), TIN surfaces (`Mesh` on `LS-TIN-<name>`, kept for
/// `LS_VOLUME` / `LS_DATUM`) and alignments (stationed centrelines, with
/// profile and cross-section views drawn below the plan). Naming kinds
/// imports only those; `preview` reports without drawing. Each parcel's area
/// computed from its lines and arcs is checked against the file's `area`.
fn import_landxml(host: &mut dyn HostApi, cmd: &str) {
    const USAGE: &str = "Usage: LS_LANDXML <file.xml> [points] [parcels] [features] \
                         [surfaces] [alignments] [preview]";
    let mut keywords = LANDXML_KINDS.to_vec();
    keywords.push("preview");
    let (path, flags) = peel_keywords(first_arg(cmd), &keywords);
    if path.is_empty() {
        host.push_info(USAGE);
        return;
    }
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            host.push_error(&format!("LS_LANDXML: cannot read \"{path}\": {e}"));
            return;
        }
    };
    if !landxml::looks_like_landxml(&text) {
        host.push_error(&format!("LS_LANDXML: \"{path}\" does not look like LandXML."));
        return;
    }
    let preview = flags.iter().any(|f| f == "preview");
    let any_kind = flags.iter().any(|f| f != "preview");
    let want = |k: &str| !any_kind || flags.iter().any(|f| f == k);

    let mut doc = landxml::read_document(&text);
    if !want("points") {
        doc.points.clear();
    }
    if !want("parcels") {
        doc.parcels.clear();
    }
    if !want("features") {
        doc.features.clear();
    }
    if !want("surfaces") {
        doc.surfaces.clear();
    }
    if !want("alignments") {
        doc.alignments.clear();
    }
    if doc.points.is_empty()
        && doc.parcels.is_empty()
        && doc.features.is_empty()
        && doc.surfaces.is_empty()
        && doc.alignments.is_empty()
    {
        host.push_error(&format!(
            "LS_LANDXML: no COGO points, parcels, plan features, TIN surfaces or alignments \
             found in \"{path}\"."
        ));
        return;
    }

    // Parcels whose lines and arcs don't enclose the area the file declares.
    let area_mismatch: Vec<String> = doc
        .parcels
        .iter()
        .filter_map(|p| {
            let declared = p.area?;
            let got = landxml::chain_area(&p.segments);
            ((got - declared).abs() > 1e-4 * declared.abs().max(1.0))
                .then(|| format!("{} ({got:.3} vs {declared:.3})", p.name))
        })
        .collect();
    let units = match &doc.units {
        Some(u) => format!(
            "{} ({})",
            if u.linear_unit.is_empty() { "?" } else { &u.linear_unit },
            if u.metric { "metric" } else { "imperial" }
        ),
        None => "not declared".to_string(),
    };
    let summary = format!(
        "{} point(s), {} parcel(s), {} plan feature(s), {} surface(s), {} alignment(s) \
         ({} profile(s), {} cross section(s)); units {units}",
        doc.points.len(),
        doc.parcels.len(),
        doc.features.len(),
        doc.surfaces.len(),
        doc.alignments.len(),
        doc.alignments.iter().map(|a| a.profiles.len()).sum::<usize>(),
        doc.alignments.iter().map(|a| a.cross_sections.len()).sum::<usize>(),
    );

    if preview {
        host.push_output(&format!("LS_LANDXML preview \"{path}\": {summary}."));
    } else {
        host.push_undo("LS_LANDXML import");
        import_landxml_points(host, &doc.points);
        import_landxml_parcels(host, &doc.parcels, doc.units.as_ref());
        for f in &doc.features {
            for (verts, closed) in landxml::chain_polylines(&f.segments) {
                let ent = EntityType::LwPolyline(lwpolyline(&verts, closed));
                tag_named(host, ent, "LS-FEATURE", XDATA_FEATURE, &f.name, &f.desc);
            }
        }
        if !doc.alignments.is_empty() {
            // Views go below everything this import put in plan.
            let mut plan = None;
            for p in &doc.points {
                landxml_draw::bbox_add(&mut plan, [p.easting, p.northing]);
            }
            let chains = doc.parcels.iter().map(|p| &p.segments).chain(doc.features.iter().map(|f| &f.segments));
            for s in chains.flatten() {
                landxml_draw::bbox_add(&mut plan, s.start());
                landxml_draw::bbox_add(&mut plan, s.end());
            }
            let metric = doc.units.as_ref().map_or(true, |u| u.metric);
            landxml_draw::draw_alignments(host, &doc.alignments, metric, plan);
        }
        for ns in &doc.surfaces {
            let layer = format!("LS-TIN-{}", ns.name);
            let mesh = mesh_from_surface(&ns.surface);
            tag_surface(host, EntityType::Mesh(mesh), &layer, &ns.name, "TIN");
            // Retain for LS_VOLUME / LS_DATUM by name (same as LS_SURFACE).
            crate::state::state().put_surface(&ns.name, ns.surface.clone());
        }
        // Re-export in the units the data came in.
        crate::state::state().landxml_units = doc.units.clone();
        host.bump_geometry();
        host.set_dirty();
        let names: Vec<&str> = doc.surfaces.iter().map(|s| s.name.as_str()).collect();
        let tins = if names.is_empty() {
            String::new()
        } else {
            format!(" Surfaces [{}] are available to LS_VOLUME / LS_DATUM.", names.join(", "))
        };
        host.push_output(&format!("LS_LANDXML: imported {summary}.{tins}"));
    }

    if area_mismatch.is_empty() {
        if doc.parcels.iter().any(|p| p.area.is_some()) {
            host.push_output("LS_LANDXML: every parcel's computed area matches the file.");
        }
    } else {
        host.push_error(&format!(
            "LS_LANDXML: {} parcel area(s) differ from the file: {}",
            area_mismatch.len(),
            area_mismatch.join("; ")
        ));
    }
    for w in doc.warnings.iter().take(10) {
        host.push_info(&format!("LS_LANDXML: {w}"));
    }
    if doc.warnings.len() > 10 {
        host.push_info(&format!("LS_LANDXML: … and {} more warning(s)", doc.warnings.len() - 10));
    }
}

/// LandXML COGO points become ordinary Land Survey points. The description
/// leads with the feature code so `LS-PT-<CODE>` layering still works.
/// `LS_RW5 <field.raw|rw5> [detail] [preview]` — reduce a FieldGenius / TDS /
/// SurvCE raw data file to coordinates and import the points like `LS_PNEZD`
/// (labels, `LS-PT-<CODE>` layers, `LANDSURVEY_POINT` XDATA). Each setup's
/// QC is reported: the station coordinates the collector stored against ours,
/// the backsight distance and elevation checks, and repeated shots of a
/// point (check shots, face 2). `preview` reports without drawing.
fn import_rw5(host: &mut dyn HostApi, cmd: &str) {
    const USAGE: &str = "Usage: LS_RW5 <field.raw|rw5> [detail] [preview]";
    let (path, flags) = peel_keywords(first_arg(cmd), &["detail", "preview"]);
    if path.is_empty() {
        host.push_info(USAGE);
        return;
    }
    let text = match read_text(path) {
        Ok(t) => t,
        Err(e) => {
            host.push_error(&format!("LS_RW5: cannot read \"{path}\": {e}"));
            return;
        }
    };
    let r = rw5::reduce(&text);
    if r.points.is_empty() {
        host.push_error(&format!(
            "LS_RW5: no points reduced from \"{path}\". {}",
            r.warnings.join("; ")
        ));
        return;
    }
    let unit = match r.unit {
        rw5::Unit::Metre => "m",
        rw5::Unit::Feet => "ft",
        rw5::Unit::UsSurveyFoot => "US ft",
    };
    let preview = flags.iter().any(|f| f == "preview");
    if !preview {
        let points: Vec<pnezd::PnezdPoint> = r
            .points
            .iter()
            .map(|p| pnezd::PnezdPoint {
                number: p.name.clone(),
                northing: p.northing,
                easting: p.easting,
                elevation: p.elevation,
                description: p.description.clone(),
            })
            .collect();
        host.push_undo("LS_RW5 import");
        draw_survey_points(host, &points, flags.iter().any(|f| f == "detail"));
        host.bump_geometry();
        host.set_dirty();
    }
    host.push_output(&format!(
        "LS_RW5{}: job {} - {} point(s) from {} setup(s), units {unit}.",
        if preview { " preview" } else { "" },
        if r.job.is_empty() { "?" } else { &r.job },
        r.points.len(),
        r.setups.len()
    ));
    for s in &r.setups {
        let mut line = format!(
            "  setup {} -> BS {}",
            s.station,
            s.backsight.as_deref().unwrap_or("-")
        );
        if let Some(d) = s.stored_delta {
            line.push_str(&format!(
                "  stored vs ours dN {} dE {} dZ {}",
                rw5::fmt_delta(d[0]),
                rw5::fmt_delta(d[1]),
                rw5::fmt_delta(d[2])
            ));
        }
        if let (Some(h), Some(z)) = (s.bs_distance_error, s.bs_elevation_error) {
            line.push_str(&format!(
                "  BS check dHD {} dZ {}",
                rw5::fmt_delta(h),
                rw5::fmt_delta(z)
            ));
        }
        host.push_output(&line);
    }
    for rep in &r.repeats {
        host.push_output(&format!(
            "  repeat of {}: dH {:.4} dZ {} (first observation kept)",
            rep.name,
            rep.dh,
            rw5::fmt_delta(rep.dz)
        ));
    }
    for w in &r.warnings {
        host.push_info(&format!("LS_RW5: {w}"));
    }
}

fn import_landxml_points(host: &mut dyn HostApi, points: &[landxml::CgPoint]) {
    if points.is_empty() {
        return;
    }
    let pts: Vec<pnezd::PnezdPoint> = points
        .iter()
        .map(|p| {
            let description = if p.desc.is_empty() {
                p.code.clone()
            } else if p.code.is_empty() || p.desc.split_whitespace().next() == Some(p.code.as_str()) {
                p.desc.clone()
            } else {
                format!("{} {}", p.code, p.desc)
            };
            pnezd::PnezdPoint {
                number: p.name.clone(),
                northing: p.northing,
                easting: p.easting,
                elevation: p.elevation.unwrap_or(0.0),
                description,
            }
        })
        .collect();
    draw_survey_points(host, &pts, false);
}

/// Parcels become closed LWPolylines (arcs as bulges) on `LS-PARCEL`, tagged
/// `LANDSURVEY_PARCEL [name, desc]`, labelled on `LS-PARCEL-LABEL` with the
/// name and area at the parcel's label point (or its area centroid).
fn import_landxml_parcels(
    host: &mut dyn HostApi,
    parcels: &[landxml::Parcel],
    units: Option<&landxml::Units>,
) {
    // One text height for every lot: ~1/80 of the subdivision's extent.
    let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
    for s in parcels.iter().flat_map(|p| &p.segments) {
        for q in [s.start(), s.end()] {
            lo = [lo[0].min(q[0]), lo[1].min(q[1])];
            hi = [hi[0].max(q[0]), hi[1].max(q[1])];
        }
    }
    let h = ((hi[0] - lo[0]).max(hi[1] - lo[1]) / 80.0).max(0.1);

    for p in parcels {
        for (verts, closed) in landxml::chain_polylines(&p.segments) {
            let ent = EntityType::LwPolyline(lwpolyline(&verts, closed));
            tag_named(host, ent, "LS-PARCEL", XDATA_PARCEL, &p.name, &p.desc);
        }
        if p.name.is_empty() {
            continue;
        }
        if let Some(at) = p.center.or_else(|| landxml_draw::chain_centroid(&p.segments)) {
            let area = p.area.unwrap_or_else(|| landxml::chain_area(&p.segments));
            let area = landxml_draw::area_text(area, units);
            landxml_draw::draw_lot_label(host, XDATA_PARCEL, &p.name, &p.desc, &area, at, h);
        }
    }
}

fn lwpolyline(verts: &[(f64, f64, f64)], closed: bool) -> LwPolyline {
    let mut lw = LwPolyline::new();
    for &(x, y, bulge) in verts {
        lw.add_point_with_bulge(Vector2::new(x, y), bulge);
    }
    lw.is_closed = closed;
    lw
}

/// Add `ent` on `layer`, tagged with an `app` record `[name, desc]`.
fn tag_named(
    host: &mut dyn HostApi,
    mut ent: EntityType,
    layer: &str,
    app: &str,
    name: &str,
    desc: &str,
) {
    ent.common_mut().layer = layer.to_string();
    let handle = host.add_entity(ent);
    let mut rec = ExtendedDataRecord::new(app);
    rec.add_value(XDataValue::String(name.to_string()));
    rec.add_value(XDataValue::String(desc.to_string()));
    host.write_record(handle, rec);
}

/// The `[name, desc]` strings of an `app` record on an entity.
fn named_tag(e: &EntityType, app: &str) -> Option<(String, String)> {
    let rec = e.common().extended_data.get_record(app)?;
    let mut strs = rec.values.iter().filter_map(|v| match v {
        XDataValue::String(s) => Some(s.clone()),
        _ => None,
    });
    let name = strs.next()?;
    Some((name, strs.next().unwrap_or_default()))
}

/// `LS_LANDXMLOUT <file.xml> [points] [parcels] [features] [surfaces]
/// [metric|imperial]` — write the drawing's Land Survey points, parcels, plan
/// features and named TIN surfaces as LandXML 1.2.
///
/// Parcels are closed LWPolylines tagged by `LS_LANDXML`, plus any closed
/// LWPolyline on a layer whose name contains PARCEL, LOT, PROPERTY or BOUNDARY
/// (named `Parcel <n>`). Plan features are the LWPolylines `LS_LANDXML` tagged.
/// Units default to those of the last LandXML import, else metric.
fn export_landxml(host: &mut dyn HostApi, cmd: &str) {
    const USAGE: &str = "Usage: LS_LANDXMLOUT <file.xml> [points] [parcels] [features] \
                         [surfaces] [alignments] [metric|imperial]";
    let mut keywords = LANDXML_KINDS.to_vec();
    keywords.extend(["metric", "imperial"]);
    let (path, flags) = peel_keywords(first_arg(cmd), &keywords);
    if path.is_empty() {
        host.push_info(USAGE);
        return;
    }
    let any_kind = flags.iter().any(|f| LANDXML_KINDS.contains(&f.as_str()));
    let want = |k: &str| !any_kind || flags.iter().any(|f| f == k);

    let mut doc = landxml::LandXmlDoc {
        units: crate::state::state().landxml_units.clone(),
        ..Default::default()
    };
    if flags.iter().any(|f| f == "metric") {
        doc.units = None; // the writer's default
    } else if flags.iter().any(|f| f == "imperial") {
        doc.units = Some(landxml::Units {
            metric: false,
            linear_unit: "USSurveyFoot".to_string(),
        });
    }

    let mut unnamed = 0usize;
    for e in host.document().entities() {
        match e {
            EntityType::Point(p) if want("points") => {
                if let Some((number, description)) = named_tag(e, XDATA_POINT) {
                    let code = description.split_whitespace().next().unwrap_or("").to_string();
                    // A bare code is not repeated as the description.
                    let desc = if description == code { String::new() } else { description };
                    doc.points.push(landxml::CgPoint {
                        name: number,
                        northing: p.location.y,
                        easting: p.location.x,
                        elevation: Some(p.location.z),
                        code,
                        desc,
                    });
                }
            }
            EntityType::LwPolyline(pl) => {
                let verts: Vec<(f64, f64, f64)> = pl
                    .vertices
                    .iter()
                    .map(|v| (v.location.x, v.location.y, v.bulge))
                    .collect();
                if let Some((name, desc)) = named_tag(e, XDATA_FEATURE) {
                    if want("features") {
                        doc.features.push(landxml::PlanFeature {
                            name,
                            desc,
                            segments: landxml::polyline_segments(&verts, pl.is_closed),
                        });
                    }
                    continue;
                }
                let alignment_part = [
                    landxml_draw::XDATA_ALIGN,
                    landxml_draw::XDATA_PROFILE,
                    landxml_draw::XDATA_XSECT,
                ]
                .iter()
                .any(|app| e.common().extended_data.get_record(app).is_some());
                if alignment_part || !want("parcels") || !pl.is_closed || verts.len() < 3 {
                    continue;
                }
                let (name, desc) = match named_tag(e, XDATA_PARCEL) {
                    Some(t) => t,
                    None => {
                        let layer = e.common().layer.to_ascii_uppercase();
                        if !["PARCEL", "LOT", "PROPERTY", "BOUNDARY"]
                            .iter()
                            .any(|k| layer.contains(k))
                        {
                            continue;
                        }
                        unnamed += 1;
                        (format!("Parcel {unnamed}"), String::new())
                    }
                };
                doc.parcels.push(landxml::Parcel {
                    name,
                    desc,
                    area: None, // computed from the boundary by the writer
                    center: None,
                    segments: landxml::polyline_segments(&verts, true),
                });
            }
            _ => {}
        }
    }
    // Lot label points: where each parcel's name label now sits, else the
    // parcel's area centroid.
    let mut labels: Vec<(String, [f64; 2])> = Vec::new();
    for e in host.document().entities() {
        if let EntityType::Text(t) = e {
            if let Some((name, _)) = named_tag(e, XDATA_PARCEL) {
                labels.push((name, landxml_draw::lot_label_center(t)));
            }
        }
    }
    for p in &mut doc.parcels {
        p.center = labels
            .iter()
            .find(|(n, _)| *n == p.name)
            .map(|(_, c)| *c)
            .or_else(|| landxml_draw::chain_centroid(&p.segments));
    }
    if want("alignments") {
        doc.alignments = landxml_draw::collect_alignments(host.document());
    }
    if want("surfaces") {
        for name in known_surface_names(host) {
            if let Ok(surface) = resolve_named_surface(host, &name) {
                doc.surfaces.push(landxml::NamedSurface { name, surface });
            }
        }
    }
    if doc.points.is_empty()
        && doc.parcels.is_empty()
        && doc.features.is_empty()
        && doc.surfaces.is_empty()
        && doc.alignments.is_empty()
    {
        host.push_error(
            "LS_LANDXMLOUT: nothing to export — no Land Survey points, parcels, \
             plan features, named surfaces or alignments in the drawing.",
        );
        return;
    }

    let (date, time) = utc_now();
    let xml = landxml::write_document(&doc, &date, &time);
    match fs::write(path, xml) {
        Ok(()) => host.push_output(&format!(
            "LS_LANDXMLOUT: wrote {} point(s), {} parcel(s), {} plan feature(s), \
             {} surface(s), {} alignment(s) to \"{path}\" ({}).",
            doc.points.len(),
            doc.parcels.len(),
            doc.features.len(),
            doc.surfaces.len(),
            doc.alignments.len(),
            match &doc.units {
                Some(u) if !u.metric => "imperial",
                _ => "metric",
            }
        )),
        Err(e) => host.push_error(&format!("LS_LANDXMLOUT: cannot write \"{path}\": {e}")),
    }
}

/// Current UTC date and time as `YYYY-MM-DD`, `HH:MM:SS`.
fn utc_now() -> (String, String) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (
        format!("{y:04}-{m:02}-{d:02}"),
        format!("{:02}:{:02}:{:02}", rem / 3600, rem % 3600 / 60, rem % 60),
    )
}

/// Build an `acadrust` `Mesh` from an engine `Surface`: nodes are `[E, N, Z]`
/// (= world `[X, Y, Z]`), triangles index those nodes.
fn mesh_from_surface(surf: &Surface) -> Mesh {
    let verts: Vec<Vector3> =
        surf.nodes.iter().map(|n| Vector3::new(n[0], n[1], n[2])).collect();
    let tris: Vec<(usize, usize, usize)> =
        surf.triangles.iter().map(|t| (t[0], t[1], t[2])).collect();
    Mesh::from_triangles(verts, &tris)
}

/// Inverse of [`mesh_from_surface`]: rebuild an engine `Surface` from a `Mesh`
/// entity's vertices and triangular faces (non-triangles and out-of-range
/// indices are skipped rather than trusted).
fn surface_from_mesh(mesh: &Mesh) -> Surface {
    let nodes: Vec<[f64; 3]> = mesh.vertices.iter().map(|v| [v.x, v.y, v.z]).collect();
    let triangles: Vec<[usize; 3]> = mesh
        .faces
        .iter()
        .filter_map(|f| match f.vertices[..] {
            [a, b, c] if a < nodes.len() && b < nodes.len() && c < nodes.len() => {
                Some([a, b, c])
            }
            _ => None,
        })
        .collect();
    Surface { nodes, triangles }
}

/// `LS_DATUM <surface> <elevation>` — cut/fill of one surface against a
/// horizontal plane. Draws the TIN, the datum contour, and a result label.
fn datum_volume(host: &mut dyn HostApi, cmd: &str) {
    let args: Vec<&str> = cmd.split_whitespace().skip(1).collect();
    if args.len() < 2 {
        host.push_info("Usage: LS_DATUM <surface.csv|xml> <elevation>");
        return;
    }
    let elev: f64 = match args[1].parse() {
        Ok(v) => v,
        Err(_) => {
            host.push_info(&format!("LS_DATUM: elevation must be a number, got \"{}\"", args[1]));
            return;
        }
    };
    let surf = match resolve_named_surface(host, args[0]) {
        Ok(s) => s,
        Err(e) => {
            let avail = known_surface_names(host);
            host.push_error(&format!(
                "LS_DATUM: no surface \"{}\" ({e}). Known surfaces: [{}].",
                args[0],
                avail.join(", ")
            ));
            return;
        }
    };

    let (cf, contour) = surf.cut_fill_to_datum_detailed(elev);
    host.push_output(&format!(
        "LS_DATUM (vs elev {elev}): cut {:.3} (above), fill {:.3} (below), net {:.3} \
         [{} pts, {} triangles].",
        cf.cut,
        cf.fill,
        cf.net,
        surf.nodes.len(),
        surf.triangles.len()
    ));

    host.push_undo("LS_DATUM draw");
    let nt = draw_tin(host, &surf, "LS-TIN-DATUM", "DATUM");
    let mut batch = Batch::new();
    for s in &contour {
        let ent = EntityType::Line(Line::from_points(
            Vector3::new(s[0][0], s[0][1], elev),
            Vector3::new(s[1][0], s[1][1], elev),
        ));
        batch.add_tagged(ent, "LS-DATUM", surface_record("DATUM", "CONTOUR"));
    }
    let nl = batch.commit(host);
    let (minx, maxx, miny, maxy) = surf.extent();
    let height = ((maxx - minx).max(maxy - miny) / 50.0).max(1.0);
    let ent = EntityType::Text(
        Text::with_value(
            format!("ELEV {elev}  CUT {:.2}  FILL {:.2}  NET {:.2}", cf.cut, cf.fill, cf.net),
            Vector3::new((minx + maxx) / 2.0, (miny + maxy) / 2.0, 0.0),
        )
        .with_height(height),
    );
    tag_surface(host, ent, "LS-VOLUME-LABEL", "DATUM", "LABEL");
    host.bump_geometry();
    host.set_dirty();
    host.push_output(&format!(
        "LS_DATUM: drew TIN ({nt} edges), datum contour ({nl} seg), result label."
    ));
}

/// `LS_VOLUME <top.csv> <bottom.csv> [grid_step] [draw]` — earthwork volume
/// between two PNEZD point surfaces. Reports the exact TIN-overlay cut/fill/net
/// and, when a `grid_step` is given, the grid (column) method too. Add the
/// `draw` keyword to draw both TINs, the cut/fill boundary line, and a result
/// label. File-based and command-line-driven so the same two point files can be
/// fed to MicroSurvey / Civil 3D for a ground-truth cross-check. (Paths must not
/// contain spaces.)
fn volume(host: &mut dyn HostApi, cmd: &str) {
    // Tokens: surface names (or file paths) are positional; a bare positive
    // number is the grid step; the literal "draw" turns on drawing.
    let mut names: Vec<String> = Vec::new();
    let mut grid_step: Option<f64> = None;
    let mut draw = false;
    for a in cmd.split_whitespace().skip(1) {
        if a.eq_ignore_ascii_case("draw") {
            draw = true;
        } else if let Ok(v) = a.parse::<f64>() {
            if v > 0.0 {
                grid_step = Some(v);
            }
        } else {
            names.push(a.to_string());
        }
    }

    // Default to the surfaces named "top" and "bottom" built this session.
    let (top_tok, bot_tok) = match names.as_slice() {
        [] => ("top".to_string(), "bottom".to_string()),
        [_one] => {
            host.push_info(
                "Usage: LS_VOLUME [<top> <bottom>] [grid_step] [draw]  — names of \
                 imported surfaces (or file paths); defaults to top/bottom.",
            );
            return;
        }
        [a, b, ..] => (a.clone(), b.clone()),
    };

    let top = match resolve_named_surface(host, &top_tok) {
        Ok(s) => s,
        Err(e) => {
            let avail = known_surface_names(host);
            host.push_error(&format!(
                "LS_VOLUME: no surface \"{top_tok}\" ({e}). Known surfaces: [{}]. \
                 Build Surface first, then click Volume.",
                avail.join(", ")
            ));
            return;
        }
    };
    let bottom = match resolve_named_surface(host, &bot_tok) {
        Ok(s) => s,
        Err(e) => {
            let avail = known_surface_names(host);
            host.push_error(&format!(
                "LS_VOLUME: no surface \"{bot_tok}\" ({e}). Known surfaces: [{}].",
                avail.join(", ")
            ));
            return;
        }
    };

    host.push_output(&format!("LS_VOLUME: {top_tok} (top) minus {bot_tok} (bottom)."));
    let detailed = surface::composite_cut_fill_detailed(&top, &bottom);
    let cf = detailed.cut_fill;
    host.push_output(&format!(
        "LS_VOLUME (exact TIN overlay): cut {:.3}, fill {:.3}, net {:.3} \
         [top {} pts, bottom {} pts].",
        cf.cut,
        cf.fill,
        cf.net,
        top.nodes.len(),
        bottom.nodes.len()
    ));

    if let Some(step) = grid_step {
        let g = surface::grid_cut_fill(&top, &bottom, step);
        host.push_output(&format!(
            "LS_VOLUME (grid @ {:.3}): cut {:.3}, fill {:.3}, net {:.3}, \
             plan area {:.3}, {} cells.",
            step, g.cut, g.fill, g.net, g.plan_area, g.n_cells
        ));
    }

    if draw {
        host.push_undo("LS_VOLUME draw");
        let nt = draw_tin(host, &top, "LS-TIN-TOP", "TOP");
        let nb = draw_tin(host, &bottom, "LS-TIN-BOTTOM", "BOTTOM");
        let nl = draw_segments(host, &detailed.cutfill_line, "LS-CUTFILL", "CUTFILL");
        draw_volume_label(host, &top, &cf);
        host.bump_geometry();
        host.set_dirty();
        host.push_output(&format!(
            "LS_VOLUME: drew TOP TIN ({nt} edges), BOTTOM TIN ({nb} edges), \
             cut/fill line ({nl} seg), result label."
        ));
    }
}

/// Read a surface from a file, auto-detecting LandXML (a TIN with explicit
/// faces) vs a PNEZD point file (triangulated here). Nodes map X=Easting,
/// Y=Northing, Z=Elevation. Pure — returns `Err(message)` rather than touching
/// the host, so it composes into both the reporting and resolving paths.
fn read_surface_quiet(path: &str) -> Result<Surface, String> {
    let text = read_text(path).map_err(|e| format!("cannot read \"{path}\": {e}"))?;
    if landxml::looks_like_landxml(&text) {
        return landxml::read_first_surface(&text)
            .map(|ns| ns.surface)
            .ok_or_else(|| format!("no TIN surface in LandXML \"{path}\""));
    }
    let outcome = pnezd::parse(&text);
    if outcome.points.len() < 3 {
        return Err(format!(
            "\"{path}\" has {} usable point(s); need at least 3 for a surface",
            outcome.points.len()
        ));
    }
    let nodes: Vec<[f64; 3]> = outcome
        .points
        .iter()
        .map(|p| [p.easting, p.northing, p.elevation])
        .collect();
    Ok(Surface::from_points(&nodes))
}

/// Read a surface from a file, reporting any error to the host console.
fn read_surface(host: &mut dyn HostApi, path: &str) -> Option<Surface> {
    match read_surface_quiet(path) {
        Ok(s) => Some(s),
        Err(e) => {
            host.push_error(&format!("LandSurvey: {e}"));
            None
        }
    }
}

/// Resolve a token to a surface: first a surface built/imported this session
/// (by name, case-insensitive), then tagged geometry already in the drawing
/// (so names survive DWG save/reopen — see [`find_surface_in_document`]),
/// otherwise a file path. Lets commands operate on already-imported surfaces
/// without re-entering coordinates.
fn resolve_named_surface(host: &mut dyn HostApi, token: &str) -> Result<Surface, String> {
    {
        let st = crate::state::state();
        if let Some(s) = st.get_surface(token) {
            return Ok(s.clone());
        }
    }
    if let Some((name, surf)) = find_surface_in_document(host.document(), token) {
        // Cache under the canonical tagged name so later commands skip the scan.
        let mut st = crate::state::state();
        st.put_surface(&name, surf.clone());
        return Ok(surf);
    }
    read_surface_quiet(token)
}

/// The `LANDSURVEY_SURFACE` tag on an entity, as `(name, kind)`, if present.
fn surface_tag(e: &EntityType) -> Option<(&str, &str)> {
    let rec = e.common().extended_data.get_record(XDATA_SURFACE)?;
    let mut strs = rec.values.iter().filter_map(|v| match v {
        XDataValue::String(s) => Some(s.as_str()),
        _ => None,
    });
    Some((strs.next()?, strs.next()?))
}

/// Rebuild the surface named `token` from tagged geometry in the drawing.
/// Returns the canonical tagged name with the surface.
///
/// Two sources, matching what the import paths draw:
/// * a `Mesh` tagged `[name, "TIN"]` (`LS_LANDXML`) — vertices/faces are the
///   exact surface geometry;
/// * TIN-edge `Line`s tagged `[name, "TIN"]` (`LS_SURFACE` / `draw_tin`) — the
///   unique endpoints re-triangulated with [`Surface::from_points`], the same
///   Delaunay builder that produced them, which reproduces the original TIN.
fn find_surface_in_document(doc: &codec::CadDocument, token: &str) -> Option<(String, Surface)> {
    // Pass 1: exact geometry from a tagged Mesh.
    for e in doc.entities() {
        let EntityType::Mesh(m) = e else { continue };
        match surface_tag(e) {
            Some((name, "TIN")) if name.eq_ignore_ascii_case(token) => {
                let surf = surface_from_mesh(m);
                if !surf.nodes.is_empty() && !surf.triangles.is_empty() {
                    return Some((name.to_string(), surf));
                }
            }
            _ => {}
        }
    }

    // Pass 2: unique endpoints of the tagged TIN edges.
    let mut canonical: Option<String> = None;
    let mut seen: std::collections::HashSet<[u64; 3]> = std::collections::HashSet::new();
    let mut nodes: Vec<[f64; 3]> = Vec::new();
    for e in doc.entities() {
        let EntityType::Line(l) = e else { continue };
        match surface_tag(e) {
            Some((name, "TIN")) if name.eq_ignore_ascii_case(token) => {
                canonical.get_or_insert_with(|| name.to_string());
                for p in [l.start, l.end] {
                    // Endpoints repeat bit-exactly across shared edges.
                    if seen.insert([p.x.to_bits(), p.y.to_bits(), p.z.to_bits()]) {
                        nodes.push([p.x, p.y, p.z]);
                    }
                }
            }
            _ => {}
        }
    }
    if nodes.len() >= 3 {
        let surf = Surface::from_points(&nodes);
        if !surf.triangles.is_empty() {
            return Some((canonical.unwrap_or_else(|| token.to_string()), surf));
        }
    }
    None
}

/// Names of surfaces available by name — those stored this session plus those
/// recoverable from tagged drawing geometry — for "did you mean" messages.
fn known_surface_names(host: &mut dyn HostApi) -> Vec<String> {
    let mut names: Vec<String> = {
        let st = crate::state::state();
        st.surface_names().iter().map(|s| s.to_string()).collect()
    };
    for e in host.document().entities() {
        if let Some((name, "TIN")) = surface_tag(e) {
            if !names.iter().any(|n| n.eq_ignore_ascii_case(name)) {
                names.push(name.to_string());
            }
        }
    }
    names
}

/// Uppercased file stem of `path`, used as a surface name / layer suffix.
fn layer_stem(path: &str) -> String {
    std::path::Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().to_uppercase())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "SURFACE".to_string())
}

/// Draw a surface's unique TIN edges as 3-D `Line` entities on `layer`, each
/// tagged `LANDSURVEY_SURFACE [name, "TIN"]`. Returns the edge count.
fn draw_tin(host: &mut dyn HostApi, surf: &Surface, layer: &str, name: &str) -> usize {
    let mut batch = Batch::new();
    for e in surf.edges() {
        let a = surf.nodes[e[0]];
        let b = surf.nodes[e[1]];
        let ent = EntityType::Line(Line::from_points(
            Vector3::new(a[0], a[1], a[2]),
            Vector3::new(b[0], b[1], b[2]),
        ));
        batch.add_tagged(ent, layer, surface_record(name, "TIN"));
    }
    batch.commit(host)
}

/// Draw plan-view 2-D segments (e.g. the cut/fill boundary) on `layer`.
fn draw_segments(
    host: &mut dyn HostApi,
    segs: &[[[f64; 2]; 2]],
    layer: &str,
    name: &str,
) -> usize {
    let mut batch = Batch::new();
    for s in segs {
        let ent = EntityType::Line(Line::from_points(
            Vector3::new(s[0][0], s[0][1], 0.0),
            Vector3::new(s[1][0], s[1][1], 0.0),
        ));
        batch.add_tagged(ent, layer, surface_record(name, "CUTFILL"));
    }
    batch.commit(host)
}

/// Place a result label (cut/fill/net) at the centre of the top surface's
/// extent, sized relative to the surface so it is legible at any scale.
fn draw_volume_label(host: &mut dyn HostApi, top: &Surface, cf: &surface::CutFill) {
    let (minx, maxx, miny, maxy) = top.extent();
    let cx = (minx + maxx) / 2.0;
    let cy = (miny + maxy) / 2.0;
    let height = ((maxx - minx).max(maxy - miny) / 50.0).max(1.0);
    let label = format!("CUT {:.2}  FILL {:.2}  NET {:.2}", cf.cut, cf.fill, cf.net);
    let ent = EntityType::Text(
        Text::with_value(label, Vector3::new(cx, cy, 0.0)).with_height(height),
    );
    tag_surface(host, ent, "LS-VOLUME-LABEL", "VOLUME", "LABEL");
}

/// Set the layer, add the entity, and tag it with `LANDSURVEY_SURFACE`
/// `[name, kind]` (via `write_record`, which registers the APPID for round-trip).
fn tag_surface(host: &mut dyn HostApi, mut ent: EntityType, layer: &str, name: &str, kind: &str) {
    ent.common_mut().layer = layer.to_string();
    let handle = host.add_entity(ent);
    host.write_record(handle, surface_record(name, kind));
}

/// A `LANDSURVEY_SURFACE [name, kind]` record.
fn surface_record(name: &str, kind: &str) -> ExtendedDataRecord {
    let mut rec = ExtendedDataRecord::new(XDATA_SURFACE);
    rec.add_value(XDataValue::String(name.to_string()));
    rec.add_value(XDataValue::String(kind.to_string()));
    rec
}

/// `LS_RTS <baseN> <baseE> <rot_deg> <scale> [<toN> <toE>]` — Rotate/Translate/
/// Scale every entity in the drawing about a base point (CCW+ rotation). With a
/// `<toN> <toE>` the base is moved there (translation); omit it to rotate/scale
/// in place. Implemented as translate(-base) -> scale -> rotate -> translate(+to),
/// which reproduces the engine's `Conformal::from_base_swing`.
fn rts(host: &mut dyn HostApi, cmd: &str) {
    const USAGE: &str = "Usage: LS_RTS <baseN> <baseE> <rot_deg> <scale> [<toN> <toE>]";
    let toks: Vec<&str> = cmd.split_whitespace().skip(1).collect();
    if toks.is_empty() {
        host.push_info(USAGE);
        return;
    }
    // Strict: this command transforms EVERY entity in the drawing, so a typo
    // must be an error — the old lenient parse dropped the bad token and
    // silently shifted the remaining arguments one slot left.
    if toks.len() != 4 && toks.len() != 6 {
        host.push_error(&format!(
            "LS_RTS: expected 4 or 6 arguments, got {}. {USAGE}",
            toks.len()
        ));
        return;
    }
    let mut nums = Vec::with_capacity(toks.len());
    for t in &toks {
        match t.parse::<f64>() {
            Ok(v) if v.is_finite() => nums.push(v),
            _ => {
                host.push_error(&format!("LS_RTS: \"{t}\" is not a finite number. {USAGE}"));
                return;
            }
        }
    }
    let (bn, be, rot_deg, scale) = (nums[0], nums[1], nums[2], nums[3]);
    if scale <= 0.0 {
        host.push_error(&format!("LS_RTS: scale must be positive, got {scale}."));
        return;
    }
    let (tn, te) = if nums.len() == 6 { (nums[4], nums[5]) } else { (bn, be) };
    let rot = rot_deg.to_radians();

    // Check before push_undo: an empty drawing must not leave a stray undo
    // entry that makes the user's next Ctrl-Z appear to do nothing.
    if host.document().entities().next().is_none() {
        host.push_info("LS_RTS: no entities in the drawing to transform.");
        return;
    }
    host.push_undo("LS_RTS");
    // Out-of-process, document_mut() is a local snapshot (OCS #250): transform
    // a clone of each entity and push it back through update_entity, which is
    // an RPC that mutates the real host document in place (handle preserved).
    let snapshot: Vec<EntityType> = host.document().entities().cloned().collect();
    let mut count = 0usize;
    for mut ent in snapshot {
        let e = ent.as_entity_mut();
        // base -> origin, scale + rotate about origin, then origin -> destination.
        e.translate(Vector3::new(-be, -bn, 0.0));
        e.apply_scaling(scale);
        e.apply_rotation(Vector3::new(0.0, 0.0, 1.0), rot);
        e.translate(Vector3::new(te, tn, 0.0));
        if host.update_entity(ent) {
            count += 1;
        }
    }
    host.bump_geometry();
    host.set_dirty();
    let plural = if count == 1 { "entity" } else { "entities" };
    host.push_output(&format!(
        "LS_RTS: transformed {count} {plural} — rot {rot_deg}\u{b0} CCW+, scale {scale}, \
         base ({bn},{be}) -> ({tn},{te})."
    ));
}

/// `LS_HELMERT <pairs_file> [apply]` — least-squares 2-D conformal (Helmert)
/// fit from control pairs (`srcN, srcE, dstN, dstE` per line). Reports the
/// transform + per-pair residuals + RMS. With a trailing `apply`, transforms
/// every entity in the drawing by the fitted transform.
fn helmert(host: &mut dyn HostApi, cmd: &str) {
    let rest = cmd
        .splitn(2, char::is_whitespace)
        .nth(1)
        .map(str::trim)
        .unwrap_or("");
    if rest.is_empty() {
        host.push_info(
            "Usage: LS_HELMERT <pairs_file> [apply|stages|anim|teach]  (lines: srcN, srcE, dstN, \
             dstE; 'stages' draws the steps, 'apply' transforms the drawing, 'anim'/'teach' export \
             an animated SVG explainer)",
        );
        return;
    }
    // A trailing mode keyword (path may contain spaces, so strip from the end).
    let lower = rest.to_ascii_lowercase();
    let (path, apply, stages, anim, teach) = if lower.ends_with(" apply") {
        (rest[..rest.len() - 6].trim(), true, false, false, false)
    } else if lower.ends_with(" stages") {
        (rest[..rest.len() - 7].trim(), false, true, false, false)
    } else if lower.ends_with(" teach") {
        (rest[..rest.len() - 6].trim(), false, false, true, true)
    } else if lower.ends_with(" anim") {
        (rest[..rest.len() - 5].trim(), false, false, true, false)
    } else {
        (rest, false, false, false, false)
    };

    let text = match read_text(path) {
        Ok(t) => t,
        Err(e) => {
            host.push_error(&format!("LS_HELMERT: cannot read \"{path}\": {e}"));
            return;
        }
    };
    let pairs = match transform::parse_control_pairs(&text) {
        Ok(p) => p,
        Err(e) => {
            host.push_error(&format!("LS_HELMERT: \"{path}\": {e}"));
            return;
        }
    };
    if pairs.len() < 2 {
        host.push_error(&format!(
            "LS_HELMERT: \"{path}\" has {} control pair(s); need at least 2.",
            pairs.len()
        ));
        return;
    }
    let steps = match transform::helmert_fit_explained(&pairs) {
        Ok(s) => s,
        Err(e) => {
            host.push_error(&format!("LS_HELMERT: {e}"));
            return;
        }
    };
    let t = steps.transform;
    let (res, rms) = transform::fit_residuals(&t, &pairs);
    let max = res.iter().cloned().fold(0.0_f64, f64::max);

    // The fit, as 7 explicit steps.
    host.push_output("LS_HELMERT — 2D 4-parameter conformal (Rotate/Translate/Scale):");
    host.push_output(&format!(
        "  step 1  source centroid : E {:.4}, N {:.4}",
        steps.src_centroid.0, steps.src_centroid.1
    ));
    host.push_output(&format!(
        "  step 2  target centroid : E {:.4}, N {:.4}",
        steps.dst_centroid.0, steps.dst_centroid.1
    ));
    host.push_output(&format!(
        "  step 3  cross-cov sums  : Sxx {:.4}, Sxy {:.4}, S|d|^2 {:.4}",
        steps.sxx, steps.sxy, steps.sum_sq
    ));
    host.push_output(&format!("  step 4  scale  s        : {:.8}", steps.scale()));
    host.push_output(&format!("  step 5  rotation theta  : {:.6}\u{b0} CCW+", steps.rotation_deg()));
    host.push_output(&format!("  step 6  translation     : E {:.4}, N {:.4}", t.c, t.d));
    host.push_output(&format!(
        "  step 7  residuals       : RMS {:.5}, max {:.5}  ({} pairs)",
        rms, max, pairs.len()
    ));
    for (i, r) in res.iter().enumerate().take(12) {
        host.push_output(&format!("            pair {:>2}: {:.5}", i + 1, r));
    }

    if stages {
        host.push_undo("LS_HELMERT stages");
        let n = draw_helmert_stages(host, &steps, &pairs, max);
        host.bump_geometry();
        host.set_dirty();
        host.push_output(&format!(
            "LS_HELMERT: drew {n} stage entities (layers LS-HMT-0-SRC..3-FINAL, TARGET, RESID, PATH)."
        ));
    }

    if anim {
        match viz::helmert_anim_svg(&pairs, teach) {
            Ok(svg) => write_anim_file(host, "helmert", &svg),
            Err(e) => host.push_error(&format!("LS_HELMERT: {e}")),
        }
    }

    if apply {
        let scale = t.scale();
        let rot = t.rotation_deg().to_radians();
        host.push_undo("LS_HELMERT apply");
        // Same snapshot + update_entity pattern as LS_RTS (OCS #250).
        let snapshot: Vec<EntityType> = host.document().entities().cloned().collect();
        let mut count = 0usize;
        for mut ent in snapshot {
            let e = ent.as_entity_mut();
            // E' = s*R*(E,N) + (c,d): scale & rotate about origin, then translate.
            e.apply_scaling(scale);
            e.apply_rotation(Vector3::new(0.0, 0.0, 1.0), rot);
            e.translate(Vector3::new(t.c, t.d, 0.0));
            if host.update_entity(ent) {
                count += 1;
            }
        }
        host.bump_geometry();
        host.set_dirty();
        host.push_output(&format!("LS_HELMERT: applied fit to {count} entit{}.", if count == 1 { "y" } else { "ies" }));
    }
}

/// `LS_RESECT <shots.csv>` — free-station resection. Solves the occupied
/// (unknown) point from shots to known control: combined (direction + distance,
/// least-squares similarity) when ≥2 shots carry distances, else angle-only
/// three-point (Tienstra). Draws the station, a ray to each known point, and a
/// label. CSV lines: `knownN, knownE, direction_deg[, distance][, name]`.
fn resect(host: &mut dyn HostApi, cmd: &str) {
    let rest = first_arg(cmd);
    if rest.is_empty() {
        host.push_info(
            "Usage: LS_RESECT <shots.csv> [anim]  (lines: knownN, knownE, direction_deg, distance, \
             name; blank distance = angle-only; 'anim' exports an animated-SVG explainer)",
        );
        return;
    }
    // A trailing `anim` keyword exports the explainer (path may contain spaces).
    let (arg, want_anim) = if rest.to_ascii_lowercase().ends_with(" anim") {
        (rest[..rest.len() - 5].trim(), true)
    } else {
        (rest, false)
    };
    let text = match read_text(arg) {
        Ok(t) => t,
        Err(e) => {
            host.push_error(&format!("LS_RESECT: cannot read \"{arg}\": {e}"));
            return;
        }
    };
    let shots = resection::parse_resection_shots(&text);
    if shots.len() < 2 {
        host.push_error(&format!(
            "LS_RESECT: \"{arg}\" has {} usable shot(s); need at least 2.",
            shots.len()
        ));
        return;
    }
    let with_dist = shots.iter().filter(|s| s.distance.is_some()).count();

    // Solve: combined (≥2 distances) else angle-only three-point (exactly 3).
    let (station, caption): ((f64, f64), String) = if with_dist >= 2 {
        let r = match resection::resection_combined(&shots) {
            Ok(r) => r,
            Err(e) => {
                host.push_error(&format!("LS_RESECT: {e}"));
                return;
            }
        };
        host.push_output("LS_RESECT — combined (direction + distance, least-squares):");
        host.push_output(&format!("  station (E,N) : {:.4}, {:.4}", r.station.0, r.station.1));
        host.push_output(&format!(
            "  orientation   : {:.4}\u{b0}  (grid azimuth = reading + orientation)",
            r.orientation_deg
        ));
        host.push_output(&format!(
            "  scale check   : {:.8}{}",
            r.scale,
            if r.scale_blunder(1e-3) {
                "  <-- WARNING: strays from 1.0 (distance/EDM blunder?)"
            } else {
                ""
            }
        ));
        host.push_output(&format!("  residuals RMS : {:.5} ({} shots)", r.rms, r.residuals.len()));
        for (name, res) in r.residuals.iter().take(12) {
            host.push_output(&format!("            {name:>6}: {res:.5}"));
        }
        let cap = format!(
            "combined resection \u{b7} orient {:.3}\u{b0} \u{b7} scale {:.5} \u{b7} RMS {:.4}",
            r.orientation_deg, r.scale, r.rms
        );
        (r.station, cap)
    } else if shots.len() == 3 && with_dist == 0 {
        let r = match resection::resection_three_point_shots(&shots) {
            Ok(r) => r,
            Err(e) => {
                host.push_error(&format!("LS_RESECT: {e}"));
                return;
            }
        };
        host.push_output("LS_RESECT — angle-only three-point (Tienstra):");
        host.push_output(&format!("  station (E,N) : {:.4}, {:.4}", r.station.0, r.station.1));
        host.push_output(&format!(
            "  orientation   : {:.4}\u{b0}  (grid azimuth = reading + orientation)",
            r.orientation_deg
        ));
        host.push_output(&format!(
            "  subtended     : BPC {:.4}\u{b0}, CPA {:.4}\u{b0}, APB {:.4}\u{b0}",
            r.subtended_deg[0], r.subtended_deg[1], r.subtended_deg[2]
        ));
        host.push_output(&format!(
            "  amplification : {:.1}x{}",
            r.amplification,
            if r.amplification > resection::DANGER_CIRCLE_WARN_AMPLIFICATION {
                "  <-- WARNING: near danger circle"
            } else {
                ""
            }
        ));
        let cap = format!(
            "angle-only three-point \u{b7} orient {:.3}\u{b0} \u{b7} amplification {:.1}x",
            r.orientation_deg, r.amplification
        );
        (r.station, cap)
    } else {
        host.push_error(&format!(
            "LS_RESECT: need >=2 distance shots (combined) or exactly 3 angle shots \
             (three-point); got {} shots, {with_dist} with distances.",
            shots.len()
        ));
        return;
    };
    // Survey points need a visible point style.
    {
        let header = &mut host.document_mut().header;
        if header.point_display_mode == 0 {
            header.point_display_mode = 3;
        }
        if header.point_display_size == 0.0 {
            header.point_display_size = 5.0;
        }
    }

    host.push_undo("LS_RESECT draw");
    add_on_layer(
        host,
        EntityType::Point(CadPoint::at(Vector3::new(station.0, station.1, 0.0))),
        "LS-RESECT-STATION",
    );
    let mut spread = 0.0_f64;
    for s in &shots {
        add_on_layer(
            host,
            EntityType::Point(CadPoint::at(Vector3::new(s.known.0, s.known.1, 0.0))),
            "LS-RESECT-KNOWN",
        );
        add_on_layer(
            host,
            EntityType::Line(Line::from_points(
                Vector3::new(station.0, station.1, 0.0),
                Vector3::new(s.known.0, s.known.1, 0.0),
            )),
            "LS-RESECT-RAYS",
        );
        spread = spread.max((s.known.0 - station.0).hypot(s.known.1 - station.1));
    }
    let h = (spread / 30.0).max(1.0);
    add_on_layer(
        host,
        EntityType::Text(
            Text::with_value(
                format!("STA E{:.3} N{:.3}", station.0, station.1),
                Vector3::new(station.0 + h, station.1 + h, 0.0),
            )
            .with_height(h),
        ),
        "LS-RESECT-LABEL",
    );
    host.bump_geometry();
    host.set_dirty();
    host.push_output(&format!(
        "LS_RESECT: drew station + {} ray(s) (layers LS-RESECT-STATION/KNOWN/RAYS/LABEL).",
        shots.len()
    ));

    // Optional animated-SVG explainer (known control fixed, station converges).
    if want_anim {
        let knowns: Vec<(f64, f64)> = shots.iter().map(|s| s.known).collect();
        let names: Vec<&str> = shots.iter().map(|s| s.name.as_str()).collect();
        let svg = viz::resection_anim_svg(&knowns, &names, station, &caption);
        write_anim_file(host, "resection", &svg);
    }
}

/// Draw the Helmert application as discrete stages so the transform can be seen:
/// each control point as source -> scaled -> rotated -> final (with its path),
/// the actual targets, residual vectors, and the two centroids. Returns the
/// entity count.
fn draw_helmert_stages(
    host: &mut dyn HostApi,
    steps: &transform::HelmertSteps,
    pairs: &[((f64, f64), (f64, f64))],
    max_resid: f64,
) -> usize {
    // Survey points need a visible point style.
    {
        let header = &mut host.document_mut().header;
        if header.point_display_mode == 0 {
            header.point_display_mode = 3;
        }
        if header.point_display_size == 0.0 {
            header.point_display_size = 5.0;
        }
    }
    let stage_layers = ["LS-HMT-0-SRC", "LS-HMT-1-SCALED", "LS-HMT-2-ROTATED", "LS-HMT-3-FINAL"];
    let mut n = 0usize;
    for &((se, sn), (de, dn)) in pairs {
        let st = steps.stages(se, sn);
        for (i, &(x, y)) in st.iter().enumerate() {
            add_on_layer(host, EntityType::Point(CadPoint::at(Vector3::new(x, y, 0.0))), stage_layers[i]);
            n += 1;
        }
        add_on_layer(host, EntityType::Point(CadPoint::at(Vector3::new(de, dn, 0.0))), "LS-HMT-TARGET");
        for w in 0..3 {
            let a = Vector3::new(st[w].0, st[w].1, 0.0);
            let b = Vector3::new(st[w + 1].0, st[w + 1].1, 0.0);
            add_on_layer(host, EntityType::Line(Line::from_points(a, b)), "LS-HMT-PATH");
        }
        let fin = Vector3::new(st[3].0, st[3].1, 0.0);
        let tgt = Vector3::new(de, dn, 0.0);
        add_on_layer(host, EntityType::Line(Line::from_points(fin, tgt)), "LS-HMT-RESID");
        n += 4;
    }
    for &(cx, cy) in &[steps.src_centroid, steps.dst_centroid] {
        add_on_layer(host, EntityType::Point(CadPoint::at(Vector3::new(cx, cy, 0.0))), "LS-HMT-CENTROID");
        n += 1;
    }

    // Numbered step annotations: 1-3 in place at the source centroid, 4 along
    // the move, 5 at the target. Height sized from the source control spread.
    let (smnx, smxx, smny, smxy) = pairs.iter().fold(
        (f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY),
        |(a, b, c, e), &((se, sn), _)| (a.min(se), b.max(se), c.min(sn), e.max(sn)),
    );
    let h = ((smxx - smnx).max(smxy - smny) / 12.0).max(0.5);
    let (sg, dg) = (steps.src_centroid, steps.dst_centroid);
    let labels: [(f64, f64, String); 5] = [
        (sg.0 + h, sg.1 + h * 3.0, "1 source".to_string()),
        (sg.0 + h, sg.1 + h * 1.6, format!("2 scale x{:.5}", steps.scale())),
        (sg.0 + h, sg.1 + h * 0.2, format!("3 rotate {:.4} deg", steps.rotation_deg())),
        (
            (sg.0 + dg.0) / 2.0,
            (sg.1 + dg.1) / 2.0 + h * 1.5,
            format!("4 translate dE {:.2} dN {:.2}", dg.0 - sg.0, dg.1 - sg.1),
        ),
        (dg.0 + h, dg.1 + h * 3.0, format!("5 residual max {:.4}", max_resid)),
    ];
    for (x, y, text) in labels {
        add_on_layer(
            host,
            EntityType::Text(Text::with_value(text, Vector3::new(x, y, 0.0)).with_height(h)),
            "LS-HMT-LABEL",
        );
        n += 1;
    }
    n
}

/// Set an entity's layer and add it (no XDATA), for transient/illustrative geometry.
fn add_on_layer(host: &mut dyn HostApi, mut ent: EntityType, layer: &str) {
    ent.common_mut().layer = layer.to_string();
    host.add_entity(ent);
}

/// `LS_LIST` — list each Land Survey point (number, E/N/Z, description) read
/// back from the `LANDSURVEY_POINT` XDATA record and the point geometry.
fn list_points(host: &mut dyn HostApi) {
    // Collect first (immutable borrow of the document), then emit (needs &mut).
    let mut rows: Vec<String> = Vec::new();
    for e in host.document().entities() {
        let Some(rec) = e.common().extended_data.get_record(XDATA_POINT) else {
            continue;
        };
        // The record holds [number, description] as strings (see import_pnezd).
        let mut strs = rec.values.iter().filter_map(|v| match v {
            XDataValue::String(s) => Some(s.as_str()),
            _ => None,
        });
        let number = strs.next().unwrap_or("");
        let description = strs.next().unwrap_or("");
        let (east, north, elev) = match e {
            EntityType::Point(p) => (p.location.x, p.location.y, p.location.z),
            _ => (0.0, 0.0, 0.0),
        };
        rows.push(format!(
            "  {number:<8} E {east:.3}  N {north:.3}  Z {elev:.3}  {description}"
        ));
    }

    if rows.is_empty() {
        host.push_output("LS_LIST: no Land Survey points in drawing.");
        return;
    }
    host.push_output(&format!("LS_LIST: {} Land Survey point(s):", rows.len()));
    for r in &rows {
        host.push_output(r);
    }
}

/// `LS_IMPORTPLAN <path.json>` — import recognized plan geometry (the
/// `plan2cad` / `plat2json` pipelines' JSON) faithfully: each element becomes
/// an entity on its named layer, tagged with `LANDSURVEY_PLAN` XDATA (source
/// filename). Ordered chains (`polylines` / `plines`) import as LWPolylines;
/// line segments that merely re-state a chain's edges are skipped.
fn import_plan(host: &mut dyn HostApi, cmd: &str) {
    let arg = first_arg(cmd);
    if arg.is_empty() {
        host.push_info("Usage: LS_IMPORTPLAN <path-to-plan.json>");
        return;
    }
    let text = match fs::read_to_string(arg) {
        Ok(t) => t,
        Err(e) => {
            host.push_error(&format!("LS_IMPORTPLAN: cannot read \"{arg}\": {e}"));
            return;
        }
    };
    let plan = match plan::parse(&text) {
        Ok(p) => p,
        Err(e) => {
            host.push_error(&format!("LS_IMPORTPLAN: invalid plan JSON: {e}"));
            return;
        }
    };
    let source = std::path::Path::new(arg)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| arg.to_string());

    host.push_undo("LS_IMPORTPLAN");
    let mut n = 0usize;
    let mut layers = std::collections::BTreeSet::new();

    // Polylines first (plat2json chains — traced, and arc-fitted with bulge
    // vertices). Each chain becomes one LWPolyline; a chain whose last point
    // repeats the first imports closed. Their edges and arc segments are
    // remembered so the flattened `lines`/`arcs` duplicates the pipeline also
    // emits (for sinks without polyline support) aren't drawn twice.
    let mut chain_edges: std::collections::HashSet<plan::SegKey> =
        std::collections::HashSet::new();
    let mut chain_arcs: Vec<(f64, f64, f64, f64, f64)> = Vec::new();
    for pl in &plan.polylines {
        chain_edges.extend(pl.edge_keys());
        chain_arcs.extend(pl.bulged_segments());
        let (pts, closed) = pl.ring();
        if pts.len() < 2 {
            continue;
        }
        let mut lw = LwPolyline::new();
        for (x, y, bulge) in pts {
            lw.add_point_with_bulge(Vector2::new(*x, *y), *bulge);
        }
        lw.is_closed = closed;
        place(host, EntityType::LwPolyline(lw), &pl.layer, &source);
        layers.insert(pl.layer.clone());
        n += 1;
    }

    let mut dup = 0usize;
    for (x1, y1, x2, y2, layer) in &plan.lines {
        if chain_edges.contains(&plan::seg_key(*x1, *y1, *x2, *y2)) {
            dup += 1;
            continue;
        }
        let ent = EntityType::Line(Line::from_points(
            Vector3::new(*x1, *y1, 0.0),
            Vector3::new(*x2, *y2, 0.0),
        ));
        place(host, ent, layer, &source);
        layers.insert(layer.clone());
        n += 1;
    }
    // Matching a flattened arc entry to a bulged chain segment: both come from
    // the same rounded values, so 1e-3 drawing units is ample.
    const ARC_DUP_TOL: f64 = 1e-3;
    for (cx, cy, r, start_deg, end_deg, layer) in &plan.arcs {
        if chain_arcs.iter().any(|&seg| {
            plan::arc_duplicates_segment(*cx, *cy, *r, *start_deg, *end_deg, seg, ARC_DUP_TOL)
        }) {
            dup += 1;
            continue;
        }
        // Source angles are degrees; acadrust arc angles are radians.
        let ent = EntityType::Arc(CadArc::from_center_radius_angles(
            Vector3::new(*cx, *cy, 0.0),
            *r,
            start_deg.to_radians(),
            end_deg.to_radians(),
        ));
        place(host, ent, layer, &source);
        layers.insert(layer.clone());
        n += 1;
    }
    for (cx, cy, r, layer) in &plan.circles {
        let ent = EntityType::Circle(Circle::from_center_radius(Vector3::new(*cx, *cy, 0.0), *r));
        place(host, ent, layer, &source);
        layers.insert(layer.clone());
        n += 1;
    }
    for (x, y, value, style) in &plan.texts {
        let ent = EntityType::Text(
            Text::with_value(plan::decode_text(value), Vector3::new(*x, *y, 0.0))
                .with_height(PLAN_TEXT_HEIGHT),
        );
        // For texts the 4th field is a style/annotation-layer name.
        place(host, ent, style, &source);
        layers.insert(style.clone());
        n += 1;
    }

    host.bump_geometry();
    host.set_dirty();
    let dup_note = if dup > 0 {
        format!(" ({dup} element(s) already covered by polylines skipped)")
    } else {
        String::new()
    };
    host.push_output(&format!(
        "LS_IMPORTPLAN: {n} entities on {} layer(s) from {source}.{dup_note}",
        layers.len()
    ));
}

/// Put `ent` on `layer`, add it, and tag it with `LANDSURVEY_PLAN` XDATA.
/// The layer is set before `add_entity` (which only assigns a handle/owner, so
/// it preserves the layer); the XDATA is written via `write_record` so the
/// APPID is registered for DWG/DXF round-trip.
fn place(host: &mut dyn HostApi, mut ent: EntityType, layer: &str, source: &str) {
    ent.common_mut().layer = layer.to_string();
    let handle = host.add_entity(ent);
    let mut rec = ExtendedDataRecord::new(XDATA_PLAN);
    rec.add_value(XDataValue::String(source.to_string()));
    host.write_record(handle, rec);
}

#[cfg(test)]
mod tests {
    use super::*;
    use codec::tables::{AppId, TableEntry};
    use codec::{CadDocument, DwgReader, DwgWriter};
    use landsurvey::alignment;
    use std::io::Cursor;

    /// Minimal in-memory host: enough of `HostApi` for the dispatch paths the
    /// tests drive (document + add_entity + XDATA + command-line capture).
    /// Interactive/state/reader surfaces are unreachable from those paths and
    /// panic if hit.
    struct TestHost {
        doc: CadDocument,
        log: Vec<String>,
        /// Calls to `add_entity` / `add_entities`, for the batching tests.
        single_adds: usize,
        bulk_adds: usize,
        /// Accept this many `add_entities` calls, then return no handles, as
        /// the host does when a message is over its IPC size limit.
        bulk_limit: Option<usize>,
    }

    impl TestHost {
        fn new() -> Self {
            TestHost {
                doc: CadDocument::default(),
                log: Vec::new(),
                single_adds: 0,
                bulk_adds: 0,
                bulk_limit: None,
            }
        }
    }

    impl HostApi for TestHost {
        fn tab_index(&self) -> usize {
            0
        }
        fn document(&self) -> &CadDocument {
            &self.doc
        }
        fn document_mut(&mut self) -> &mut CadDocument {
            &mut self.doc
        }
        fn add_entity(&mut self, entity: EntityType) -> codec::Handle {
            self.single_adds += 1;
            self.doc.add_entity(entity).expect("add_entity")
        }
        fn add_entities(&mut self, entities: Vec<EntityType>) -> Vec<codec::Handle> {
            self.bulk_adds += 1;
            if self.bulk_limit.is_some_and(|limit| self.bulk_adds > limit) {
                return Vec::new();
            }
            entities
                .into_iter()
                .map(|e| self.doc.add_entity(e).expect("add_entity"))
                .collect()
        }
        fn bump_geometry(&mut self) {}
        fn read_record(
            &self,
            handle: codec::Handle,
            app_name: &str,
        ) -> Option<&ExtendedDataRecord> {
            self.doc
                .get_entity(handle)?
                .common()
                .extended_data
                .get_record(app_name)
        }
        fn write_record(&mut self, handle: codec::Handle, record: ExtendedDataRecord) -> bool {
            // Mirror the host's ensure_app_id contract (OCS #249).
            if !self.doc.app_ids.contains(&record.application_name) {
                let mut app = AppId::new(&record.application_name);
                app.set_handle(self.doc.allocate_handle());
                let _ = self.doc.app_ids.add(app);
            }
            // Replace any record for the same app, as the host does.
            match self.doc.get_entity_mut(handle) {
                Some(e) => {
                    let xd = &mut e.common_mut().extended_data;
                    let kept: Vec<_> = xd
                        .records()
                        .iter()
                        .filter(|r| r.application_name != record.application_name)
                        .cloned()
                        .collect();
                    xd.clear();
                    for r in kept {
                        xd.add_record(r);
                    }
                    xd.add_record(record);
                    true
                }
                None => false,
            }
        }
        fn remove_record(&mut self, _handle: codec::Handle, _app_name: &str) -> bool {
            false
        }
        fn push_undo(&mut self, _label: &str) {}
        fn set_dirty(&mut self) {}
        fn push_info(&mut self, msg: &str) {
            self.log.push(msg.to_string());
        }
        fn push_output(&mut self, msg: &str) {
            self.log.push(msg.to_string());
        }
        fn push_error(&mut self, msg: &str) {
            self.log.push(format!("ERROR: {msg}"));
        }
        fn start_interactive(
            &mut self,
            _command: Box<dyn ocs_plugin_api::host::InteractiveCommand>,
        ) {
            unimplemented!("not exercised by these tests")
        }
        fn plugin_state_any(
            &self,
            _plugin_id: &str,
        ) -> Option<&(dyn std::any::Any + Send + Sync)> {
            None
        }
        fn plugin_state_any_mut(
            &mut self,
            _plugin_id: &str,
        ) -> Option<&mut (dyn std::any::Any + Send + Sync)> {
            None
        }
        fn ensure_plugin_state_any(
            &mut self,
            _plugin_id: &'static str,
            _init: &mut dyn FnMut() -> Box<dyn std::any::Any + Send + Sync>,
        ) -> Option<&mut (dyn std::any::Any + Send + Sync)> {
            // Like the out-of-process host: no host-side plugin state (the
            // plugin keeps its own in crate::state).
            None
        }
        fn document_reader(&self) -> Box<dyn ocs_plugin_api::host::DocumentReader + '_> {
            unimplemented!("not exercised by these tests")
        }
    }


    /// Write `n` PNEZD rows to a temp file and return its path.
    fn pnezd_file(tag: &str, n: usize) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("ls_pnezd_{tag}_{}.csv", std::process::id()));
        let mut text = String::new();
        for i in 1..=n {
            let code = ["IP", "TOPO", "EP"][i % 3];
            text.push_str(&format!("{i},{},{},{},{code}\n", 5000.0 + i as f64, 1000.0 + (i % 97) as f64, 100.0));
        }
        std::fs::write(&path, text).expect("write pnezd");
        path
    }

    /// #38: a large import goes through `add_entities` in chunks, with no
    /// per-entity round trips, and every point keeps exactly one tag (the
    /// APPID re-write on the first point must replace, not duplicate).
    #[test]
    fn pnezd_import_is_batched_and_keeps_tags() {
        let n = 6_000; // with labels, 12 000 entities: three chunks
        let src = pnezd_file("batch", n);
        let mut host = TestHost::new();
        import_pnezd(&mut host, &format!("LS_PNEZD {}", src.display()));
        let _ = std::fs::remove_file(&src);

        assert!(!host.log.iter().any(|l| l.starts_with("ERROR")), "{:?}", host.log);
        assert!(host.log.iter().any(|l| l.contains(&format!("imported {n} labeled point(s)"))), "{:?}", host.log);
        assert_eq!(host.single_adds, 0, "per-entity add_entity calls are back");
        assert_eq!(host.bulk_adds, (2 * n).div_ceil(crate::batch::CHUNK));
        assert!(host.doc.app_ids.contains(XDATA_POINT), "APPID not registered");

        let mut seen = 0;
        for e in host.doc.entities() {
            if let EntityType::Point(_) = e {
                let records = e.common().extended_data.records();
                assert_eq!(records.iter().filter(|r| r.application_name == XDATA_POINT).count(), 1);
                seen += 1;
            }
        }
        assert_eq!(seen, n);

        // The tag on a batched point survives DWG save and reopen.
        let first = host
            .doc
            .entities()
            .find(|e| matches!(e, EntityType::Point(_)))
            .map(|e| e.common().handle)
            .expect("a point");
        let bytes = DwgWriter::write_to_vec(&host.doc).expect("dwg write");
        let rt = DwgReader::from_stream(Cursor::new(bytes)).read().expect("dwg read");
        let tag = named_tag(rt.get_entity(first).expect("point after reopen"), XDATA_POINT);
        assert_eq!(tag, Some(("1".to_string(), "TOPO".to_string())));
    }

    /// When the host drops a chunk, the import says so and counts only the
    /// points that landed.
    #[test]
    fn pnezd_import_reports_a_refused_chunk() {
        let n = 6_000;
        let src = pnezd_file("short", n);
        let mut host = TestHost::new();
        host.bulk_limit = Some(1);
        import_pnezd(&mut host, &format!("LS_PNEZD {}", src.display()));
        let _ = std::fs::remove_file(&src);

        let accepted = crate::batch::CHUNK;
        assert!(
            host.log.iter().any(|l| l.contains(&format!("accepted {accepted} of {} entities", 2 * n))),
            "{:?}",
            host.log
        );
        // Point + label per row: the first chunk holds half as many points.
        assert!(host.log.iter().any(|l| l.contains(&format!("imported {} labeled point(s)", accepted / 2))), "{:?}", host.log);
    }

    /// Register the LANDSURVEY_* APPIDs with real handles, as the fixed host
    /// `ensure_app_id` does on `write_record` (OCS #249). A standalone test
    /// document has no host, so the writer needs this done by hand for the
    /// EED app references to resolve.
    fn register_ls_app_ids(doc: &mut CadDocument) {
        for name in [XDATA_POINT, XDATA_PLAN, XDATA_SURFACE, XDATA_PARCEL, XDATA_FEATURE] {
            if !doc.app_ids.contains(name) {
                let mut app = AppId::new(name);
                app.set_handle(doc.allocate_handle());
                let _ = doc.app_ids.add(app);
            }
        }
    }

    /// Imperial LandXML with two points (one via pntRef), a lot whose east side
    /// is a CCW half circle, a fence plan feature and a one-triangle TIN.
    const LANDXML_DOC: &str = r#"<?xml version="1.0"?>
<LandXML version="1.2">
  <Units><Imperial linearUnit="USSurveyFoot"/></Units>
  <CgPoints>
    <CgPoint name="101" code="IP" desc="IP found">100 200 10.5</CgPoint>
    <CgPoint name="102" code="CB">100 220 9.25</CgPoint>
  </CgPoints>
  <Parcels>
    <Parcel name="Lot 1" area="557.079633">
      <Center>110 210</Center>
      <CoordGeom>
        <Line><Start pntRef="101"/><End pntRef="102"/></Line>
        <Curve rot="ccw"><Start>100 220</Start><Center>110 220</Center><End>120 220</End></Curve>
        <Line><Start>120 220</Start><End>120 200</End></Line>
        <Line><Start>120 200</Start><End>100 200</End></Line>
      </CoordGeom>
    </Parcel>
  </Parcels>
  <PlanFeatures>
    <PlanFeature name="FENCE"><CoordGeom><IrregularLine><PntList2D>0 0 0 10 5 10</PntList2D></IrregularLine></CoordGeom></PlanFeature>
  </PlanFeatures>
  <Surfaces><Surface name="LXRT"><Definition surfType="TIN">
    <Pnts><P id="1">0 0 1</P><P id="2">0 10 2</P><P id="3">10 0 3</P></Pnts>
    <Faces><F>1 2 3</F></Faces>
  </Definition></Surface></Surfaces>
</LandXML>"#;

    /// LS_LANDXML imports points, parcels, features and surfaces; LS_LANDXMLOUT
    /// writes them back out and the result reads back identically. One test,
    /// because the last-imported LandXML units live in process-global state.
    #[test]
    fn landxml_import_export_roundtrip() {
        let dir = std::env::temp_dir().join(format!("ls_lx_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let src = dir.join("in file.xml"); // a space in the path
        std::fs::write(&src, LANDXML_DOC).expect("write fixture");

        // preview: reports, draws nothing.
        let mut host = TestHost::new();
        import_landxml(&mut host, &format!("LS_LANDXML {} preview", src.display()));
        assert_eq!(host.doc.entities().count(), 0);
        assert!(host.log.iter().any(|l| l.contains("2 point(s), 1 parcel(s)")), "{:?}", host.log);

        // A kind filter imports only that kind.
        let mut host = TestHost::new();
        import_landxml(&mut host, &format!("LS_LANDXML {} parcels", src.display()));
        assert!(host.doc.entities().all(|e| !matches!(e, EntityType::Point(_) | EntityType::Mesh(_))));

        let mut host = TestHost::new();
        import_landxml(&mut host, &format!("LS_LANDXML {}", src.display()));
        assert!(!host.log.iter().any(|l| l.starts_with("ERROR")), "{:?}", host.log);
        assert!(host.log.iter().any(|l| l.contains("computed area matches")), "{:?}", host.log);

        let points: Vec<_> = host
            .doc
            .entities()
            .filter_map(|e| match e {
                EntityType::Point(p) => Some((named_tag(e, XDATA_POINT).unwrap(), p.location, e.common().layer.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(points.len(), 2);
        assert_eq!(points[0].0, ("101".to_string(), "IP found".to_string()));
        assert_eq!((points[0].1.x, points[0].1.y, points[0].1.z), (200.0, 100.0, 10.5));
        assert_eq!(points[0].2, "LS-PT-IP");
        // A code with no desc becomes the description.
        assert_eq!(points[1].0 .1, "CB");

        let parcel = host
            .doc
            .entities()
            .find_map(|e| match e {
                EntityType::LwPolyline(pl) if e.common().layer == "LS-PARCEL" => Some((pl.clone(), named_tag(e, XDATA_PARCEL))),
                _ => None,
            })
            .expect("parcel polyline");
        assert!(parcel.0.is_closed);
        assert_eq!(parcel.0.vertices.len(), 4);
        assert!((parcel.0.vertices[1].bulge - 1.0).abs() < 1e-12, "half circle");
        assert_eq!(parcel.1, Some(("Lot 1".to_string(), String::new())));
        assert!(host.doc.entities().any(|e| matches!(e, EntityType::Text(t)
            if t.value == "Lot 1" && e.common().layer == "LS-PARCEL-LABEL")));
        assert!(host.doc.entities().any(|e| matches!(e, EntityType::LwPolyline(_))
            && named_tag(e, XDATA_FEATURE).is_some_and(|t| t.0 == "FENCE")));
        assert!(host.doc.entities().any(|e| matches!(e, EntityType::Mesh(_))));

        // Export (units carried over from the import) and read back.
        let out = dir.join("out.xml");
        export_landxml(&mut host, &format!("LS_LANDXMLOUT {}", out.display()));
        assert!(host.log.iter().any(|l| l.contains("(imperial)")), "{:?}", host.log);
        let back = landxml::read_document(&std::fs::read_to_string(&out).expect("exported"));
        let orig = landxml::read_document(LANDXML_DOC);
        assert_eq!(back.units, orig.units);
        assert_eq!(back.points, orig.points);
        assert_eq!(back.parcels.len(), 1);
        assert_eq!(back.parcels[0].name, "Lot 1");
        let close = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9;
        for (a, b) in back.parcels[0].segments.iter().zip(&orig.parcels[0].segments) {
            assert!(close(a.start(), b.start()) && close(a.end(), b.end()));
            assert!((a.sweep() - b.sweep()).abs() < 1e-12);
        }
        assert!((back.parcels[0].area.unwrap() - 557.079633).abs() < 1e-5);
        assert_eq!(back.features[0].segments, orig.features[0].segments);
        let tin = back.surfaces.iter().find(|s| s.name == "LXRT").expect("surface exported");
        assert_eq!(tin.surface.nodes, orig.surfaces[0].surface.nodes);

        // A `metric` flag overrides the carried-over units.
        export_landxml(&mut host, &format!("LS_LANDXMLOUT {} points metric", out.display()));
        let back = landxml::read_document(&std::fs::read_to_string(&out).unwrap());
        assert_eq!(back.units.map(|u| u.metric), Some(true));
        assert!(back.parcels.is_empty() && back.points.len() == 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A real Aplitop alignment (lines, arcs, 7 clothoids, a parabolic
    /// profile) survives LS_LANDXML → drawing → LS_LANDXMLOUT.
    #[test]
    fn landxml_alignment_roundtrip_through_drawing() {
        const XML: &str =
            include_str!("../crates/landsurvey/tests/fixtures/landxml-alignments/aplitop-1.xml");
        let dir = std::env::temp_dir().join(format!("ls_lx_al_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let src = dir.join("aplitop.xml");
        std::fs::write(&src, XML).expect("write fixture");

        let mut host = TestHost::new();
        import_landxml(&mut host, &format!("LS_LANDXML {} alignments", src.display()));
        assert!(host.log.iter().any(|l| l.contains("1 alignment(s) (1 profile(s)")), "{:?}", host.log);
        let on = |layer: &str| host.doc.entities().filter(|e| e.common().layer == layer).count();
        assert_eq!(on("LS-ALIGN"), 1);
        assert_eq!(on("LS-PROF-PVI") >= 1, true);
        assert_eq!(on("LS-PROF-FG"), 1);
        // Station labels at the geometry points, e.g. the first spiral's TS.
        assert!(host.doc.entities().any(|e| matches!(e, EntityType::Text(t) if t.value == "0+049.841")));

        let out = dir.join("out.xml");
        export_landxml(&mut host, &format!("LS_LANDXMLOUT {} alignments", out.display()));
        let back = landxml::read_document(&std::fs::read_to_string(&out).expect("exported"));
        let orig = landxml::read_document(XML);
        let _ = std::fs::remove_dir_all(&dir);
        let (a, b) = (&orig.alignments[0], &back.alignments[0]);
        assert_eq!((b.name.as_str(), b.sta_start), ("Horizontal", 0.0));
        assert_eq!(a.segments.len(), b.segments.len());
        for (x, y) in a.segments.iter().zip(&b.segments) {
            assert_eq!(std::mem::discriminant(x), std::mem::discriminant(y));
            assert!((x.length() - y.length()).abs() < 1e-6);
            for t in [0.25, 0.5, 0.75] {
                let (p, q) = (x.at(x.length() * t).0, y.at(y.length() * t).0);
                assert!((p[0] - q[0]).hypot(p[1] - q[1]) < 1e-6);
            }
        }
        let (alignment::ProfileData::Design(p), alignment::ProfileData::Design(q)) =
            (&a.profiles[0].data, &b.profiles[0].data)
        else {
            panic!("design profiles");
        };
        assert_eq!(p.len(), q.len());
        for (u, v) in p.iter().zip(q) {
            assert!((u.sta - v.sta).abs() < 1e-6 && (u.elev - v.elev).abs() < 1e-6);
            assert!((u.curve_len - v.curve_len).abs() < 1e-9);
        }
    }

    /// Cross sections and a ground profile draw as views and export back.
    #[test]
    fn landxml_sections_and_ground_profile_roundtrip() {
        const XML: &str = r#"<LandXML><Units><Metric linearUnit="meter"/></Units>
<Alignments><Alignment name="CL" staStart="100">
 <CoordGeom><Line><Start>0 0</Start><End>0 200</End></Line></CoordGeom>
 <Profile><ProfSurf name="EG"><PntList2D>100 50 200 52.5 300 51</PntList2D></ProfSurf></Profile>
 <CrossSects>
  <CrossSect sta="150"><CrossSectSurf name="Existing"><PntList2D>-20 50 0 51 20 50.5</PntList2D></CrossSectSurf>
   <CrossSectSurf name="Design" desc="road"><PntList2D>-6 51.2 0 51.32 6 51.2</PntList2D></CrossSectSurf></CrossSect>
  <CrossSect sta="250"><CrossSectSurf name="Existing"><PntList2D>-20 52 20 51</PntList2D></CrossSectSurf></CrossSect>
 </CrossSects></Alignment></Alignments></LandXML>"#;
        let dir = std::env::temp_dir().join(format!("ls_lx_xs_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let src = dir.join("xs.xml");
        std::fs::write(&src, XML).expect("write");
        let mut host = TestHost::new();
        import_landxml(&mut host, &format!("LS_LANDXML {}", src.display()));
        assert!(host.log.iter().any(|l| l.contains("(1 profile(s), 2 cross section(s))")), "{:?}", host.log);
        assert_eq!(host.doc.entities().filter(|e| e.common().layer == "LS-XSEC-EG").count(), 2);
        assert!(host.doc.entities().any(|e| matches!(e, EntityType::Text(t) if t.value == "STA 0+150.000")));

        let out = dir.join("out.xml");
        export_landxml(&mut host, &format!("LS_LANDXMLOUT {} alignments", out.display()));
        let back = landxml::read_document(&std::fs::read_to_string(&out).expect("exported"));
        let _ = std::fs::remove_dir_all(&dir);
        let orig = landxml::read_document(XML);
        let (a, b) = (&orig.alignments[0], &back.alignments[0]);
        assert_eq!(b.sta_start, 100.0);
        assert_eq!(a.cross_sections.len(), b.cross_sections.len());
        for (x, y) in a.cross_sections.iter().zip(&b.cross_sections) {
            assert_eq!(x.sta, y.sta);
            assert_eq!(x.surfaces.len(), y.surfaces.len());
            for (s, t) in x.surfaces.iter().zip(&y.surfaces) {
                assert_eq!((s.name.as_str(), s.desc.as_str()), (t.name.as_str(), t.desc.as_str()));
                for (u, v) in s.points.iter().zip(&t.points) {
                    assert!((u[0] - v[0]).abs() < 1e-9 && (u[1] - v[1]).abs() < 1e-9, "{u:?} {v:?}");
                }
            }
        }
        let alignment::ProfileData::Surface(eg) = &b.profiles[0].data else {
            panic!("surface profile")
        };
        assert!((eg[1][0] - 200.0).abs() < 1e-9 && (eg[1][1] - 52.5).abs() < 1e-9);
    }

    /// Lot labels carry name and area; moving the label moves the exported
    /// label point, and an unlabelled parcel exports its area centroid.
    #[test]
    fn landxml_lot_labels() {
        let dir = std::env::temp_dir().join(format!("ls_lx_lbl_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let src = dir.join("lot.xml");
        std::fs::write(&src, LANDXML_DOC).expect("write");
        let mut host = TestHost::new();
        import_landxml(&mut host, &format!("LS_LANDXML {} parcels", src.display()));
        // Imperial file: area in square feet and acres.
        assert!(host.doc.entities().any(|e| matches!(e, EntityType::Text(t)
            if t.value == "557 sq ft (0.013 ac)")), "area label");

        // Move the name label 5 east, 3 north.
        for e in host.doc.entities_mut() {
            if let EntityType::Text(t) = e {
                if t.value == "Lot 1" {
                    t.insertion_point.x += 5.0;
                    t.insertion_point.y += 3.0;
                }
            }
        }
        let out = dir.join("out.xml");
        export_landxml(&mut host, &format!("LS_LANDXMLOUT {} parcels", out.display()));
        let back = landxml::read_document(&std::fs::read_to_string(&out).expect("exported"));
        let c = back.parcels[0].center.expect("label point");
        assert!((c[0] - 215.0).abs() < 1e-9 && (c[1] - 113.0).abs() < 1e-9, "{c:?}");

        // Untagged boundary: centroid of a 30 x 20 rectangle.
        let mut host = TestHost::new();
        let rect = [(0.0, 0.0, 0.0), (30.0, 0.0, 0.0), (30.0, 20.0, 0.0), (0.0, 20.0, 0.0)];
        let mut ent = EntityType::LwPolyline(lwpolyline(&rect, true));
        ent.common_mut().layer = "LOTS".to_string();
        host.add_entity(ent);
        export_landxml(&mut host, &format!("LS_LANDXMLOUT {} parcels metric", out.display()));
        let back = landxml::read_document(&std::fs::read_to_string(&out).expect("exported"));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(back.parcels[0].center, Some([15.0, 10.0]));
    }

    /// An untagged closed LWPolyline on a property/lot layer exports as a
    /// parcel; an open one, or one on another layer, does not.
    #[test]
    fn landxml_export_untagged_property_boundary() {
        let mut host = TestHost::new();
        let square = [(0.0, 0.0, 0.0), (30.0, 0.0, 0.0), (30.0, 20.0, 0.0), (0.0, 20.0, 0.0)];
        for (layer, closed) in [("PROPERTY_LINE", true), ("PROPERTY_LINE", false), ("ROAD", true)] {
            let mut ent = EntityType::LwPolyline(lwpolyline(&square, closed));
            ent.common_mut().layer = layer.to_string();
            host.add_entity(ent);
        }
        let out = std::env::temp_dir().join(format!("ls_lx_prop_{}.xml", std::process::id()));
        export_landxml(&mut host, &format!("LS_LANDXMLOUT {} parcels metric", out.display()));
        let back = landxml::read_document(&std::fs::read_to_string(&out).expect("exported"));
        let _ = std::fs::remove_file(&out);
        assert_eq!(back.parcels.len(), 1);
        assert_eq!(back.parcels[0].name, "Parcel 1");
        assert!((back.parcels[0].area.unwrap() - 600.0).abs() < 1e-9);
    }

    /// Opt-in (`LANDXML_SAMPLE`, see the engine test of the same name): the
    /// real Land Desktop subdivision survives import → drawing → export with
    /// every lot's area intact.
    #[test]
    fn landxml_sample_roundtrip_through_drawing() {
        let Ok(path) = std::env::var("LANDXML_SAMPLE") else {
            return;
        };
        let mut host = TestHost::new();
        import_landxml(&mut host, &format!("LS_LANDXML {path} points parcels alignments"));
        let orig = landxml::read_document(&std::fs::read_to_string(&path).unwrap());
        if orig.parcels.iter().any(|p| p.area.is_some()) {
            assert!(host.log.iter().any(|l| l.contains("computed area matches")), "{:?}", host.log);
        }
        let out = std::env::temp_dir().join(format!("ls_lx_sample_{}.xml", std::process::id()));
        export_landxml(&mut host, &format!("LS_LANDXMLOUT {} points parcels alignments", out.display()));
        let back = landxml::read_document(&std::fs::read_to_string(&out).expect("exported"));
        let _ = std::fs::remove_file(&out);
        assert_eq!(back.points.len(), orig.points.len());
        assert_eq!(back.parcels.len(), orig.parcels.len());
        for (b, o) in back.parcels.iter().zip(&orig.parcels) {
            assert_eq!(b.name, o.name);
            let (ab, ao) = (landxml::chain_area(&b.segments), landxml::chain_area(&o.segments));
            assert!((ab - ao).abs() < 1e-6 * ao.max(1.0), "{}: {ab} vs {ao}", o.name);
        }
        assert_eq!(back.alignments.len(), orig.alignments.len());
        for (b, o) in back.alignments.iter().zip(&orig.alignments) {
            assert_eq!(b.name, o.name);
            assert!((b.length() - o.length()).abs() < 1e-6, "{}", o.name);
            assert_eq!(b.profiles.len(), o.profiles.len(), "{}", o.name);
            for (p, q) in b.profiles.iter().zip(&o.profiles) {
                let (p, q) = (p.points(), q.points());
                assert_eq!(p.len(), q.len());
                assert!(p.iter().zip(&q).all(|(u, v)| (u[0] - v[0]).abs() < 1e-6 && (u[1] - v[1]).abs() < 1e-6));
            }
            assert_eq!(b.cross_sections.len(), o.cross_sections.len(), "{}", o.name);
            for (x, y) in b.cross_sections.iter().zip(&o.cross_sections) {
                assert_eq!(x.surfaces.len(), y.surfaces.len(), "{} sta {}", o.name, y.sta);
                for (s, t) in x.surfaces.iter().zip(&y.surfaces) {
                    assert!(s.points.iter().zip(&t.points).all(|(u, v)| (u[0] - v[0]).abs() < 1e-6 && (u[1] - v[1]).abs() < 1e-6));
                }
            }
        }
        eprintln!(
            "sample: {} points, {} parcels, {} alignments, {} profiles, {} sections, {} entities",
            back.points.len(),
            back.parcels.len(),
            back.alignments.len(),
            back.alignments.iter().map(|a| a.profiles.len()).sum::<usize>(),
            back.alignments.iter().map(|a| a.cross_sections.len()).sum::<usize>(),
            host.doc.entities().count()
        );
    }

    /// LS_RW5 reduces a raw file into labelled Land Survey points and reports
    /// each setup's QC; `preview` draws nothing.
    #[test]
    fn rw5_import_draws_points_and_reports_setups() {
        const RAW: &str = "\
JB,NMDEMO,DT10-23-2010,TM23:14:07
MO,AD0,UN1,SF1.000000,EC0,EO0.0,AU0
LS,HI1.651,HR0.107
NEZ,OP27,200.000,200.000,50.000
OC,OP27,N 200.0000,E 200.0000,EL50.0000,--TH CN
BK,OP27,BP99,BS180.00000,BC0.00000
SS,OP27,FP33,AR202.41320,ZE90.11050,SD45.2564,--TH CN
SS,OP27,FP40,AR90.00000,ZE90.00000,SD10.0000,--OIP LOT 28
LS,HI1.655,HR0.107
OC,OP33,N 241.7529,E 217.4590,EL51.3981,--TH CN
BK,OP33,BP27,BS202.41320,BC0.00000
BR,OP33,BP27,AR0.00000,ZE93.43130,SD45.3524
SS,OP33,FP27,AR0.00000,ZE93.43130,SD45.3524,--CHK 27
";
        let path = std::env::temp_dir().join(format!("ls_rw5_{}.raw", std::process::id()));
        std::fs::write(&path, RAW).expect("write raw");

        let mut host = TestHost::new();
        import_rw5(&mut host, &format!("LS_RW5 {} preview", path.display()));
        assert_eq!(host.doc.entities().count(), 0);
        assert!(host.log.iter().any(|l| l.contains("3 point(s) from 2 setup(s)")), "{:?}", host.log);

        let mut host = TestHost::new();
        import_rw5(&mut host, &format!("LS_RW5 {}", path.display()));
        let _ = std::fs::remove_file(&path);
        let points: Vec<_> = host
            .doc
            .entities()
            .filter_map(|e| match e {
                EntityType::Point(p) => Some((named_tag(e, XDATA_POINT).unwrap().0, p.location, e.common().layer.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(points.len(), 3);
        let p33 = points.iter().find(|p| p.0 == "33").unwrap();
        assert!((p33.1.y - 241.7529).abs() < 5e-5 && (p33.1.x - 217.4590).abs() < 5e-5);
        assert_eq!(p33.2, "LS-PT-TH");
        assert!(points.iter().any(|p| p.0 == "40" && p.2 == "LS-PT-OIP"));
        // QC: FieldGenius' stored station agrees, the BS check and the check
        // shot back to 27 are reported.
        assert!(host.log.iter().any(|l| l.contains("setup 33 -> BS 27") && l.contains("stored vs ours dN +0.0000")), "{:?}", host.log);
        assert!(host.log.iter().any(|l| l.contains("BS check dHD +0.0007")), "{:?}", host.log);
        assert!(host.log.iter().any(|l| l.contains("repeat of 27")), "{:?}", host.log);
    }

    #[test]
    fn peel_keywords_keeps_spaced_paths() {
        let (p, k) = peel_keywords("C:/My Data/a b.xml Points preview", &["points", "preview"]);
        assert_eq!(p, "C:/My Data/a b.xml");
        assert_eq!(k, vec!["preview".to_string(), "points".to_string()]);
        assert_eq!(peel_keywords("\"x y.xml\"", &["points"]).0, "x y.xml");
    }

    #[test]
    fn utc_now_formats() {
        let (d, t) = utc_now();
        assert_eq!((d.len(), &d[4..5], &d[7..8]), (10, "-", "-"));
        assert_eq!((t.len(), &t[2..3]), (8, ":"));
        assert!(d.as_str() >= "2026-01-01");
    }

    /// LS_INVERSE draw labels with the current MicroSurvey styles, and
    /// LS_LABELSTYLE LOAD switches them to a job's config.json styles. One
    /// test, because the label styles live in process-global plugin state.
    #[test]
    fn inverse_draw_uses_msannotate_label_styles() {
        fn labels(host: &TestHost) -> Vec<(String, String, Color, f64)> {
            host.doc
                .entities()
                .filter_map(|e| match e {
                    EntityType::Text(t) => Some((
                        t.value.clone(),
                        e.common().layer.clone(),
                        e.common().color,
                        t.height,
                    )),
                    _ => None,
                })
                .collect()
        }

        let mut host = TestHost::new();
        label_style_cmd(&mut host, "LS_LABELSTYLE RESET");
        // 36 m due east.
        inverse(&mut host, "LS_INVERSE 1000 2000 1000 2036 draw");
        let got = labels(&host);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].0, "N90\u{b0}00'00\"E");
        assert_eq!((got[0].1.as_str(), got[0].2), ("BEARING1", Color::from_index(1)));
        assert_eq!(got[1].0, "36.00");
        assert_eq!((got[1].1.as_str(), got[1].2), ("DISTANCE1", Color::from_index(3)));

        let job = std::env::temp_dir().join(format!("ls_msj_{}", std::process::id()));
        std::fs::create_dir_all(&job).expect("create job dir");
        std::fs::write(
            job.join("config.json"),
            r#"{"bearingstyleindex":1,
                "bearingstyles":[{"name":"BEARING1"},
                  {"name":"BEARING2","layername":"BRG","color":2,"leroy":80,"quadrantspacing":1}],
                "distancestyleindex":0,
                "distancestyles":[{"name":"DISTANCE1","decimalplaces":3,"layername":"DST"}]}"#,
        )
        .expect("write config.json");

        let mut host = TestHost::new();
        label_style_cmd(&mut host, &format!("LS_LABELSTYLE LOAD {}", job.display()));
        label_style_cmd(&mut host, "LS_LABELSTYLE SCALE 500");
        inverse(&mut host, "LS_INVERSE 0 0 20 -0.6 draw");
        label_style_cmd(&mut host, "LS_LABELSTYLE RESET");
        let _ = std::fs::remove_dir_all(&job);

        let got = labels(&host);
        // quadrantspacing 1, plus MSCAD's space-padding of 1-digit degrees.
        assert_eq!(got[0].0, "N  1\u{b0}43'06\" W");
        assert_eq!((got[0].1.as_str(), got[0].2), ("BRG", Color::from_index(2)));
        assert!((got[0].3 - 1.016).abs() < 1e-9, "leroy 80 at 1:500 = 1.016 m");
        assert_eq!(got[1].0, "20.009");
        assert_eq!(got[1].1, "DST");
        assert!((got[1].3 - 0.762).abs() < 1e-9, "leroy 60 at 1:500 = 0.762 m");
    }

    /// LS_IMPORTPLAN with plat2json `--vectorize trace` output: the chain
    /// imports as one closed LWPolyline, the flattened duplicate `lines` are
    /// skipped, and independent lines / texts still import.
    #[test]
    fn import_plan_polylines_become_lwpolylines_and_dedupe_lines() {
        // A closed triangle chain, its three flattened duplicates (one
        // reversed, as Hough/tracing may emit), one unrelated line, one text.
        let json = r#"{
            "lines": [
                [0.0, 0.0, 10.5, 0.0, "PROPERTY_LINE"],
                [10.5, 0.0, 10.5, 7.25, "PROPERTY_LINE"],
                [0.0, 0.0, 10.5, 7.25, "PROPERTY_LINE"],
                [100.0, 100.0, 200.0, 200.0, "EASEMENT"]
            ],
            "arcs": [],
            "circles": [],
            "texts": [[5.0, 5.0, "N88%%d16'E", "BEARING"]],
            "polylines": [
                [[0.0, 0.0], [10.5, 0.0], [10.5, 7.25], [0.0, 0.0], "PROPERTY_LINE"]
            ]
        }"#;
        let path = std::env::temp_dir().join(format!("ls_plan_{}.json", std::process::id()));
        std::fs::write(&path, json).expect("write plan json");

        let mut host = TestHost::new();
        import_plan(&mut host, &format!("LS_IMPORTPLAN {}", path.display()));
        let _ = std::fs::remove_file(&path);

        let mut polylines = 0;
        let mut lines = 0;
        let mut texts = 0;
        for e in host.doc.entities() {
            match e {
                EntityType::LwPolyline(pl) => {
                    polylines += 1;
                    assert!(pl.is_closed, "ring chain must import closed");
                    assert_eq!(pl.vertices.len(), 3, "closing vertex must be dropped");
                    assert_eq!(pl.common.layer, "PROPERTY_LINE");
                }
                EntityType::Line(l) => {
                    lines += 1;
                    assert_eq!(l.common.layer, "EASEMENT", "only the non-duplicate line");
                }
                EntityType::Text(_) => texts += 1,
                other => panic!("unexpected entity: {other:?}"),
            }
        }
        assert_eq!((polylines, lines, texts), (1, 1, 1));

        let last = host.log.last().expect("report line");
        assert!(last.contains("3 entities"), "report: {last}");
        assert!(last.contains("3 element(s) already covered"), "report: {last}");
    }

    /// LS_IMPORTPLAN with the VERBATIM output of plat2json's fit_arcs.py for a
    /// synthetic plat (one straight 3-vertex chain + one 40 m arc, 10°→80°):
    /// both chains import as LWPolylines — the arc chain with its bulge — and
    /// every flattened duplicate (`lines` and the `arcs` entry) is skipped.
    /// This is the cross-repo interchange contract, pinned end to end.
    #[test]
    fn import_plan_fit_arcs_output_roundtrip() {
        let json = r#"{"lines": [[0.0, 0.0, 30.0, 0.0, "PROPERTY_LINE"], [30.0, 0.0, 30.0, 20.0, "PROPERTY_LINE"]], "arcs": [[100.0, 50.0, 40.0, 10.0, 80.0, "PROPERTY_LINE"]], "circles": [], "texts": [], "polylines": [[[0.0, 0.0], [30.0, 0.0], [30.0, 20.0], "PROPERTY_LINE"], [[139.3923, 56.9459, 0.315299], [106.9459, 89.3923], "PROPERTY_LINE"]]}"#;
        let path = std::env::temp_dir().join(format!("ls_arcs_{}.json", std::process::id()));
        std::fs::write(&path, json).expect("write plan json");

        let mut host = TestHost::new();
        import_plan(&mut host, &format!("LS_IMPORTPLAN {}", path.display()));
        let _ = std::fs::remove_file(&path);

        let mut chains = Vec::new();
        for e in host.doc.entities() {
            match e {
                EntityType::LwPolyline(pl) => chains.push(pl),
                other => panic!("everything should dedupe into polylines, got {other:?}"),
            }
        }
        assert_eq!(chains.len(), 2);
        let straight = chains.iter().find(|p| p.vertices.len() == 3).expect("straight chain");
        assert!(straight.vertices.iter().all(|v| v.bulge == 0.0));
        let arc = chains.iter().find(|p| p.vertices.len() == 2).expect("arc chain");
        assert_eq!(arc.vertices[0].bulge, 0.315299, "bulge = tan(70°/4)");
        assert_eq!(arc.vertices[1].bulge, 0.0);

        let last = host.log.last().expect("report line");
        assert!(last.contains("2 entities"), "report: {last}");
        assert!(
            last.contains("3 element(s) already covered"),
            "2 lines + 1 arc deduped: {last}"
        );
    }

    /// LS_IMPORTPLAN with the VERBATIM output of plat2json's cogo_assemble.py
    /// (plan-JSON `plan2/assoc-v1`) for a synthetic 4-course diamond plat that
    /// closes exactly: the `lines` block imports as Line entities on its layer,
    /// and the assoc-only keys (courses/rings/report/…) are ignored. Pins the
    /// back-compat path the emitter's docstring promises — its `lines` entries
    /// MUST carry the layer string or the whole file is rejected (the 0.4.4-era
    /// regression this guards against).
    #[test]
    fn import_plan_cogo_assemble_output_roundtrip() {
        let json = r#"{
 "schema": "plan2/assoc-v1",
 "unit": "ft",
 "source": {
  "key": "synth_assoc_key.json",
  "reads": "synth_assoc_reads.json",
  "pdf": "synthetic_diamond.pdf",
  "page": 1
 },
 "rotation_deg": 0.0,
 "rotation_inliers": "4/4",
 "scale_unit_per_pt": 0.499995,
 "scale_samples": 4,
 "courses": [
  {
   "seg": 0,
   "px": [
    300.0,
    500.0,
    500.0,
    300.0
   ],
   "flags": [],
   "bearing": "N 45°00'00\" E",
   "azimuth": 45.0,
   "sense": -1,
   "distance": 141.42,
   "distance_raw": "141.42'"
  },
  {
   "seg": 1,
   "px": [
    500.0,
    300.0,
    300.0,
    100.0
   ],
   "flags": [],
   "bearing": "N 45°00'00\" W",
   "azimuth": 315.0,
   "sense": -1,
   "distance": 141.42,
   "distance_raw": "141.42'"
  },
  {
   "seg": 2,
   "px": [
    300.0,
    100.0,
    100.0,
    300.0
   ],
   "flags": [],
   "bearing": "S 45°00'00\" W",
   "azimuth": 225.0,
   "sense": -1,
   "distance": 141.42,
   "distance_raw": "141.42'"
  },
  {
   "seg": 3,
   "px": [
    100.0,
    300.0,
    300.0,
    500.0
   ],
   "flags": [],
   "bearing": "S 45°00'00\" E",
   "azimuth": 135.0,
   "sense": -1,
   "distance": 141.42,
   "distance_raw": "141.42'"
  }
 ],
 "rings": [
  {
   "segs": [
    0,
    3,
    2,
    1
   ],
   "legs": 4,
   "perimeter": 565.68,
   "misclosure": 0.0,
   "theta_ring": 0.0,
   "precision": "exact"
  }
 ],
 "links": [],
 "lines": [
  [
   300.0,
   500.0,
   500.0,
   300.0,
   "PROPERTY_LINE"
  ],
  [
   500.0,
   300.0,
   300.0,
   100.0,
   "PROPERTY_LINE"
  ],
  [
   300.0,
   100.0,
   100.0,
   300.0,
   "PROPERTY_LINE"
  ],
  [
   100.0,
   300.0,
   300.0,
   500.0,
   "PROPERTY_LINE"
  ]
 ],
 "report": {
  "segments": 4,
  "edges": 4,
  "faces": 1,
  "faces_fully_coursed": 1,
  "courses": 4,
  "courses_clean": 4,
  "courses_flagged": 0,
  "rings_closed": 1,
  "rings_beyond_tolerance": 0,
  "links_evaluated": 0,
  "links_not_evaluable": 0,
  "links_beyond_tolerance": 0,
  "needs_human": false
 }
}"#;
        let path = std::env::temp_dir().join(format!("ls_assoc_{}.json", std::process::id()));
        std::fs::write(&path, json).expect("write plan json");

        let mut host = TestHost::new();
        import_plan(&mut host, &format!("LS_IMPORTPLAN {}", path.display()));
        let _ = std::fs::remove_file(&path);

        let mut lines = Vec::new();
        for e in host.doc.entities() {
            match e {
                EntityType::Line(l) => {
                    assert_eq!(l.common.layer, "PROPERTY_LINE");
                    lines.push(l);
                }
                other => panic!("assoc plans carry only lines, got {other:?}"),
            }
        }
        assert_eq!(lines.len(), 4, "one Line per planarized edge");

        let last = host.log.last().expect("report line");
        assert!(last.contains("4 entities on 1 layer(s)"), "report: {last}");
    }

    /// An `arcs` entry that does NOT match any bulged chain segment still
    /// imports as a standalone Arc entity (e.g. curve-table arcs from
    /// arc_refine.py alongside traced chains).
    #[test]
    fn import_plan_standalone_arc_still_imports() {
        let json = r#"{
            "arcs": [[500.0, 500.0, 25.0, 0.0, 45.0, "CURVE"]],
            "polylines": [[[139.3923, 56.9459, 0.315299], [106.9459, 89.3923], "PROPERTY_LINE"]]
        }"#;
        let path = std::env::temp_dir().join(format!("ls_arc2_{}.json", std::process::id()));
        std::fs::write(&path, json).expect("write plan json");

        let mut host = TestHost::new();
        import_plan(&mut host, &format!("LS_IMPORTPLAN {}", path.display()));
        let _ = std::fs::remove_file(&path);

        let mut arcs = 0;
        let mut polylines = 0;
        for e in host.doc.entities() {
            match e {
                EntityType::Arc(a) => {
                    arcs += 1;
                    assert_eq!(a.common.layer, "CURVE");
                }
                EntityType::LwPolyline(_) => polylines += 1,
                other => panic!("unexpected entity: {other:?}"),
            }
        }
        assert_eq!((polylines, arcs), (1, 1));
    }

    /// Stock acadrust (e88a9a6+, OCS #249): `ExtendedData::records` survive a
    /// DWG write/read with NO plugin-side codec. Regression guard for the fix
    /// that let us delete the old `xdata_persist` module.
    ///
    /// Layer half mirrors the fixed host contract: since OCS #252 (v0.7.4,
    /// `Scene::ensure_layer`) the host auto-registers a novel entity layer
    /// with a real handle on `add_entity`/`update_entity`. A bare CadDocument
    /// has no host, so the test registers it the same way, then asserts the
    /// registered layer survives the round-trip.
    #[test]
    fn stock_records_and_registered_layer_roundtrip() {
        let mut doc = CadDocument::default();
        register_ls_app_ids(&mut doc);
        let mut pt = EntityType::Point(CadPoint::at(Vector3::new(5000.0, 4000.0, 101.5)));
        pt.common_mut().layer = "LS-PT-IP".to_string();
        let mut rec = ExtendedDataRecord::new(XDATA_POINT);
        rec.add_value(XDataValue::String("101".into()));
        rec.add_value(XDataValue::String("IRON PIN".into()));
        pt.common_mut().extended_data.add_record(rec);
        // What the host's ensure_layer does since #252: table entry + real handle.
        let mut layer = codec::tables::layer::Layer::new("LS-PT-IP");
        layer.handle = doc.allocate_handle();
        let _ = doc.layers.add(layer);
        let h = doc.add_entity(pt).expect("add point");

        let bytes = DwgWriter::write_to_vec(&doc).expect("dwg write");
        let rt = DwgReader::from_stream(Cursor::new(bytes))
            .read()
            .expect("dwg read");
        let ent = rt.get_entity(h).expect("point entity");
        let got: Vec<String> = ent
            .common()
            .extended_data
            .get_record(XDATA_POINT)
            .expect("LANDSURVEY_POINT record lost — OCS #249 regressed?")
            .values
            .iter()
            .filter_map(|v| match v {
                XDataValue::String(s) => Some(s.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(got, vec!["101".to_string(), "IRON PIN".to_string()]);
        assert_eq!(
            ent.common().layer,
            "LS-PT-IP",
            "registered layer lost on round-trip — OCS #252/#67 regressed?"
        );
        assert!(rt.layers.contains("LS-PT-IP"), "layer table entry lost");
    }

    /// A small non-cocircular TIN: unit square footprint with a lifted corner
    /// plus an interior point, so the Delaunay result is unambiguous.
    fn sample_surface() -> Surface {
        let nodes = [
            [0.0, 0.0, 5.0],
            [10.0, 0.0, 5.0],
            [0.0, 10.0, 5.0],
            [10.0, 10.0, 8.0],
            [4.0, 3.0, 6.0],
        ];
        Surface::from_points(&nodes)
    }

    fn tag(ent: &mut EntityType, name: &str, kind: &str) {
        let mut rec = ExtendedDataRecord::new(XDATA_SURFACE);
        rec.add_value(XDataValue::String(name.to_string()));
        rec.add_value(XDataValue::String(kind.to_string()));
        ent.common_mut().extended_data.add_record(rec);
    }

    fn doc_with_tagged_mesh(name: &str) -> CadDocument {
        let mut doc = CadDocument::default();
        let mut ent = EntityType::Mesh(mesh_from_surface(&sample_surface()));
        ent.common_mut().layer = format!("LS-TIN-{name}");
        tag(&mut ent, name, "TIN");
        doc.add_entity(ent).expect("add mesh");
        doc
    }

    #[test]
    fn mesh_surface_roundtrip_is_exact() {
        let surf = sample_surface();
        let back = surface_from_mesh(&mesh_from_surface(&surf));
        assert_eq!(back.nodes, surf.nodes);
        assert_eq!(back.triangles, surf.triangles);
    }

    #[test]
    fn rebuilds_surface_from_tagged_mesh_case_insensitive() {
        let doc = doc_with_tagged_mesh("EG");
        let (name, surf) = find_surface_in_document(&doc, "eg").expect("found");
        assert_eq!(name, "EG");
        assert_eq!(surf.nodes, sample_surface().nodes);
        assert_eq!(surf.triangles, sample_surface().triangles);
        assert!(find_surface_in_document(&doc, "OTHER").is_none());
    }

    #[test]
    fn rebuilds_surface_from_tagged_tin_edge_lines() {
        // Draw the sample surface the way LS_SURFACE / draw_tin does: one
        // tagged Line per unique TIN edge.
        let surf = sample_surface();
        let mut doc = CadDocument::default();
        for e in surf.edges() {
            let a = surf.nodes[e[0]];
            let b = surf.nodes[e[1]];
            let mut ent = EntityType::Line(Line::from_points(
                Vector3::new(a[0], a[1], a[2]),
                Vector3::new(b[0], b[1], b[2]),
            ));
            tag(&mut ent, "TOPO1", "TIN");
            doc.add_entity(ent).expect("add line");
        }
        let (name, back) = find_surface_in_document(&doc, "topo1").expect("found");
        assert_eq!(name, "TOPO1");
        // Same point set through the same Delaunay builder → identical TIN
        // (node order may differ; compare counts + plan area + volume-bearing
        // sums instead of raw vectors).
        assert_eq!(back.nodes.len(), surf.nodes.len());
        assert_eq!(back.triangles.len(), surf.triangles.len());
        assert!((back.area_2d() - surf.area_2d()).abs() < 1e-9);
    }

    #[test]
    fn surface_survives_dwg_save_reopen() {
        // The end-to-end story on stock acadrust (e88a9a6+, OCS #249): tag →
        // DWG bytes → reopen → rebuild by name. Records encode to EED on write
        // and decode back on read with no plugin-side codec.
        let mut doc = doc_with_tagged_mesh("EG");
        register_ls_app_ids(&mut doc);
        let bytes = DwgWriter::write_to_vec(&doc).expect("dwg write");
        let rt = DwgReader::from_stream(Cursor::new(bytes))
            .read()
            .expect("dwg read");
        let (name, surf) = find_surface_in_document(&rt, "EG")
            .expect("surface recoverable after DWG round-trip");
        assert_eq!(name, "EG");
        assert_eq!(surf.nodes, sample_surface().nodes);
        assert_eq!(surf.triangles, sample_surface().triangles);
    }
}
