# `makepad-asset-server` — the standalone Asset Server host

The asset store is inherently **multiplayer**. asset-ui, the VJ, the sandbox
and headless workers are all clients of one catalog, and they open and close
independently of each other. So the catalog's lifetime must not be any one
client's lifetime.

Historically the Asset UI *embedded* the server: it took `<root>/server.lock`,
served the catalog, ran the chat broker and the events hub — and every time
that window was closed or rebuilt, every other connected client lost the
store mid-session (`503 state unavailable`). This binary is the fix: the same
service, in its own process, that survives every window.

```
cargo build --release -p makepad-app-asset-server
./target/release/makepad-asset-server
```

With no flags it serves the checkout's standard store root
(`local/asset-ui/asset-server`, or `$AI_CONTENT_ASSET_ROOT`) on ephemeral
ports, announces itself on the LAN beacon, publishes
`local/ai_content_library`, and coordinates fleet jobs. `--help` lists every
flag.

---

## What this process carries

| Part | Where it lives | Notes |
| --- | --- | --- |
| Catalog + CAS, control/data planes | `AssetServer::start` | search, blobs, ranges/ETags, retire |
| Auth, tokens, grants | ” | `--root/admin-token` bootstrapped at start |
| Job queue, worker/lease protocol | ” | plus `GET /v1/job-profiles` |
| Chat broker (+ client-executed tool parking for game sessions) | ” | `--chat-fleet` / LAN fleet discovery |
| Games publish path, operations, import routes | ” | |
| Committed events hub (`/v1/events`) | ” | every client's live view |
| LAN discovery beacon | ” | `--no-beacon` to stay silent |
| Lease janitor + bounded blob GC | ” | runs whether or not anyone polls |
| **ai-content library publisher** | `makepad_asset_importer::watch` | `--library` / `--no-library` |
| **GPU-fleet job coordinator + profile announce** | `makepad_asset_importer::gen_service` | `--no-jobs` / `--no-announce` / `--fleet` |

The last two are the loops the Asset UI used to run *only while it was
hosting*. They are headless — no `Cx`, no window, no GPU surface — so they
belong beside the server, and running them here is what makes a UI-less
deployment a complete fleet citizen. Without the coordinator, jobs any client
enqueues sit at "waiting for agent" forever.

## What deliberately stays client-side

These are **content-deriving** loops that need resources only a UI process
has. They reach the catalog as ordinary clients; moving them here would mean
giving a headless daemon a window.

- **Thumbnail / preview backfill** (`apps/asset-ui`: `thumbnail_renderer.rs`,
  the splat + mesh offscreen renders). Needs a real `Cx`, render passes and
  GPU readback.
- **Classic-game import** (`import_classic.rs`) and **pack import** wizards —
  interactive, and their icon bakes are GPU renders. The one-shot headless
  equivalents already exist as `makepad-asset-importer --import-pack /
  --import-games / --import-music / --import-ai-library` when a batch run is
  wanted.
- **Stems / lyrics analysis bake** (`analysis.rs`) — a large local model on
  the operator's machine, driven from the surface that asks for it.
- **Generation pipelines** driven from the Create surface (`pipeline.rs`) —
  these are *requests*; the queue and the dispatch live server-side.

## Recommended deployment

One daemon, every app attached:

```bash
# 1. the store (leave it running; it survives every UI restart)
./target/release/makepad-asset-server > /tmp/asset-server.log 2>&1 &

# 2. every client, told never to hold the root itself
export ASSET_UI_ASSET_EMBED=never
./target/release/makepad-app-asset-ui
./target/release/makepad-vj
```

`ASSET_UI_ASSET_EMBED=never` (aliases: `no`, `off`, `0`, `false`, `attach`,
`client`) makes the Asset UI a pure client: it never takes
`<root>/server.lock`, at startup or during a succession. Without it the app
still attaches when the daemon holds the root — but whichever process starts
*first* wins the lock, so a UI launched before the daemon would put the store
back inside a window.

The VJ and the sandbox are already pure clients; they need no flag.

### How a client finds the server

Three ways, in order of precedence:

1. `ASSET_UI_ASSET_SERVER=ip:controlport:dataport` — an explicit pin.
2. `<root>/listen`, `<root>/server-id`, `<root>/admin-token` — what the
   daemon writes at startup. This is the attach path, and it is why the
   default bind is `0.0.0.0:0`: the *file* is the address of record.
3. The LAN discovery beacon — how a client on another machine finds it with
   no configuration at all.

## What survives what

- **Closing / rebuilding / crashing any app** — the server keeps serving.
  Every other client stays connected; nothing is lost.
- **Restarting the daemon** — attached clients notice the silence within
  ~3.5 s, say so honestly in the status chip, and rejoin the new process as
  soon as it rewrites `listen`. Cursors and leases are re-established; the
  events journal reports an explicit `gap` rather than a silent hole.
- **A daemon crash with `EmbedPolicy::Auto`** — an attached Asset UI succeeds
  it (takes the lock, starts the loops, reconnects to itself). This is the
  transition-mode safety net, not the deployment.
- **A daemon crash with `ASSET_UI_ASSET_EMBED=never`** — no app fills the
  vacancy; clients wait and rejoin whoever takes the root next. Restart the
  daemon.
- **Two daemons on one root** — refused immediately and by name
  (`server root: locked by another server process`). One process per root is
  a law: the job routing metadata assumes a single enqueuer, and two writers
  over one WAL catalog would be two sources of recovery truth.

## Bouncing the daemon safely

```bash
# find it (it holds the lock and wrote the listen file)
cat local/asset-ui/asset-server/listen        # ip:control:data
pkill -TERM -f 'makepad-asset-server'         # clean shutdown: joins every thread
./target/release/makepad-asset-server > /tmp/asset-server.log 2>&1 &
```

SIGTERM/SIGINT shut down in order: background loops first (so nothing is
still publishing into a closing catalog), then the planes, then the state
thread. Startup logs what recovery found — `recovered N cas temps / M leases`
— which is the honest report of what the previous life left behind.

## Isolated instances

Never point a scratch instance at the live root, and never let one beacon:

```bash
./target/release/makepad-asset-server \
  --root /tmp/scratch-store --work /tmp/scratch-work \
  --no-beacon --no-jobs --no-library
```

`--no-beacon` keeps peers hunting for the real store from finding the scratch
one; `--no-jobs` keeps it from claiming the real fleet's queued work.

## Tests

`cargo test -p makepad-app-asset-server --release` covers the composition:
the host serves a catalog over real sockets and shuts down cleanly, a second
host on one root is refused by name, a missing library directory never costs
the catalog its server, and the defaults are the deployment defaults. The
parts themselves are covered where they live — `libs/asset/store` (HTTP,
chat, events, jobs, security, operations suites) and `libs/asset/importer`
(watch, coordinator, gen-service). The attach/succession contract is in
`apps/asset-ui/src/asset_store_state.rs`.
