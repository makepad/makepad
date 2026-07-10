# __PROJECT_NAME__ — a kid's game, live inside Makepad

A child asks for changes by voice through the Game Maker app; you make them by
editing **`game.splash`** — the whole game is that one file, a Makepad splash
script evaluated live. **Every clean edit hot-reloads the world the kid is
watching, instantly, while you keep working.** A broken edit never replaces the
running game — the last working world stays up and the error waits for you in
`./tools/ag errors`.

**The complete `game.*` API reference is in your system prompt** (the Splash
Game DSL Guide). Verbs not listed there do not exist. This file is the
project-local workflow.

## Checking your work — ALWAYS

1. After every edit run `./tools/ag errors`. Empty = your edit is live in front
   of the kid. An error = the kid still sees the OLD world; fix it.
   (Broken edits and runtime crashes are also reported back to you
   automatically as a "⚠ game error" message — but `ag errors` after each edit
   remains YOUR check; don't wait to be told.)
2. To playtest: `./tools/ag test 120 tools/tapes/selftest.json` restarts the
   game, replays the frame-numbered input tape, and writes `.agent/sheet.png`
   (a grid of frames over time) + `.agent/probe.txt` (pos/vel of probed tags
   every 15 frames). **Read the image**, and read the numbers — "the jump
   clears the step" should be a probe line you saw, not a hope.
3. `./tools/ag peek` = 4 screenshots of the live game + entity state, without
   interrupting the kid.
4. `./tools/ag logs` tails the game log (your `game.log()` lines + eval reports).
5. If the game feels slow: `./tools/ag perf` — per-phase frame profile (script,
   physics, draw). Keep script under ~2ms; if `wait` dominates, it's not your
   game's fault, leave it be.

Tapes are JSON: `{"probe": ["player"], "events": [{"f":5,"press":"right"},
{"f":30,"press":"jump"},{"f":33,"release":"jump"}]}` — actions are the input
names (`left right up down jump shoot grab`). Same tape, same frames: runs are
repeatable.

## House style

- Everything is procedural: colored shapes and synthesized sound. No image,
  model, or audio files — no files besides game.splash.
- Give creatures a face (`game.part` eyes) and a name (`game.label`), and give
  actions sounds (`game.sfx`) — without being asked. That's what makes it real.
- Outdoor game? `game.sky({})` + `game.terrain({smooth: true, bands: [...]})`.
- Small, visible changes. Tune constants and add shapes; avoid big rewrites.
- Movers are ~0.8×1.6×0.8. Keep the playfield within the terrain you built.

## Gotchas

- **Hex colors containing the letter `e` need the `#x` prefix**: `#x2ecc71`.
- `let` and `fn` go at the TOP of the file, before other statements.
- The file re-runs from the top on each edit — don't accumulate state across
  edits. `game.time()` restarts at 0 on every reload.
- `on_touch` fires every overlapping tick — latch or remove, or you'll play 60
  win jingles a second.
