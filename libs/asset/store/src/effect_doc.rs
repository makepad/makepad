//! What an effect document says about itself, read from its text without
//! evaluating it: the few facts a catalog needs to FILE a document — which
//! lane it belongs in — before anything has run it.
//!
//! Compiled on every target (the observer is native-only, but a browser
//! build files its bundled presets by the same rules).

/// The tags that say what an EFFECT does to the picture — the VJ's
/// GENERATIVE and TRANSFORM chips each query one of them (the strings are
/// the wire contract). A transition carries none: it has its own lane.
pub const GENERATIVE_TAG: &str = "generative";
pub const TRANSFORM_TAG: &str = "transform";
pub const HYBRID_TAG: &str = "hybrid";

/// Engines whose documents work ON the incoming picture when they do not
/// say otherwise: the fullscreen family, the picture cut into tiles, and the
/// picture hung on shapes. Every other engine draws a scene of its own.
/// A document on one of these that draws its own picture anyway (the audio
/// visualizers on `screen`) declares `category: "generative"`.
pub const TRANSFORM_ENGINES: &[&str] = &["screen", "tiles", "videomesh"];

/// What an effect does to the picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectCategory {
    /// Makes its own moving picture out of time and music; a deck plays it
    /// the way it plays a clip.
    Generative,
    /// Works on a picture that is already there — a clip, or a generative
    /// on the deck; it runs in a deck's effect slot.
    Transform,
    /// Takes the incoming picture but leaves it barely recognizable. Not a
    /// third kind of effect: an effect nobody has yet decided is one or the
    /// other, so it is listed under both.
    Hybrid,
}

impl EffectCategory {
    pub fn from_name(name: &str) -> Option<EffectCategory> {
        match name {
            "generative" => Some(EffectCategory::Generative),
            "transform" => Some(EffectCategory::Transform),
            "hybrid" => Some(EffectCategory::Hybrid),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            EffectCategory::Generative => GENERATIVE_TAG,
            EffectCategory::Transform => TRANSFORM_TAG,
            EffectCategory::Hybrid => HYBRID_TAG,
        }
    }

    /// The catalog tags an effect of this category carries. A hybrid
    /// carries both lanes' tags beside its own, so each chip finds it with
    /// the one tag it asks for.
    pub fn tags(self) -> &'static [&'static str] {
        match self {
            EffectCategory::Generative => &[GENERATIVE_TAG],
            EffectCategory::Transform => &[TRANSFORM_TAG],
            EffectCategory::Hybrid => &[HYBRID_TAG, GENERATIVE_TAG, TRANSFORM_TAG],
        }
    }

    /// Whether an effect of this category belongs in `lane` (a category's
    /// own lane; a hybrid belongs in both).
    pub fn is_in(self, lane: EffectCategory) -> bool {
        self == lane || self == EffectCategory::Hybrid
    }
}

/// The category a document declares (`category: "generative"`,
/// `"transform"` or `"hybrid"`), if it declares one this module knows.
pub fn declared_category(source: &str) -> Option<EffectCategory> {
    let value = field_str(source, "category")?;
    let word = value.split(|c: char| c == '"' || c.is_whitespace()).next()?;
    EffectCategory::from_name(word)
}

/// What an effect document does to the picture: what it declares, or —
/// when it declares nothing this module knows — what its engine does. An
/// engine never makes a document hybrid; only its author does.
pub fn effect_category(source: &str) -> EffectCategory {
    if let Some(category) = declared_category(source) {
        return category;
    }
    match field_str(source, "engine") {
        Some(engine) if TRANSFORM_ENGINES.contains(&engine.as_str()) => EffectCategory::Transform,
        _ => EffectCategory::Generative,
    }
}

/// The value of a top-level `key:` in the document, as written.
///
/// Deliberately a scanner and not a parser: the store publishes a document
/// it does not evaluate (the VJ's own `EffectDoc::parse` is the only
/// authority on what a document MEANS), and it needs a few facts out of it
/// — what to call it, whether it belongs in the transition lane, and what
/// it does to the picture. `key` must OPEN its line (the document's own
/// style) or follow a `{`/`,`, so neither `// engine: …` in the prose header
/// nor an `engine:` inside a shader body is ever mistaken for the declaration.
pub(crate) fn field_str(source: &str, key: &str) -> Option<String> {
    let needle = format!("{key}:");
    let bytes = source.as_bytes();
    let mut from = 0usize;
    while let Some(rel) = source[from..].find(&needle) {
        let at = from + rel;
        from = at + needle.len();
        let line_start = bytes[..at]
            .iter()
            .rposition(|b| *b == b'\n')
            .map(|i| i + 1)
            .unwrap_or(0);
        let opens_line = bytes[line_start..at].iter().all(u8::is_ascii_whitespace);
        let prev = bytes[..at]
            .iter()
            .rev()
            .find(|b| !b.is_ascii_whitespace())
            .copied();
        if !opens_line && !matches!(prev, None | Some(b'{') | Some(b',')) {
            continue;
        }
        let rest = source[at + needle.len()..]
            .lines()
            .next()
            .unwrap_or_default()
            .trim();
        let value = rest
            .split(&[',', '}'][..])
            .next()
            .unwrap_or_default()
            .trim()
            .trim_matches('"');
        return Some(value.to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_effect_category_is_declared_or_follows_the_engine() {
        let doc = |body: &str| format!("// prose that says category: \"hybrid\"\n{{\n{body}}}\n");
        // Undeclared: the engine decides, and no engine makes a hybrid.
        for (engine, category) in [
            ("screen", EffectCategory::Transform),
            ("tiles", EffectCategory::Transform),
            ("videomesh", EffectCategory::Transform),
            ("particles", EffectCategory::Generative),
            ("raymarch", EffectCategory::Generative),
        ] {
            let source = doc(&format!("    engine: \"{engine}\"\n"));
            assert_eq!(declared_category(&source), None, "{engine}");
            assert_eq!(effect_category(&source), category, "{engine}");
        }
        // Declared beats the engine, a trailing comment included.
        let visualizer = doc("    engine: \"screen\"\n    category: \"generative\" // draws its own\n");
        assert_eq!(effect_category(&visualizer), EffectCategory::Generative);
        let pixels = doc("    engine: \"particles\"\n    category: \"hybrid\"\n");
        assert_eq!(effect_category(&pixels), EffectCategory::Hybrid);
        // A word this module does not know falls back to the engine.
        let typo = doc("    engine: \"tiles\"\n    category: \"generatve\"\n");
        assert_eq!(declared_category(&typo), None);
        assert_eq!(effect_category(&typo), EffectCategory::Transform);
    }

    #[test]
    fn a_hybrid_is_listed_in_both_lanes() {
        use EffectCategory::*;
        assert_eq!(Hybrid.tags(), &[HYBRID_TAG, GENERATIVE_TAG, TRANSFORM_TAG]);
        assert_eq!(Generative.tags(), &[GENERATIVE_TAG]);
        assert!(Hybrid.is_in(Generative) && Hybrid.is_in(Transform));
        assert!(!Generative.is_in(Transform) && !Transform.is_in(Generative));
        for category in [Generative, Transform, Hybrid] {
            assert_eq!(EffectCategory::from_name(category.name()), Some(category));
        }
    }
}
