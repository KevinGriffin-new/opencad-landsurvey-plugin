//! Per-tab plugin state, stored via the host's `ensure_plugin_state` helper
//! under the manifest id (`opencad.landsurvey`).

use landsurvey::surface::Surface;

/// A surface built/imported this session, retained so commands like `LS_VOLUME`
/// can operate on it by name without re-reading the source file.
pub struct StoredSurface {
    pub name: String,
    pub surface: Surface,
}

/// Document-scoped Land Survey state.
#[derive(Default)]
pub struct LandSurveyState {
    /// Points imported into this document during the current session.
    pub imported: usize,
    /// Surfaces built/imported this session, newest last.
    pub surfaces: Vec<StoredSurface>,
}

impl LandSurveyState {
    /// Store `surface` under `name`, replacing any existing surface with the
    /// same name (case-insensitive).
    pub fn put_surface(&mut self, name: &str, surface: Surface) {
        if let Some(slot) = self
            .surfaces
            .iter_mut()
            .find(|s| s.name.eq_ignore_ascii_case(name))
        {
            slot.surface = surface;
        } else {
            self.surfaces.push(StoredSurface {
                name: name.to_string(),
                surface,
            });
        }
    }

    /// Look up a stored surface by name (case-insensitive).
    pub fn get_surface(&self, name: &str) -> Option<&Surface> {
        self.surfaces
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case(name))
            .map(|s| &s.surface)
    }

    /// Names of the stored surfaces, in import order.
    pub fn surface_names(&self) -> Vec<&str> {
        self.surfaces.iter().map(|s| s.name.as_str()).collect()
    }
}
