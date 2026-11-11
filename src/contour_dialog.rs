//! Native configuration dialog for `LS_CONTOURS`.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use ocs_plugin_api::dialog::{DialogControl, DialogDef, DialogResponse, DialogRow, DialogValue};
use ocs_plugin_api::host::{CommandStep, InteractiveCommand};

pub const INTERNAL_COMMAND: &str = "LS_CONTOURS_APPLY";
const SURFACE_ID: &str = "surface";
const INTERVAL_ID: &str = "minor_interval";
const MAJOR_ID: &str = "major_every";
const MAJOR_COLOR_ID: &str = "major_color";
const MINOR_COLOR_ID: &str = "minor_color";

/// A compact, named ACI palette. The two selectors use this same palette
/// independently; the index stored in a contour layer remains a CAD ACI color.
pub const ACI_PALETTE: &[(i16, &str)] = &[
    (1, "Rojo"),
    (2, "Amarillo"),
    (3, "Verde"),
    (4, "Cian"),
    (5, "Azul"),
    (6, "Magenta"),
    (8, "Gris oscuro"),
    (9, "Gris claro"),
    (30, "Naranja"),
    (140, "Azul celeste"),
];

#[derive(Debug, Clone, PartialEq)]
pub struct ContourConfig {
    pub surface: String,
    pub minor_interval: f64,
    pub major_every: i64,
    pub major_color: i16,
    pub minor_color: i16,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResponseError {
    Cancelled,
    Invalid(String),
}

/// Parameters carried once between the modal callback and the follow-up dispatch.
static PENDING: OnceLock<Mutex<HashMap<u64, ContourConfig>>> = OnceLock::new();
static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn pending() -> &'static Mutex<HashMap<u64, ContourConfig>> {
    PENDING.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn build_dialog(surfaces: &[String], error: Option<&str>) -> DialogDef {
    let mut dialog = DialogDef::new("Contour Wizard")
        .submit_label("Generar")
        .cancel_label("Cancelar")
        .width(440.0)
        .with_row(DialogRow::single(DialogControl::Choice {
            id: SURFACE_ID.into(),
            label: "Superficie TIN".into(),
            options: surfaces.to_vec(),
            default_index: 0,
        }))
        .with_row(DialogRow::new(vec![
            DialogControl::Text {
                id: INTERVAL_ID.into(),
                label: "Equidistancia".into(),
                default: "0.5".into(),
            },
            DialogControl::Text {
                id: MAJOR_ID.into(),
                label: "Frecuencia principales".into(),
                default: "5".into(),
            },
        ]))
        .with_control(DialogControl::InfoText {
            text: "Representación de curvas (ACI). Generar cambiará el color de LS-CONTOUR-MAJ y LS-CONTOUR-MIN, conservando sus demás propiedades.".into(),
            align_h: None,
        })
        .with_row(DialogRow::new(vec![
            DialogControl::Choice {
                id: MAJOR_COLOR_ID.into(),
                label: "Principales".into(),
                options: palette_options(),
                default_index: 0,
            },
            DialogControl::Choice {
                id: MINOR_COLOR_ID.into(),
                label: "Secundarias".into(),
                options: palette_options(),
                default_index: 4,
            },
        ]));
    if let Some(error) = error {
        dialog = dialog.with_control(DialogControl::InfoText {
            text: error.to_owned(),
            align_h: None,
        });
    }
    dialog
}

pub fn validate_response(
    response: DialogResponse,
    available_surfaces: &[String],
) -> Result<ContourConfig, ResponseError> {
    let DialogResponse::Submitted(values) = response else {
        return Err(ResponseError::Cancelled);
    };
    let surface = match values.get(SURFACE_ID) {
        Some(DialogValue::Choice { index, text })
            if *index < available_surfaces.len()
                && available_surfaces[*index].eq_ignore_ascii_case(text) =>
        {
            available_surfaces[*index].clone()
        }
        _ => {
            return Err(ResponseError::Invalid(
                "Selecciona una superficie TIN válida.".into(),
            ))
        }
    };
    let interval_text = match values.get(INTERVAL_ID) {
        Some(DialogValue::Text(text)) => text.trim(),
        _ => {
            return Err(ResponseError::Invalid(
                "Equidistancia ausente o con tipo incorrecto.".into(),
            ))
        }
    };
    let minor_interval = interval_text
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && *v > 0.0)
        .ok_or_else(|| {
            ResponseError::Invalid("La equidistancia debe ser finita y mayor que cero.".into())
        })?;
    let major_text = match values.get(MAJOR_ID) {
        Some(DialogValue::Text(text)) => text.trim(),
        _ => {
            return Err(ResponseError::Invalid(
                "Frecuencia ausente o con tipo incorrecto.".into(),
            ))
        }
    };
    let major_every = major_text
        .parse::<i64>()
        .ok()
        .filter(|v| *v > 0)
        .ok_or_else(|| {
            ResponseError::Invalid("La frecuencia debe ser un entero positivo.".into())
        })?;
    let major_color = validate_palette_choice(values.get(MAJOR_COLOR_ID)).ok_or_else(|| {
        ResponseError::Invalid("Selecciona un color ACI válido para curvas principales.".into())
    })?;
    let minor_color = validate_palette_choice(values.get(MINOR_COLOR_ID)).ok_or_else(|| {
        ResponseError::Invalid("Selecciona un color ACI válido para curvas secundarias.".into())
    })?;
    Ok(ContourConfig {
        surface,
        minor_interval,
        major_every,
        major_color,
        minor_color,
    })
}

fn palette_options() -> Vec<String> {
    ACI_PALETTE
        .iter()
        .map(|(aci, name)| {
            let rgb = codec::Color::from_index(*aci)
                .rgb()
                .map(|(r, g, b)| format!(" #{r:02X}{g:02X}{b:02X}"))
                .unwrap_or_default();
            format!("ACI {aci} — {name}{rgb}")
        })
        .collect()
}

fn validate_palette_choice(value: Option<&DialogValue>) -> Option<i16> {
    let Some(DialogValue::Choice { index, text }) = value else {
        return None;
    };
    let (aci, _) = ACI_PALETTE.get(*index)?;
    (palette_options().get(*index)? == text).then_some(*aci)
}

/// Store config under a monotonic, nonzero opaque token. Never overwrites an entry.
pub fn store_config(config: ContourConfig) -> Option<u64> {
    let id = NEXT_ID
        .try_update(
            std::sync::atomic::Ordering::Relaxed,
            std::sync::atomic::Ordering::Relaxed,
            |current| current.checked_add(1),
        )
        .ok()?;
    if id == 0 {
        return None;
    }
    let mut values = pending().lock().unwrap_or_else(|e| e.into_inner());
    if values.contains_key(&id) {
        return None;
    }
    values.insert(id, config);
    Some(id)
}

pub fn take_config(id: u64) -> Option<ContourConfig> {
    if id == 0 {
        return None;
    }
    pending()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&id)
}

pub fn parse_internal_id(command: &str) -> Option<u64> {
    let mut parts = command.split_whitespace();
    if !parts.next()?.eq_ignore_ascii_case(INTERNAL_COMMAND) {
        return None;
    }
    let id_text = parts.next()?;
    if parts.next().is_some() || id_text.is_empty() || !id_text.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let id = id_text.parse::<u64>().ok()?;
    (id != 0).then_some(id)
}

pub struct ContourDialogCommand {
    surfaces: Vec<String>,
    message: Option<String>,
    closed: bool,
}

impl ContourDialogCommand {
    pub fn new(surfaces: Vec<String>) -> Self {
        Self {
            surfaces,
            message: None,
            closed: false,
        }
    }
}

impl InteractiveCommand for ContourDialogCommand {
    fn prompt(&self) -> String {
        "Configurar curvas de nivel".into()
    }
    fn on_point(&mut self, _pt: [f64; 3]) -> CommandStep {
        CommandStep::Cancel
    }
    fn modal_dialog(&self) -> Option<DialogDef> {
        (!self.closed).then(|| build_dialog(&self.surfaces, self.message.as_deref()))
    }
    fn on_dialog_response(&mut self, response: DialogResponse) -> CommandStep {
        match validate_response(response, &self.surfaces) {
            Err(ResponseError::Cancelled) => {
                self.closed = true;
                CommandStep::Cancel
            }
            Err(ResponseError::Invalid(message)) => {
                self.message = Some(message);
                CommandStep::NeedPoint
            }
            Ok(config) => match store_config(config) {
                Some(id) => {
                    self.closed = true;
                    CommandStep::Dispatch(format!("{INTERNAL_COMMAND} {id}"))
                }
                None => {
                    self.message =
                        Some("No se pudo preparar la generación; inténtalo de nuevo.".into());
                    CommandStep::NeedPoint
                }
            },
        }
    }
}

#[cfg(test)]
fn response_map(
    surface_index: usize,
    surface_name: &str,
    interval: &str,
    major: &str,
) -> HashMap<String, DialogValue> {
    let mut values = HashMap::from([
        (
            SURFACE_ID.into(),
            DialogValue::Choice {
                index: surface_index,
                text: surface_name.into(),
            },
        ),
        (INTERVAL_ID.into(), DialogValue::Text(interval.into())),
        (MAJOR_ID.into(), DialogValue::Text(major.into())),
    ]);
    let options = palette_options();
    values.insert(
        MAJOR_COLOR_ID.into(),
        DialogValue::Choice {
            index: 0,
            text: options[0].clone(),
        },
    );
    values.insert(
        MINOR_COLOR_ID.into(),
        DialogValue::Choice {
            index: 4,
            text: options[4].clone(),
        },
    );
    values
}

#[cfg(test)]
mod tests {
    use super::*;

    fn surfaces() -> Vec<String> {
        vec!["EG".into(), "FG".into()]
    }

    #[test]
    fn dialog_has_surface_list_and_defaults() {
        let dialog = build_dialog(&surfaces(), None);
        assert_eq!(dialog.submit_label, "Generar");
        assert_eq!(dialog.cancel_label, "Cancelar");
        let controls: Vec<_> = dialog
            .rows
            .iter()
            .flat_map(|row| &row.controls)
            .collect();
        assert_eq!(dialog.rows.len(), 4, "compact form content rows");
        assert_eq!(dialog.rows[1].controls.len(), 2, "numeric fields share a row");
        assert_eq!(dialog.rows[3].controls.len(), 2, "color selectors share a row");
        assert!(controls.iter().any(|control| matches!(control,
            DialogControl::Choice { id, options, .. }
                if id == SURFACE_ID && options == &surfaces()
        )));
        assert!(controls.iter().any(|control| matches!(control,
            DialogControl::Text { id, default, .. }
                if id == INTERVAL_ID && default == "0.5"
        )));
        assert!(controls.iter().any(|control| matches!(control,
            DialogControl::Text { id, default, .. }
                if id == MAJOR_ID && default == "5"
        )));
        assert!(controls.iter().any(|control| matches!(control,
            DialogControl::Choice { id, default_index, options, .. }
                if id == MAJOR_COLOR_ID && *default_index == 0 && options.len() == ACI_PALETTE.len()
        )));
        assert!(controls.iter().any(|control| matches!(control,
            DialogControl::Choice { id, default_index, options, .. }
                if id == MINOR_COLOR_ID && *default_index == 4 && options.len() == ACI_PALETTE.len()
        )));
        assert!(controls.iter().any(|control| matches!(control,
            DialogControl::InfoText { text, .. }
                if text.contains("Representación de curvas")
                    && text.contains("LS-CONTOUR-MAJ")
                    && text.contains("LS-CONTOUR-MIN")
        )));
    }

    #[test]
    fn strict_validation_rejects_bad_or_incomplete_values() {
        for bad in ["0", "-1", "NaN", "inf", "", "x"] {
            assert!(
                validate_response(
                    DialogResponse::Submitted(response_map(0, "EG", bad, "5")),
                    &surfaces()
                )
                .is_err(),
                "{bad}"
            );
        }
        for bad in ["0", "-1", "1.5", "NaN", "", "x"] {
            assert!(
                validate_response(
                    DialogResponse::Submitted(response_map(0, "EG", "0.5", bad)),
                    &surfaces()
                )
                .is_err(),
                "{bad}"
            );
        }
        assert!(validate_response(DialogResponse::Submitted(HashMap::new()), &surfaces()).is_err());
        assert!(validate_response(
            DialogResponse::Submitted(response_map(9, "EG", "0.5", "5")),
            &surfaces()
        )
        .is_err());
        assert!(validate_response(
            DialogResponse::Submitted(response_map(0, "MISSING", "0.5", "5")),
            &surfaces()
        )
        .is_err());
    }

    #[test]
    fn color_choices_are_independent_and_strictly_validated() {
        let mut values = response_map(0, "EG", "0.5", "5");
        let options = palette_options();
        values.insert(
            MAJOR_COLOR_ID.into(),
            DialogValue::Choice {
                index: 2,
                text: options[2].clone(),
            },
        );
        values.insert(
            MINOR_COLOR_ID.into(),
            DialogValue::Choice {
                index: 8,
                text: options[8].clone(),
            },
        );
        let config = validate_response(DialogResponse::Submitted(values.clone()), &surfaces())
            .expect("both colors validate");
        assert_eq!(config.major_color, 3);
        assert_eq!(config.minor_color, 30);

        values.insert(
            MAJOR_COLOR_ID.into(),
            DialogValue::Choice {
                index: usize::MAX,
                text: options[0].clone(),
            },
        );
        assert!(validate_response(DialogResponse::Submitted(values), &surfaces()).is_err());

        let mut values = response_map(0, "EG", "0.5", "5");
        values.insert(MINOR_COLOR_ID.into(), DialogValue::Text("ACI 5".into()));
        assert!(validate_response(DialogResponse::Submitted(values), &surfaces()).is_err());
    }

    #[test]
    fn cancel_and_close_are_noop_command_steps() {
        let mut command = ContourDialogCommand::new(surfaces());
        assert!(matches!(
            command.on_dialog_response(DialogResponse::Cancelled),
            CommandStep::Cancel
        ));
        assert!(command.modal_dialog().is_none());
        let mut command = ContourDialogCommand::new(surfaces());
        assert!(matches!(command.on_enter(), CommandStep::Cancel));
    }

    #[test]
    fn valid_response_handoff_is_single_use_and_closes_modal() {
        let mut command = ContourDialogCommand::new(surfaces());
        let step = command.on_dialog_response(DialogResponse::Submitted(response_map(
            1, "FG", "0.25", "4",
        )));
        let CommandStep::Dispatch(line) = step else {
            panic!("expected dispatch");
        };
        assert!(command.modal_dialog().is_none());
        let id = parse_internal_id(&line).unwrap();
        assert_eq!(
            take_config(id),
            Some(ContourConfig {
                surface: "FG".into(),
                minor_interval: 0.25,
                major_every: 4,
                major_color: 1,
                minor_color: 5,
            })
        );
        assert_eq!(take_config(id), None);
        let next_id = store_config(ContourConfig {
            surface: "EG".into(),
            minor_interval: 1.0,
            major_every: 5,
            major_color: 1,
            minor_color: 5,
        })
        .unwrap();
        assert_ne!(id, next_id, "handoff ids must not be reused");
        assert!(take_config(next_id).is_some());
        assert_eq!(take_config(next_id), None, "handoff is consumed once");
        assert!(parse_internal_id("LS_CONTOURS_APPLY 0").is_none());
        assert!(parse_internal_id("LS_CONTOURS_APPLY 7 extra").is_none());
        assert!(parse_internal_id("LS_CONTOURS_APPLY nope").is_none());
    }
}
