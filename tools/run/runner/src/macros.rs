//! The host's lexer extension (Host::raw_body_macros): fn-like macros whose bodies are another
//! language, kept as one token each, which the macro's expansion reads as text.

/// Registers the host's raw-body macros. Call once, before anything is parsed.
pub fn register(names: &'static [&'static str]) {
    crate::lexer::register_raw_body(names);
}
