use makepad_ai_hub::{
    local::{InstallState, LocalModels},
    registry::LicenseRestriction,
};
use makepad_ai_hub_ui::{ModelRowInstallState, ModelRowState};

pub const BODY_MODEL_ID: &str = "sam3dbody";
pub const BODY_MODEL_ROLE: &str = "native-body";

pub fn body_model_row(models: &LocalModels) -> ModelRowState {
    let spec = models.spec(BODY_MODEL_ID);
    let bytes_from_spec = spec
        .map(|spec| spec.files.iter().filter_map(|file| file.size).sum())
        .unwrap_or(0);
    let (bytes_done, bytes_total, state) = match models.install_state(BODY_MODEL_ID) {
        InstallState::NotInstalled { bytes_total } => {
            (0, bytes_total.max(bytes_from_spec), ModelRowInstallState::NotInstalled)
        }
        InstallState::Partial {
            bytes_done,
            bytes_total,
        } => (bytes_done, bytes_total, ModelRowInstallState::NotInstalled),
        InstallState::Installed => (
            bytes_from_spec,
            bytes_from_spec,
            ModelRowInstallState::Installed,
        ),
    };
    let license = spec.and_then(|spec| spec.license.as_ref());
    ModelRowState {
        model_id: BODY_MODEL_ID.to_string(),
        name: "SAM 3D Body".to_string(),
        bytes_total,
        bytes_done,
        state,
        license_name: license
            .map(|license| license.name.clone())
            .unwrap_or_else(|| "Licence unavailable".to_string()),
        restriction: license
            .map(|license| restriction_name(license.restriction).to_string())
            .unwrap_or_else(|| "restricted".to_string()),
    }
}

pub fn body_model_status(models: &LocalModels, downloading: bool) -> String {
    if !models.license_acknowledged(BODY_MODEL_ID) {
        return "licence not accepted".to_string();
    }
    match models.install_state(BODY_MODEL_ID) {
        InstallState::Installed => "installed · 2.8 GB · Metal".to_string(),
        InstallState::NotInstalled { .. } => "not installed · 2.8 GB".to_string(),
        InstallState::Partial {
            bytes_done,
            bytes_total,
        } => {
            let percent = bytes_done.saturating_mul(100) / bytes_total.max(1);
            if downloading {
                format!("downloading {percent} %")
            } else {
                format!("not installed · {percent} % downloaded")
            }
        }
    }
}

fn restriction_name(restriction: LicenseRestriction) -> &'static str {
    match restriction {
        LicenseRestriction::None => "none",
        LicenseRestriction::NonCommercial => "non-commercial",
        LicenseRestriction::Community => "community",
        LicenseRestriction::Restricted => "restricted",
    }
}
