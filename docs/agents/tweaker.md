# Tweaker: live styling and source write-back

Read this when the user is giving visual feedback or when applying a live
style change. Follow [AGENTS.md](../../AGENTS.md) for instance ownership and
capture policy. The overlay implementation is
[widgets/src/tweaker.rs](../../widgets/src/tweaker.rs).

## Interaction and routes

Toggle with Shift+F10 or `GET /tweak?on=1`. The overlay adds a property
sidebar and intercepts pointer events over the app body: selecting a button
outlines it without firing its action. Turn it off before testing ordinary
widget clicks.

| Route | Result |
|---|---|
| `/tweak?on=1` or `on=0` | Enable/disable the overlay |
| `/tweak?annotate=1` or `annotate=0` | Enable/disable freehand annotation; Alt-drag also draws |
| `/tweak/state` | Selection, reflected properties, hover, edits, and annotations |
| `POST /tweak/apply` | Apply one property or a Splash chunk and wait for relayout |
| `/tweak/diff` | Ordered edit log |
| `/tweak/clear` | Clear edit log and annotations |
| `/tweak/final` | Coalesced initial/final values and annotations; may include a PNG |
| `/tweak/grab` | App-owned grab including the overlay |

The apply body can name one property:

```json
{"path":"main_window.body.panel","prop":"padding","value":"Inset{left: 20}"}
```

Or merge a Splash chunk onto the selected instance:

```json
{"path":"main_window.body.panel","splash":"{draw_bg +: {border_radius: 8}}"}
```

Use the selected widget's reflected property names; not every draw type
exposes the same radius/color fields. In `/tweak/state`, `set:1` means a
property was explicitly applied.

Sidebar edits emit `TWEAK sidebar <path> <prop> <old> -> <new>` in the app
log. Listen through `/log?since=N`; do not poll `/tweak/state` for changes.
Read state when inspecting a selection or verifying an applied value.

## Structural edits: the designer

The Build tab's design session edits the Splash source of the file that
declares a widget: insert, move, delete, wrap, rename and set properties.
Each edit is a byte-range hunk inside the `script_mod!` body, previewed
live in the running app. The app never writes the file; the agent applies
the patch. The same session is reachable over HTTP, and a person's Build
tab edits and an agent's routes share one undo history.

| Route | Result |
|---|---|
| `/design/open?path=P` | Start a session on the file declaring `P` (default: the pinned widget) |
| `/design/state` | File, edit count, status, `can_undo`/`can_redo`, `landing`, hunk labels |
| `/design/palette` | Insertable types with their default bodies |
| `/design/insert?path=P&place=before\|after\|inside&type=T&body=B` | Insert `T` (body defaults to the palette's); answers the new widget's path in `select` |
| `/design/move?path=P&to=Q&place=...` | Move `P` next to or into `Q` |
| `/design/delete`, `duplicate`, `wrap`, `up`, `down`, `out` `?path=P` | Structural edits of `P` |
| `/design/rename?path=P&name=N` | Name, rename or (empty `name`) unname a literal |
| `/design/set?path=P&key=K&value=V` | Write a property into the literal (`+:` for a new typed property) |
| `/design/bake` | Write every value-ledger tweak on this file's widgets into the source |
| `/design/undo`, `redo`, `reset` | Step the source edits |
| `/design/patch`, `/design/commit` | The unified diff; `commit` adds `base_hash`, `new_hash`, `base_matches_disk` and the hunks |
| `/design/verify` | Every drawn widget of the file and whether its literal is found |
| `/design/close` | End the session; writes `local/design/<file>.patch` |

Edits answer after the frame that shows them, so a `/g` right after sees
the change; send `if_user_seq` like any other mutation. An edit whose
preview the runtime rejects is rolled back and reported in `status`. An op
on a `#(...)` placeholder is refused (it needs a rebuild), and so is an
edit to a widget declared in another file. To persist: read
`/design/commit`, check `base_matches_disk`, apply `diff` to the file, then
rebuild, relaunch and verify as below.

## Motion: the tween inspector

The Motion tab lists every animation the app's `TweenHost`s play (the
Splash `tween` driver, stack navigation, the widgets and stories built on
the tween engine) and shows the chosen one on a timeline: its labels on
the ruler, one lane per child indented by depth, and the playhead. Drag on
the timeline to scrub (the animation pauses, callbacks stay quiet, and it
plays on at release if it was playing); the chips pause, restart, set
the speed, loop (repeat forever) and yoyo; the divider between the
lane names and the timeline drags. Hosts publish only while the tab is up.

`/tweak/op?op=motion` answers the same data as JSON: `open`, and per host
`id`, `name`, `age` and `roots` (`name`, `kind`, `len`, `time`, `paused`,
`linked` (0 once a kept animation completed), `repeat`, `yoyo`, `scale`,
`lanes`, `labels`). Add `host=<id>&root=<k>` with one of
`seek=<seconds>`, `pause=1`, `resume=1`, `restart=1`, `scale=<x>`,
`loop=1|0` (repeat forever, or none) or `yoyo=1|0` to
control the k-th top-level animation of that host; the host applies it on
its next event, so read again after a frame. `open=1` publishes without
the tab.

## Persist the result

The overlay never writes source. With a design session open,
`/design/bake` turns the value tweaks into hunks and `/design/commit`
returns the patch; without one, at the end of a styling session:

1. Read `/tweak/final`. If annotations produced a `png`, inspect it; the
   strokes are part of the user's feedback.
2. Resolve each dotted widget path to its `script_mod!` declaration.
   Skip anonymous `-` segments when searching. `/d` shows the same ids.
3. Write properties at the most specific existing instance declaration.
   Create an instance override only if no appropriate site exists.
4. Preserve typed-property merging with `+:`. Plain walk/layout properties
   are assigned directly with the correct typed value.
5. Preserve returned source spellings, adding the Rust-macro `#x` color
   escape where required.
6. Rebuild and relaunch. Verify values through reflection or snapshots,
   and inspect a new frame when the visual result matters.
7. Finish your test instance through `/gq`, or the cleanup fallback in
   the runbook.

Use these HTTP routes directly; this workflow does not require a helper
under `local/`.
