# Relax: known gaps

Open bugs with a minimal repro, ready for a contributor to take. Each one names what rustc does,
what Relax does, and where to start. Fix one, add its repro as a fixture (see CONTRIBUTING.md), and
delete its entry here in the same PR.

Every repro is a standalone cargo package (`[workspace]` in its Cargo.toml). Compare
`cargo run --release` with `relax run --quiet <package>` from the package directory.

## 1. A struct whose last field may be unsized reorders that field when sized

```rust
trait Tr { fn get(&self) -> u32; }
impl Tr for u32 { fn get(&self) -> u32 { *self } }
struct W<T: ?Sized> { n: u8, t: T }
fn main() {
    let w: W<u32> = W { n: 7, t: 0xdead_beef };
    let d: &W<dyn Tr> = &w;
    println!("n={} t={:#x} off={}", d.n, d.t.get(), (&w.t as *const u32 as usize) - (&w as *const W<u32> as usize));
    let b: Box<W<dyn Tr>> = Box::new(W { n: 1, t: 42u32 });
    println!("boxed t={}", b.t.get());
}
```

- rustc: `n=7 t=0xdeadbeef off=4` / `boxed t=42`.
- Relax (x86-64, 2026-10-24): `n=239 t=0x7 off=0` / `boxed t=32257`.

`W<u32>` gets `t` at offset 0 and `n` at 4, while the `W<dyn Tr>` view reads `n` at 0 and the tail
at 4. rustc never reorders a field that may be unsized: `StructKind::MaybeUnsized` keeps the last
field last (rustc_abi/src/layout.rs, `univariant`, `end = fields.len() - 1`), so the sized and
unsized views agree; the dyn tail's offset is the sized prefix rounded up to the alignment the
vtable gives (rustc_codegen_ssa/src/size_of_val.rs). Relax has the same rule
(libs/relax/types/src/layout.rs, `decl_maybe_unsized`, `Kind::MaybeUnsized`), so find the path
that lays out `W<u32>` here without it. Fixture: tools/run/fixture/diff/.

## 2. Derive and bang macros resolve by bare name across the whole graph

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

## 3. `type Error = std::fmt::Error;` in a `serde::Serializer` impl: E0277 (not reproduced)

Seen in a large graph (PR #1326): "E0277: item `Error` has an incompatible type for trait" on a
`Serializer` impl with `type Error = std::fmt::Error;`. rustc accepts it (serde has
`impl ser::Error for fmt::Error`). A standalone serde 1.0.229 package with such a `Serializer`
(spelled `std::fmt::Error`, or `fmt::Error` after `use std::fmt;`) type-checks and runs under Relax
with rustc's output. A repro must find the shape that fails: the impl inside a dependency crate,
`serde_core` without the `std` feature, or a facade re-export.

## 4. One check run spun at full CPU for five minutes (not reproduced)

Seen once (PR #1326) on a serde + `instability` graph: a check spun for five minutes and finished
in 1.4 s on every repeat. If you see it, attach to the live process (`makepad inspect <pid>
why-hung`) before it ends; a stack beats a repeat.

## 5. Random tier fuzzer: seed 0x166db5d4c1538fe3 disagrees (register allocation)

`cargo test --release -p relax-codegen --test fuzz_tiers random_programs_agree_across_tiers`
draws fresh seeds on each run, so it can fail on your machine without any change from you. One
seed found on 2026-10-24 (x86-64): the register-allocation check reports `v27` reading `r7`, which
holds `[65]`. Reproduce and shrink it with `FUZZ_REDUCE=166db5d4c1538fe3` on the same test. That
prints the smallest program that still disagrees and the first tier where it does. Then add the
seed to `FOUND_SEEDS` in libs/relax/codegen/tests/fuzz_tiers.rs.

## 6. Late lints still print when the check has an error

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

## 7. `let mut x` assigned but never used: Relax reports at `x`, rustc at `mut x`

```rust
struct WriteOnly { w: u32 }
fn make_wo() -> u32 { let mut x = WriteOnly { w: 1 }; x.w = 2; 0 }
fn main() { make_wo(); }
```

- rustc: `2:27: warning: variable `x` is assigned to, but never used` (the span starts at `mut`).
- Relax: the same warning at 2:31.

Start in types/src/lint_liveness.rs (the span of the binding it reports).
