//! The crunch knobs of a pack, shared by Stage's web export and
//! `cargo makepad wasm --pack`: a preset ([`PackPreset`]) gives the
//! defaults ([`PackPreset::options`]) and every knob can be set on its own.

use crate::js::FILM_JS_STRIPPED;
use std::collections::BTreeSet;

/// How hard the pack crunches, as a preset of knobs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PackPreset {
    /// Playback only: everything the analysis proves unused goes.
    #[default]
    Film,
    /// What interactive apps need: text input, complete fonts, the
    /// shader compiler for shaders made at run time.
    App,
    /// No stripping (debugging).
    Full,
}

/// Fonts: cut by the coverage analysis (open text ships complete), or
/// complete.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontsKnob {
    Analysed,
    Full,
}

/// Shaders: prebuilt only (no compiler in the wasm), or prebuilt plus
/// the compiler for shaders made at run time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShadersKnob {
    PrebuiltOnly,
    KeepCompiler,
}

/// Kernels: compiled ahead of time only, or plus the run-time kernel
/// compiler.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KernelsKnob {
    AotOnly,
    KeepCompiler,
}

/// Pictures: in the wasm, beside it, or by size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssetsKnob {
    BySize,
    Embedded,
    External,
}

/// Every crunch choice; [`PackPreset::options`] gives the defaults.
#[derive(Clone, Debug, PartialEq)]
pub struct PackOptions {
    pub preset: PackPreset,
    pub fonts: FontsKnob,
    pub shaders: ShadersKnob,
    pub kernels: KernelsKnob,
    /// The JS sections stripped (`crate::js::SECTIONS` names).
    pub js_stripped: Vec<String>,
    pub assets: AssetsKnob,
}

/// The JS sections an interactive app keeps whether or not a collect run
/// saw them used (a run cannot prove an app never saves or pastes).
pub const APP_JS_KEPT: [&str; 5] = ["text-input", "clipboard", "history", "storage", "file-dialog"];

impl PackPreset {
    pub fn options(self) -> PackOptions {
        let all: Vec<String> = FILM_JS_STRIPPED.iter().map(|s| s.to_string()).collect();
        match self {
            PackPreset::Film => PackOptions { preset: self, fonts: FontsKnob::Analysed, shaders: ShadersKnob::PrebuiltOnly, kernels: KernelsKnob::AotOnly, js_stripped: all, assets: AssetsKnob::BySize },
            PackPreset::App => PackOptions {
                preset: self,
                fonts: FontsKnob::Full,
                shaders: ShadersKnob::KeepCompiler,
                kernels: KernelsKnob::KeepCompiler,
                js_stripped: all.into_iter().filter(|s| !APP_JS_KEPT.contains(&s.as_str())).collect(),
                assets: AssetsKnob::BySize,
            },
            PackPreset::Full => PackOptions { preset: self, fonts: FontsKnob::Full, shaders: ShadersKnob::KeepCompiler, kernels: KernelsKnob::KeepCompiler, js_stripped: Vec::new(), assets: AssetsKnob::Embedded },
        }
    }

    pub fn parse(s: &str) -> Option<PackPreset> {
        match s {
            "film" => Some(PackPreset::Film),
            "app" => Some(PackPreset::App),
            "full" => Some(PackPreset::Full),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            PackPreset::Film => "film",
            PackPreset::App => "app",
            PackPreset::Full => "full",
        }
    }
}

impl Default for PackOptions {
    fn default() -> Self {
        PackPreset::Film.options()
    }
}

impl PackOptions {
    /// The sections to strip when a collect run says which ones the app
    /// needs (`needed`, the manifest's `js.txt`): every section of
    /// [`crate::js::SECTIONS`] it does not name, minus what the preset keeps
    /// ([`APP_JS_KEPT`] for the app preset unless `strict`; nothing is
    /// stripped for full).
    pub fn js_stripped_for(&self, needed: &BTreeSet<String>, strict: bool) -> Vec<String> {
        if self.preset == PackPreset::Full {
            return Vec::new();
        }
        crate::js::SECTIONS
            .iter()
            .filter(|s| !needed.contains(**s))
            .filter(|s| strict || self.preset != PackPreset::App || !APP_JS_KEPT.contains(*s))
            .map(|s| s.to_string())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needed_sections_are_kept() {
        let needed: BTreeSet<String> = ["text-input".to_string()].into();
        let app = PackPreset::App.options();
        let stripped = app.js_stripped_for(&needed, false);
        assert!(!stripped.iter().any(|s| s == "text-input" || s == "storage"));
        assert!(stripped.iter().any(|s| s == "midi"));
        let strict = app.js_stripped_for(&needed, true);
        assert!(strict.iter().any(|s| s == "storage") && !strict.iter().any(|s| s == "text-input"));
        assert!(PackPreset::Full.options().js_stripped_for(&needed, true).is_empty());
    }
}
