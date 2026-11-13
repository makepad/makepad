# Relax std comparison

Run `libs/relax/compiler/std/bench/run.sh <label> [relax executable]` on each machine you compare,
for example `run.sh mac` and `run.sh linux`. The label names the results (logs-<label>/,
results-<label>.md). Pass the runner you built, target/release/relax: left out, the executable is
target/release/makepad, the Makepad project's runner. The script does not sync, install software,
build Relax, or alter its dependencies.
It uses the existing release Relax executable and an offline release rustc build.
Every build and measured process is invoked with nice -n 10.

A is rustc with real std; C is Relax with its std, using exactly the A source
and wrappers. Shared workload bodies are in src/benches.rs, the std paths they
use in src/prelude_real.rs. Default hashers intentionally differ; LiveId and Fx
rows control that variable.

Fixed batch counts in benches::batch_size target 50–300 ms in A without
changing the specified operation counts within each workload. Table times
are per batch, with the same count in every column; checksums sum the
individual workload checksums with wrapping arithmetic.

Each workload is deterministic, black-boxes inputs/results, warms up once,
and prints the best of five measurements with two decimal places and a
checksum. Input construction, allocation, checksum work and destruction
are included; process start and JIT boot are excluded. Each C workload has
an isolated entry point and unit graph.
This lets later workloads run even if one JIT compilation fails. Raw build
and per-workload logs, generated entry points, units, and the markdown
result table remain under this directory. Binary/JIT output uses the host's
requested target directory. All mismatches and execution failures are
retained in the table; unavailable C is never reported as a checksum pass.

These are standalone CPU workloads with no UI loop, per-frame work or app
changes. Results include checksum overhead and are not claims about an
application's frame latency. Timings should be repeated outside the sandbox
if nice reports `setpriority: Operation not permitted`.
