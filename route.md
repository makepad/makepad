# route.md — AI trip planner (apps/route)

A voice-first, AI-driven route/trip planner built on the makepad map stack.
The product framing (from the mobile/server architecture notes): **a
replacement for the Tesla GPS app** — but conversational. You talk to it
while planning or driving:

- "Take me to Groningen, charge somewhere nice halfway."
- "Give me pictures of sights to see along the way."
- "Is it raining at the next charge stop? Can you move it so it doesn't rain?"
- "Find a supermarket near the destination that's still open when we arrive."
- "What's that big building on the left?" (VLM on camera/map)

Target device: **M5 iPad Pro**. That decides almost everything below: local
LLM + vision runs on makepad-ggml on-device, RAM is a hard budget (assume
~16GB total, model weights alone ~7GB), map database lives on a server with a
bounded local cache, and every data structure that is in-RAM today must
eventually stream from disk/network.

The core architectural idea (user directive): **two-tier LLM**. A local
Qwen3.5-9B is the always-on dispatcher — it handles the voice loop, runs the
cheap deterministic tools, and *decides* when to escalate: ask a cloud LLM
for open-ended knowledge, fire a cloud image search, etc. Cloud is a tool
the local model calls, not the other way round.

---

## 1. What we already have

The repo already contains almost every organ this app needs; nothing here is
green-field. Inventory, by role:

### Map rendering & interaction — `widgets/src/map` (feature `maps`)
- `MapView` widget: vector tiles (mbtiles, own path via `mbtiles_path`
  property), 3D buildings, tilt/rotation, terrain.
- Programmatic camera: `fly_to`, `set_center`, `set_map_zoom`, `set_tilt`,
  `set_rotation`.
- Route display: `set_route(points)`, `set_route_progress`, `clear_route`;
  markers `set_markers` + `marker_clicked`; position puck `set_puck`.
- Live overlays already built in: `set_rain_frames` (radar animation),
  `set_wind_field`, `set_terrain_overlay`, `set_overlay_paths`.
- Events: `tapped`, `long_pressed`, `viewport_changed`, `pin_tapped`.
- Coordinate mapping both ways (`screen_to_lon_lat` / `lon_lat_to_screen`).

### Navigation — `libs/map_nav` + `examples/map`
- `SearchIndex` (places/streets/addresses/category queries), `RouteGraph`
  (A*, car/bike/foot, ~20-30ms province scale), `NavSession` turn-by-turn.
- Data: NH detail graph + `europe-places.search` + `europe.searchdb`
  (disk-streamed pread design — the model for everything mobile) +
  `europe-major.graph` long-haul fallback.
- `examples/map` (1.8k lines) is a working navigator: search panel, routing,
  simulated drive, follow camera, layer toggles. apps/route harvests its
  patterns (worker-thread request/response, overlay plumbing) rather than
  forking the whole file.

### Geodata & weather — `libs/geodata`
- 9 built NL overlay layers (chargers, nature, transit, buildings-age,
  demographics, terrain, noise, flood, wijkbuurt) as per-layer mbtiles.
- **The LLM query surface already exists**: every vector layer carries a
  `features` sidecar; `query::LayerDb` does point/radius/bbox queries.
  This was built explicitly for LLM reasoning (goal #2 of that crate).
- `RadarSync`: KNMI rain-radar nowcast poll/cache, app-embeddable. Nowcast
  covers ~the next 2 hours; beyond that we need a forecast API (gap below).
- Chargers layer is NDW OCPI with kW tiers — the charge-planning substrate.

### AI plumbing — `libs/makepad_ai`
- `Agent` trait + `AgentChat`, streaming `AgentEvent`s.
- Backends: Claude API (tool use fully wired: `ToolDefinition`,
  `ToolUse`/`ToolResult` blocks, `StopReason::ToolUse`), Claude Code, ACP,
  Gemini, OpenAI-compatible (= any local server too).
- This is the cloud leg and the tool-schema vocabulary. Missing: a backend
  that runs on our *in-process* llama (gap below).

### Local LLM + vision — `libs/llama` on makepad-ggml
- Qwen3.5-9B UD-Q4_K_XL running locally, prefill bug fixed (batch 32),
  ~28 tok/s generation.
- **VLM shipped**: vision.rs mmproj + ViT, byte-exact vs llama-mtmd-cli.
  192-token image encodes in ~103ms. Open: activation-memory pass for iPad
  (currently ~3.8MB/patch peak).
- No chat/tool-call harness yet — sessions are raw; QwenFilter hand-builds
  ChatML (gap below).

### Voice — `libs/voice`, `libs/voice2`, `libs/tts`, `libs/converse`
- Silero VAD: pure-Rust port, validated, ~425µs/chunk, wired as packet gate.
- STT: whisper transcriber (apple/cpu/metal paths in voice2); Apple speech
  as fallback. Whisper-on-ggml port is an open thread.
- TTS: kokoro (own port; DC-ring fixed) + Apple TTS fallback.
- `ConversePipeline`: VAD → STT → `TranscriptFilter` → `makepad_ai` Agent →
  `SpeechOutput`. The filter is already a *local LLM in the loop*:
  `QwenFilter` judges SEND/SKIP at ~3-5s/judgement (latency levers known:
  shorter prompt, prefix-KV reuse).

### Vehicle — `libs/tesla`
- `TeslaClient`: vehicles list, charge state (SoC), location, wake, with a
  credentials file flow. Non-streaming by design.

### Web image search — `examples/ddgo`
- DuckDuckGo image search fully worked out in a Splash script: fetch page →
  extract `vqd` token → `i.js` JSON endpoint → thumbnails via
  `http_resource`. Needs a Rust port as a tool (gap below), but the
  protocol reverse-engineering is done.

### Testing & automation
- `MAKEPAD=headless` render-to-png (also the pixel source for VLM-on-map).
- Studio remote bridge for driving the app in integration tests.
- `examples/map/tests` shows the UI-suite pattern.

---

## 2. What we need to build

Ordered roughly by how much they block everything else:

1. **Tool broker** (new module in apps/route, later maybe a lib): one
   registry of typed tools — name, JSON schema, executor. Rendered to
   `makepad_ai::ToolDefinition` for cloud backends *and* to Qwen ChatML
   `<tools>` blocks for the local model. Executors run on worker threads,
   results come back as compact text/JSON digests sized for small contexts.
2. **`LocalAgent`**: an `Agent` impl backed by in-process `LlamaSession` —
   ChatML chat template, streaming, `<tool_call>` JSON parse (Hermes-style,
   which Qwen3.5 emits), cancellation/barge-in. This makes local and cloud
   interchangeable behind the same trait.
3. **Escalation router**: the local model gets `ask_cloud` (knowledge,
   summarization, long-form answers) and `search_images` etc. as *tools*.
   Policy prompt: answer locally when tools suffice; escalate for world
   knowledge, reviews, anything the local 9B would hallucinate. Cloud calls
   surface visibly in the UI (and are skippable offline).
4. **Trip domain model** (`trip.rs`): the single source of truth — ordered
   stops, legs with polylines/ETAs, charge stops with SoC in/out, weather
   annotations. The LLM only ever references it by stable ids
   (`stop_2`, `leg_3`); replans mutate this struct, the map mirrors it.
   The LLM never carries route geometry in context.
5. **Corridor query** ("points along route"): sample the route polyline,
   radius-query `LayerDb` + `SearchIndex` per sample, dedupe, rank, emit an
   LLM-digestible list: `km 42 | +3min detour | Zaanse Schans | windmills,
   heritage`. This is the workhorse tool for "sights along the way",
   "chargers ahead", "supermarket near arrival".
6. **Weather beyond nowcast**: radar answers "now + 2h" numerically (never
   ask the VLM to eyeball radar pixels — user rule). For "at the charge
   stop at 15:40" we need a point-forecast API (Open-Meteo: free, no key,
   hourly precip/wind — fits the geodata politeness rules: cache, recheck
   gates, If-Modified-Since). New `libs/geodata` live-source module next to
   `RadarSync`.
7. **EV energy/charge planner**: consumption model (speed² + climb — per-edge
   climb baking from the terrain layer is a known layers.md follow-up),
   SoC-feasible charge-stop insertion from the chargers layer (kW tiers),
   constraint-based re-siting ("not raining", "has food", "≥150kW"). Tesla
   client supplies live SoC.
8. **DDG image search in Rust**: port the ddgo script flow onto
   `cx` HTTP requests; image cards UI (thumbnail grid via `http_resource`
   equivalents), rate-limited and cached.
9. **Sights/knowledge**: the search index knows *names and categories*, not
   *why something is interesting*. That's exactly the escalation split:
   corridor tool finds candidates locally → cloud LLM ranks/describes → images
   via DDG. Optional later: offline Wikivoyage/Wikipedia extracts as a
   geodata layer so "sights" degrades gracefully offline.
10. **Voice wiring**: instantiate `ConversePipeline` in apps/route (the map
    integration was explicitly deferred in the converse work). Barge-in
    (VAD gates TTS), driving mode (short spoken answers, no cards).
11. **Assistant UI**: transcript panel (M0 has the stub), card stack over
    the map (image grids, charger cards with kW/price, weather strips,
    confirm/cancel for replans), every tool effect mirrored visibly on the
    map (fly_to, markers, route redraw) so the user always sees what the
    agent did.
12. **iPad hardening**: VLM activation-memory pass (open item), NH
    SearchIndex/RouteGraph moved to the europe.searchdb pread model,
    tile fetch through an HTTP server + bounded local cache (the tile.rs
    load path was designed to admit this), model residency policy (9B Q4
    ~6GB + mmproj 875MB + whisper + kokoro must coexist with the map).

---

## 3. Architecture

```
                ┌─────────────────────────────────────────────┐
                │                 apps/route                  │
                │                                             │
  mic ──► VAD ──► whisper STT ──► QwenFilter (SEND/SKIP)      │
                │                     │                       │
                │                     ▼                       │
                │              LocalAgent (Qwen3.5-9B)        │
                │              system prompt + TOOLS          │
                │       ┌───────┬─────────┬──────────┐        │
                │       ▼       ▼         ▼          ▼        │
                │   map.*   route.*   geo.* weather.* …       │
                │   (tool broker — deterministic Rust)        │
                │       │                                     │
                │       │   escalation tools                  │
                │       ├─► cloud.ask ──► makepad_ai (Claude) │
                │       ├─► images.search ──► DuckDuckGo      │
                │       └─► vision.describe ─► local VLM      │
                │                     │                       │
                │                     ▼                       │
                │   TripModel (source of truth) ◄─► MapView   │
                │                     │                       │
                │                     ▼                       │
                │           kokoro TTS + card UI              │
                └─────────────────────────────────────────────┘

  Data planes:
  on-device: mbtiles cache, search/graph indexes, overlay layers, models
  own server: full Europe tile/search/graph DB (mobile can't hold 33GB+)
  external:  Open-Meteo, KNMI radar, DuckDuckGo, Tesla Fleet API, Claude
```

Principles:
- **Tools are numeric and deterministic.** The LLM orchestrates; Rust
  computes. Rain at a point comes from radar/forecast *data*, never from a
  model looking at pixels. The VLM is for human-facing imagery (user
  photos, camera, "what am I looking at").
- **State lives in Rust.** TripModel, not the chat transcript, is truth.
  Tool results are compact digests with stable ids; the context window
  stays small enough for a 9B with 2-4k context to stay sharp.
- **Local first, cloud visible.** Everything that can be answered from
  on-device data is. Cloud escalation is explicit, logged in the
  transcript, and absent offline — the app must degrade to a fully
  functional offline navigator.
- **Every agent action is visible.** If the model flies the camera, drops
  markers, or replans, the UI shows it happening. No silent mutations;
  destructive replans (changing an active route) get a confirm card unless
  hands-free mode says speak-to-confirm.

## 4. Tool suite (v1 spec)

Names/args stabilize in M1; grouped by namespace. "Digest" = compact,
line-oriented text designed for small contexts.

**map.** — camera & display (all backed by existing MapViewRef API)
| tool | args | effect |
|---|---|---|
| map.fly_to | lon, lat, zoom? | camera move |
| map.show_trip | trip? | fit route bounds, draw route + stop markers |
| map.set_markers | [{lon,lat,label,kind}] | ad-hoc markers (search results, sights) |
| map.set_layer | layer, on | rain/wind/chargers/terrain/… overlays |
| map.screenshot | — | png for vision.describe / debugging |

**geo.** — search & describe (SearchIndex + LayerDb)
| tool | args | returns |
|---|---|---|
| geo.search | query, near?, category? | candidate list with ids, lon/lat, kind |
| geo.describe | lon, lat, radius? | digest of layers at point (district, nature, noise, flood, buildings age) |
| geo.chargers | near \| corridor, min_kw?, limit? | charger digest (kW tier, operator, distance/detour) |

**route.** — trip planning (RouteGraph + TripModel)
| tool | args | returns |
|---|---|---|
| route.plan | from, to, via[], mode | new TripModel; digest: legs, km, ETA, SoC profile |
| route.add_stop / remove_stop | stop ref, position? | replan digest + ETA delta |
| route.move_stop | stop_id, constraint ("no rain", "≥150kW", "near food") | candidate re-sitings with tradeoffs, applies best or asks |
| route.along | kinds[], max_detour_min?, limit? | corridor digest (the workhorse — sights/POI/chargers along route) |
| route.status | — | where are we, next maneuver, ETA, SoC at arrival |

**weather.**
| tool | args | returns |
|---|---|---|
| weather.now | lon, lat | radar nowcast: rain mm/h now and +30/60/120min |
| weather.at | lon, lat, time | Open-Meteo point forecast (precip prob, mm, wind, temp) |
| weather.along_trip | trip | per-stop/per-leg forecast at each ETA — one digest table |

**vehicle.** (libs/tesla)
| tool | args | returns |
|---|---|---|
| vehicle.status | — | SoC, range est, charging state, location |

**images. / vision.**
| tool | args | returns |
|---|---|---|
| images.search | query, n? | DDG image results → thumbnail card grid; returns captions+ids to LLM |
| vision.describe | image_ref \| screenshot \| camera, question | local VLM answer |

**cloud.**
| tool | args | returns |
|---|---|---|
| cloud.ask | question, context_digest | Claude answer (knowledge, ranking, prose) — the escalation valve |

**session.**
| tool | args | returns |
|---|---|---|
| trip.save / trip.load | name | persist/restore TripModel |
| prefs.set / prefs.get | key, value | avoid-highways, min charger kW, home, units |

## 5. Worked example — "is it raining at the next charge stop? move it"

1. `route.status` → next charge stop is `stop_2` (Fastned Lelystad), ETA 15:38.
2. `weather.at(stop_2, 15:38)` → 82% precip, 2.1mm/h. Speak: "Yes — likely
   raining there around 15:40. Want me to find a dry alternative?"
3. User: "yes" → `route.move_stop(stop_2, constraint: "no rain")`:
   - planner queries chargers in the SoC-feasible window (km 140–230,
     ≥150kW), gets 6 candidates from the chargers layer;
   - `weather.at` each candidate at its *shifted* ETA;
   - scores: dry ∧ min detour ∧ kW tier; picks Ionity Harderwijk
     (+4 min, 0% precip).
4. Tool applies the replan to TripModel → map redraws route, old stop marker
   fades, new one drops, confirm card shows "+4 min, arrive 17:52".
5. TTS: "Moved charging to Ionity Harderwijk — dry there, costs four
   minutes." — total cloud calls used: zero.

"Pictures of sights along the way" for contrast: `route.along(kinds:
[tourism, nature, heritage])` locally → `cloud.ask` ranks the 15 candidates
with one-line whys → `images.search` top 4 → card grid + markers; the local
model narrates. Two cloud touches, both visible.

## 6. Milestones

**Status 2026-07-30 (2): M2 shipped and live-verified** — `local_agent.rs`
runs the in-process Qwen3.5-9B (pure-Rust ggml, no external processes) as
the default dispatcher: append-only session = resident system+tools prefix
(~1.9k tok), streaming, per-token cancel, Qwen's real tool template (XML
function blocks, NOT Hermes JSON — §2.2 corrected). First live turn: valid
route_plan call, prefill 116 tok 0.6s, gen 20.5 tok/s, ~7s round trip.
`cloud_ask` escalation tool wired (Claude side-agent; graceful offline
error). Open: reliability measurement across the tool suite, the ggml
per-token graph-compile patch, 4B comparison. Also new: platform memory
watchdog + studio zombie-build orphan guard (the "157 GB leak").

**Status 2026-07-30: M0 + M1 shipped**, plus early pulls from later
milestones: platform geo API (`cx.start_location_updates` on
macOS/iOS/Android/web — was M6-adjacent), map layers/themes parity with
examples/map behind `map_set_layer`/`map_set_theme`, re-applyable trip
snapshots (`>` rows in the transcript), and the on-disk drive history
(`local/route_history/*.jsonl`, schema reserves timelapse `media` + `synced`
for the video/server phases). Typed `/tool_name {json}` runs any broker tool
without the LLM (the test path — verified via studio bridge). Cloud agent
loop is wired but needs ANTHROPIC_API_KEY (env or repo-root file). Runnable
from studio (`makepad.splash`).

- **M0 — skeleton (this commit).** `apps/route` crate: MapView + assistant
  panel + typed prompt stub. Workspace member; builds; route.md.
- **M1 — tool broker + typed chat, cloud brain first.** Broker + TripModel
  + first tools (geo.search, map.fly_to/show_trip/set_markers, route.plan,
  route.along, weather.now via RadarSync). Drive it with the *Claude
  backend* (tool-use already works there) to prove the loop end-to-end
  before any local-LLM work. Typed input only.
- **M2 — local dispatcher.** `LocalAgent` on LlamaSession (ChatML +
  Hermes tool-calls), escalation router (`cloud.ask` demoted to a tool),
  latency work (prefix-KV reuse across turns matters here more than in the
  filter). Measure tool-call reliability on the 9B; keep the cloud backend
  as a config fallback.
- **M3 — voice.** ConversePipeline wired in: VAD → whisper → filter →
  LocalAgent → kokoro. Barge-in, driving mode (terse speech, no cards
  unless asked). Apple STT/TTS as fallback path on iPad until the
  whisper/kokoro ggml ports land.
- **M4 — imagery.** DDG port + image cards; vision.describe over user
  photos/camera/map screenshots (bounded by the VLM memory pass).
- **M5 — EV brain.** Energy model (terrain climb baking), SoC-feasible
  charge planning, route.move_stop constraints, weather.along_trip,
  Tesla live SoC. This is the headline demo milestone.
- **M6 — iPad productization.** Memory budget enforcement, pread/streamed
  indexes, server tile fetch + bounded cache, model residency, packaging.

Testing throughout: broker tools get plain unit tests (they're
deterministic); agent loop gets recorded-fixture tests (canned LLM
responses replayed); UI flows via studio bridge + headless render suites
(the examples/map/tests pattern).

## 7. Risks & open questions

- **Qwen3.5-9B tool-calling reliability** under Q4 quantization with 25+
  tools: may need tool-subsetting per turn (router prompt picks a
  namespace first), few-shot in system prompt, or grammar-constrained
  decode (ggml-side sampler work). M2's first job is measuring this.
- **Local latency.** Filter judgements are ~3-5s today; a dispatch turn
  with 2-3 tool round-trips must stay conversational. Levers: prefix-KV
  reuse (system prompt + tools cached once), shorter digests, 4B model for
  the filter, streaming TTS start on first sentence.
- **RAM ceiling.** 9B (6GB) + mmproj (0.9GB) + whisper + kokoro + map
  caches on a 16GB iPad is tight; may force the 4B as dispatcher with 9B
  swapped in, or quantized KV. Needs a real budget spreadsheet in M6.
- **DDG fragility/politeness**: vqd flow is unofficial; cache hard, rate
  limit, degrade gracefully (feature dies, app doesn't).
- **Forecast dependency**: Open-Meteo is the pragmatic pick; keep the
  weather module swappable (KNMI EPS later, and radar stays primary <2h).
- **Charging data quality**: NDW OCPI is NL; Europe-wide chargers layer
  needed for long trips (geodata layer addition, same recipe).
- **Trip-active replanning UX** while driving: confirm-by-voice design,
  and never mutate the active NavSession without an explicit yes.
- **Where does the broker live long-term?** Start in apps/route; if
  gamemaker/aichat want tools too, extract `libs/agent_tools` then, not
  now.

## 8. Directory shape (target)

```
apps/route/
  Cargo.toml
  src/
    main.rs        app shell, UI, event wiring
    trip.rs        TripModel (M1)
    broker.rs      tool registry + schemas + dispatch (M1)
    tools/         one module per namespace: map.rs geo.rs route.rs
                   weather.rs vehicle.rs images.rs cloud.rs (M1+)
    local_agent.rs LlamaSession Agent impl (M2)
    voice.rs       ConversePipeline wiring (M3)
    cards.rs       card-stack UI widgets (M3/M4)
  tests/           broker unit tests + headless UI suite
```
