//! makepad's macros under the loader: Relax's proc-macro host JIT-compiles them like any proc
//! macro (relax pmhost.rs); the loader only tells the lexer that `script_mod!` bodies are
//! Splash, not Rust. Written against `crate::lexer`, so the same file serves the `makepad`
//! binary and tools that include Relax's modules directly (the bring-up driver, hrcheck).

/// Registers makepad's lexer extension. Call once, before anything is parsed.
pub fn register() {
    // Splash bodies: lexed by the Splash tokenizer, never as Rust
    crate::lexer::register_raw_body(&["script_mod"]);
}
