# Source-matched native proc macros

This package depends on the enclosing Makepad checkout's expansion libraries by path.
The `relax-tokens` feature of `makepad-micro-proc-macro` selects the Relax Rust token model
for these normal dependencies. Cargo's resolver 2 keeps rustc's host proc-macro builds
on the standard `proc_macro` API.

`build.rs` embeds the actual packages' manifests and source files for runtime identity
checks and generates bindings from those same wrapper sources. It removes only
proc-macro entry annotations and redirects their token imports. There is no snapshot
or manual sync step; changing the checkout's sources rebuilds the identity and bindings.

The compiler validates every source and manifest in a macro's dependency closure, its
edition, feature policy, and dependency aliases before dispatching to this library.
Changing a wrapper, helper, implementation, or dependency makes that unit use the JIT.
New or unsupported proc macros also use the JIT. No disk cache or runtime build is added.

Review feature policy in `compiler/src/native_macros.rs` when updating: only the current
wrapper's unused `default`/`nightly` markers and live-id's empty `default` are accepted.
Expansion implementations have no features; the program's proc-macro helper must have
`relax-tokens` disabled. The live-id library is an unused dependency
of the widget wrapper, retained for complete graph validation; it is not linked here.

`MAKEPAD_RUN_NATIVE_MACROS=0` selects the hosted path for comparisons.
`MAKEPAD_RUN_NATIVE_VERIFY=1` runs both paths and compares the entire encoded token stream,
including every source span. `MAKEPAD_RUN_PARSE_VERIFY=1` compares worker results with fresh
collection-scope parses, including AST arrays, tokens, source/hygiene maps, macro
publications, and file preparation. Use those flags for verification, not timing.

Derives execute concurrently on the existing parse workers, for Rust files.
The serial-loader fallback retains the front end's derive pre-expansion pool for Rust
source. Extra nested derive pools are unnecessary for the normal worker path.
