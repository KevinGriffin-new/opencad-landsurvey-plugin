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
    match verb.as_str() {
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

/// `LS_PNEZD <path>` — import a PNEZD CSV as `Point` entities tagged with
/// `LANDSURVEY_POINT` XDATA (point number + description).
fn import_pnezd(host: &mut dyn HostApi, cmd: &str) {
    let arg = first_arg(cmd);
    if arg.is_empty() {
        host.push_info("Usage: LS_PNEZD <path-to-pnezd.csv>");
        return;
    }
    let text = match fs::read_to_string(arg) {
        Ok(t) => t,
        Err(e) => {
            host.push_error(&format!("LS_PNEZD: cannot read \"{arg}\": {e}"));
            return;
        }
    };

    let outcome = pnezd::parse(&text);
    if outcome.points.is_empty() {
        host.push_error(&format!(
            "LS_PNEZD: no valid points in \"{arg}\" ({} line(s) skipped).",
            outcome.skipped
        ));
        return;
    }

    host.push_undo("LS_PNEZD import");
    let mut added = 0usize;
    for p in &outcome.points {
        // World mapping: X = Easting, Y = Northing, Z = Elevation.
        let entity = EntityType::Point(CadPoint::at(Vector3::new(p.easting, p.northing, p.elevation)));
        let handle = host.add_entity(entity);

        // write_record registers the APPID so the tag round-trips through DWG/DXF.
        let mut rec = ExtendedDataRecord::new(XDATA_POINT);
        rec.add_value(XDataValue::String(p.number.clone()));
        rec.add_value(XDataValue::String(p.description.clone()));
        host.write_record(handle, rec);
        added += 1;
    }

    let total = {
        let st = ensure_plugin_state(host, PLUGIN_ID, LandSurveyState::default);
        st.imported += added;
        st.imported
    };

    // Survey points must be visible. PDMODE defaults to 0 (a 1px dot); promote
    // the document to a visible point style on first import, without clobbering
    // a style the user already chose (e.g. via the host's DDPTYPE).
    {
        let header = &mut host.document_mut().header;
        if header.point_display_mode == 0 {
            header.point_display_mode = 3; // '×' glyph
        }
        if header.point_display_size == 0.0 {
            header.point_display_size = 15.0; // world units
        }
    }

    host.bump_geometry();
    host.set_dirty();
    let skipped = if outcome.skipped > 0 {
        format!(", {} line(s) skipped", outcome.skipped)
    } else {
        String::new()
    };
    host.push_output(&format!(
        "LS_PNEZD: imported {added} point(s){skipped}. {total} this session."
    ));
}

/// `LS_INVERSE <N1> <E1> <N2> <E2>` — distance + bearing between two coords.
fn inverse(host: &mut dyn HostApi, cmd: &str) {
    let nums: Vec<f64> = cmd
        .split_whitespace()
        .skip(1)
        .filter_map(|t| t.parse::<f64>().ok())
        .collect();
    if nums.len() < 4 {
        host.push_info("Usage: LS_INVERSE <N1> <E1> <N2> <E2>");
        return;
    }
    let inv = cogo::inverse(nums[0], nums[1], nums[2], nums[3]);
    host.push_output(&format!(
        "LS_INVERSE: distance {:.4}, azimuth {:.4}\u{b0}, bearing {}",
        inv.distance,
        inv.azimuth_deg,
        cogo::azimuth_to_bearing(inv.azimuth_deg)
    ));
    // `LS_INVERSE <N1> <E1> <N2> <E2> anim` → export an animated-SVG explainer.
    if cmd.split_whitespace().any(|t| t.eq_ignore_ascii_case("anim")) {
        let svg = viz::inverse_anim_svg(nums[0], nums[1], nums[2], nums[3]);
        write_anim_file(host, "inverse", &svg);
    }
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
        Err(_) => {
            let avail = stored_surface_names(host);
            host.push_error(&format!(
                "LS_DATUM: no surface \"{}\" (not imported, not a readable file). \
                 Imported surfaces: [{}].",
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
        Err(_) => {
            let avail = stored_surface_names(host);
            host.push_error(&format!(
                "LS_VOLUME: no surface \"{top_tok}\" (not imported, not a readable file). \
                 Imported surfaces: [{}]. Build Surface first, then click Volume.",
                avail.join(", ")
            ));
            return;
        }
    };
    let bottom = match resolve_named_surface(host, &bot_tok) {
        Ok(s) => s,
        Err(_) => {
            let avail = stored_surface_names(host);
            host.push_error(&format!(
                "LS_VOLUME: no surface \"{bot_tok}\" (not imported, not a readable file). \
                 Imported surfaces: [{}].",
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
/// (by name, case-insensitive), otherwise a file path. Lets commands operate on
/// already-imported surfaces without re-entering coordinates.
fn resolve_named_surface(host: &mut dyn HostApi, token: &str) -> Result<Surface, String> {
    {
        let st = ensure_plugin_state(host, PLUGIN_ID, LandSurveyState::default);
        if let Some(s) = st.get_surface(token) {
            return Ok(s.clone());
        }
    }
    read_surface_quiet(token)
}

/// Names of surfaces stored this session, for "did you mean" error messages.
fn stored_surface_names(host: &mut dyn HostApi) -> Vec<String> {
    let st = ensure_plugin_state(host, PLUGIN_ID, LandSurveyState::default);
    st.surface_names().iter().map(|s| s.to_string()).collect()
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
    let nums: Vec<f64> = cmd
        .split_whitespace()
        .skip(1)
        .filter_map(|t| t.parse::<f64>().ok())
        .collect();
    if nums.len() < 4 {
        host.push_info("Usage: LS_RTS <baseN> <baseE> <rot_deg> <scale> [<toN> <toE>]");
        return;
    }
    let (bn, be, rot_deg, scale) = (nums[0], nums[1], nums[2], nums[3]);
    let (tn, te) = if nums.len() >= 6 { (nums[4], nums[5]) } else { (bn, be) };
    let rot = rot_deg.to_radians();

    host.push_undo("LS_RTS");
    let mut count = 0usize;
    for ent in host.document_mut().entities_mut() {
        let e = ent.as_entity_mut();
        // base -> origin, scale + rotate about origin, then origin -> destination.
        e.translate(Vector3::new(-be, -bn, 0.0));
        e.apply_scaling(scale);
        e.apply_rotation(Vector3::new(0.0, 0.0, 1.0), rot);
        e.translate(Vector3::new(te, tn, 0.0));
        count += 1;
    }
    if count == 0 {
        host.push_info("LS_RTS: no entities in the drawing to transform.");
        return;
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
    let pairs = transform::parse_control_pairs(&text);
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
        let mut count = 0usize;
        for ent in host.document_mut().entities_mut() {
            let e = ent.as_entity_mut();
            // E' = s*R*(E,N) + (c,d): scale & rotate about origin, then translate.
            e.apply_scaling(scale);
            e.apply_rotation(Vector3::new(0.0, 0.0, 1.0), rot);
            e.translate(Vector3::new(t.c, t.d, 0.0));
            count += 1;
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

/// `LS_LIST` — count entities carrying the `LANDSURVEY_POINT` XDATA record.
fn list_points(host: &mut dyn HostApi) {
    let count = host
        .document()
        .entities()
        .filter(|e| e.common().extended_data.get_record(XDATA_POINT).is_some())
        .count();
    host.push_output(&format!("LS_LIST: {count} Land Survey point(s) in drawing."));
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
