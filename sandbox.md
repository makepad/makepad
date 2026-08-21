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

**The mundane-asks harness** (checked in, `apps/sandbox/tools/
mundane-suite`) is the mechanized form of this scoring: `cargo test -p
makepad-mundane-suite` = the deterministic tier (canned tool sequences
against the headless world-tools executor — routing, continuity,
refusal shapes); `mundane_suite --offline` prints the whole ask table;
`MUNDANE_BROKER=… mundane_suite --live` runs the asks through a real
broker + fleet, one session at a time. The ask list is DATA
(`asks.toml`, user-extendable); recurring live-sweep failure shapes get
promoted into it. See the crate README for scoring columns.

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

## 4.4 The lego architecture — how the AI knows what is what

What the village convergence settled into is a layered vocabulary of
PREBUILT WHOLES, each layer teaching the one above it:

1. **Import = normalize to declared facts.** Every set arrives on the
   shared contract: complete-model roles, footprints + per-set scale
   (canonical meters, round 2), spawn anchors, actor placements
   (.place), turntable views. Vendor quirks die here — the catalog
   only ever speaks the contract.
2. **Vision annotation = semantics.** The turntable sheets pass through
   the VLM into dense construction lines + role/category/connectivity
   tags — versioned, idempotent, hard-replaceable. This is literally
   "the AI knows what each asset IS".
3. **Catalog SQL = retrieval.** The facts from 1+2 are queryable; the
   taught query patterns surface complete-object rows first.
4. **Context = the recipe.** The big-lego law: compose PREBUILT wholes
   by tier — streamed WORLDS (game.map) ≻ complete BUILDINGS ≻ props ≻
   vehicles ≻ characters. Named known-good assets with measured scales,
   spacing laws, the two verbs (create base / emit addon). Piece-level
   assembly is absent by default and swaps in only on explicit user
   intent — and returns properly as an ENGINE-owned assembly construct,
   never freehand.
5. **Engine = wrong things inexpressible.** Ground-snap by construction,
   shell-piece refusal (with the intent escape), kill-plane, missing
   model renders nothing, generic mode verbs. The best validator is an
   API where the mistake has no syntax.
6. **Judge = the rubric as regression tests.** Integrity assertions +
   the human rubric ("would a person call this a village?") + rendered
   screenshot checks, run per cycle like unit tests; a teaching change
   that regresses them is reverted like bad code.

Failures route DOWN the stack to the cheapest layer that owns them —
retrieval failures fix the query patterns, semantics failures re-prompt
the annotator, geometry failures move into the engine. The same six
layers serve every domain: villages today, doom/quake/duke worlds
through game.map, mixups through the shared actor vocabulary.

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
- **SHIPPED verb set** (each a `// @addon:` marked chunk on the host's
  append lane): `world.spawn({model})` — single catalog thing, the game
  picks form/ground/scale · `world.set_player_model({model})` — the
  become swap · `world.tune({time})` — idempotent world knobs ·
  `world.add_addon({name, src})` — the GENERAL verb: bulk adds and
  primitive builds as ONE 10-30 line chunk (loops welcome), no source
  echo · `world.remove({tag|ids})` — undoes spawns, places and addons by
  name/tag. `world.place` remains the degenerate single-object addon.
  Still designed-not-built: `replace_addon` · `list_addons` (world.list
  covers placements today).
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
   with a waypoint loop. Only the ambulance blinks. (replace_addon is the
   remaining unbuilt verb; today this walks the source path.)
4. "make the ambulance driveable" → `replace_addon("ambulance", …)`
   wiring the vehicle rig / drive-mode entity (same verb the base's cars
   use) — an addon can change WHAT something is, not just add things.
5. "remove it" → `world.remove({tag: "ambulance"})` (shipped).

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

## 4.6 Worlds are store assets with revision chains (design, 2026-08-21)

The user's design session closed the loop the thin-client law always
implied: the current `local/sandbox/current/game.splash` file as the
source of truth is an app-owned-content violation waiting to be felt —
and it was felt the moment "New game" was requested ("so I don't mess
with the maps you've been iterating on").

1. **A game in the games list is a TEMPLATE asset in the store.** Play
   INSTANTIATES from it; playing never mutates the template.
2. **The player's modifications are a REVISION CHAIN on a store asset.**
   The store already has the machinery (head revisions, `arev_` ids,
   history, trim-history maintenance). Playing a published game forks a
   per-player game asset whose provenance parent is the template; every
   chat edit advances its head.
3. **UNDO = step the head back one revision** (a store-side op). The
   §4.5 addon architecture composes perfectly: an edit IS a small chunk,
   so revisions are cheap and their diffs readable — an edit's revision
   is essentially its addon.
4. **The local game.splash becomes a CACHE of the head revision**, not
   the truth (digest-keyed caches are legal; durable content is not).
5. **History depth is a store policy knob per kind** — the trim-history
   maintenance ("keep newest N") must not eat game lineages; game-kind
   assets likely want deeper or user-controlled history. Contract-level:
   coordinate before schema changes.

**The container structure** (user: "a kinda container structure for a
map — its spawns, its code 'game logic' and so on"): the game asset is
a MULTI-FILE container using the store's declared file roles (the
stems/lyrics side-channels proved the pattern — append-only role tags,
no schema bump, all-or-none groups, validated at publish+admission):

- role **MAP/BASE** — the base level source (terrain, structures,
  roads; procgen output lands here with its seed+params recorded);
- role **ADDONS** — the ordered addon chunks (spawns, bulk adds; the
  user's ambulance lives here);
- role **LOGIC** — game-logic source (objectives, handlers, HUD) — what
  logic-edits rewrite;
- role **KNOBS** — declared tunables + current values (the tune verb's
  target);
- role **META** — already exists (title/description/thumbnail).

One asset revision captures the whole container consistently (one head
= one coherent world state); an edit touches ONE part but mints a
container revision — cheap diffs, readable history, undo = head
rollback. The flat game.splash becomes the ASSEMBLED form
(base → knobs → addons → logic); eval consumes the assembly, and
get_source can serve per-part (a logic edit fetches only LOGIC — the
model reads less). Each §4.5 verb owns a part by construction.
**Persistence invariant** (user: spawned objects are serialized level
state, not session ephemera): every mutating verb lands in the
persisted source — pinned by the harness's
`every_mutating_verb_survives_a_reload` (kill + reload from head ⇒
spawn/addon/tune/swap/remove all survive). LEVEL state (placements,
addons, knobs, player body) is the revision chain; RUNTIME state
(score, mid-walk positions) stays save_data territory.

**Global group** (user, superseding local slots): "these games need to
be shared between multiplayers, and persisted on the asset server …
make this 'games' list global so it's the same for everyone connected."
So for NOW: the games list is ONE shared list — the container asset IS
the shared world, everyone's edits advance ITS revision chain, and the
revision history is the shared undo. No per-player forks or private
drafts yet ("New game" creates a fresh DRAFT GAME ASSET in the global
group, visible to all). The games panel is already a catalog query with
events, so global-by-construction; play = fetch head, edit = publish a
new revision, panels update everywhere. LIVE co-presence (two players
in one world at once) stays the LAN/rooms machinery — this is the
CONTENT layer.

**Observability** (user: "needs to be observable by other asset server
clients — 'hey the global list has been updated' … a watchable resource
or a subscribable SQL structure"): CATALOG EVENTS are that structure,
and they already work — the games panel went 10→13 live over the
subscription when the showcases published, no restart. Verified surface
(store `host/events.rs` + `routes_control.rs`):

- every games mutation emits a first-class kind — `game_published` /
  `game_quarantined` (each carries namespace + game_id + the REVISION,
  so per-edit revisions announce themselves) and `game_alias_set` /
  `game_alias_cleared`; the journal appends inside the same state-thread
  closure as the commit, so event order IS commit order;
- the sandbox panel already uses the subscription as THE update path
  (no polling): any event not provably non-game sets refresh_pending →
  one re-query; alias_set events with revisions invalidate resident
  content precisely;
- the journal is a bounded ring with epoch'd cursors — a lagging or
  restarted reader gets `gap: true` and resyncs, never a silent hole.

Design consequences: "create game" must PUBLISH revision 1 immediately
(an unpublished draft emits nothing — publishing the empty base world
is what makes the new game appear on everyone's panel). Nice-to-have,
contract-adjacent (clear with the coordinator): game events carry
`content_kind: Some("game")` so clients can filter without the
honest-None refresh.

Sequencing: design confirmation with the coordinator first (the create-
game + per-edit-revision publish path is contract-level), then the
sandbox-side part split (assembled eval), then the store-facing half
(roles, per-edit revisions, undo). Local slots are DEAD as a design —
do not build them.

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
