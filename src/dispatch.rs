//! Command routing for the Land Survey add-on. All `LS_` commands land here via
//! `BuiltinPlugin::dispatch`. Geometry/COGO math lives in the host-free
//! `landsurvey` engine crate; this file is the glue that turns engine output
//! into `acadrust` entities + XDATA on the active document.

use std::fs;

use acadrust::xdata::{ExtendedDataRecord, XDataValue};
use acadrust::{Arc as CadArc, Circle, EntityType, Line, Point as CadPoint, Text, Vector3};

use ocs_plugin_api::host::{ensure_plugin_state, HostApi};

use crate::state::LandSurveyState;
use crate::PLUGIN_ID;

use landsurvey::{cogo, plan, pnezd};

/// XDATA application carrying survey metadata on a `Point` entity.
/// Record values: `[String(point_number), String(description)]`.
pub const XDATA_POINT: &str = "LANDSURVEY_POINT";

/// XDATA application tagging entities imported from a recognized plan.
/// Record values: `[String(source_filename)]`.
pub const XDATA_PLAN: &str = "LANDSURVEY_PLAN";

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
