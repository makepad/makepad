# Livecoding VJ effects — the coding-agent contract

You write a `.splash` file. A second later it is in the VJ's grid with a
thumbnail. You edit that same file. The effect **running on screen right
now** recompiles in place. You read one file to find out whether it
compiled.

That is the whole loop. This document is how to drive it.

For what a document IS — engines, stages, shader hooks, signals, dials —
read `apps/vj/src/effects/CONTRACT.md` first. This file only covers the
edit → live cycle.

---

## 1. Where to put the file

Two directories are observed. Both behave identically.

| directory | what it is for |
|---|---|
| `local/vjfx/` | **the scratch origin.** Untracked. Drop new documents here. Created on first run. |
| `apps/vj/resources/effects/` | **the seed origin** — the bundled preset library itself. Editing `42_whatever.splash` here republishes that preset. |

Override the scratch origin with `VJ_FX_ORIGIN=<dir>` (the compile answers
move with it). Sub-directories are walked, up to four levels; hidden files
and symlinks are skipped.

Nothing is copied. The store hashes your file **where it lies** and
catalogues path + size + digest (`libs/asset/store/src/blobrefs.rs`), so
the file you edit stays the one and only copy, and `git` sees exactly what
you wrote.

## 2. Naming, and what the name means

**The alias is the file stem.** `local/vjfx/91_ion_veil.splash` publishes as
`vjfx/91_ion_veil`.

Consequences worth internalising:

- **Same stem = same asset.** Save the file again and the store publishes a
  NEW REVISION of that same asset and re-points the alias at it. That is
  what the running slot watches for.
- **A new stem = a new asset.** Renaming a file makes a second library
  entry; the old one stays until somebody retires it.
- **A bundled preset's stem is that preset.** Editing
  `apps/vj/resources/effects/09_synthwave.splash` republishes
  `vjfx/09_synthwave`. The compiled-in copy in the binary stops being the
  head; a rebuild will not take it back (the seed pass never rewrites a
  head it did not itself publish). If you want the bundled bytes back,
  restore the file — the observer publishes whatever is on disk.
- Stems must be `[a-z0-9._-]`-ish and at most 64 characters (the alias
  charset). A file the store cannot name is skipped silently.
- Follow the library convention for new documents:
  `NN_snake_name.splash`.

**Transitions.** A document lands in the TRANSITION lane when it declares
`engine: "transition"`, `"screen"` or `"tiles"`, or when it says
`transition: true` at the top level. The tag is what the TRANSITION chip
filters on and what the transition slot's type gate accepts.

## 3. What happens when you save

1. **Debounce + stability.** The origin watcher (FSEvents / inotify /
   ReadDirectoryChangesW) collects the editor's write burst for ~80 ms,
   then waits for the file's size and mtime to hold still for ~150 ms. A
   half-written document never becomes a revision.
2. **Digest compare.** If the head revision's `Source` blob already equals
   your file's sha256, **nothing happens** — no revision, no event, no
   thumbnail. Touching a file, or a `git checkout` that restores identical
   bytes, costs one hash.
3. **Publish.** Otherwise: a new revision under the same alias, published
   by reference. Tags: `vjeffect`, `livecoded`, plus `transition` when
   declared.
4. **Event.** The store's `/v1/events` journal carries the republish; the
   VJ's catalog subscriber picks it up within a poll.
5. **Grid tile refreshes.** The tile forgets the revision it remembered and
   re-resolves — keeping its current picture until the new manifest lands,
   so the grid never blanks.
6. **Thumbnail regenerates.** Animated thumbnails are cached by REVISION
   id, so a new revision has no cache and the offscreen bank re-renders a
   fresh 30-frame sheet.
7. **Running slot reloads.** If FX A, FX B or TRANSITION is running that
   asset, the VJ re-fetches the new head's `Source` blob and loads it into
   the **same slot key** — `VjFxView::set_effect_source` replaces the
   document in place, so the effect recompiles without the slot emptying,
   the knobs resetting under your hand, or the program mix moving.

Steps 1–3 are `libs/asset/store/src/observe.rs`; 4–7 are `apps/vj/src/main.rs`
(`pump_subscriber` → `reload_fx_slot_from_store` → `load_fx_slot`).

## 4. Reading the compile result

Two files, both under the scratch origin (`local/vjfx/` unless
`VJ_FX_ORIGIN` moved it):

```
local/vjfx/status/<stem>.status   ← poll this
local/vjfx/compile.log            ← the stream, one line per outcome
```

`<stem>.status` is overwritten on every outcome and always starts with the
verdict:

```
compile ok
doc: 91_ion_veil
revision: arev_9f3c…
t: 1787473982375
```

or

```
compile error: draw shader 'DrawVjFxParticles' failed to compile and will NOT be drawn:
  line 14: unknown function 'wobble'
doc: 91_ion_veil
revision: arev_9f3c…
t: 1787473983102
```

So the poll is `grep -q '^compile ok' local/vjfx/status/91_ion_veil.status`,
and the diagnosis is the rest of the file.

`compile.log` is the same verdicts as one line each, appended, tail-capped:

```
1787473982375 91_ion_veil rev=arev_9f3c1b2d4e5f6a compile ok
1787473983102 91_ion_veil rev=arev_77aa11bb22cc33 compile error: draw shader …
```

### What "ok" actually means

Two different failures can eat your edit, and both are covered:

- **The document does not evaluate** (script/parse error, bad engine name,
  malformed hook). Reported the instant it happens, straight from
  `VjFxView::set_effect_source`'s error.
- **The document evaluates but its draw shader does not compile.** A draw
  shader compiles at DRAW time, not at load, and its failure is reported
  through `error!` — so `compile ok` is only ever written after the
  document has actually been drawn and a short settle window has passed
  with nothing to report. The mechanism is a tap on the app's own log
  (`makepad_error_log::set_log_tap`), not a second validator: whatever the
  app tells a human is what lands in your file.

A shader that fails to compile is skipped, so the on-screen symptom is a
flat clear-color region — never a fallback that quietly renders something
else. A black thumbnail is NOT by itself a verdict (an input-shaping
`screen`/`transition` document renders black with no program behind it),
which is why the answer comes from the compiler's own words.

### When the answer arrives

An outcome is written whenever the document is loaded and drawn:

- as soon as its grid tile's animated thumbnail renders (this is the usual
  path for a brand-new document — the offscreen bank picks it up when the
  tile is in the video grid), or
- immediately when it is loaded into an FX slot, hot reload included.

Only documents that live in an observed origin get files written; the rest
of the library rendering its own thumbnails is not news.

## 5. The fast cycle

```bash
# 1. host the store in the VJ itself, and drive it over HTTP
cargo build --release -p makepad-vj
VJ_ASSET_EMBED=always ./target/release/makepad-vj --remote > /tmp/vj.log 2>&1 &
P=$(grep -o 'listening on 127.0.0.1:[0-9]*' /tmp/vj.log | grep -o '[0-9]*$')

# 2. write a document
cat > local/vjfx/91_ion_veil.splash <<'EOF'
// ION VEIL — <what this teaches>.
{
    name: "Ion Veil"
    engine: "particles"
    ...
}
EOF

# 3. read the verdict (it lands within a second or two of the save)
cat local/vjfx/status/91_ion_veil.status

# 4. look at it. `/snap` only sees tiles that are ON SCREEN, so type the
#    name into the grid's filter box first, then click the tile (an FX
#    click with no slot armed lands on the faded-out side and fades in).
curl -s "http://127.0.0.1:$P/click?x=68&y=616&wait=1"   # the filter box
curl -s "http://127.0.0.1:$P/t?t=Ion"
curl -s "http://127.0.0.1:$P/snap?q=Ion"
curl -s "http://127.0.0.1:$P/click?x=…&y=…&wait=1"
curl -s "http://127.0.0.1:$P/g"

# 5. edit the SAME file — the running slot recompiles in place; grab again
#    to see the change, and re-read the .status file.

# 6. always
curl -s "http://127.0.0.1:$P/gq"
```

`VJ_ASSET_EMBED=always` is what makes the VJ host its own store, which is
what makes it the process that observes your directories. See §7.

For look iteration WITHOUT the app, the gallery rig is still the sharper
instrument — deterministic frames, whole-library sweeps, no store at all:
`VJFX_DOC=<name> ./target/release/examples/effect_gallery --remote` and the
`VJFX_CAPTURE` / `VJFX_SWEEP` / `VJFX_INPUT` levers in CONTRACT.md
§"Verifying a look change". Use the gallery to get the look right; use
livecoding to see it in the mix, on the beat, against real content.

## 6. The laws

These are not style notes. Both have shipped as bugs.

- **A look must be a BOUNDED function of `time` and `beat`.** `beat` is the
  host's SONG clock: an effect cued an hour into a set gets its first frame
  at beat ~6000 and `time` free-runs for as long as the slot holds the
  document. Anything steering geometry — a fold angle, a scale, a domain
  size — must be periodic (`sin`/`cos`/`fract` of the clock), clamped, or
  both. `angle = 0.52 + beat * 0.010` looks lovely for the two seconds a
  capture runs and is a BLACK FRAME on the deck. "Never repeats" is still
  available honestly: sum two slow sines whose frequency ratio is not a
  simple fraction. Verify with `VJFX_CAPTURE=600@1.0` (t = 0 / 60 / 600 /
  3600 s) before calling a document done. A bundled preset that uses the
  raw beat count must be named in
  `seed::registry_tests::every_beat_count_use_is_reviewed`, with the
  reason.
- **Never name an effect's inspiration source.** Not in `name:`, not in the
  comment block, not in the file stem, not in the shader. Describe what the
  look IS — the motion, the material, the colour — not what it is "like".
  The comment block is the description the catalog shows; write it to
  teach the pattern the document demonstrates.
- **A document carries its own shader.** The pixel math goes IN THE FILE,
  as a `shader:` block subclassing the engine's draw shader. Self-
  containedness beats DRY: two documents of one family each carry their own
  copy of the same look function. This is what makes a file forkable, and
  it is what makes livecoding worth anything — an author who can only
  re-tune `amp: 0.6` can only make a variation.

## 7. Who observes, and why it is off by default

Admitting content by reference means "read any file this process can read,
then serve it by digest" — exactly the privilege of the process itself.
That is fine for an app hosting its own store on loopback and is not fine
for anything reachable off the box, so `BlobRefPolicy` is off unless an
embedder turns it on, and refuses any non-loopback caller when it is.

Therefore: **the process that HOSTS the store owns the watch.**

| you are running | observes? |
|---|---|
| VJ with `VJ_ASSET_EMBED=always` (or `auto` with nothing else up) | yes — it hosts |
| asset-ui holding the catalog root | yes — it hosts |
| VJ attached to asset-ui's store | no — asset-ui is doing it |
| VJ attached to a store on another machine | **no.** Local paths mean nothing there. |

If you are livecoding and nothing appears, that is the first thing to
check: the VJ's status line says which store it is on, and the app log
carries `[observe] watching …` from whichever process started the watcher.

The VJ starts its observer on the same worker thread that seeds the bundled
library, and starts it AFTER seeding — so a preset file you edited while
the app was off wins over the compiled-in bytes without racing for it.

## 8. Where the code is

| file | what |
|---|---|
| `libs/asset/store/src/observe.rs` | the watch service: debounce, write-stability, digest compare, publish-by-reference |
| `libs/asset/store/tests/http/observe_http.rs` | the end-to-end proof (drop, edit, republish, no copy, prompt stop) |
| `libs/filesystem_watcher/` | the per-platform directory watcher |
| `apps/vj/src/livecode.rs` | origins, the log tap, the `.status` / `compile.log` writer |
| `apps/vj/src/main.rs` | republish → slot reload (`reload_fx_slot_from_store`), observer start |
| `apps/asset-ui/src/asset_store_state.rs` | `start_observe_loop`, hosted-only |
| `apps/vj/src/effects/CONTRACT.md` | what a document is |
| `apps/vj/src/effects/IDEAS.md` | the look backlog |
