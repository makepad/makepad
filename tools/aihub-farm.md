# Windows AIHub updates and generation checks

Build the `makepad-app-ai-hub` package in release mode on Windows from a
known source checkout. Record the source revision or file manifest and the
SHA256 of `target/release/makepad-app-ai-hub.exe`; the HTTP package version alone
cannot distinguish two builds of the same version. Preserve the node's
existing CUDA environment and model cache.

CUDA kernels target the build host GPU by default. Build separate artifacts
with `MAKEPAD_GGML_CUDA_ARCH=89` for RTX 4090 and `120a` for RTX 5090/RTX PRO 6000,
and set `MAKEPAD_GGML_REQUIRE_CUDA=1` so a missing toolkit fails the build.
The build only uses the toolkit named by `MAKEPAD_CUDA_ROOT` (on the Ada
boxes `C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v12.4`); it never
probes `CUDA_PATH`.
Use the node's compatible CUDA runtime: the current Ada machines have CUDA 12,
while Blackwell uses CUDA 13. A successful HTTP health check alone does not
prove an artifact contains compatible GPU kernels; run generation below.

Stage that binary with `cargo-makepad tunnel <host>:8384 push <binary>
<relative-payload-path>`, then run:

```sh
./target/release/cargo-makepad tunnel <host>:8384 --no-sync run \
  tools/aihub-node-update.ps1 -Payload <relative-payload-path> -Sha256 <sha256> \
  -CudaArch <89-or-120a>
```

The updater verifies the payload and GPU architecture, identifies the process listening on 8123,
checks that it is an idle AIHub, and finds its existing `.cmd` launcher.
It preserves the executable path used by firewall rules, all launcher
settings, and the cache. It temporarily suspends only watchdog tasks naming
that installation directory, backs up the executable, replaces it, launches
it hidden, and verifies health with the same durable node identity. Failure
restores the previous executable and the watchdog configuration. It refuses
unknown launch layouts and active jobs rather than interrupting them.

## Services

`tools/aihub-node-install-services.ps1`, run once per box from an elevated
PowerShell, installs the node (`MakepadAiNode`, :8123) and the tunnel
(`MakepadTunnel`, :8384) as Windows services. Both start at boot without a
logged-on user and restart on failure. `tools/service_host` runs each of
them.

The node runs in the console user's session as that user when someone is
logged on, because local-use admission watches that session; otherwise it
runs as LocalSystem in session 0. Its binary is in `C:\ai\services\node`.

The tunnel is the TLS server from `tools/remote` (see
`tools/remote/TUNNEL.md`). It runs as the virtual account
`NT SERVICE\MakepadTunnel`, not as an administrator. That account may:
- modify the tunnel's working directory, the cargo cache, `C:\ai\services\node`
  and its own `C:\ai\services\tunnel`;
- start, stop and query `MakepadAiNode`, and no other service. These are the
  `admin node-*` actions.

The installer keeps the tunnel's existing key and certificate, so client
pins stay valid. It probes the new service end to end before removing the
old watchdog task, and restores that task if the probe fails.

The firewall allows both ports from the local subnet only. The service host
and its configs live in `C:\ai\services`, which only administrators can
write. Node logs are in `C:\ai\services\logs`. The node keeps its cache
directory and with it its identity (`node-key`).

The updater above handles both layouts. For a service node it reads the
binary from `C:\ai\services\MakepadAiNode.cfg` and stops and starts the
service instead of the launcher.

After the node's normal activity quiet period, verify real generation:

```sh
python3 tools/aihub-farm-smoke.py \
  http://<host-a>:8123 http://<host-b>:8123 http://<host-c>:8123 \
  --output <local-report-directory>
```

Use `--count 3` on one node to exercise three concurrent submissions queued
through a single GPU. Each request has its own origin and lease keepalive.
The checker records real progress stages, validates artifact hashes and PNG
dimensions, and writes the generated images beside its JSON report. Transient
poll/keepalive overload responses retry the same job within a bounded budget. It
respects local-use admission, skips nodes already doing other work, and only
cancels its own timed-out jobs. A skipped node produces a nonzero exit and
an explicit reason; it is not counted as a successful generation.

`GET /jobs` lists active work. Finished job records remain accessible at
`GET /job/<job-id>` until their service process is replaced. Collect those
records **before** restarting a node when investigating a failure; they
include lifecycle timestamps, stages, and cancellation errors that an
untimestamped service log may not contain.

Fleet image requests without an explicit queue policy try compatible idle
GPUs first. The client uses `queue_policy=reject` for atomic admission, so
simultaneous requests that observe the same idle health snapshot can move to
different servers. A proven Busy/QueueFull refusal returns immediately; when
all eligible servers are busy, the scheduler deliberately queues the request.
Explicit queue policies and fixed-node requests retain their requested behavior.

If an accepted job remains queued behind a local-use pause for three seconds,
Flow looks for an alternative. It submits a replacement only after the old
job is confirmed cancelled, preserving the model, prompt, seed and independent
lease. With no alternative it retains the queued job and reports the reason.
An uncertain cancellation or submission response never permits duplicate work.
Queued and loading stages propagate to the chat picker even at zero percent.

A Windows GPU-counter failure can legitimately pause a node even when its
HTTP service is healthy. Do not bypass that activity gate to clear a queue;
route work to another admitted node and inspect the counter evidence. A node
may recover through its normal quiet period without restarting the service.
