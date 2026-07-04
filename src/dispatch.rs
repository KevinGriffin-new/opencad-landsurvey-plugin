//! Command routing for the Land Survey add-on. All `LS_` commands land here via
//! `BuiltinPlugin::dispatch`. Geometry/COGO math lives in the host-free
//! `landsurvey` engine crate; this file is the glue that turns engine output
//! into `acadrust` entities + XDATA on the active document.

use std::fs;

use acadrust::xdata::{ExtendedDataRecord, XDataValue};
use acadrust::entities::Mesh;
use acadrust::{Arc as CadArc, Circle, EntityType, Line, Point as CadPoint, Text, Vector3};

use ocs_plugin_api::host::{ensure_plugin_state, HostApi};

use crate::state::LandSurveyState;
use crate::PLUGIN_ID;

use landsurvey::surface::{self, Surface};
use landsurvey::{cogo, landxml, plan, pnezd, resection, transform, viz};

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
                "Land Survey plugin ready. Commands: LS_PNEZD, LS_IMPORTPLAN, LS_INVERSE, LS_LIST.",
            );
            true
        }
        "LS_PNEZD" => {
            import_pnezd(host, cmd);
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

    let text = match fs::read_to_string(path) {
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

    // Label/marker sizing follows MicroSurvey MSannotate proportions: a SMALL
    // precise marker with the point number hugging it (~half a text-height
    // away). The number — not the marker — is the prominent element. Text
    // height scales to the survey extent so it stays legible at zoom-extents.
    let h = label_height(&outcome.points);
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
    let auto_label =
        !ensure_plugin_state(host, PLUGIN_ID, LandSurveyState::default).suppress_labels;

    host.push_undo("LS_PNEZD import");
    let mut added = 0usize;
    for p in &outcome.points {
        let layer = code_layer(p.code());
        // World mapping: X = Easting, Y = Northing, Z = Elevation.
        let mut pt =
            EntityType::Point(CadPoint::at(Vector3::new(p.easting, p.northing, p.elevation)));
        pt.common_mut().layer = layer.clone();
        let handle = host.add_entity(pt);

        // write_record registers the APPID so the tag round-trips through DWG/DXF.
        let mut rec = ExtendedDataRecord::new(XDATA_POINT);
        rec.add_value(XDataValue::String(p.number.clone()));
        rec.add_value(XDataValue::String(p.description.clone()));
        host.write_record(handle, rec);

        if auto_label {
            draw_point_label(host, p, h, off, &layer, detail);
        }
        added += 1;
    }

    let total = {
        let st = ensure_plugin_state(host, PLUGIN_ID, LandSurveyState::default);
        st.imported += added;
        st.imported
    };

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

/// `LS_AUTOLABEL [ON|OFF]` — toggle whether `LS_PNEZD` draws point-number
/// labels on import. With no argument, reports the current state.
fn auto_label_toggle(host: &mut dyn HostApi, cmd: &str) {
    let arg = first_arg(cmd).trim().to_ascii_uppercase();
    if !matches!(arg.as_str(), "" | "ON" | "OFF") {
        host.push_info("Usage: LS_AUTOLABEL [ON|OFF]");
        return;
    }
    let on = {
        let st = ensure_plugin_state(host, PLUGIN_ID, LandSurveyState::default);
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
    host: &mut dyn HostApi,
    p: &pnezd::PnezdPoint,
    h: f64,
    off: f64,
    layer: &str,
    show_detail: bool,
) {
    let x = p.easting + off;
    let y = p.northing + off;
    add_on_layer(
        host,
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
        add_on_layer(
            host,
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

    let h = (inv.distance / 20.0).max(1.0); // text height ~ 5% of the line
    let gap = 0.4 * h; // clearance between each label body and the line

    // Work in the TEXT's own frame so spacing is symmetric regardless of the
    // readable-flip. Text is baseline-anchored; glyphs rise from the baseline
    // toward `up` and read along `read`. Put each label's CENTRE at ±(gap+h/2)
    // along `up` (so its body clears the line by `gap` on its side), then step
    // back to the baseline (−h/2 along `up`) and centre it along the line
    // (−half-width along `read`).
    let read = (rot.cos(), rot.sin());
    let up = (-rot.sin(), rot.cos());
    let d = gap + 0.5 * h;

    let labels = [
        (cogo::azimuth_to_bearing(inv.azimuth_deg), 1.0_f64), // bearing, +up side
        (format!("{:.3}", inv.distance), -1.0),               // distance, −up side
    ];
    for (text, sign) in labels {
        let half_w = 0.5 * text.chars().count() as f64 * h * 0.6;
        let bx = mx + (sign * d - 0.5 * h) * up.0 - half_w * read.0;
        let by = my + (sign * d - 0.5 * h) * up.1 - half_w * read.1;
        add_on_layer(
            host,
            EntityType::Text(
                Text::with_value(text, Vector3::new(bx, by, 0.0))
                    .with_height(h)
                    .with_rotation(rot),
            ),
            LAYER,
        );
    }

    host.bump_geometry();
    host.set_dirty();
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
        let st = ensure_plugin_state(host, PLUGIN_ID, LandSurveyState::default);
        st.put_surface(&name, surf);
    }
    host.push_output(&format!(
        "LS_SURFACE: stored \"{name}\" — {npts} pts, {ntri} triangles, {edges} TIN edges on \
         layer {layer}; plan area {area:.3}. (Use it in LS_VOLUME / LS_DATUM by name.)"
    ));
}

/// `LS_LANDXML <path>` (and the host's `LANDXMLIMPORT` verb) — import LandXML
/// TIN surface(s) as `Mesh` entities. Per OpenCADStudio issue #157: the entity
/// type is `Mesh`; `<P>` is `northing easting [elev]` → world X=E/Y=N/Z=Z; units
/// are imported as-is; invisible `<F i="1">` faces are skipped — all handled in
/// the engine parser (`landxml`). Each surface is drawn on `LS-TIN-<NAME>`,
/// tagged `LANDSURVEY_SURFACE`, and retained by name for LS_VOLUME / LS_DATUM.
fn import_landxml(host: &mut dyn HostApi, cmd: &str) {
    let arg = first_arg(cmd);
    if arg.is_empty() {
        host.push_info("Usage: LS_LANDXML <path-to-surface.xml>  (LandXML TIN surface)");
        return;
    }
    let text = match fs::read_to_string(arg) {
        Ok(t) => t,
        Err(e) => {
            host.push_error(&format!("LS_LANDXML: cannot read \"{arg}\": {e}"));
            return;
        }
    };
    if !landxml::looks_like_landxml(&text) {
        host.push_error(&format!("LS_LANDXML: \"{arg}\" does not look like LandXML."));
        return;
    }
    let surfaces = landxml::read_surfaces(&text);
    if surfaces.is_empty() {
        host.push_error(&format!("LS_LANDXML: no TIN surface found in \"{arg}\"."));
        return;
    }

    host.push_undo("LS_LANDXML import");
    let mut total_tris = 0usize;
    let mut names: Vec<String> = Vec::new();
    for ns in &surfaces {
        let layer = format!("LS-TIN-{}", ns.name);
        let mesh = mesh_from_surface(&ns.surface);
        tag_surface(host, EntityType::Mesh(mesh), &layer, &ns.name, "TIN");
        total_tris += ns.surface.triangles.len();
        names.push(ns.name.clone());
        // Retain for LS_VOLUME / LS_DATUM by name (same as LS_SURFACE).
        let st = ensure_plugin_state(host, PLUGIN_ID, LandSurveyState::default);
        st.put_surface(&ns.name, ns.surface.clone());
    }
    host.bump_geometry();
    host.set_dirty();
    host.push_output(&format!(
        "LS_LANDXML: imported {} surface(s) [{}] as Mesh — {total_tris} triangle(s) total. \
         (Use in LS_VOLUME / LS_DATUM by name.)",
        surfaces.len(),
        names.join(", ")
    ));
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
    let mut nl = 0usize;
    for s in &contour {
        let ent = EntityType::Line(Line::from_points(
            Vector3::new(s[0][0], s[0][1], elev),
            Vector3::new(s[1][0], s[1][1], elev),
        ));
        tag_surface(host, ent, "LS-DATUM", "DATUM", "CONTOUR");
        nl += 1;
    }
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
    let text = fs::read_to_string(path).map_err(|e| format!("cannot read \"{path}\": {e}"))?;
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
        let st = ensure_plugin_state(host, PLUGIN_ID, LandSurveyState::default);
        if let Some(s) = st.get_surface(token) {
            return Ok(s.clone());
        }
    }
    if let Some((name, surf)) = find_surface_in_document(host.document(), token) {
        // Cache under the canonical tagged name so later commands skip the scan.
        let st = ensure_plugin_state(host, PLUGIN_ID, LandSurveyState::default);
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
fn find_surface_in_document(doc: &acadrust::CadDocument, token: &str) -> Option<(String, Surface)> {
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
        let st = ensure_plugin_state(host, PLUGIN_ID, LandSurveyState::default);
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
    let mut n = 0;
    for e in surf.edges() {
        let a = surf.nodes[e[0]];
        let b = surf.nodes[e[1]];
        let ent = EntityType::Line(Line::from_points(
            Vector3::new(a[0], a[1], a[2]),
            Vector3::new(b[0], b[1], b[2]),
        ));
        tag_surface(host, ent, layer, name, "TIN");
        n += 1;
    }
    n
}

/// Draw plan-view 2-D segments (e.g. the cut/fill boundary) on `layer`.
fn draw_segments(
    host: &mut dyn HostApi,
    segs: &[[[f64; 2]; 2]],
    layer: &str,
    name: &str,
) -> usize {
    for s in segs {
        let ent = EntityType::Line(Line::from_points(
            Vector3::new(s[0][0], s[0][1], 0.0),
            Vector3::new(s[1][0], s[1][1], 0.0),
        ));
        tag_surface(host, ent, layer, name, "CUTFILL");
    }
    segs.len()
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
    let mut rec = ExtendedDataRecord::new(XDATA_SURFACE);
    rec.add_value(XDataValue::String(name.to_string()));
    rec.add_value(XDataValue::String(kind.to_string()));
    host.write_record(handle, rec);
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

    let text = match fs::read_to_string(path) {
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
    let text = match fs::read_to_string(arg) {
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
    } else if shots.len() == 3 {
        let (a, b, c) = (shots[0].known, shots[1].known, shots[2].known);
        let sep = |x: f64, y: f64| (x - y).rem_euclid(360.0).min((y - x).rem_euclid(360.0));
        let (ra, rb, rc) = (shots[0].direction_deg, shots[1].direction_deg, shots[2].direction_deg);
        let ang = [sep(rb, rc), sep(rc, ra), sep(ra, rb)]; // [∠BPC, ∠CPA, ∠APB]
        let p = match resection::resection_three_point(a, b, c, ang) {
            Ok(p) => p,
            Err(e) => {
                host.push_error(&format!("LS_RESECT: {e}"));
                return;
            }
        };
        host.push_output("LS_RESECT — angle-only three-point (Tienstra):");
        host.push_output(&format!("  station (E,N) : {:.4}, {:.4}", p.0, p.1));
        host.push_output(&format!(
            "  subtended     : BPC {:.4}\u{b0}, CPA {:.4}\u{b0}, APB {:.4}\u{b0}",
            ang[0], ang[1], ang[2]
        ));
        (p, "angle-only three-point (Tienstra)".to_string())
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
/// `plan2cad` pipeline's JSON) faithfully: each element becomes an entity on
/// its named layer, tagged with `LANDSURVEY_PLAN` XDATA (source filename).
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

    for (x1, y1, x2, y2, layer) in &plan.lines {
        let ent = EntityType::Line(Line::from_points(
            Vector3::new(*x1, *y1, 0.0),
            Vector3::new(*x2, *y2, 0.0),
        ));
        place(host, ent, layer, &source);
        layers.insert(layer.clone());
        n += 1;
    }
    for (cx, cy, r, start_deg, end_deg, layer) in &plan.arcs {
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
    host.push_output(&format!(
        "LS_IMPORTPLAN: {n} entities on {} layer(s) from {source}.",
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
    use acadrust::tables::{AppId, TableEntry};
    use acadrust::{CadDocument, DwgReader, DwgWriter};
    use std::io::Cursor;

    /// Register the LANDSURVEY_* APPIDs with real handles, as the fixed host
    /// `ensure_app_id` does on `write_record` (OCS #249). A standalone test
    /// document has no host, so the writer needs this done by hand for the
    /// EED app references to resolve.
    fn register_ls_app_ids(doc: &mut CadDocument) {
        for name in [XDATA_POINT, XDATA_PLAN, XDATA_SURFACE] {
            if !doc.app_ids.contains(name) {
                let mut app = AppId::new(name);
                app.set_handle(doc.allocate_handle());
                let _ = doc.app_ids.add(app);
            }
        }
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
        let mut layer = acadrust::tables::layer::Layer::new("LS-PT-IP");
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
