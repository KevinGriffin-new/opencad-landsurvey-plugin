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

    fn ribbon_groups(&self) -> Vec<RibbonGroup> {
        vec![
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
                    RibbonItem::LargeTool(ToolDef {
                        id: "LS_LIST",
                        label: "List Points",
                        icon: IconKind::Glyph("\u{2261}"), // ≡
                        event: ModuleEvent::Command("LS_LIST".to_string()),
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
                    // Clicking prints usage; the user then types
                    // `LS_VOLUME <top.csv> <bottom.csv> [grid_step] [draw]`.
                    RibbonItem::LargeTool(ToolDef {
                        id: "LS_VOLUME",
                        label: "Volume",
                        icon: IconKind::Glyph("\u{2206}"), // ∆ (cut/fill)
                        event: ModuleEvent::Command("LS_VOLUME".to_string()),
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
        ]
    }
}
