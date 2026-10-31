//! What the models are doing right now, as one value: the design agent's top
//! progress bar reads `TranslatorView::activity()` and draws it.

use crate::models::{fraction, ModelState, State};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Idle,
    /// A model file is downloading or being checked.
    Downloading,
    /// A model is being loaded (first translation after start, or a hub check).
    Loading,
    /// The vision model reads the picture.
    Reading,
    Translating,
    Listening,
    Speaking,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Activity {
    pub kind: Kind,
    /// 0..1 when the work has a measurable length (downloads), None for an
    /// indeterminate bar.
    pub progress: Option<f32>,
}

impl Activity {
    #[cfg(test)]
    pub const IDLE: Activity = Activity { kind: Kind::Idle, progress: None };
}

/// What is going on, most urgent first: speech, then the picture and the
/// translation, then loading, then downloads (which run in the background).
pub struct Inputs<'a> {
    pub models: &'a [ModelState],
    pub loading: bool,
    pub reading: bool,
    pub translating: bool,
    pub listening: bool,
    pub speaking: bool,
}

pub fn derive(i: &Inputs) -> Activity {
    let kind = if i.speaking {
        Kind::Speaking
    } else if i.reading {
        Kind::Reading
    } else if i.translating {
        Kind::Translating
    } else if i.listening {
        Kind::Listening
    } else if i.loading {
        Kind::Loading
    } else if i.models.iter().any(|m| matches!(m.state, State::Downloading { .. } | State::Verifying)) {
        Kind::Downloading
    } else {
        Kind::Idle
    };
    let progress = (kind == Kind::Downloading).then(|| {
        let total: f64 = i.models.iter().map(|m| m.bytes as f64).sum::<f64>().max(1.0);
        let done: f64 = i.models.iter().map(|m| fraction(&m.state) as f64 * m.bytes as f64).sum();
        (done / total) as f32
    });
    Activity { kind, progress }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Capability;

    fn model(state: State, bytes: u64) -> ModelState {
        ModelState { id: "m", capability: Capability::Text, label: "m", bytes, state }
    }

    fn quiet(models: &[ModelState]) -> Inputs<'_> {
        Inputs { models, loading: false, reading: false, translating: false, listening: false, speaking: false }
    }

    #[test]
    fn idle_when_nothing_runs() {
        assert_eq!(derive(&quiet(&[model(State::Ready, 10)])), Activity::IDLE);
    }

    #[test]
    fn downloads_report_the_share_done_of_everything() {
        let models = [
            model(State::Downloading { done: 50, total: 100, bytes_per_s: 1 }, 100),
            model(State::Absent, 100),
        ];
        let a = derive(&quiet(&models));
        assert_eq!(a.kind, Kind::Downloading);
        assert!((a.progress.unwrap() - 0.25).abs() < 1e-6);
    }

    #[test]
    fn speech_outranks_translation_outranks_download() {
        let models = [model(State::Verifying, 1)];
        let mut i = quiet(&models);
        i.translating = true;
        assert_eq!(derive(&i).kind, Kind::Translating);
        i.speaking = true;
        assert_eq!(derive(&i).kind, Kind::Speaking);
        assert_eq!(derive(&i).progress, None);
    }
}
