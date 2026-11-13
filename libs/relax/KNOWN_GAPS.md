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

## 6. A constant the trait solver evaluates keeps the solver's typing mode (no repro known)

rustc's `try_evaluate_const` (traits/mod.rs) evaluates in the post-analysis environment whatever
the solver's mode; Relax's `evaluate_const` (types/src/next_solver/goals.rs) passes the solver's
own mode (Coherence in the overlap check) for every constant but a repeat count.

## 7. A provided crate's `std::env` does not see its program's run-env values

A package's `[package.metadata.relax.run-env]` values are read only by the program: its JIT-run
code, and platform code reading a setting that only tunes or reports that program's own run. They
never reach a reader that decides what the process loads, executes or opens, what the person's
invocation decided, or what the loader hands the next image (base/src/progenv.rs `set_run_env`).

A provided crate (built by rustc into the loader, such as Makepad's platform crates) has two
reads. `relax_api::env::var` / `var_os` is the program's environment: under the loader progenv's
program view (`program_var_os`: the program's values, then the process environment), without
one `std::env`. `std::env` is the loader process's. Makepad's HAL reads its logs, traces,
profilers, tuning settings and test hooks through the first; its backend, devices, paths,
drivers, window visibility and focus, app identity, session facts and handovers through the
second.

- cargo: no run-env (Relax's own table); every crate of the program reads one environment.
- Relax: `relax_api::env::var("APP_MODE")` gives the run-env's value in JIT-run code and in a
  provided crate's rustc code; `std::env::var("APP_MODE")` gives it in JIT-run code and
  `NotPresent` in a provided crate's rustc code (tools/run/fixture/modules_harness.rs
  `a_packages_run_env_is_its_programs_and_grants_nothing`).

What stays out of reach:
- A C library in the process (the Vulkan loader, libva, GStreamer) and a third-party dependency of
  a provided crate read libc's environment, so run-env values never reach them. That is the
  protection: a value that names a library or a path stays the person's.
- HAL source that JIT-run code reaches compiles from source, where `std::env` is the program's
  environment: the app's event loop calls `OpenReq::from_env`, so a run-env's
  `MAKEPAD_HAL_BACKEND` or `MAKEPAD_HIDE_WINDOWS` picks how the app's own window opens, as the
  program's own `set_var` does (under cargo too). It grants nothing the program could not ask for
  itself; the host's window, devices and paths are read by the host's code.
- A run from the stores (`makepad run <git url>`, runner/src/lib.rs `run_spec`) starts the host
  before it has the package's manifest, so a setting the host reads once at its start (the HAL's
  log switch, `MAKEPAD_GPU_PROF`) is read before the run-env is applied. A run from a checkout
  applies the run-env first.

Start in libs/relax_api/src/lib.rs (`env`) for the read, and in tools/run/runner/src/lib.rs
`run_spec` for a store run's order.
