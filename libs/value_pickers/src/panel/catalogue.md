# Editor panel widgets

The panel is the body of `let editor_panel = fn() { View{ … } }`: start it with
`flow: Down width: Fill spacing: 10`, then the widgets below, one record each.
Every widget binds itself to a path in the scene's code (`path: "WORD_SPARKS.count"`,
`path: "LYRIC_LOOK.font"`, `path: "camera_keys[3].zoom"`): it shows what is written
there and sets it as it moves (live while dragged, one undo step on release).
Only `path` is needed; the rest has defaults taken from the value (its range, its
unit, its kind). Units stay as written (`0.12s`, `90deg`, `2.2rad`, `12px`).
Optional on every widget: `label: "Glow"` (else the path's last part),
`on_change: |v| { … }` (after each change: drive other paths with `scene.set`),
`on_commit: |v| { … }` (when a gesture ends). Backspace over a control (or its menu's
Reset) puts back the value the panel opened with.
`when: "beats 9-16"` on a module (or a control) says when in the film it acts: a beat
range, a sung word (`"the lyric 'AGI'"`), or seconds (`"2.4s-5s"`). The module shows it
under its title, the timeline marks it, and the panel dims what is not acting at the
playhead. Give every module a `when` unless it acts throughout.

## Layout
- `PanelModule{title: "Sparks" tint: "#ff8a3d" when: "beats 9-16" bypass: "SPARKS.on" …widgets}`: a rack
  unit. Holds any widgets; its heading's "···" has snapshots 1-4 (click an empty one to
  store, a stored one to recall, shift-click to store again), a morph between snapshots
  1 and 2, Randomise (within each control's range; colours kept unless switched), Reset,
  and an on/off switch when `bypass` names a bool. Where the host takes asks,
  an AI button offers "something different", "reinvent", "randomise creatively" and five variants
  of what the module's controls hold (the scene's agent rewrites them; one undo step).
  A module is folded to its one-line heading until it is opened (a click on the heading, or the
  picture selecting what it holds); `open: true` opens it from the start (the first, "Overview",
  module). Its snapshots, morph, randomise, reset and AI are under the heading's "···".
- `PanelGroup{title: "Timing" …widgets}`: a sub-group of a module in a rounded inset; every group
  starts folded each time its module opens (the person opens the one they want).
- `PanelEffects{path: "Motion.layers[2].effects" title: "Glow"}`: an effects stack attached to a
  node (a layer's `effects`, a Group's `post`): bypass, key parameters, reorder, add, remove. The
  scene's post stack is in the structure column already: never bind `Motion.post` in a module.
- `PanelHeader{}`: first in the panel: a search over the controls and an index of the modules.
- `PanelRow{…widgets}`: widgets side by side (knobs, faders, dials).
- `PanelMood{}` inside a module: a pad between the module's four slots (corners 1-4).

## Generators first
Most looks come from the library's generators (palette harmonies, arpeggios, euclidean rhythms,
spark patterns, camera moves, layouts, noise fields), which run at once. The usual control is a
knob or a `PanelSeed` on a generator call's arguments, each tagged `/**name*/` in the code:
`PanelSeed{path: "/**spark seed*/"}`, `PanelKnob{path: "/**pulses*/" min: 1 max: 16}`.
Write a new seeded, parameterised generator into the shared module only when none fits.

## Numbers
Every number is a value field: its label and value inside one compact field; drag sideways
anywhere in it (Shift fine, Ctrl snaps to round steps), click to type (an expression `3.5*2`,
`+0.5` relative, units `200ms` `90deg`), right click for its menu. `min`/`max` are a SOFT
range: they set the slider's fill and the drag's speed, and the range grows whenever a value
goes past them, so give the range the value usually lives in, not a wall. Only where the
value's domain demands it give a hard limit: `hard_min: 0` (a radius, a size, a count, a
duration), `hard_max: 1` (an opacity, a mix). `step: 0.05` only when the value moves in steps.
- `PanelSlider{path: "WORD_SPARKS.lift"}`: a field with a fill bar, `min: 0 max: 2` when wanted.
- `PanelNumber{path: "WORD_SPARKS.window"}`: a field without the fill, ‹ › steps on hover; for
  values without a natural range (positions, offsets, times).
- `PanelKnob{path: "WORD_SPARKS.count" min: 0 max: 64}`: compact, label under it; put several in a `PanelRow{…}`.
  Its readout is a field (type into it).
- `PanelFader{path: "PEN_SPARKS.intensity"}`: a vertical fader, for a row of levels in a `PanelRow{…}`.
- `PanelAngle{path: "WORD_SPARKS.up"}`: a dial (deg or rad as written), compact like a knob.
- `PanelRange{path: "CAMERA.zoom_range"}`: a two-number value (`[a, b]`): two fields, from and to.
- `PanelXY{path: "EYE" min: -6 max: 6}`: a pad for a two-number value (`vec2`, `pt`, `sheet(x, y)`), with its x and y fields.
- `PanelVec3{path: "LIGHT.dir"}`: a vector (`vec2`, `vec3`) as a row of fields X Y (Z); drag up or
  down onto the row, or hold Alt, to move every part together.
- `PanelSteps{path: "HITS.punch_beats" min: 0 max: 24}`: a list of numbers as bars; drag across to draw.
- `PanelPoints{path: "SHAPE.pts" joined: true}`: a list of x, y pairs on a map; drag a point.
- `PanelMacro{label: "Energy" targets: "WORD_SPARKS.lift 0.2 1.2, PEN_SPARKS.speed 100 400"}`:
  one knob driving several paths, each from its first number (at 0) to its second (at 1).
- `PanelSeed{path: "PEN_SPARKS.seed"}`: a number and its Shuffle.
- `PanelEnvelope{attack: "FLASH.attack" decay: "FLASH.decay" sustain: "FLASH.level" release: "FLASH.fade"}`:
  four paths as one shape over the song (`at: 12.5` places it); drag its corners.
- `PanelMeter{path: "SPARKS.energy"}`: what the film holds at a path as it plays (a bar and its last
  seconds); knobs and sliders also show a modulated value's reach as a ring.

## Scenes (an edit's panel)
- `PanelScene{node: "p_open"}`: a scene of the film (a clip of the edit): its picture, name and time in the
  film with the playhead; a click opens its own panel. A section's module starts with its scenes.

## Time and motion
- `PanelAnchor{path: "TITLE.at"}`: a time on the song: click a beat or a sung word to put it
  there (written as `beat(n)`, `W.word.start` or seconds, as the value was).
- `PanelKeys{path: "CAMERA.keys"}`: keys over time (`keys([t, v, …], …)`, `[[t, v], …]`); drag a
  key in time and value.
- `PanelMod{path: "WORD_SPARKS.lift"}`: modulation on a path: a source (lfo, beat, word, noise,
  time), its rate and depth; written as `modulate(base, @lfo, {rate: 0.5 depth: 0.2})`. A knob
  on the same path keeps editing the base and shows the modulation as a ring. Only on a plain
  number in a record the drawing reads each frame (`WORD_SPARKS.lift`), never on a value a
  function computes while the scene is built: that one stays at its base.

## Choices
- `PanelToggle{path: "GRID.on"}`
- `PanelChoice{path: "PEN_MOTION.travel_ease" choices: "linear ease_in_out ease_out_back"}`
  (a name stays a name `@…`, a string a string).
- `PanelEase{path: "PAPER.reveal_ease"}`: the eases, each drawn.
- `PanelCurve{path: "CARD.ease"}`: a `cubic(x1, y1, x2, y2)` with its two handles and presets.
- `PanelFn{path: "CREATURE.builder"}`: the functions with the same parameters, to swap in.

## Colour and type
- `PanelColor{path: "BONE"}`: a row that opens the colour picker (HDR glow where the
  value is a `color(r, g, b)`).
- `PanelGradient{path: "SKY.stops"}`: a list of colours; a stop opens the picker; + and −.
- `PanelPalette{}` (the scene's palette; `scope: "song"` a file's, or a tag's): swatches
  with how many places use each; one edited, or all moved: harmony rules from the chosen
  colour, warmth / hue / contrast / chroma / ramp / glow (drag the amount), random
  (keeping lightness, the accent, the paper), shuffled roles, the frame's colours, saved
  palettes, A/B mix.
- `PanelFont{path: "SANS"}`: a row that opens the font picker (families in their own
  face, a variable font's axes, a static family's faces). `sample: "I see"` sets the words.
- `PanelText{path: "TITLE.text"}`, `PanelWords{path: "LYRIC.lines"}` (several lines).

## A whole panel
```
flow: Down width: Fill spacing: 10
PanelModule{title: "Sparks" tint: "#ff8a3d"
    PanelRow{
        PanelKnob{path: "WORD_SPARKS.count" min: 0 max: 64}
        PanelKnob{path: "WORD_SPARKS.lift"}
        PanelAngle{path: "WORD_SPARKS.up"}
    }
    PanelRow{
        PanelFader{path: "PEN_SPARKS.intensity" label: "Level"}
        PanelFader{path: "PEN_SPARKS.speed"}
    }
    PanelSeed{path: "PEN_SPARKS.seed"}
    PanelMood{}
}
PanelModule{title: "Lyrics"
    PanelFont{path: "SANS"}
    PanelColor{path: "BONE"}
    PanelEase{path: "PAPER.reveal_ease"}
}
PanelGroup{title: "Palette" PanelPalette{}}
```
