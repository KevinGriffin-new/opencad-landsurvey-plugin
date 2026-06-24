//! Land Survey — an external Open CAD Studio add-on, built as a `cdylib` the
//! host loads at runtime.
//!
//! It depends only on `ocs_plugin_api` (with the `host` feature) and `acadrust`
//! (the host's entity model) — never on the `OpenCADStudio` binary — so it
//! targets the stable contract: a [`PluginManifest`], a [`CadModule`] ribbon
//! tab ([`ribbon`]), a [`BuiltinPlugin`] entry point, and the `export_plugin!`
//! C-ABI export. Domain math lives in the host-free `landsurvey` engine crate
//! (`crates/landsurvey/`); [`dispatch`] is the glue that wraps it in commands,
//! entities, and XDATA.

use ocs_plugin_api::host::{BuiltinPlugin, HostApi};
use ocs_plugin_api::manifest::{ApiVersion, PluginManifest};
use ocs_plugin_api::ribbon::CadModule;

mod dispatch;
mod ribbon;
mod state;

/// Reverse-DNS plugin id; the key for per-tab state and the plugins folder.
pub const PLUGIN_ID: &str = "opencad.landsurvey";

// Keep these fields in sync with `plugin.toml`.
static MANIFEST: PluginManifest = PluginManifest {
    id: PLUGIN_ID,
    name: "Land Survey",
    version: "0.3.2",
    description: "Survey points, PNEZD & LandXML import, TIN surfaces, earthwork volumes, COGO, coordinate transforms (RTS / Helmert), and animated SVG explainers",
    api_version: ApiVersion::CURRENT,
    ribbon_order: 50,
    // Both XDATA applications this plugin writes (see dispatch.rs). Declaring
    // them lets the host pre-register the APPIDs for DWG/DXF round-trip.
    xdata_apps: &[dispatch::XDATA_POINT, dispatch::XDATA_PLAN, dispatch::XDATA_SURFACE],
    command_prefixes: &["LS_"],
};

/// The plugin entry point handed to the host.
struct LandSurveyPlugin;

impl BuiltinPlugin for LandSurveyPlugin {
    fn manifest(&self) -> &'static PluginManifest {
        &MANIFEST
    }

    fn ribbon(&self) -> Box<dyn CadModule> {
        Box::new(ribbon::LandSurveyModule)
    }

    fn dispatch(&self, host: &mut dyn HostApi, cmd: &str) -> bool {
        dispatch::handle(host, cmd)
    }
}

// Emit the C-ABI symbols the host loader looks for.
ocs_plugin_api::export_plugin!(LandSurveyPlugin);
