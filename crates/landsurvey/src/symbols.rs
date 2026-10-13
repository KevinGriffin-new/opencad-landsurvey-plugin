//! Survey plan symbols: the British Columbia standard monument symbols of the
//! General Survey Instruction Regulation, B.C. Reg. 229/2001, s. 29(1),
//! drawn for this project from the regulation's table. Each has a found and
//! (usually) a placed form; found fills the inner shape.
//!
//! Geometry is in **plotted millimetres** about the symbol's insertion point,
//! so a symbol is drawn at `drawing units per plotted mm` (see
//! [`units_per_mm`]) times any library scale. Where the regulation gives two
//! dimensions they are read as outer and inner size; a single one is the
//! overall size.
//!
//! Names follow MicroSurvey's `ICM_*` monument blocks (`FSIP` found standard
//! iron post, `PSIP` placed, …), so an AutoMAP library row whose `SYMBOL` is
//! `BLOCKS\ICM_FSIP.DWG` resolves to the built-in symbol.

use std::f64::consts::PI;

/// One drawing primitive, in plotted mm about the insertion point.
#[derive(Debug, Clone, PartialEq)]
pub enum Prim {
    /// Circle outline.
    Circle { c: [f64; 2], r: f64 },
    /// Filled disc.
    Disc { c: [f64; 2], r: f64 },
    /// Line string (closed when `closed`).
    Poly { pts: Vec<[f64; 2]>, closed: bool },
    /// Filled convex polygon.
    Fill { pts: Vec<[f64; 2]> },
}

impl Prim {
    /// The primitive scaled by `s` and moved to `at`.
    pub fn place(&self, at: [f64; 2], s: f64) -> Prim {
        let p = |q: &[f64; 2]| [at[0] + q[0] * s, at[1] + q[1] * s];
        match self {
            Prim::Circle { c, r } => Prim::Circle { c: p(c), r: r * s },
            Prim::Disc { c, r } => Prim::Disc { c: p(c), r: r * s },
            Prim::Poly { pts, closed } => Prim::Poly { pts: pts.iter().map(p).collect(), closed: *closed },
            Prim::Fill { pts } => Prim::Fill { pts: pts.iter().map(p).collect() },
        }
    }
}

/// A plan symbol.
#[derive(Debug, Clone, PartialEq)]
pub struct Symbol {
    /// Short name, as in MicroSurvey's `ICM_<name>` blocks.
    pub name: &'static str,
    /// What it marks, as the regulation names it.
    pub title: &'static str,
    pub found: bool,
    pub prims: Vec<Prim>,
}

/// Every built-in symbol.
pub fn builtins() -> Vec<Symbol> {
    NAMES.iter().filter_map(|n| builtin(n)).collect()
}

const NAMES: [&str; 21] = [
    "FSCP", "PSCP", "FSRP", "PSRP", "FLP", "PLP", "FSC", "PSC", "FSIP", "PSIP", "FSWP", "PSWP", "CM",
    "DIP", "NSSIP", "NSRIP", "AI", "AP", "FTH", "PTH", "UMP",
];

/// The built-in symbol called `name` (case-insensitive; an `ICM_` prefix and a
/// `_WIPEOUT` suffix, as in MicroSurvey's block names, are ignored).
pub fn builtin(name: &str) -> Option<Symbol> {
    let upper = name.trim().to_ascii_uppercase();
    let n = upper.strip_prefix("ICM_").unwrap_or(&upper);
    let n = n.strip_suffix("_WIPEOUT").unwrap_or(n);
    let found = n.starts_with('F');
    let (name, title, prims): (&'static str, &'static str, Vec<Prim>) = match n {
        "FSCP" | "PSCP" => (
            if found { "FSCP" } else { "PSCP" },
            "standard concrete post (Type 1)",
            vec![circle(3.5), inner_circle(1.5, found)],
        ),
        "FSRP" | "PSRP" => (
            if found { "FSRP" } else { "PSRP" },
            "standard rock post (Type 2)",
            vec![circle(3.5), shape(square(2.0), found)],
        ),
        "FLP" | "PLP" => (if found { "FLP" } else { "PLP" }, "lead plug (Type 3)", vec![shape(square(2.5), found)]),
        "FSC" | "PSC" => {
            let r = 1.5;
            let d = r * (PI / 4.0).cos();
            let mut prims = vec![
                circle(2.0 * r),
                Prim::Poly { pts: vec![[-d, -d], [d, d]], closed: false },
                Prim::Poly { pts: vec![[-d, d], [d, -d]], closed: false },
            ];
            if found {
                prims.push(Prim::Fill { pts: sector(r, 45.0, 135.0) });
                prims.push(Prim::Fill { pts: sector(r, 225.0, 315.0) });
            }
            (if found { "FSC" } else { "PSC" }, "standard capped post (Type 4)", prims)
        }
        // MicroSurvey's `OIP` / `IP` codes name the iron post directly.
        "FSIP" | "PSIP" | "OIP" | "IP" => {
            let found = n == "FSIP" || n == "OIP";
            (
                if found { "FSIP" } else { "PSIP" },
                "standard iron post (Type 5)",
                vec![if found { Prim::Disc { c: [0.0, 0.0], r: 1.25 } } else { circle(2.5) }],
            )
        }
        "FSWP" | "PSWP" => {
            let h = 1.25;
            let mut prims = vec![
                Prim::Poly { pts: square(2.5), closed: true },
                Prim::Poly { pts: vec![[-h, -h], [h, h]], closed: false },
                Prim::Poly { pts: vec![[-h, h], [h, -h]], closed: false },
            ];
            if found {
                prims.push(Prim::Fill { pts: vec![[-h, h], [h, h], [0.0, 0.0]] });
                prims.push(Prim::Fill { pts: vec![[-h, -h], [0.0, 0.0], [h, -h]] });
            }
            (if found { "FSWP" } else { "PSWP" }, "standard wooden post (Type 6)", prims)
        }
        "CM" => ("CM", "control monument", vec![circle(3.5), Prim::Fill { pts: triangle(2.5) }]),
        "DIP" => (
            "DIP",
            "old pattern Dominion iron post",
            vec![Prim::Poly { pts: square(3.0), closed: true }, Prim::Fill { pts: square(1.5) }],
        ),
        "NSSIP" => (
            "NSSIP",
            "non-standard square iron post",
            vec![Prim::Poly { pts: square(3.0), closed: true }, Prim::Fill { pts: diamond(2.5, 2.5) }],
        ),
        "NSRIP" => (
            "NSRIP",
            "non-standard round iron post",
            vec![Prim::Poly { pts: square(3.0), closed: true }, Prim::Disc { c: [0.0, 0.0], r: 1.0 }],
        ),
        "AI" => (
            "AI",
            "angle iron",
            vec![Prim::Poly { pts: triangle(3.5), closed: true }, Prim::Fill { pts: triangle(1.0) }],
        ),
        "AP" => ("AP", "aluminum post", vec![Prim::Fill { pts: diamond(2.0, 2.5) }]),
        "FTH" | "PTH" => (if found { "FTH" } else { "PTH" }, "traverse hub", vec![shape(triangle(2.5), found)]),
        "UMP" => (
            "UMP",
            "unmarked measured point",
            vec![
                Prim::Poly { pts: vec![[-0.5, 0.0], [0.5, 0.0]], closed: false },
                Prim::Poly { pts: vec![[0.0, -0.5], [0.0, 0.5]], closed: false },
            ],
        ),
        _ => return None,
    };
    let found = matches!(name, "CM" | "DIP" | "NSSIP" | "NSRIP" | "AI" | "AP") || name.starts_with('F');
    Some(Symbol { name, title, found, prims })
}

/// The built-in symbol a library `SYMBOL` file names (`BLOCKS\ICM_FSIP.DWG`
/// → `FSIP`), when there is one.
pub fn for_symbol_file(path: &str) -> Option<Symbol> {
    let file = path.rsplit(['\\', '/']).next().unwrap_or(path);
    let stem = file.rsplit_once('.').map_or(file, |(s, _)| s);
    builtin(stem)
}

/// The built-in symbol for a code when no library is loaded, following the
/// MicroSurvey convention in the user's own library: an `O` prefix is found.
pub fn default_for_code(code: &str) -> Option<Symbol> {
    let name = match code.to_ascii_uppercase().as_str() {
        "OIP" => "FSIP",
        "IP" => "PSIP",
        "OCM" | "CM" => "CM",
        "ODIP" | "DIP" => "DIP",
        "OAI" | "AI" => "AI",
        "OAP" | "AP" => "AP",
        "OLP" => "FLP",
        "LP" => "PLP",
        "OCP" => "FSC",
        "CP" => "PSC",
        "OTH" => "FTH",
        "UMP" => "UMP",
        _ => return None,
    };
    builtin(name)
}

/// Drawing units per plotted millimetre at plot scale `1:scale`, in metres or
/// (US survey / international) feet.
pub fn units_per_mm(scale: f64, feet: bool) -> f64 {
    if feet {
        scale / 304.8
    } else {
        scale / 1000.0
    }
}

fn circle(d: f64) -> Prim {
    Prim::Circle { c: [0.0, 0.0], r: d / 2.0 }
}

fn inner_circle(d: f64, filled: bool) -> Prim {
    if filled {
        Prim::Disc { c: [0.0, 0.0], r: d / 2.0 }
    } else {
        circle(d)
    }
}

/// Outline, or filled when `filled`.
fn shape(pts: Vec<[f64; 2]>, filled: bool) -> Prim {
    if filled {
        Prim::Fill { pts }
    } else {
        Prim::Poly { pts, closed: true }
    }
}

fn square(w: f64) -> Vec<[f64; 2]> {
    let h = w / 2.0;
    vec![[-h, -h], [h, -h], [h, h], [-h, h]]
}

fn diamond(w: f64, h: f64) -> Vec<[f64; 2]> {
    vec![[0.0, -h / 2.0], [w / 2.0, 0.0], [0.0, h / 2.0], [-w / 2.0, 0.0]]
}

/// Equilateral triangle of side `s`, point up, centred on its centroid.
fn triangle(s: f64) -> Vec<[f64; 2]> {
    let h = s * 3f64.sqrt() / 2.0;
    vec![[-s / 2.0, -h / 3.0], [s / 2.0, -h / 3.0], [0.0, 2.0 * h / 3.0]]
}

/// A circular sector (convex for sweeps up to 180°), from `a0` to `a1` degrees.
fn sector(r: f64, a0: f64, a1: f64) -> Vec<[f64; 2]> {
    let n = 8;
    let mut pts = vec![[0.0, 0.0]];
    for i in 0..=n {
        let a = (a0 + (a1 - a0) * i as f64 / n as f64).to_radians();
        pts.push([r * a.cos(), r * a.sin()]);
    }
    pts
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Half the extent of a symbol (its "radius" in plotted mm).
    fn half_size(s: &Symbol) -> f64 {
        s.prims
            .iter()
            .map(|p| match p {
                Prim::Circle { c, r } | Prim::Disc { c, r } => c[0].hypot(c[1]) + r,
                Prim::Poly { pts, .. } | Prim::Fill { pts } => pts.iter().map(|q| q[0].abs().max(q[1].abs())).fold(0.0, f64::max),
            })
            .fold(0.0, f64::max)
    }

    #[test]
    fn all_builtins_resolve_with_regulation_sizes() {
        let all = builtins();
        assert_eq!(all.len(), 21);
        // Outer sizes from the s. 29(1) table (mm), halved.
        for (name, half) in [
            ("FSCP", 1.75), ("PSRP", 1.75), ("FLP", 1.25), ("PSC", 1.5), ("FSIP", 1.25), ("PSIP", 1.25),
            ("FSWP", 1.25), ("CM", 1.75), ("DIP", 1.5), ("NSSIP", 1.5), ("NSRIP", 1.5), ("AP", 1.25),
            ("UMP", 0.5),
        ] {
            let s = builtin(name).unwrap();
            assert!((half_size(&s) - half).abs() < 1e-9, "{name}: {}", half_size(&s));
        }
    }

    #[test]
    fn found_and_placed_differ_by_fill() {
        let filled = |s: &Symbol| s.prims.iter().filter(|p| matches!(p, Prim::Disc { .. } | Prim::Fill { .. })).count();
        for (f, p) in [("FSCP", "PSCP"), ("FSRP", "PSRP"), ("FLP", "PLP"), ("FSC", "PSC"), ("FSIP", "PSIP"), ("FSWP", "PSWP"), ("FTH", "PTH")] {
            let (f, p) = (builtin(f).unwrap(), builtin(p).unwrap());
            assert!(f.found && !p.found, "{} {}", f.name, p.name);
            assert!(filled(&f) > 0 && filled(&p) == 0, "{} / {}", f.name, p.name);
        }
    }

    #[test]
    fn microsurvey_names_and_codes() {
        assert_eq!(for_symbol_file("BLOCKS\\ICM_FSIP.DWG").unwrap().name, "FSIP");
        assert_eq!(for_symbol_file("blocks/icm_dip_wipeout.dwg").unwrap().name, "DIP");
        assert_eq!(for_symbol_file("C:\\x\\ICM_CM.dwg").unwrap().title, "control monument");
        assert!(for_symbol_file("BLOCKS\\MY_POST.DWG").is_none());
        assert_eq!(default_for_code("oip").unwrap().name, "FSIP");
        assert_eq!(default_for_code("IP").unwrap().name, "PSIP");
        assert_eq!(default_for_code("OCM").unwrap().name, "CM");
        assert!(default_for_code("TH").is_none());
    }

    #[test]
    fn placement_and_scale() {
        // A 2.5 mm iron post at 1:500 is 1.25 m across.
        let f = units_per_mm(500.0, false);
        let placed = builtin("PSIP").unwrap().prims[0].place([100.0, 200.0], f);
        assert_eq!(placed, Prim::Circle { c: [100.0, 200.0], r: 0.625 });
        assert!((units_per_mm(1200.0, true) - 3.937007874).abs() < 1e-8);
    }

    #[test]
    fn fills_are_convex() {
        for s in builtins() {
            for p in &s.prims {
                if let Prim::Fill { pts } = p {
                    let n = pts.len();
                    let cross = |i: usize| {
                        let (a, b, c) = (pts[i], pts[(i + 1) % n], pts[(i + 2) % n]);
                        (b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0])
                    };
                    let signs: Vec<bool> = (0..n).map(|i| cross(i) >= -1e-12).collect();
                    assert!(signs.iter().all(|&s| s) || signs.iter().all(|&s| !s), "{}", s.name);
                }
            }
        }
    }
}
