//! Optional child-to-host loading progress on the existing custom channel.

use makepad_platform::makepad_micro_serde::*;
use makepad_platform::Cx;
use makepad_studio_protocol::AppToStudio;

#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct AppStatus {
    pub text: String,
    /// Progress within the named operation, when its total is known.
    pub fraction: Option<f64>,
}

#[derive(SerJson, DeJson)]
struct StatusEnvelope {
    makepad_app_status: AppStatus,
}

impl AppStatus {
    pub fn parse(json: &str) -> Option<Self> {
        if !json.contains("\"makepad_app_status\"") {
            return None;
        }
        let mut status = StatusEnvelope::deserialize_json(json).ok()?.makepad_app_status;
        status.fraction = status.fraction.filter(|value| value.is_finite()).map(|value| value.clamp(0.0, 1.0));
        Some(status)
    }

    /// Standalone apps have no host to notify. Call when the status changes.
    pub fn send(&self, cx: &Cx) {
        if cx.in_makepad_studio() {
            Cx::send_studio_message(AppToStudio::Custom(StatusEnvelope { makepad_app_status: self.clone() }.serialize_json()));
        }
    }

    pub fn label(&self) -> String {
        match self.fraction {
            Some(fraction) => format!("{} · {:.0}%", self.text, fraction * 100.0),
            None => self.text.clone(),
        }
    }
}
