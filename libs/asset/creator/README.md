# Asset creator and Flow

Native creator generation executes as flow-server instances. `engine::run_in`
compiles a named `PipelineSpec` and its work orders into an ordinary Splash
Flow, submits a separate instance, and translates observations into creator
progress events. Flow owns dependency scheduling, hub admission/retry,
cancellation and content-addressed results. Request settings are instance
inputs; reusable graph definitions contain the wiring. Binary inputs use the
Flow data plane and retain their MIME types. Seeds preserve all 64 bits.

`runner::generate_bytes` and the shipping `FleetTransport` submit a single Gen
node through Flow. The VJ submits its full declared pipeline through Flow and
retains a shared session on its worker. Asset validation and publication still
use the existing creator/asset-client contract. Composite recipes currently
submit their generation steps as individual Flow instances and perform their
validation/publication sequence in the creator.

## Embedded or remote

The default `flow-host` Cargo feature links an embedded server. Disable default
features for a client-only build:

```toml
makepad-asset-creator = { path = "../creator", default-features = false }
```

Call `CreatorFlow::configured()` on a worker, or use `CreatorFlow::shared()` to
share a session across concurrent jobs. Retain the returned session for the
app/worker lifetime. Configuration, in precedence order:

| Setting | Behavior |
| --- | --- |
| `CREATOR_FLOW_CONTROL`, `CREATOR_FLOW_DATA`, `CREATOR_FLOW_TOKEN` | Connect to explicit remote socket addresses, e.g. `10.0.0.10:8800` and `10.0.0.10:8801`. Both addresses and the server token are required. |
| `CREATOR_FLOW_ROOT` | Read this local root's authenticated endpoint records; attach if it is already hosted, otherwise start a host when compiled in. |
| `CREATOR_FLOW_EMBED=never` | Require an existing server. Client-only builds always behave this way. |
| No creator overrides | Use `FLOW_ROOT`, or the ordinary `~/.makepad/flow` root. |

For explicit programmatic ownership use `CreatorFlow::start(config)` or
`CreatorFlow::open(config)`, accepting Flow's `FlowServerConfig`. `attach(root)`
and `connect(endpoints, token, server_id)` never own or stop the remote server.
An embedded host stops when its final owning session is dropped. A remote
server retains submitted instances after the client disconnects; cancel a run
explicitly when that is the intended action.

All operations are blocking and belong on workers. The UI consumes its existing
non-blocking event interface. `submit`, `snapshot`, `cancel` and `wait` also
support running an already registered Flow directly, without a `PipelineSpec`.

## Headless execution

From the repository root:

```sh
cargo build --release -p makepad-asset-creator --bin makepad-creator-run
CREATOR_FLOW_ROOT=/path/to/flow-root target/release/makepad-creator-run \
  --stages text,image --prompt 'a rusted lighthouse' --out /path/to/output
```

Explicit remote endpoints use the same environment variables. `--base URL`
retains compatibility with a fixed hub provider in an isolated embedded Flow.
The older `engine::run` provider adapter and custom `GenerationTransport`
fixtures remain compatibility entry points; production fleet execution uses
Flow. Custom transports that only implement `route` retain direct execution.

The bundled [Image to Pixal3D](../../flow/recipes/templates/image-to-pixal3d.splash)
Flow accepts an image and a JSON settings input. It connects native matting to
Pixal3D reconstruction and a mesh output. Native sampler/decoder stages execute
inside the hub's model backend. See the [native model notes](../../ai/models/trellis/README.md)
for settings and verification.

## Verification

```sh
cargo test --release -p makepad-asset-creator
cargo check --release -p makepad-asset-creator --no-default-features
```

The remote integration fixture starts its own isolated host and exercises
separate instances, a 1 MiB image, full-width seeds, model provenance and
cancellation of an accepted job without cancelling a completed invocation.
