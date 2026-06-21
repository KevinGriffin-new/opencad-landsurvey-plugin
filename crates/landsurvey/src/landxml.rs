//! Minimal LandXML TIN-surface reader — `std` only, no XML dependency.
//!
//! Reads just the subset needed to turn a LandXML `<Surface>` into a
//! [`crate::surface::Surface`]: the `<Pnts>`/`<P>` point list and the
//! `<Faces>`/`<F>` triangle list. It is a targeted scanner, not a general XML
//! parser — enough for the TIN surfaces Civil 3D, MicroSurvey, and Softree
//! Terrain export.
//!
//! Conventions (matching the LandXML 1.2 spec and the reference reader):
//! * `<P id="k">` text is `northing easting elevation`; we store nodes as
//!   `[easting, northing, elevation]` to match [`crate::surface`] (X=E, Y=N).
//! * `<F>` text is three point **ids** (not positional indices); faces with the
//!   invisible flag `i="1"` are skipped (they pad the convex hull and would
//!   inflate area/volume).

use std::collections::HashMap;

use crate::surface::Surface;

/// A named surface read from LandXML.
#[derive(Debug, Clone)]
pub struct NamedSurface {
    pub name: String,
    pub surface: Surface,
}

/// Read every `<Surface>` in a LandXML document.
pub fn read_surfaces(xml: &str) -> Vec<NamedSurface> {
    let mut out = Vec::new();
    let starts = find_all(xml, "<Surface ");
    for (i, &s) in starts.iter().enumerate() {
        let end = starts.get(i + 1).copied().unwrap_or(xml.len());
        if let Some(ns) = parse_surface(&xml[s..end]) {
            out.push(ns);
        }
    }
    out
}

/// Read the first `<Surface>` in a LandXML document, if any.
pub fn read_first_surface(xml: &str) -> Option<NamedSurface> {
    read_surfaces(xml).into_iter().next()
}

/// True if `text` looks like a LandXML document.
pub fn looks_like_landxml(text: &str) -> bool {
    let head = &text[..text.len().min(512)];
    head.contains("<LandXML") || (head.contains("<?xml") && text.contains("<Surface"))
}

fn parse_surface(slice: &str) -> Option<NamedSurface> {
    // Name lives on the opening <Surface ...> tag.
    let open_end = slice.find('>')?;
    let name = attr(&slice[..open_end], "name").unwrap_or("Surface").to_string();

    // Points: scan within <Pnts> ... </Pnts>.
    let pnts = between(slice, "<Pnts", "</Pnts>").unwrap_or("");
    let mut nodes: Vec<[f64; 3]> = Vec::new();
    let mut id_to_index: HashMap<String, usize> = HashMap::new();
    let mut cur = 0;
    while let Some(rel) = pnts[cur..].find("<P") {
        let start = cur + rel;
        let after = pnts[start + 2..].chars().next();
        if after != Some(' ') && after != Some('>') {
            cur = start + 2; // e.g. some other tag starting with "<P..."
            continue;
        }
        let gt = match pnts[start..].find('>') {
            Some(g) => start + g,
            None => break,
        };
        let open = &pnts[start..gt];
        let text_end = match pnts[gt + 1..].find("</P>") {
            Some(e) => gt + 1 + e,
            None => break,
        };
        let text = &pnts[gt + 1..text_end];
        if let (Some(id), Some(node)) = (attr(open, "id"), parse_xyz(text)) {
            id_to_index.insert(id.to_string(), nodes.len());
            nodes.push(node);
        }
        cur = text_end + 4;
    }

    // Faces: scan within <Faces> ... </Faces>.
    let faces = between(slice, "<Faces", "</Faces>").unwrap_or("");
    let mut triangles: Vec<[usize; 3]> = Vec::new();
    let mut cur = 0;
    while let Some(rel) = faces[cur..].find("<F") {
        let start = cur + rel;
        let after = faces[start + 2..].chars().next();
        if after != Some(' ') && after != Some('>') {
            cur = start + 2;
            continue;
        }
        let gt = match faces[start..].find('>') {
            Some(g) => start + g,
            None => break,
        };
        let open = &faces[start..gt];
        let text_end = match faces[gt + 1..].find("</F>") {
            Some(e) => gt + 1 + e,
            None => break,
        };
        let text = &faces[gt + 1..text_end];
        cur = text_end + 4;

        if attr(open, "i") == Some("1") {
            continue; // invisible face — excluded from the surface
        }
        let ids: Vec<&str> = text.split_whitespace().collect();
        if ids.len() >= 3 {
            if let (Some(&a), Some(&b), Some(&c)) = (
                id_to_index.get(ids[0]),
                id_to_index.get(ids[1]),
                id_to_index.get(ids[2]),
            ) {
                triangles.push([a, b, c]);
            }
        }
    }

    if nodes.is_empty() || triangles.is_empty() {
        return None;
    }
    Some(NamedSurface {
        name,
        surface: Surface { nodes, triangles },
    })
}

/// Parse a `<P>` text body `northing easting [elevation]` into `[E, N, Z]`.
fn parse_xyz(text: &str) -> Option<[f64; 3]> {
    let mut it = text.split_whitespace();
    let northing: f64 = it.next()?.parse().ok()?;
    let easting: f64 = it.next()?.parse().ok()?;
    let elev: f64 = it.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
    Some([easting, northing, elev])
}

/// Value of attribute `name` (`name="value"`) in an opening-tag string.
fn attr<'a>(open_tag: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("{name}=\"");
    let i = open_tag.find(&needle)? + needle.len();
    let rest = &open_tag[i..];
    let j = rest.find('"')?;
    Some(&rest[..j])
}

/// Substring strictly between the end of `open` (its first `>` after the match)
/// and `close`.
fn between<'a>(s: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let o = s.find(open)?;
    let gt = s[o..].find('>')? + o + 1;
    let c = s[gt..].find(close)? + gt;
    Some(&s[gt..c])
}

fn find_all(s: &str, needle: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut cur = 0;
    while let Some(rel) = s[cur..].find(needle) {
        let at = cur + rel;
        out.push(at);
        cur = at + needle.len();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_inline_tin() {
        // A 10x10 square split into 2 triangles, one invisible face that would
        // double the area if (wrongly) included.
        let xml = r#"<?xml version="1.0"?>
<LandXML><Surfaces>
<Surface name="EG">
  <Definition surfType="TIN">
    <Pnts>
      <P id="0">0 0 5</P>
      <P id="1">0 10 5</P>
      <P id="2">10 10 5</P>
      <P id="3">10 0 5</P>
    </Pnts>
    <Faces>
      <F>0 1 2</F>
      <F>0 2 3</F>
      <F i="1">0 1 3</F>
    </Faces>
  </Definition>
</Surface>
</Surfaces></LandXML>"#;
        let ns = read_first_surface(xml).expect("a surface");
        assert_eq!(ns.name, "EG");
        assert_eq!(ns.surface.nodes.len(), 4);
        // Invisible face excluded -> exactly the 2 real triangles, area 100.
        assert_eq!(ns.surface.triangles.len(), 2);
        assert!((ns.surface.area_2d() - 100.0).abs() < 1e-9);
        // P order is "northing easting elev" -> node [E, N, Z]; P id=1 is N=0,E=10.
        assert_eq!(ns.surface.nodes[1], [10.0, 0.0, 5.0]);
        assert!(looks_like_landxml(xml));
    }
}
