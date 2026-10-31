# Warm check server

`makepad serve [--workspace DIR] [--idle-min 30]` resolves and canonicalizes the workspace
with Relax's resolver. It publishes `PORT TOKEN PID\n` in
`<workspace>/target/makepad-run/serve/<16 lowercase hex digits>/endpoint`. Cargo target-dir overrides
select `<target>/makepad-run`; `MAKEPAD_RUN_CACHE_DIR` explicitly replaces that directory. Run
`makepad clean` from the workspace root or a member subdirectory to remove the entire makepad-run
directory, including serve files and startup manifests. The directory key is the
low 64 bits of Relax's Sip128 hash of the canonical root. The token is 32 OS-random bytes,
hex encoded. The endpoint and lock are private files. An OS file lock prevents duplicate
servers and is released on crashes; a subsequent server replaces a stale endpoint.
A second live server invocation prints the endpoint and exits successfully.

All examples below are single JSON lines. Replace `TOKEN` and `/workspace/main.rs`.
IDs are integers within JSON's exactly representable range. Errors have the shape
`{"id":1,"error":{"code":-32602,"message":"missing string `pkg`"}}`.
An invalid token returns -32001 and closes the connection. Malformed JSON returns -32700;
unknown methods return -32601. Input lines are limited to 8 MiB and JSON nesting to 128.
Coordinates are one-based source lines and UTF-8 byte columns, using J1's mapping for
macro expansion and `.rx` source. Definition ranges identify the declaration token.

## Protocol

Check (optional `paths` filters returned diagnostics; checking/invalidation stays package-wide):

```json
{"id":1,"token":"TOKEN","method":"check","params":{"pkg":"serve_fixture"}}
{"id":1,"result":{"diagnostics":[],"summary":{"errors":0,"warnings":0,"ms":0.123,"rechecked":1,"rss_mb":42.0}}}
```

Diagnostics are J1 objects without modification. Summary `ms` and `rechecked` describe
the last actual check/edit, including edits applied by `didChange`. An unchanged request
returns that snapshot. `rss_mb` is process RSS sampled after J2 trims its caches.

Unsaved editor buffer (JSON null discards the override and reloads from disk):

```json
{"id":2,"token":"TOKEN","method":"didChange","params":{"path":"/workspace/main.rs","text":"#![no_std]\nfn main() {}\n"}}
{"id":2,"result":null}
```

Overlays survive disk notifications, apply to all already-loaded packages containing the
file, and are applied when a package is first loaded.

Subscribe (stays on that connection, loads the package on first use):

```json
{"id":3,"token":"TOKEN","method":"subscribe","params":{"pkg":"serve_fixture"}}
{"id":3,"result":null}
{"event":"diagnostics","pkg":"serve_fixture","diagnostics":[],"summary":{"errors":0,"warnings":0,"ms":0.123,"rechecked":1,"rss_mb":42.0}}
```

Events follow disk edits, including edits first detected by a concurrent check. The
watcher polls source stamps every 100 ms. Slow clients have a bounded outbound queue
and a five-second write timeout.

Status:

```json
{"id":4,"token":"TOKEN","method":"status","params":{}}
{"id":4,"result":{"workspace":"/workspace","packages":[{"pkg":"serve_fixture","loaded_ms":100.0,"last_check_ms":0.123,"items":4,"rss_mb":42.0}],"uptime_s":12.0}}
```

Schema:

```json
{"id":5,"token":"TOKEN","method":"schema","params":{"pkg":"serve_fixture","path":"crate::Counter"}}
{"id":5,"result":{"kind":"struct","path":"serve_fixture::Counter","generics":[],"fields":[{"name":"value","type":"i32","docs":"Current count."}],"variants":[],"methods":[{"name":"type_id","signature":"fn type_id(&Counter) -> TypeId","docs":""},{"name":"borrow","signature":"fn borrow(&Counter) -> &Counter","docs":""},{"name":"borrow_mut","signature":"fn borrow_mut(&mut Counter) -> &mut Counter","docs":""},{"name":"from","signature":"fn from(Counter) -> Counter","docs":""},{"name":"read","signature":"fn read(&Counter) -> i32","docs":"Read the count."}],"traits":["core::any::Any","core::borrow::Borrow","core::borrow::BorrowMut","core::convert::From"],"docs":"A documented counter.\nUsed by the editor fixture.","definition":{"file":"/workspace/main.rs","line":4,"col":12,"end_line":4,"end_col":19}}}
```

Schema uses root-scope resolution, AST docs and generics, collected field types, and
inherent/trait impls, including blanket impls whose arguments and bounds are determined by the queried type. Call-site-dependent blanket impls are omitted. `.rx` types additionally expose configurable fields from the
crate’s retained `rx-core` schema as `properties: [{name,type}]`.

Hover (`pkg` may be supplied to load the package before querying; otherwise an unloaded file’s nearest package manifest is used):

```json
{"id":6,"token":"TOKEN","method":"hover","params":{"path":"/workspace/main.rs","line":16,"col":17}}
{"id":6,"result":{"kind":"local","name":"local","type":"Counter","docs":"","definition":{"file":"/workspace/main.rs","line":15,"col":9,"end_line":15,"end_col":14}}}
```

Local, field and method resolution reads typeck body tables, rechecking the enclosing
function after J2 trims those tables. Method types substitute the resolved generic
arguments. An unresolved token returns null. Packages with parse errors reject queries
rather than returning locations from the previous syntax snapshot.

Definition:

```json
{"id":7,"token":"TOKEN","method":"definition","params":{"path":"/workspace/main.rs","line":18,"col":19}}
{"id":7,"result":[{"file":"/workspace/main.rs","line":13,"col":4,"end_line":13,"end_col":10}]}
```

An unresolved definition returns an empty array.

Shutdown:

```json
{"id":8,"token":"TOKEN","method":"shutdown","params":{}}
{"id":8,"result":null}
```

Shutdown drains outbound responses, retires sessions, and removes the exact endpoint
and lock paths. Ctrl+C/SIGTERM and expiry after the configured time with no connections
use the same cleanup path.

## CLI

- `makepad check [--json] PKG`: connect if a live endpoint authenticates; otherwise one-shot.
- `makepad check --no-serve [--json] PKG`: always one-shot.
- `makepad check --serve [--json] PKG`: start a detached server if necessary, with stdin closed
  and stdout/stderr appended to `serve.log` beside the endpoint; wait up to 30 minutes.
- `makepad schema [--serve] PKG ITEM`
- `makepad hover [--serve] PKG FILE LINE COL`
- `makepad definition [--serve] PKG FILE LINE COL`

Query commands emit one JSON result, falling back to a one-shot session when no server is available; `--serve` explicitly requests persistent startup. Check emits J1 diagnostic lines followed by the
same four-field JSON summary as one-shot, with integer milliseconds; human diagnostics
and summary go to stderr. Compilation-changing options (`--units`, `--deep`, features,
watch, etc.) keep their one-shot behavior and do not implicitly connect. Explicit
`--serve` combined with those options returns an error instead of ignoring options.

## Validation

Run `./run.sh /absolute/path/to/makepad`. The Python socket client creates an isolated copy
of the adjacent fixture, sets both Relax and Cargo offline, and retains logs and sources
under the printed temporary directory. It compares diagnostic lines and summary fields
with one-shot output, normalizing only milliseconds, and prints warm check/edit timings.

The compiler has process-global registries, so all package work currently runs on one
server worker. Socket reads/writes run independently. This is stricter serialization
than one writer per package, and avoids sharing compiler pointers between threads.

The supplementary `python3 queries.py /absolute/path/to/makepad` gate exercises query
fallbacks without a listener, including `.rx` coordinates/properties, negative blanket
impl coverage, and warm J2 edits over the existing `check --watch` pipe. It does not
replace the socket gate. See [REPORT.md](REPORT.md) for this implementation's validation
results and deviations.
