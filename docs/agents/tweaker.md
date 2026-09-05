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

## Persist the result

The overlay never writes source. At the end of a styling session:

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
