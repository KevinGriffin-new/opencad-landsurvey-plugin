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
        ]
    }
}
