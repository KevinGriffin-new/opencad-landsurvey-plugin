//! Keep Land Survey XDATA round-tripping through OCS DWG save/reopen.
//!
//! `acadrust` stores plugin XDATA in [`ExtendedData::records`] during editing,
//! but the DWG writer only serializes [`ExtendedData::raw_dwg_eed`]; on DWG
//! read the inverse happens: only `raw_dwg_eed` is populated. `HostApi::
//! write_record` registers the APPID but never encodes, so without this module
//! every `LANDSURVEY_*` tag (and any layer without a table entry — see
//! OpenCADStudio issue #67) is lost on the first save/reopen.
//!
//! Adapted from the HydroComplete plugin's `xdata_persist` module
//! (mf4633/opencad-hydrocomplete-plugin, GPL-3.0-only), with one fix: narrow
//! (pre-AC1021) string decoding preserves the bytes instead of dropping them —
//! Land Survey records are all strings (point number / description).
//!
//! [`hydrate_document`] decodes raw blobs back into records after open;
//! [`commit_document`] encodes records into raw blobs (and registers APPIDs +
//! layer-table entries) after every handled command, so whenever the user
//! saves, the raw bytes are already in place.

use acadrust::tables::layer::Layer as DocLayer;
use acadrust::tables::{AppId, TableEntry};
use acadrust::xdata::{ExtendedDataRecord, XDataValue};
use acadrust::{CadDocument, DxfVersion, Handle};
use ocs_plugin_api::host::HostApi;

use crate::dispatch::{XDATA_PLAN, XDATA_POINT, XDATA_SURFACE};

/// All Land Survey application names (must match `plugin.toml` `xdata_apps`).
pub const LS_XDATA_APPS: &[&str] = &[XDATA_POINT, XDATA_PLAN, XDATA_SURFACE];

fn is_ls_app(name: &str) -> bool {
    LS_XDATA_APPS.contains(&name)
}

/// AC1021+ (R2007+) DWGs store XDATA strings as UTF-16 code units.
fn dwg_strings_wide(doc: &CadDocument) -> bool {
    doc.version >= DxfVersion::AC1021
}

/// Register the Land Survey APPIDs so DWG/DXF writers can resolve app handles.
pub fn ensure_xdata_app_ids(doc: &mut CadDocument) {
    for &name in LS_XDATA_APPS {
        if !doc.app_ids.contains(name) {
            let mut app = AppId::new(name);
            app.set_handle(doc.allocate_handle());
            let _ = doc.app_ids.add(app);
        } else if doc.app_ids.get(name).is_some_and(|a| a.handle.is_null()) {
            let h = doc.allocate_handle();
            if let Some(app) = doc.app_ids.get_mut(name) {
                app.set_handle(h);
            }
        }
    }
}

/// Register layer table entries for every layer referenced by entities.
///
/// OCS/acadrust drops unknown layer names to `"0"` on DWG save when the layer
/// has no table entry with a real handle (OpenCADStudio issue #67) — without
/// this, the per-feature-code `LS-*` layers all collapse onto layer 0.
pub fn ensure_layers_for_entities(doc: &mut CadDocument) {
    let names: std::collections::HashSet<String> = doc
        .entities()
        .map(|e| e.common().layer.clone())
        .filter(|n| !n.is_empty())
        .collect();
    for name in names {
        if doc.layers.contains(&name) {
            continue;
        }
        let mut layer = DocLayer::new(&name);
        layer.handle = doc.allocate_handle();
        let _ = doc.layers.add(layer);
    }
}

// ── raw-EED codec ───────────────────────────────────────────────────────────
// Byte layout mirrors the DWG XDATA stream: a 1-byte value code, then the
// value. Strings: wide = [u16 len][UTF-16 units]; narrow = [u8 len][u16
// codepage][bytes].

fn encode_string(s: &str, wide: bool) -> Vec<u8> {
    let mut b = Vec::new();
    b.push(0);
    if wide {
        let units: Vec<u16> = s.encode_utf16().collect();
        b.extend_from_slice(&(units.len() as u16).to_le_bytes());
        for u in units {
            b.extend_from_slice(&u.to_le_bytes());
        }
    } else {
        let bytes = &s.as_bytes()[..s.len().min(255)];
        b.push(bytes.len() as u8);
        b.extend_from_slice(&0u16.to_le_bytes());
        b.extend_from_slice(bytes);
    }
    b
}

fn encode_value(value: &XDataValue, wide: bool) -> Vec<u8> {
    match value {
        XDataValue::String(s) => encode_string(s, wide),
        XDataValue::ControlString(s) => {
            let mut b = vec![2];
            b.push(if s == "}" { 1 } else { 0 });
            b
        }
        // Code 3 uses the same layout as 0 in practice.
        XDataValue::LayerName(s) => encode_string(s, wide),
        XDataValue::BinaryData(data) => {
            let mut b = vec![4, data.len().min(255) as u8];
            b.extend_from_slice(&data[..data.len().min(255)]);
            b
        }
        XDataValue::Handle(h) => {
            let mut b = vec![5];
            b.extend_from_slice(&h.value().to_le_bytes());
            b
        }
        XDataValue::Point3D(p)
        | XDataValue::Position3D(p)
        | XDataValue::Displacement3D(p)
        | XDataValue::Direction3D(p) => {
            let code = match value {
                XDataValue::Position3D(_) => 11,
                XDataValue::Displacement3D(_) => 12,
                XDataValue::Direction3D(_) => 13,
                _ => 10,
            };
            let mut b = vec![code];
            b.extend_from_slice(&p.x.to_le_bytes());
            b.extend_from_slice(&p.y.to_le_bytes());
            b.extend_from_slice(&p.z.to_le_bytes());
            b
        }
        XDataValue::Real(v) | XDataValue::Distance(v) | XDataValue::ScaleFactor(v) => {
            let code = match value {
                XDataValue::Distance(_) => 41,
                XDataValue::ScaleFactor(_) => 42,
                _ => 40,
            };
            let mut b = vec![code];
            b.extend_from_slice(&v.to_le_bytes());
            b
        }
        XDataValue::Integer16(v) => {
            let mut b = vec![70];
            b.extend_from_slice(&v.to_le_bytes());
            b
        }
        XDataValue::Integer32(v) => {
            let mut b = vec![71];
            b.extend_from_slice(&v.to_le_bytes());
            b
        }
    }
}

fn encode_record(record: &ExtendedDataRecord, wide: bool) -> Vec<u8> {
    let mut bytes = Vec::new();
    for value in &record.values {
        bytes.extend_from_slice(&encode_value(value, wide));
    }
    bytes
}

fn decode_string(bytes: &[u8], i: &mut usize, wide: bool) -> Option<String> {
    if wide {
        let n = u16::from_le_bytes([*bytes.get(*i)?, *bytes.get(*i + 1)?]) as usize;
        *i += 2;
        let mut units = Vec::with_capacity(n);
        for _ in 0..n {
            let u = u16::from_le_bytes([*bytes.get(*i)?, *bytes.get(*i + 1)?]);
            *i += 2;
            units.push(u);
        }
        String::from_utf16(&units).ok()
    } else {
        let n = *bytes.get(*i)? as usize;
        *i += 1 + 2; // length byte + codepage u16
        let s = bytes.get(*i..*i + n)?;
        *i += n;
        Some(String::from_utf8_lossy(s).into_owned())
    }
}

fn decode_value(bytes: &[u8], i: &mut usize, wide: bool) -> Option<XDataValue> {
    let code = *bytes.get(*i)?;
    *i += 1;
    match code {
        0 => decode_string(bytes, i, wide).map(XDataValue::String),
        2 => {
            let ctrl = *bytes.get(*i)?;
            *i += 1;
            Some(XDataValue::ControlString(if ctrl == 1 {
                "}".into()
            } else {
                "{".into()
            }))
        }
        3 => decode_string(bytes, i, wide).map(XDataValue::LayerName),
        4 => {
            let n = *bytes.get(*i)? as usize;
            *i += 1;
            let data = bytes.get(*i..*i + n)?.to_vec();
            *i += n;
            Some(XDataValue::BinaryData(data))
        }
        5 => {
            let h = u64::from_le_bytes(bytes.get(*i..*i + 8)?.try_into().ok()?);
            *i += 8;
            Some(XDataValue::Handle(Handle::new(h)))
        }
        10..=13 => {
            let x = f64::from_le_bytes(bytes.get(*i..*i + 8)?.try_into().ok()?);
            *i += 8;
            let y = f64::from_le_bytes(bytes.get(*i..*i + 8)?.try_into().ok()?);
            *i += 8;
            let z = f64::from_le_bytes(bytes.get(*i..*i + 8)?.try_into().ok()?);
            *i += 8;
            let p = acadrust::types::Vector3::new(x, y, z);
            Some(match code {
                11 => XDataValue::Position3D(p),
                12 => XDataValue::Displacement3D(p),
                13 => XDataValue::Direction3D(p),
                _ => XDataValue::Point3D(p),
            })
        }
        40 => {
            let v = f64::from_le_bytes(bytes.get(*i..*i + 8)?.try_into().ok()?);
            *i += 8;
            Some(XDataValue::Real(v))
        }
        41 => {
            let v = f64::from_le_bytes(bytes.get(*i..*i + 8)?.try_into().ok()?);
            *i += 8;
            Some(XDataValue::Distance(v))
        }
        42 => {
            let v = f64::from_le_bytes(bytes.get(*i..*i + 8)?.try_into().ok()?);
            *i += 8;
            Some(XDataValue::ScaleFactor(v))
        }
        70 => {
            let v = i16::from_le_bytes(bytes.get(*i..*i + 2)?.try_into().ok()?);
            *i += 2;
            Some(XDataValue::Integer16(v))
        }
        71 => {
            let v = i32::from_le_bytes(bytes.get(*i..*i + 4)?.try_into().ok()?);
            *i += 4;
            Some(XDataValue::Integer32(v))
        }
        _ => None,
    }
}

fn decode_record(app_name: &str, bytes: &[u8], wide: bool) -> Option<ExtendedDataRecord> {
    let mut rec = ExtendedDataRecord::new(app_name);
    let mut i = 0usize;
    while i < bytes.len() {
        let value = decode_value(bytes, &mut i, wide)?;
        rec.add_value(value);
    }
    if rec.is_empty() {
        return None;
    }
    Some(rec)
}

// ── document-level hydrate / commit ─────────────────────────────────────────

/// Populate parsed `records` from DWG `raw_dwg_eed` blobs (after open).
pub fn hydrate_document(doc: &mut CadDocument) {
    ensure_xdata_app_ids(doc);
    let wide = dwg_strings_wide(doc);
    let app_by_handle: std::collections::HashMap<u64, String> = doc
        .app_ids
        .iter()
        .map(|a| (a.handle.value(), a.name.clone()))
        .collect();
    for ent in doc.entities_mut() {
        let blobs: Vec<(u64, Vec<u8>)> = ent.common().extended_data.raw_dwg_eed.clone();
        for (app_handle, bytes) in blobs {
            let Some(name) = app_by_handle.get(&app_handle) else {
                continue;
            };
            if !is_ls_app(name) {
                continue;
            }
            let xd = &mut ent.common_mut().extended_data;
            if xd.get_record(name).is_some() {
                continue;
            }
            if let Some(rec) = decode_record(name, &bytes, wide) {
                xd.add_record(rec);
            }
        }
    }
}

/// Encode in-memory `LANDSURVEY_*` records into `raw_dwg_eed` for DWG save,
/// preserving other applications' blobs. Returns `true` when any entity's raw
/// bytes actually changed (callers use this to avoid dirtying the drawing on
/// read-only commands like `LS_LIST`).
pub fn commit_document(doc: &mut CadDocument) -> bool {
    ensure_xdata_app_ids(doc);
    ensure_layers_for_entities(doc);
    let wide = dwg_strings_wide(doc);
    let app_by_name: std::collections::HashMap<String, u64> = doc
        .app_ids
        .iter()
        .map(|a| (a.name.clone(), a.handle.value()))
        .collect();
    let ls_handles: std::collections::HashSet<u64> = LS_XDATA_APPS
        .iter()
        .filter_map(|name| app_by_name.get(*name).copied())
        .collect();

    let mut changed = false;
    for ent in doc.entities_mut() {
        let mut next: Vec<(u64, Vec<u8>)> = ent
            .common()
            .extended_data
            .raw_dwg_eed
            .iter()
            .filter(|(h, _)| !ls_handles.contains(h))
            .cloned()
            .collect();
        for record in ent.common().extended_data.records() {
            if !is_ls_app(&record.application_name) {
                continue;
            }
            let Some(app_handle) = app_by_name.get(&record.application_name).copied() else {
                continue;
            };
            next.push((app_handle, encode_record(record, wide)));
        }
        let xd = &mut ent.common_mut().extended_data;
        if xd.raw_dwg_eed != next {
            xd.raw_dwg_eed = next;
            changed = true;
        }
    }
    changed
}

// ── host wrappers (called around every dispatched command) ──────────────────

/// Decode our raw EED into records so commands see tags from a reopened DWG.
pub fn hydrate_host(host: &mut dyn HostApi) {
    hydrate_document(host.document_mut());
}

/// Re-encode records into raw EED so a subsequent host save persists them.
pub fn commit_host(host: &mut dyn HostApi) {
    if commit_document(host.document_mut()) {
        host.set_dirty();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use acadrust::types::Vector3;
    use acadrust::{DwgReader, DwgWriter, EntityType, Point as CadPoint};
    use std::io::Cursor;

    fn point_with_tag(number: &str, description: &str, layer: &str) -> EntityType {
        let mut pt = EntityType::Point(CadPoint::at(Vector3::new(5000.0, 4000.0, 101.5)));
        pt.common_mut().layer = layer.to_string();
        let mut rec = ExtendedDataRecord::new(XDATA_POINT);
        rec.add_value(XDataValue::String(number.to_string()));
        rec.add_value(XDataValue::String(description.to_string()));
        pt.common_mut().extended_data.add_record(rec);
        pt
    }

    fn point_record(ent: &EntityType) -> Vec<String> {
        ent.common()
            .extended_data
            .get_record(XDATA_POINT)
            .expect("LANDSURVEY_POINT record")
            .values
            .iter()
            .filter_map(|v| match v {
                XDataValue::String(s) => Some(s.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn encode_decode_roundtrip_wide_and_narrow() {
        let mut rec = ExtendedDataRecord::new(XDATA_POINT);
        rec.add_value(XDataValue::String("101".into()));
        rec.add_value(XDataValue::String("IRON PIN, NE corner".into()));
        for wide in [true, false] {
            let bytes = encode_record(&rec, wide);
            let back = decode_record(XDATA_POINT, &bytes, wide).expect("decode");
            assert_eq!(point_values(&back), vec!["101", "IRON PIN, NE corner"]);
        }
    }

    fn point_values(rec: &ExtendedDataRecord) -> Vec<String> {
        rec.values
            .iter()
            .filter_map(|v| match v {
                XDataValue::String(s) => Some(s.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn point_xdata_and_layer_survive_dwg_roundtrip() {
        let mut doc = CadDocument::default();
        let h = doc
            .add_entity(point_with_tag("101", "IRON PIN", "LS-PT-IP"))
            .expect("add point");
        commit_document(&mut doc);
        assert!(doc.layers.contains("LS-PT-IP"), "layer must be registered");
        assert!(
            doc.entities()
                .any(|e| !e.common().extended_data.raw_dwg_eed.is_empty()),
            "commit should populate raw_dwg_eed"
        );

        let bytes = DwgWriter::write_to_vec(&doc).expect("dwg write");
        let mut rt = DwgReader::from_stream(Cursor::new(bytes))
            .read()
            .expect("dwg read");

        assert!(
            rt.layers.contains("LS-PT-IP"),
            "LS-PT-IP layer lost on DWG round-trip"
        );
        let ent = rt.get_entity(h).expect("point entity");
        assert_eq!(ent.common().layer, "LS-PT-IP", "entity layer reset to 0");

        // Simulate a fresh open: only raw blobs, no parsed records.
        for ent in rt.entities_mut() {
            let raw = ent.common().extended_data.raw_dwg_eed.clone();
            ent.common_mut().extended_data.clear();
            ent.common_mut().extended_data.raw_dwg_eed = raw;
        }
        hydrate_document(&mut rt);

        let ent = rt.get_entity(h).expect("point entity");
        assert_eq!(
            point_record(ent),
            vec!["101".to_string(), "IRON PIN".to_string()],
            "LANDSURVEY_POINT record lost after DWG round-trip"
        );
    }

    #[test]
    fn commit_reports_change_only_when_bytes_differ() {
        let mut doc = CadDocument::default();
        doc.add_entity(point_with_tag("7", "REBAR", "LS-PT-RB"))
            .expect("add point");
        assert!(commit_document(&mut doc), "first commit encodes new bytes");
        assert!(
            !commit_document(&mut doc),
            "second commit with no edits must be a no-op"
        );
    }

    #[test]
    fn commit_preserves_foreign_app_blobs() {
        let mut doc = CadDocument::default();
        let h = doc
            .add_entity(point_with_tag("9", "HUB", "LS-PT-HUB"))
            .expect("add point");
        // A blob from some other application (unknown handle) must survive.
        let foreign = (0xDEAD_u64, vec![1, 2, 3]);
        doc.get_entity_mut(h)
            .unwrap()
            .common_mut()
            .extended_data
            .raw_dwg_eed
            .push(foreign.clone());
        commit_document(&mut doc);
        let ent = doc.get_entity(h).unwrap();
        assert!(
            ent.common().extended_data.raw_dwg_eed.contains(&foreign),
            "foreign EED blob must be preserved"
        );
    }
}
