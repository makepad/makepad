# Source-matched native proc macros

This package depends by path on the expansion libraries of the proc macros it runs natively,
which this repository carries beside the compiler: Relax's `#[relax_api]` (libs/relax_api) and the
live_id and micro_serde macros (libs/live_id, libs/micro_serde). That is the base set. A host
binary adds sets of its own: a crate it links generates one with `gen.rs` and the binary registers
it (`register`, relax_runner's `Host::native_sets`) before the compiler reads the sets.
The `relax-tokens` feature of `makepad-micro-proc-macro` selects the Relax Rust token model
for these normal dependencies. Cargo's resolver 2 keeps rustc's host proc-macro builds
on the standard `proc_macro` API.

`gen.rs` (run by `build.rs`, and by a host set's build script) embeds the actual packages'
manifests and source files for runtime identity checks and generates bindings from those same
wrapper sources. It removes only
proc-macro entry annotations and redirects their token imports. There is no snapshot
or manual sync step; changing those sources rebuilds the identity and bindings.

The compiler validates every source and manifest in a macro's dependency closure, its
edition, feature policy, and dependency aliases before dispatching to this library.
Changing a wrapper, helper, implementation, or dependency makes that unit use the JIT.
New or unsupported proc macros also use the JIT. No disk cache or runtime build is added.

Review feature policy in each set's rows (`build.rs` here, the host set's build script) when
updating: a row lists the features a graph may build its package with.
Expansion implementations have no features; the program's proc-macro helper must have
`relax-tokens` disabled. The live-id library is an unused dependency
of the widget wrapper, retained for complete graph validation; it is not linked here.

`RELAX_PARSE_VERIFY=1` compares worker results with fresh
collection-scope parses, including AST arrays, tokens, source/hygiene maps, macro
publications, and file preparation. Use it for verification, not timing.

Derives execute concurrently on the existing parse workers, for Rust files.
The serial-loader fallback retains the front end's derive pre-expansion pool for Rust
source. Extra nested derive pools are unnecessary for the normal worker path.
