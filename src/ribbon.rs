//! The "Land Survey" ribbon tab. Pure data — the host renders these types.

use ocs_plugin_api::ribbon::{CadModule, IconKind, ModuleEvent, RibbonGroup, RibbonItem, ToolDef};

/// The ribbon tab shown when "Land Survey" is the active tab.
pub struct LandSurveyModule;

impl CadModule for LandSurveyModule {
    fn id(&self) -> &'static str {
        "landsurvey"
    }

    fn title(&self) -> &'static str {
        "Land Survey"
    }

    fn ribbon_groups(&self) -> &[RibbonGroup] {
        // Host contract since OCS 73b05e1: return a borrowed slice; build the
        // tree once and cache it (the canonical OnceLock pattern).
        static GROUPS: std::sync::OnceLock<Vec<RibbonGroup>> = std::sync::OnceLock::new();
        GROUPS.get_or_init(|| vec![
            RibbonGroup {
                title: "Points",
                tools: vec![
                    // The host pops a native file picker and dispatches
                    // "LS_PNEZD <path>" back with the path's original case.
                    RibbonItem::LargeTool(ToolDef {
                        id: "LS_PNEZD",
                        label: "Import PNEZD",
                        icon: IconKind::Glyph("\u{2295}"), // ⊕
                        event: ModuleEvent::PluginFileDialog {
                            command: "LS_PNEZD".to_string(),
                            title: "Import PNEZD points".to_string(),
                            filter_name: "PNEZD point file".to_string(),
                            extensions: vec!["csv".to_string(), "txt".to_string()],
                        },
                    }),
                    // Native picker -> "LS_RAW <path>": reduce a data
                    // collector raw file (RW5: FieldGenius / TDS / SurvCE;
                    // Sokkia SDR), the format detected from the content.
                    RibbonItem::LargeTool(ToolDef {
                        id: "LS_RAW",
                        label: "Import Raw",
                        icon: IconKind::Glyph("\u{2316}"), // ⌖ (instrument)
                        event: ModuleEvent::PluginFileDialog {
                            command: "LS_RAW".to_string(),
                            title: "Import data collector raw file".to_string(),
                            filter_name: "Raw data (RW5, FieldGenius, Sokkia SDR)".to_string(),
                            extensions: vec!["raw".to_string(), "rw5".to_string(), "sdr".to_string()],
                        },
                    }),
                    RibbonItem::LargeTool(ToolDef {
                        id: "LS_LIST",
                        label: "List Points",
                        icon: IconKind::Glyph("\u{2261}"), // ≡
                        event: ModuleEvent::Command("LS_LIST".to_string()),
                    }),
                    // Field-to-finish: connect coded points into figures.
                    RibbonItem::LargeTool(ToolDef {
                        id: "LS_FINISH",
                        label: "Linework",
                        icon: IconKind::Glyph("\u{2933}"), // ⤳
                        event: ModuleEvent::Command("LS_FINISH".to_string()),
                    }),
                    // Native picker -> "LS_LIBRARY LOAD <path>": the code
                    // library LS_FINISH connects and styles linework from.
                    RibbonItem::LargeTool(ToolDef {
                        id: "LS_LIBRARY",
                        label: "Code Library",
                        icon: IconKind::Glyph("\u{2637}"), // ☷
                        event: ModuleEvent::PluginFileDialog {
                            command: "LS_LIBRARY LOAD".to_string(),
                            title: "Load a feature code library".to_string(),
                            filter_name: "AutoMAP library (CSV)".to_string(),
                            extensions: vec!["csv".to_string()],
                        },
                    }),
                ],
            },
            RibbonGroup {
                title: "Plan",
                tools: vec![RibbonItem::LargeTool(ToolDef {
                    id: "LS_IMPORTPLAN",
                    label: "Import Plan",
                    icon: IconKind::Glyph("\u{25A6}"), // ▦
                    event: ModuleEvent::PluginFileDialog {
                        command: "LS_IMPORTPLAN".to_string(),
                        title: "Import recognized plan geometry".to_string(),
                        filter_name: "Plan geometry JSON".to_string(),
                        extensions: vec!["json".to_string()],
                    },
                })],
            },
            RibbonGroup {
                title: "COGO",
                tools: vec![RibbonItem::LargeTool(ToolDef {
                    id: "LS_INVERSE",
                    label: "Inverse",
                    icon: IconKind::Glyph("\u{2220}"), // ∠
                    event: ModuleEvent::Command("LS_INVERSE".to_string()),
                })],
            },
            RibbonGroup {
                title: "Transform",
                tools: vec![
                    RibbonItem::LargeTool(ToolDef {
                        id: "LS_RTS",
                        // Clicking prints usage; the user then types
                        // `LS_RTS <baseN> <baseE> <rot_deg> <scale> [<toN> <toE>]`.
                        label: "RTS",
                        icon: IconKind::Glyph("\u{27F3}"), // ⟳ rotate/translate/scale
                        event: ModuleEvent::Command("LS_RTS".to_string()),
                    }),
                    // Native picker -> "LS_HELMERT <path>": best-fit from control
                    // pairs (append " apply" on the command line to transform).
                    RibbonItem::LargeTool(ToolDef {
                        id: "LS_HELMERT",
                        label: "Helmert",
                        icon: IconKind::Glyph("\u{2245}"), // ≅ (best-fit)
                        event: ModuleEvent::PluginFileDialog {
                            command: "LS_HELMERT".to_string(),
                            title: "Helmert fit from control pairs".to_string(),
                            filter_name: "Control pairs (srcN,srcE,dstN,dstE)".to_string(),
                            extensions: vec!["csv".to_string(), "txt".to_string()],
                        },
                    }),
                ],
            },
            RibbonGroup {
                title: "Surface",
                tools: vec![
                    // Native picker -> "LS_SURFACE <path>": build + draw a TIN.
                    RibbonItem::LargeTool(ToolDef {
                        id: "LS_SURFACE",
                        label: "Build Surface",
                        icon: IconKind::Glyph("\u{25B3}"), // △ (TIN)
                        event: ModuleEvent::PluginFileDialog {
                            command: "LS_SURFACE".to_string(),
                            title: "Build surface from points or LandXML".to_string(),
                            filter_name: "PNEZD or LandXML surface".to_string(),
                            extensions: vec![
                                "csv".to_string(),
                                "txt".to_string(),
                                "xml".to_string(),
                                "landxml".to_string(),
                            ],
                        },
                    }),
                    // Native picker -> "LS_LANDXML <path>": import LandXML points,
                    // parcels, plan features and TIN surfaces (surfaces as Mesh,
                    // per OpenCADStudio issue #157).
                    RibbonItem::LargeTool(ToolDef {
                        id: "LS_LANDXML",
                        label: "LandXML",
                        icon: IconKind::Glyph("\u{229E}"), // ⊞ (mesh)
                        event: ModuleEvent::PluginFileDialog {
                            command: "LS_LANDXML".to_string(),
                            title: "Import LandXML points, parcels and surfaces".to_string(),
                            filter_name: "LandXML".to_string(),
                            extensions: vec!["xml".to_string(), "landxml".to_string()],
                        },
                    }),
                    // The host has no save dialog for plugins: clicking prints
                    // usage; the user then types `LS_LANDXMLOUT <file.xml>`.
                    RibbonItem::LargeTool(ToolDef {
                        id: "LS_LANDXMLOUT",
                        label: "LandXML Out",
                        icon: IconKind::Glyph("\u{21E5}"), // ⇥ (export)
                        event: ModuleEvent::Command("LS_LANDXMLOUT".to_string()),
                    }),
                    // Clicking prints usage; the user then types
                    // `LS_VOLUME <top.csv> <bottom.csv> [grid_step] [draw]`.
                    RibbonItem::LargeTool(ToolDef {
                        id: "LS_VOLUME",
                        label: "Volume",
                        icon: IconKind::Glyph("\u{2206}"), // ∆ (cut/fill)
                        event: ModuleEvent::Command("LS_VOLUME".to_string()),
                    }),
                    // Click to show command usage; supply the surface name in the command line.
                    RibbonItem::LargeTool(ToolDef {
                        id: "LS_CONTOURS",
                        label: "Contours",
                        icon: IconKind::Glyph("≋"), // contour lines
                        event: ModuleEvent::Command("LS_CONTOURS".to_string()),
                    }),
                    // Clicking prints usage; the user then types
                    // `LS_DATUM <surface> <elevation>`.
                    RibbonItem::LargeTool(ToolDef {
                        id: "LS_DATUM",
                        label: "To Datum",
                        icon: IconKind::Glyph("\u{2261}"), // ≡ (level plane)
                        event: ModuleEvent::Command("LS_DATUM".to_string()),
                    }),
                ],
            },
        ])
    }
}
