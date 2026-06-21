//! Minimal R12 (AC1009) ASCII DXF writer — pure string building, `std` only.
//!
//! Just enough to dump survey geometry for headless preview in any CAD viewer
//! (LibreCAD, ODA, AutoCAD, Open CAD Studio): named/coloured layers plus `LINE`,
//! `TEXT`, and `POINT` entities. Not a general DXF library — it emits the small
//! subset the `landsurvey-cli` needs so TIN/cut-fill output can be eyeballed
//! without launching the host app.

/// Accumulates layers and entities, then renders a complete DXF document.
#[derive(Debug, Default)]
pub struct DxfBuilder {
    layers: Vec<(String, i32)>, // (name, AutoCAD Color Index)
    lines: Vec<(String, [f64; 3], [f64; 3])>,
    texts: Vec<(String, [f64; 3], f64, String)>,
    points: Vec<(String, [f64; 3])>,
}

impl DxfBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a layer with an ACI colour (1=red 2=yellow 3=green 4=cyan
    /// 5=blue 6=magenta 7=white). Idempotent; the first colour wins.
    pub fn add_layer(&mut self, name: &str, color: i32) {
        if !self.layers.iter().any(|(n, _)| n == name) {
            self.layers.push((name.to_string(), color));
        }
    }

    pub fn line(&mut self, layer: &str, a: [f64; 3], b: [f64; 3]) {
        self.lines.push((layer.to_string(), a, b));
    }

    pub fn text(&mut self, layer: &str, pos: [f64; 3], height: f64, s: &str) {
        self.texts.push((layer.to_string(), pos, height, s.to_string()));
    }

    pub fn point(&mut self, layer: &str, p: [f64; 3]) {
        self.points.push((layer.to_string(), p));
    }

    /// Render the full DXF document (HEADER + TABLES + ENTITIES + EOF).
    pub fn build(&self) -> String {
        let mut s = String::new();

        // HEADER — declare R12 so viewers parse the entity subset.
        s.push_str("0\nSECTION\n2\nHEADER\n9\n$ACADVER\n1\nAC1009\n0\nENDSEC\n");

        // TABLES — a LAYER table so layers exist with colours.
        s.push_str("0\nSECTION\n2\nTABLES\n0\nTABLE\n2\nLAYER\n70\n");
        s.push_str(&format!("{}\n", self.layers.len().max(1)));
        if self.layers.is_empty() {
            // Always provide layer 0 so the table is well-formed.
            push_layer(&mut s, "0", 7);
        } else {
            for (name, color) in &self.layers {
                push_layer(&mut s, name, *color);
            }
        }
        s.push_str("0\nENDTAB\n0\nENDSEC\n");

        // ENTITIES.
        s.push_str("0\nSECTION\n2\nENTITIES\n");
        for (layer, a, b) in &self.lines {
            s.push_str("0\nLINE\n8\n");
            s.push_str(layer);
            s.push('\n');
            push_xyz(&mut s, 10, *a);
            push_xyz(&mut s, 11, *b);
        }
        for (layer, p) in &self.points {
            s.push_str("0\nPOINT\n8\n");
            s.push_str(layer);
            s.push('\n');
            push_xyz(&mut s, 10, *p);
        }
        for (layer, pos, height, value) in &self.texts {
            s.push_str("0\nTEXT\n8\n");
            s.push_str(layer);
            s.push('\n');
            push_xyz(&mut s, 10, *pos);
            s.push_str(&format!("40\n{:.6}\n1\n{}\n", height, value));
        }
        s.push_str("0\nENDSEC\n0\nEOF\n");
        s
    }
}

fn push_layer(s: &mut String, name: &str, color: i32) {
    s.push_str("0\nLAYER\n2\n");
    s.push_str(name);
    s.push_str(&format!("\n70\n0\n62\n{}\n6\nCONTINUOUS\n", color));
}

/// Emit a coordinate group at base code `base` (10/20/30 -> x/y/z).
fn push_xyz(s: &mut String, base: i32, p: [f64; 3]) {
    s.push_str(&format!(
        "{}\n{:.6}\n{}\n{:.6}\n{}\n{:.6}\n",
        base,
        p[0],
        base + 10,
        p[1],
        base + 20,
        p[2]
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emits_wellformed_dxf_with_entities() {
        let mut d = DxfBuilder::new();
        d.add_layer("LS-TIN-TOP", 3);
        d.line("LS-TIN-TOP", [0.0, 0.0, 0.0], [10.0, 0.0, 0.0]);
        d.text("LS-TIN-TOP", [5.0, 5.0, 0.0], 2.0, "HELLO");
        d.point("LS-TIN-TOP", [1.0, 1.0, 1.0]);
        let dxf = d.build();

        // Structure.
        assert!(dxf.starts_with("0\nSECTION\n2\nHEADER"));
        assert!(dxf.contains("2\nTABLES"));
        assert!(dxf.contains("2\nENTITIES"));
        assert!(dxf.trim_end().ends_with("EOF"));
        // Layer + entities present.
        assert!(dxf.contains("LS-TIN-TOP"));
        assert_eq!(dxf.matches("\nLINE\n").count(), 1);
        assert_eq!(dxf.matches("\nTEXT\n").count(), 1);
        assert_eq!(dxf.matches("\nPOINT\n").count(), 1);
        assert!(dxf.contains("\n1\nHELLO\n"));
    }

    #[test]
    fn duplicate_layer_keeps_first_color() {
        let mut d = DxfBuilder::new();
        d.add_layer("L", 3);
        d.add_layer("L", 1);
        assert_eq!(d.layers.len(), 1);
        assert_eq!(d.layers[0].1, 3);
    }
}
