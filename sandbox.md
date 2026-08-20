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

**The two canonical benchmark prompts** (run cold, every iteration; the
wording is FROZEN so runs stay comparable):

1. `make me a little village with driveable cars and people walking
   around, and let me walk around in it` — the villa test (the
   template.splash starting world is the quality/feel bar): kit pieces on
   the grid, car-kit vehicles as *driveable* entities (walk up, interact,
   drive, get out), rigged villagers wandering
   (`game.character` + `game.wander`), and the player as a VISIBLE
   third-person character (`game.player_character({model})`) — the
   visible-body default is TAUGHT, the prompt never has to ask for it.
2. `build me a playable doom with available assets` — the anchor-assembly
   test: a doom world as the fps-mode level, billboard actors placed at
   their spawn anchors, player_start honored, doors as bound states.

Scored per run: geometric integrity (§4 — grounded, on-grid, no loose
pieces, no interpenetration), player-is-a-visible-character, cars
enterable, villagers wandering, tokens + wallclock. "Great" = both
prompts, cold, produce something you'd actually play — and survive the
iteration benchmark's follow-up edits (§4.5).

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

**GEOMETRIC INTEGRITY is a requirement, not a style preference** (added
after the first rendered village put wall panels in the sky): things SIT
ON THE GROUND — a placement lands at its footprint's ground offset on the
ground plane unless explicitly elevated onto something; kit pieces are
never free-floating; tiles meet on the kit's grid module without gaps;
no interpenetrating placements. Enforced at every layer that can carry
it: metadata (declared ground offset + footprint), splash defaults
(placement verbs ground-snap by construction — floating must be asked
for, not defended against), context (teach it), and the validator.

A **validation tool** (overlap/gap/fit + ground-contact report) feeds
results back into the chat so the model self-corrects instead of
shipping floating houses — and the eval harness asserts the integrity
invariants on every benchmark run, creation and iteration alike.

The iteration log (`local/agent_state/sandbox-llm/loop.md`) records, per
iteration: prompt, what Qwen did, screenshot, verdict, which layer was
fixed. That log is the map of what a local 27B needs to be a level designer.

## 4.5 Iterating a live world — base + addon scripts

"Make a little town" is turn ONE. The design requirement is turn two:
"add an ambulance" — and it must not regenerate 700 tokens or reset the
world. Iteration is a first-class capability, taught in the context and
scored in the benchmarks, not an afterthought.

**The mechanism: a level = BASE source + an ordered set of named ADDONS.**

- An addon is a small self-contained splash unit that evals in its own
  scope against the LIVE world. Every entity it creates is tagged with the
  addon's identity (the placed.rs marker mechanism, generalized from
  single placements to script units).
- Tools: `world.add_addon(name, src)` · `replace_addon(name, src)` ·
  `remove_addon(name)` · `list_addons()`. `world.place` remains the
  degenerate single-object addon.
- **The state law**: the base NEVER re-evals on an addon op — so player
  transform, score, and entity state survive every edit structurally, not
  by carefulness. Replace despawns exactly that addon's entities and
  evals the new version; remove is undo for free.
- Persistence: the level's source of truth is base + addon list.
  Flattening to one source is an explicit "bake", never implicit.
- Addons are also the STREAMING unit (an addon appears the moment its
  script closes) and the natural unit of the model's edit turn: 30-300
  tokens instead of a full re-emit.

**Two verbs in the taught context.** Part B teaches CREATE (set_source,
for a new level or an asked-for rewrite) and ITERATE (emit ONE addon)
as distinct patterns, with a worked multi-turn example:

1. "make a little town" → base: road grid, houses, two cars (kit-scoped).
2. "add an ambulance" → query catalog (car-kit has one) → addon
   `ambulance`: one vehicle spawned NEAR A ROAD read from the base's
   grid. ~50 tokens. Nothing else changes.
3. "make it drive around the block" → `replace_addon("ambulance", …)`
   with a waypoint loop. Only the ambulance blinks.
4. "make the ambulance driveable" → `replace_addon("ambulance", …)`
   wiring the vehicle rig / drive-mode entity (same verb the base's cars
   use) — an addon can change WHAT something is, not just add things.
5. "remove it" → `remove_addon("ambulance")`.

**Why executable chunks, not diffs.** Two ways a background model can
return an edit: a DIFF against the current source, or a self-contained
EXECUTABLE CHUNK that concatenates into the final world. Chunks win for
a local 27B: a diff must hit exact lines in a source the model saw one
turn ago (fragile, and a mis-anchored hunk corrupts silently), while a
chunk either evals or it doesn't — and the eval error is teachable
feedback the loop already routes. The flattened level IS the
concatenation of base + addons in order, so "bake" is trivial and the
addon list doubles as an edit history.

**The knobs law**: bases declare their tunables as NAMED constants
(car_speed, tree_count, sky preset) — taught style, validated by the
loop. Then "make the cars slower" is a parameter-override addon touching
one name, not a base rewrite.

**The iteration benchmark** (beside the two creation benchmarks, replayed
cold as a scripted conversation): create village → add an ambulance →
add a fire station near it (spatial reference to a prior addon) → make
the cars slower (knob override) → remove the ambulance. Scored per turn:
delta-shaped (any set_source after turn 1 = fail even if the world looks
right), correct target, state preserved (harness asserts player/score
untouched on additive edits), tokens per edit, wallclock. Sessions are
judged, not turns — a model that creates well but trashes the world on
turn two fails the requirement.

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
