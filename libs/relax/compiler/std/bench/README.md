# Relax std comparison

Run `./libs/relax/compiler/std/bench/run.sh mac` from this checkout. On node 165,
after the usual std_gate.sh source sync, run
`~/fastrust-std/src/libs/relax/compiler/std/bench/run.sh 165` there. The runner does
not sync, install software, build Relax, or alter its dependencies.
It uses the existing release Relax executable and offline release rustc builds.
Cargo uses four jobs. Every build and measured process is invoked with nice -n 10.

A is rustc with real std; B is rustc with Relax's std shim; C is Relax with
its std, using exactly the A source and wrappers. Shared workload bodies
are in src/benches.rs. B's primitive operations are explicitly redirected
through free functions so host slice/str methods cannot silently substitute
for Relax algorithms. Its iterator traits and thread spawning are host std;
its containers, formatting, parsing, sorting, Rc/RefCell, and mpsc are Relax.
Default hashers intentionally differ; LiveId and Fx rows control that variable.

One exception is hashset_string: Relax's shim String does not implement the
host Hash trait. B uses real std for that entire row (including key creation
and the key vector), visibly marked B=real. C still uses Relax std.
The other fallback is rc_refcell_churn: the optimized shim panicked with
`RefCell already borrowed` in the initial probe (preserved in
logs-mac/rc_refcell_churn-B-shim-failure.log). Its custom UnsafeCell is
recognized by Relax but is not rustc's interior-mutability lang item.
B therefore uses host Rc, RefCell and Vec for this entire row.

The shim build sets relax_check and relax_shim, as the dependencies' own
Cargo configurations require.

Fixed batch counts in benches::batch_size target 50–300 ms in A without
changing the specified operation counts within each workload. Table times
are per batch, with the same count in every column; checksums sum the
individual workload checksums with wrapping arithmetic.

Each workload is deterministic, black-boxes inputs/results, warms up once,
and prints the best of five measurements with two decimal places and a
checksum. Input construction, allocation, checksum work and destruction
are included; process start and JIT boot are excluded. Each C workload has
an isolated entry point/unit graph following /tmp/rp/runone.sh's schema.
This lets later workloads run even if one JIT compilation fails. Raw build
and per-workload logs, generated entry points, units, and the markdown
result table remain under this directory. Binary/JIT output uses the host's
requested target directory. All mismatches and execution failures are
retained in the table; unavailable C is never reported as a checksum pass.

These are standalone CPU workloads with no UI loop, per-frame work or app
changes. Results include checksum overhead and are not claims about an
application's frame latency. Timings should be repeated outside the sandbox
if nice reports `setpriority: Operation not permitted`.

Validation on this Mac: 51 existing release tests passed (core_vec,
coll_hash_map, coll_btree, coll_heap_rc, text_str, text_string, fmt_int,
fmt_float, fmt_parse, and check_os's futex_backend). Raw output is in
logs-mac/tests.log and logs-mac/tests-os.log. No test files were added.
The initial unbatched calibration table is logs-mac/calibration.md;
results-mac.md is the final batched comparison.
