# Sandbox — AI-authored games on the asset system

The sandbox is a **generic game engine with modes** (fps, race, …) that is a
**thin client of the asset server**, whose levels are **written by an LLM in
splash**, from **assets that already exist in the store**. This document is
the design of that loop and the doctrine for iterating it until it is great.

## 1. Topology — one hub, thin everything

```
apps (sandbox game / vj / asset-ui)
      │  each connects and gets ITS OWN CHAT (thin client: send turns, render turns)
      ▼
ASSET SERVER — the one and only routing point for ALL AI
      │  owns: job queues, workflow orchestration, the store,
      │  the grok/codex connections, the asset-ai fleet connections
      ▼
asset-ai agents (fleet, e.g. .169) — EXECUTE the chat LLM (Qwen3.8-27B, ~160 tok/s w/ MTP)
      │  the model's toolcalls route back and execute ON THE SERVER:
      │    query_assets(sql) / schema()  — interrogate the catalog (read-only)
      │    enqueue jobs / workflows      — LATER: start generation (see §6)
      │    external AI (grok/codex)      — LATER: delegate splash authoring (see §5)
      ▼
results are ASSETS in the store → catalog events → the apps see them arrive
game: generated splash hot-reloads; all content streams from the server
```

Laws this rests on (recorded in project memory, enforced in reviews):

- **Thin client**: no app owns durable content state. RAM + evict + refetch
  by digest. The store (catalog + CAS) is the single source of truth.
- **AI routing**: no app talks to a model, provider, or fleet box directly.
  The server owns every AI connection. No model is ever loaded into an app.
- **Writers declare, readers obey**: all metadata is explicit (views, cells,
  anchors, tile facts). A reader that guesses from sizes/aspects is a bug.
- **One map contract**: per-game facts live in DATA (door_N, hazard_N,
  anchors, style config). `if doom` in engine code is always the wrong fix.

## 2. The chat context — two parts, token-frugal

The context that teaches Qwen is a first-class engineered artifact,
assembled **server-side** at chat creation, versioned next to the dispatch.

- **Part A — identical for every UI**: the architecture in ~10 lines, the
  tool contracts (rendered in Qwen's exact trained tool template), alias
  conventions (`kenney/car-kit/race`, `doom/doom/worlds/doom1/e1m1`), and
  the method: `schema() → narrow SELECT → answer in aliases`.
- **Part B — game only**: the **splash level-authoring context** — the
  splash DSL guide (root `splash.md`), how store aliases bind into a level,
  grid/kit placement conventions, mode selection (fps/race), examples.
  Written dual-use: it is Qwen's own instruction today and, verbatim, the
  brief handed to grok when delegation is wired (§5).
- **Dynamic layer**: a compact live schema summary (generated from
  sqlite_master, never hand-written), store headline facts (counts, kits +
  their grid modules), assembled fresh per conversation.

**Token economy is a design principle, not polish.** This is a local 27B,
not a frontier cloud model: the whole context stack stays well under ~8k
tokens (budget-asserted in tests); tools are terse; results are compact
tables, not chatty JSON; splash outputs are SHORT because the APIs are
high-level (at 160 tok/s, a 300-token level is 2 s and a 3000-token level
is 20 s — brevity is latency). `tokens per successful level` is a logged
first-class metric beside quality.

## 3. Phase 1 — Qwen builds the levels itself

Qwen, armed with Part A + Part B, queries the store and **emits splash level
source directly** (no external AI — don't hammer grok). The source goes
through the authoring/publish path as a game revision; the sandbox
hot-reloads it; assets stream from the server.

**The two canonical benchmark prompts** (run cold, every iteration):

1. `make me a little village with driveable cars` — the kit-composition
   test: kenney buildings/roads snapping on the grid, car-kit vehicles
   wired as *driveable* entities via the race/drive mode.
2. `build me a playable doom with available assets` — the anchor-assembly
   test: a doom world as the fps-mode level, billboard actors placed at
   their spawn anchors, player_start honored, doors as bound states.

"Great" = both prompts, cold, produce something you'd actually play.

## 4. The iteration doctrine — fix at the cheapest layer

Every failure is a diagnosis of WHICH layer to improve. All layers are in
bounds, orderd by cheapness:

1. **Tool ergonomics** — if Qwen fumbles a call, simplify the tool: fewer
   args, forgiving inputs, errors that read as guidance. A 27B rewards
   ergonomic tools far more than prompt scolding.
2. **Asset metadata** — extend what is queryable whenever a failure traces
   to "the model couldn't know that". Geometric tile facts are measured at
   import (exact, free): footprint, bbox, per-kit grid module (GCD of
   footprints), ground offset. Declared, never guessed.
3. **Context / SQL shaping** — what schema summary and store facts the
   model sees; query patterns taught by example.
4. **Splash / engine APIs** — the whole domain is tweakable. When splash
   makes something hard to express, give splash a *generic* high-level
   primitive: kit-grid placement, road/row/fill constructs, spawn-at-anchor,
   billboard entities, vehicle rigs, door-state binding. The best validator
   is an API in which broken geometry is inexpressible. Engine additions are
   MODE-level or fully generic — never game-named.
5. **Vision enrichment** — for metadata that cannot be derived any other
   way: run the asset's baked turntable sheet (+ a top-down render for
   tiles) through the Qwen vision stack → structured JSON → published back
   as declared metadata (semantic category, edge/socket hints, orientation).
   An optional, versioned, idempotent enrichment pass, routed through the
   server like all AI. Built when the transcripts show it's needed.

A **validation tool** (overlap/gap/fit report) feeds results back into the
chat so the model self-corrects instead of shipping floating houses.

The iteration log (`local/agent_state/sandbox-llm/loop.md`) records, per
iteration: prompt, what Qwen did, screenshot, verdict, which layer was
fixed. That log is the map of what a local 27B needs to be a level designer.

## 5. Phase 2 — delegation (later)

The chat LLM gains a toolcall to hand Part B + the task to a heavyweight
external model (grok/codex, server-owned connections) for the level source,
with Qwen staying the orchestrator. The seam exists; it stays unwired until
phase 1 plateaus. Phase-1 transcripts are the calibration data for what the
delegation brief must contain.

## 6. Generation (later, user-gated)

Firing up asset **generation** from the chat is deliberately out of phase 1:
existing assets are the 99% use case. The job-enqueue tool seam remains in
the architecture, but it is not in the taught context yet. When wired:
generation is expensive, so the chat **proposes and the human approves** —
cost surfaced, explicit confirmation, never fire-and-forget from the model.

## 7. The stack beneath — everything in the repo feeds this layer

This project is the top of the whole codebase; each subsystem below was
built (or rebuilt) as a tool for it:

- **Asset store** (`libs/asset/*`): content-addressable CAS + catalog,
  retire/GC, keep-alive + batched HTTP, fast/bulk client lanes, catalog
  events. The single durable home of all content.
- **Own SQLite engine** (`libs/sqlite_query`): from-scratch SQLite-3-format
  reader/SQL/writer — the substrate of `query_assets`; WAL-interop lets the
  broker read the live catalog the server is writing.
- **Importers** (`libs/asset/importer`): kenney/classic-game/music/AI-library
  pipelines emitting ONE contract — welded watertight geometry, door/lift/
  hazard/sky nodes, spawn anchors, declared thumbnail views (fft/wave/anim
  ranges/turntables), tags, readable aliases. Plus HD FFT tiles and (soon)
  geometric tile facts for kit composition.
- **Game/render substrate** (`libs/render`, `apps/sandbox`): the generic
  engine + modes, GPU lightmaps/AO, level collision + nav grid + the
  player-nav walker (also the map PREVIEW everywhere), splash hot-reload.
- **Shared widgets** (`libs/asset/widgets`): AssetThumb + ContentPreview —
  one thumbnail/preview implementation across asset-ui/vj/(sandbox soon).
- **Audio stack**: from-scratch MP3/Vorbis decoders (SIMD-tuned), the
  Vorbis encoder (in flight), BS-RoFormer stem separation (Metal+CUDA),
  Whisper (Metal+CUDA) + attention-DTW word alignment — feeding the DJ,
  karaoke, spectrogram thumbnails, and the stems/lyrics side-channels that
  will live on the store as declared asset files.
- **VJ/DJ** (`apps/vj`): the performance surface on the same store — decks,
  stems, karaoke, the disciplined beat clock (one continuous slewed clock,
  source ladder, protocol seam for external sync).
- **LLM serving** (`libs/ai/llm` + fleet): Qwen3.8-27B with MTP speculative
  decoding (~160 tok/s), IQ-quant kernels, fast model loads/swaps — the
  speed that makes a local model a usable level designer.
- **The content-generation fleet** (`libs/ai/models/*`, asset-ai agents,
  own CUDA/Metal runtimes): the pipelines §6's generation tool will enqueue
  — image (flux1-schnell, flux2-dev, klein edit), matting (birefnet), depth
  (da3), upscale (realesrgan), 3D mesh (trellis), PBR paint (hunyuan-paint),
  rigging (skintokens), motion (hy-motion), splats (triposplat), worlds
  (flashworld), video (minimax-h3), music (music3, ace-step), SFX, speech
  (kokoro) — all registered as live job profiles on the server, executing on
  the fleet, publishing products-only into the store. Today they're driven
  from the apps' GEN surfaces; in §6 they become what the chat proposes and
  the human approves.
- **Remote protocol** (`--remote`, AGENTS.md): every app is drivable and
  observable headlessly — the loop's eyes and hands during iteration.

## 8. Current state (2026-08-20, evening)

- SHIPPED end to end: game-profile chat sessions on the asset server
  (create body `client: "game"`), broker-side `assets.query`/`assets.schema`
  (chat crate `catalog_sql` module over makepad-sqlite, content tables
  only), the layered teaching context (`context.rs` + `context/*.md`),
  client-executed world tools with the parked-turn round trip
  (`POST /v1/chat/sessions/{id}/tool-result`), and the sandbox as a thin
  chat client: tool chips, streamed turns, `world.set_source` landing
  through the authoring intent log with eval + last-good rollback and an
  unresolved-model report. First closed loop 2026-08-20 14:34; iteration
  log: local/agent_state/sandbox-llm/loop.md (4 iterations).
- Store: doom worlds + billboards + music + 10 games; the kenney Load All
  was landing live during iteration 4 (library 1282 models and climbing).
- Benchmarks: park-style primitive levels PASS cold (iteration 3 built a
  playable fish-catching park unprompted). #2 (doom) blocked on TWO
  things: the serving tier's chat lane runs max_context 8192 (turn
  overflowed at 9064) and there is no map verb yet (§4 design:
  `game.map(alias, {mode})` over the shared LevelCollision walker +
  generic billboard entities). #1 (village) unblocks with the import.
- Serving reality check (measured): the .169 chat lane decodes ~55 tok/s —
  MTP speculation is NOT active on that path; the 8k context cap and the
  MTP flags are the two fleet-side items to fix before the next round.
