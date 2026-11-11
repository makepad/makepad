# Relax: known gaps

Open bugs with a minimal repro, ready for a contributor to take. Each one names what rustc does,
what Relax does, and where to start. Fix one, add its repro as a test (see CONTRIBUTING.md), and
delete its entry here in the same PR.

Every repro is a standalone cargo package (`[workspace]` in its Cargo.toml). Compare
`cargo run --release` with `relax run --quiet <package>` from the package directory.

## 1. Derive and bang macros resolve by bare name across the whole graph

Workspace with four members:

- `a_mac` and `b_mac`: proc-macro crates, each exporting `#[proc_macro_derive(Who)]`, which emits
  `impl <Name> { pub fn who() -> &'static str { "a" } }` (`"b"` in `b_mac`).
- `lib_b`: depends on `b_mac`, `#[derive(b_mac::Who)] pub struct InB;`.
- `app`: depends on `a_mac` and `lib_b`, `use a_mac::Who; #[derive(Who)] struct InApp;`, prints
  `InApp::who()` and `lib_b::InB::who()`.

- rustc: `app=a b=b`.
- Relax: `app=a b=a`. `lib_b`'s derive, written with the full path `b_mac::Who`, expands
  `a_mac`'s macro.

The registry (libs/relax/front/src/macro_reg.rs, `derives` / `macros`) is keyed by the macro's
bare name and the first registered entry wins silently. Real collisions: `stable` (instability and
rustversion), `Error` (thiserror and derive_more). Attribute macros already require the macro
crate in the invoking crate's dependency closure (`attr_macro_at`). Derives and bang macros need
rustc's rule: resolve the path at the invocation (the `use` that brought it in, or the crate the
path names) among the crates the invoking crate depends on.

## 2. `type Error = std::fmt::Error;` in a `serde::Serializer` impl: E0277 (not reproduced)

Seen in a large graph (PR #1326): "E0277: item `Error` has an incompatible type for trait" on a
`Serializer` impl with `type Error = std::fmt::Error;`. rustc accepts it (serde has
`impl ser::Error for fmt::Error`). A standalone serde 1.0.229 package with such a `Serializer`
(spelled `std::fmt::Error`, or `fmt::Error` after `use std::fmt;`) type-checks and runs under Relax
with rustc's output. A repro must find the shape that fails: the impl inside a dependency crate,
`serde_core` without the `std` feature, or a facade re-export.

## 3. One check run spun at full CPU for five minutes (not reproduced)

Seen once (PR #1326) on a serde + `instability` graph: a check spun for five minutes and finished
in 1.4 s on every repeat. If you see it, attach to the live process (`relax inspect <pid>
why-hung`) before it ends; a stack beats a repeat.

## 4. Late lints still print when the check has an error

```rust
#![allow(dead_code)]
fn BadName() {}
struct lower_struct;
fn bad() -> u32 { "no" }
fn main() {}
```

- rustc: E0308 and the `lower_struct` warning (non_camel_case_types is an early lint). No
  `BadName` warning: rustc_interface passes.rs `analysis` stops at an error of the required analyses
  (lint errors excluded) before `misc_checking_3`, so the late lint pass (rustc_lint::check_crate)
  never runs.
- Relax: E0308 and both warnings (non_snake_case is reported too).

dead_code, unnameable_types and unfulfilled_lint_expectations already follow that rule
(compiler/src/check.rs `analyses_ok`, GitHub #1328). The late lints of types/src/lint_late*.rs are
emitted while the bodies are checked, so they need to be held back (or dropped) once an error is
known. Start with a "late" mark on the lints rustc runs in LateLintPass.

## 5. `let mut x` assigned but never used: Relax reports at `x`, rustc at `mut x`

```rust
struct WriteOnly { w: u32 }
fn make_wo() -> u32 { let mut x = WriteOnly { w: 1 }; x.w = 2; 0 }
fn main() { make_wo(); }
```

- rustc: `2:27: warning: variable `x` is assigned to, but never used` (the span starts at `mut`).
- Relax: the same warning at 2:31.

Start in types/src/lint_liveness.rs (the span of the binding it reports).
