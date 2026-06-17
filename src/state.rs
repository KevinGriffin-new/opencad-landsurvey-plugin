//! Per-tab plugin state, stored via the host's `ensure_plugin_state` helper
//! under the manifest id (`opencad.landsurvey`).

/// Document-scoped Land Survey state.
#[derive(Default)]
pub struct LandSurveyState {
    /// Points imported into this document during the current session.
    pub imported: usize,
}
