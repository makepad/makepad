# Tweaker: live styling and source write-back

Read this when the user is giving visual feedback or when applying a live
style change. Follow [AGENTS.md](../../AGENTS.md) for instance ownership and
capture policy. The overlay implementation is
[widgets/families/tweaker/src/tweaker.rs](../../widgets/families/tweaker/src/tweaker.rs).

## Interaction and commands

Toggle with Shift+F10 or `tweak on=1`. The overlay adds a property
sidebar and intercepts pointer events over the app body: selecting a button
outlines it without firing its action. Turn it off before testing ordinary
widget clicks.

| Command | Result |
|---|---|
| `tweak on=1` or `on=0` | Enable/disable the overlay |
| `tweak annotate=1` or `annotate=0` | Enable/disable freehand annotation; Alt-drag also draws |
| `tweak state` | Selection, reflected properties, hover, edits, and annotations |
| `tweak apply` | Apply one property or a Splash chunk and wait for relayout |
| `tweak diff` | Ordered edit log |
| `tweak clear` | Clear edit log and annotations |
| `tweak final` | Coalesced initial/final values and annotations; may include a PNG |
| `tweak grab` | App-owned grab including the overlay |

`tweak apply` can name one property:

```sh
makepad-remote app $P tweak apply path=main_window.body.panel prop=padding 'value=Inset{left: 20}'
```

Or merge a Splash chunk onto the selected instance:

```sh
makepad-remote app $P tweak apply path=main_window.body.panel 'splash={draw_bg +: {border_radius: 8}}'
```

`tweak apply` leaves the overlay as it is: an edit through the bridge opens no
panel and takes no pointer events. Send `tweak on=1` to see the selection.

Use the selected widget's reflected property names; not every draw type
exposes the same radius/color fields. In `tweak state`, `set:1` means a
property was explicitly applied.

Sidebar edits emit `TWEAK sidebar <path> <prop> <old> -> <new>` in the app
log. Listen through `log since=N`; do not poll `tweak state` for changes.
Read state when inspecting a selection or verifying an applied value.

## Live design data

Every app with a registered widget root serves these commands over its existing remote.
Use the readable `ref` from a pick as the path on subsequent requests.

| Command | Result |
|---|---|
| `design pick at=x,y` or `design pick path=P` | Widget id, type, readable path, window-local rect and spacing bands. Insets are ordered left, top, right, bottom. |
| `design props path=P` | Current property values, their origin and the indexed source cascade. |
| `design apply path=P prop=padding 'value=Inset{left: 20}'` or `design apply path=P 'splash={...}'` | Applies to one instance, relayouts, and returns the old and new reflected values. |
| `design typed path=P prop=width 'value=Size.Fixed(200)'` | The same edit and answer as `design apply prop=`, but the value is read as data and built against the property's declared type: scalars, vector literals (`vec2f(..)`, `vec3f(..)`, `vec4f(..)` with comma-separated numbers), `theme.*` values, variants of the declared enum, fields of the declared struct, and a draw layer's inputs declared in script (`draw_bg.color`), which take a value of the kind they hold. A field of a struct the source holds as a shorthand (`padding.left` under `padding: 0`) sets the struct whole, from its current rows. It evaluates nothing and calls nothing, so it is the op for edits from outside the app (Town uses it for every peer's edit). Anything else is refused with the reason. |
| `design locate path=P` | File, half-open byte range and hexadecimal FNV-1a hash of the literal. A changed Splash token reports its byte range and both texts. A literal containing a Rust interpolation is unverifiable because the running module retains only its slot. |
| `design theme get [name=N]`, `design theme set name=N value=V`, `design theme install` | Reads theme tokens, sets one token, or requests a rebuild with the installed edits. |
| `design states path=P`, `design pose path=P layer=L state=group.state` | Lists states and tracks by draw layer, or applies one pose to the named layer. |

## Structural edits: the designer

The Build tab's design session edits the Splash source of the file that
declares a widget: insert, move, delete, wrap, rename and set properties.
Each edit is a byte-range hunk inside the `script_mod!` body, previewed
live in the running app. The app never writes the file; the agent applies
the patch. The same session is reachable through `design <op>`, and a person's Build
tab edits and an agent's commands share one undo history.

| Command | Result |
|---|---|
| `design open path=P` | Start a session on the file declaring `P` (default: the pinned widget) |
| `design state` | File, edit count, status, `can_undo`/`can_redo`, `landing`, hunk labels |
| `design palette` | Insertable types with their default bodies |
| `design insert path=P place=before\|after\|inside type=T body=B` | Insert `T` (body defaults to the palette's); answers the new widget's path in `select` |
| `design move path=P to=Q place=...` | Move `P` next to or into `Q` |
| `design delete`, `duplicate`, `wrap`, `up`, `down`, `out` `path=P` | Structural edits of `P` |
| `design rename path=P name=N` | Name, rename or (empty `name`) unname a literal |
| `design set path=P key=K value=V` | Write a property into the literal (`+:` for a new typed property) |
| `design bake` | Write every value-ledger tweak on this file's widgets into the source |
| `design undo`, `redo`, `reset` | Step the source edits |
| `design patch`, `design commit` | The unified diff; `commit` adds `base_hash`, `new_hash`, `base_matches_disk` and the hunks |
| `design verify` | Every drawn widget of the file and whether its literal is found |
| `design close` | End the session; writes `local/design/<file>.patch` |

Edits answer after the frame that shows them, so a `grab` right after sees
the change; send `if_user_seq` like any other mutation. An edit whose
preview the runtime rejects is rolled back and reported in `status`. An op
on a `#(...)` placeholder is refused (it needs a rebuild), and so is an
edit to a widget declared in another file. To persist: read
`design commit`, check `base_matches_disk`, apply `diff` to the file, then
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

`tweak op op=motion` answers the same data as JSON: `open`, and per host
`id`, `name`, `age` and `roots` (`name`, `kind`, `len`, `time`, `paused`,
`linked` (0 once a kept animation completed), `repeat`, `yoyo`, `scale`,
`lanes`, `labels`). Add `host=<id> root=<k>` with one of
`seek=<seconds>`, `pause=1`, `resume=1`, `restart=1`, `scale=<x>`,
`loop=1|0` (repeat forever, or none) or `yoyo=1|0` to
control the k-th top-level animation of that host; the host applies it on
its next event, so read again after a frame. `open=1` publishes without
the tab.

## Persist the result

The overlay never writes source. With a design session open,
`design bake` turns the value tweaks into hunks and `design commit`
returns the patch; without one, at the end of a styling session:

1. Read `tweak final`. If annotations produced a `png`, inspect it; the
   strokes are part of the user's feedback.
2. Resolve each dotted widget path to its `script_mod!` declaration.
   Skip anonymous `-` segments when searching. `ui find path=... json=1`
   reports semantic nodes with their widget paths.
3. Write properties at the most specific existing instance declaration.
   Create an instance override only if no appropriate site exists.
4. Preserve typed-property merging with `+:`. Plain walk/layout properties
   are assigned directly with the correct typed value.
5. Preserve returned source spellings, adding the Rust-macro `#x` color
   escape where required.
6. Rebuild and relaunch. Verify values through reflection or snapshots,
   and inspect a new frame when the visual result matters.
7. Finish your test instance through `gq`, or the cleanup fallback in
   the runbook.

Use these commands directly; this workflow does not require a helper
under `local/`.
