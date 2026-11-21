# Contributing to Relax

Relax is a developer JIT for Rust code from the Makepad project: it parses, type-checks and compiles Rust from source at run time.
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
RELAX_STD=/path/to/that/checkout/libs/relax/compiler/std /path/to/that/target/release/relax run ...
```

Compiled code is kept in the workspace's run dir, `target/relax` (in `CARGO_TARGET_DIR` when
that is set); `RELAX_HOME` holds only fetched git repositories. Give each side its own run dir
with `RELAX_CACHE_DIR=<dir>` so neither reuses the other's compiled code.

## Traces

- `RELAX_TRACE=pmhost`: proc-macro registry and macro world (which crate serves which
  derive, sandbox refusals).
- `RELAX_TRACE=expand:<macro>`: each expansion of that macro, with its call site.
- `RELAX_TRACE=typeck:<fn substring>,lower:<fn substring>`: phase starts for matching fns.
- `RELAX_TRACE=bt`: backtraces of internal errors.
- `--verbose` (or `RELAX_VERBOSE=1`): phase timings.
- A running program: `relax inspect <pid> help` (panics, why-hung, probes).

## Read live test results from an agent

Start one harness with `relax test --live <package> --lib` (or `--test <name>`), and find its PID
with `relax inspect list --json`. Read the first verdict with:

```sh
relax inspect <pid> events on=testverdict since=0 --json
```

Before saving an edit, keep the newest event's `seq`, then wait for the edit's outcome with:

```sh
relax inspect <pid> watch on=testverdict,patchrefused,patchfailed,patchreverted since=<seq> --json
```

A `testverdict` names the edited paths and the patch `group`, with counts and the failing tests.
Each run starts from the program's initial statics, as a fresh run of the test binary does: before
a rerun, what the earlier runs left in the statics of the program's own crates goes back (std's
crates keep theirs), and the verdict counts those statics in `statics_reset`. When a thread an
earlier test started still runs, or the program has its own `#[global_allocator]`, the tests restart
into a fresh process instead (`restarted`); `statics_carried` means that restart failed and the run
kept the earlier values.
Its `results` field is the query for each test's outcome, `test-results group=<n>`:

```sh
relax inspect <pid> test-results group=<n> --json
```

The answer starts with the `group` and `test_count`, then one JSON line per test with its `name`,
`status` (`passed`, `failed`, `ignored` or `filtered`) and `ran`. `ran` is true only when the test
executed for this edit: a passed or failed result an earlier run left (the edit did not reach the
test) has `ran:false`. The session keeps the latest completed run, and refuses a different `group`
rather than answer with another edit's results. A `patchrefused` event carries the build errors
in `reasons`. Move the `since` cursor past every event, the patch errors included.
`libs/relax/compiler/tests/live_test_results/run.py <relax binary>` checks this interface.

## Shrinking a bug

Shrink it to the smallest program that still differs from rustc. A counting `Drop` impl shows
drop-order and double-free bugs. An `eprintln!` in a vendored copy of the failing crate finds where
the behaviour diverges. Put that small program in the PR as a test file.

## Scope

Relax is an experimental developer JIT: it runs, checks and tests Rust from source, in memory, for
fast development iteration. It never writes executables, libraries or other build output, and it is
not an ahead-of-time compiler. Release builds use rustc and cargo. Turning Relax into a compiler is
permanently out of scope: a pull request that adds binary or library output, or a launcher that
stands in for one, is closed. A test that needs a built binary runs under cargo, not Relax.

## Rules

- Don't add third-party crates to Relax, and don't add regex/syn/quote. crates.io crates are
  fine as test inputs.
- Delete the code a fix replaces: no compatibility shims or fallbacks.
- There is no GitHub CI for Relax. Maintainers run their own gates before merging, so put the
  commands you ran (the build, the tests, `relax test` on your test file) and their results in the
  PR description.

## Credits

A commit that ports a pull request, or fixes what a report found, credits its author with a
`Co-Authored-By` line. Some ports went out without one; they are credited here:

- kioku's relax#235 and ymote's makepad#1534, #1536, #1538, #1539, #1540, #1542 and #1543, the
  module sandbox (2026-11-18): modules that run unmetered carry no fuel accounting; a module's
  panic text is read from its own arena, bounded, and a host read of a module's memory faults as
  the module's own; a twin's fn values work without code generation; O3 code over twins keeps
  their bounds; an enum's derived discriminant reads are safe code; vectored writes take every
  buffer; and a test filter that runs no test exits as libtest does, a test target restarted in
  place staying one.
- ymote's makepad#1550-#1553 (2026-11-16): the memo table walks, a sandboxed module's data image
  pointer words, the module load's cached twins and the run-time module's definition maps.
