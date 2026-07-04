//! Process-local plugin state. Since OCS 0.7.x the plugin runs in its own
//! runner process, and the host's `ensure_plugin_state` PANICS out-of-process
//! by design ("keep state in the plugin crate") — `dyn Any` can't cross the
//! IPC boundary. A process global is the prescribed replacement; the runner
//! process is per-plugin, so this is exactly plugin-scoped state.
//!
//! Scope change vs the old host-side store: state is now shared across tabs
//! (one runner serves every tab) instead of per-tab. For the surface store and
//! the LS_AUTOLABEL toggle that's acceptable — surfaces are keyed by name.

use std::sync::{Mutex, MutexGuard, OnceLock};

use landsurvey::surface::Surface;

static STATE: OnceLock<Mutex<LandSurveyState>> = OnceLock::new();

/// Lock the process-local Land Survey state. Dispatch is single-threaded per
/// command, but NEVER call this while an earlier guard from the same call
/// chain is still alive — `Mutex` is not reentrant.
pub fn state() -> MutexGuard<'static, LandSurveyState> {
    STATE
        .get_or_init(|| Mutex::new(LandSurveyState::default()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

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
    /// When true, `LS_PNEZD` imports points WITHOUT drawing number labels
    /// (toggled by `LS_AUTOLABEL OFF`). Default false = auto-labels on.
    pub suppress_labels: bool,
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
