# Contributing to Relax

Relax is makepad's Rust JIT: it parses, type-checks and compiles Rust from source at run time.
The crates are in libs/relax (front end, types, codegen, backend, compiler) and the command-line
runner, `relax`, is in tools/run/runner. Open gaps with repros: KNOWN_GAPS.md.

## Build

Use release builds only. Debug builds are too slow to test with.

```sh
cargo build --release -p relax-compiler              # the compiler library
cargo build --release -p relax-runner --bin relax   # target/release/relax, the runner
```

`relax` provides no crates of its own: everything a program uses is compiled from source,
including Relax's std (libs/relax/compiler/std), which it reads from the checkout it was built
in. It runs crates.io proc macros the way cargo does (libs/relax/compiler/src/pmhost.rs).

## Run and compare with rustc

From a cargo package directory:

```sh
cargo run --release                          # rustc: the reference
/path/to/target/release/relax run --quiet <package>
/path/to/target/release/relax check <package>    # type- and borrow-check only
```

For a single test file:

```sh
relax test path/to/file.rs [filter]          # its #[test] fns, under Relax
rustc --edition 2021 -O --test path/to/file.rs -o /tmp/t && /tmp/t
```

rustc is always right. When the two disagree, read rustc's source for the rule and port it.
Don't special-case a test or add a heuristic that only makes one test pass.

## Unit tests

```sh
cargo test --release --workspace                                  # everything
cargo test --release -p relax-front -p relax-base -p relax-compiler  # the quicker core
```

These should be clean on Linux x86-64 and macOS arm64. Send a failure you didn't cause as a
separate fix.

## Add a test

Test files (`#[test]` fns that `rustc --test -O` builds) go in libs/relax/compiler/tests/<name>.rs.
Run yours both ways and check that they agree:

```sh
target/release/relax test libs/relax/compiler/tests/<name>.rs
rustc --edition 2021 -O --test libs/relax/compiler/tests/<name>.rs -o /tmp/t && /tmp/t
```

## A/B against another commit

The runner reads std from a checkout at run time, not from the binary. When you compare two
builds, point each one at its own commit's std:

```sh
MAKEPAD_RUN_STD=/path/to/that/checkout/libs/relax/compiler/std /path/to/that/target/release/relax run ...
```

Compiled code is kept in the workspace's run dir, `target/makepad-run` (in `CARGO_TARGET_DIR` when
that is set); `MAKEPAD_RUN_HOME` holds only fetched git repositories. Give each side its own run dir
with `MAKEPAD_RUN_CACHE_DIR=<dir>` so neither reuses the other's compiled code.

## Traces

- `MAKEPAD_RUN_TRACE=pmhost`: proc-macro registry and macro world (which crate serves which
  derive, sandbox refusals).
- `MAKEPAD_RUN_TRACE=expand:<macro>`: each expansion of that macro, with its call site.
- `MAKEPAD_RUN_TRACE=typeck:<fn substring>,lower:<fn substring>`: phase starts for matching fns.
- `MAKEPAD_RUN_TRACE=bt`: backtraces of internal errors.
- `--verbose` (or `MAKEPAD_RUN_VERBOSE=1`): phase timings.
- A running program: `relax inspect <pid> help` (panics, why-hung, probes).

## Shrinking a bug

Shrink it to the smallest program that still differs from rustc. A counting `Drop` impl shows
drop-order and double-free bugs. An `eprintln!` in a vendored copy of the failing crate finds where
the behaviour diverges. Put that small program in the PR as a test file.

## Rules

- Don't add third-party crates to Relax, and don't add regex/syn/quote. crates.io crates are
  fine as test inputs.
- Delete the code a fix replaces: no compatibility shims or fallbacks.
- There is no GitHub CI for Relax. Maintainers run their own gates before merging, so put the
  commands you ran (the build, the tests, `relax test` on your test file) and their results in the
  PR description.
