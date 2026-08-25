# DJ tab: scroll-wheel knob control + title-click reset

Date: 2026-08-25
Scope: the vj app's DJ tab (music view) knobs and faders; one opt-in addition to the shared `Slider` widget.

## Goal

Two ergonomic behaviors for the DJ tab:

1. Hovering any knob or fader lets the scroll wheel adjust it.
2. Clicking the title above a knob resets that knob to its default value.

## Background

Every DJ-tab control is the one Rust `Slider` widget under different DSL styles in
`apps/vj/src/music_view.rs`: `MusicKnob` (Rotary; EQ low/mid/high and the four
stem knobs, `default: 1.0`), the FILTER knob (`default: 0.5`), `MusicFader`
(vertical), and `CrossFader`. Titles are separate `KnobLabel` (`MusicLabel`)
widgets stacked above each knob; stem labels already have instance names
(used for paint colors), EQ/FILTER labels do not yet. The platform delivers
`Hit::FingerScroll` to the hovered widget with `scroll: Vec2d` and
`modifiers: KeyModifiers`.

## Design

### 1. Scroll-on-hover (widget: `widgets/src/slider.rs`)

- New `#[live] scroll_step: f64` on `Slider`: the fraction of the value range
  one wheel notch moves. `0.0` (the default) disables the feature entirely, so
  no slider outside the DJ tab changes behavior.
- In `handle_event`, on `Hit::FingerScroll` when `scroll_step > 0.0`:
  - Direction: scroll up increases the value (vertical wheel axis; fall back
    to the horizontal axis when the vertical component is zero, for tilt
    wheels / trackpads over the crossfader).
  - Wheel deltas are normalized to notches (sign plus magnitude scaled by the
    platform's per-notch unit; trackpad `phase`d scrolls accumulate so smooth
    gestures still work).
  - Modifier ladder, per notch, as a fraction of range:
    - Shift: 0.5% (fine)
    - none: 2.5% (standard)
    - Ctrl: 10% (coarse)
    - Ctrl+Shift: 25% (super coarse)
  - `relative_value` moves by `notches * step_fraction`, clamped to [0, 1],
    respecting the existing `step` quantization via the same
    `to_external`/`set_internal` round-trip dragging uses.
  - Emits the same `Slide(value)` then `EndSlide(value)` actions as a drag, so
    app handlers, `bind`, and the text display update through existing paths.
  - Handles `Hit::FingerScroll` the same way the existing chart/browser
    widgets do (no consume flag exists in the Hit API); the DJ tab has no
    enclosing scroll view, so no scroll conflict arises.

### 2. Title-click reset (app: `apps/vj/src/music_view.rs` + `main.rs`)

- The eight knob titles per deck become clickable: HIGH, MID, LOW, FILTER,
  DRUMS, BASS, VOCALS, OTHER. EQ and FILTER labels gain instance names
  (`deck_a_label_eq_high`, …, `deck_a_label_filter`, same for deck B); stem
  labels keep their existing names.
- `KnobLabel` is rebased from `MusicLabel` onto a flat `Button` styled to
  render exactly like today's label (same font, colors, no background or
  border — the same approach the stock `LinkLabel` takes), so it emits
  `clicked` actions and shows the hand cursor on hover.
- On click, `main.rs` resets the paired knob to its DSL default (EQ/stems
  1.0, FILTER 0.5) and applies the value through the same per-knob handler
  that runs when the knob is turned, so the mixer state follows.
- Stem-title click resets the gain only; it does not change kill-button state.
- Controls without titles (crossfader, pitch, gain, volume faders) get no
  click-reset; pitch keeps its existing reset button. Scroll still works on
  all of them.

### 3. DSL changes (DJ tab styles)

- `MusicKnob`, `StemKnob`, `MusicFader`, `CrossFader`: `scroll_step: 0.025`.
- The modifier multipliers (×0.2, ×4, ×10) live in the widget as constants;
  only the base step is styled.

## Error handling

- Scroll with `scroll_step: 0.0` falls through untouched (feature off).
- Clamping keeps scrolled values inside [min, max]; a zero-width range
  (min == max) ignores scroll.
- A title click for a knob id that fails lookup is a no-op (defensive; the
  ids are static).

## Testing

- vj UI test via the remote protocol: hover a stem knob, inject scroll,
  assert the value moved by one standard notch; inject Shift/Ctrl/Ctrl+Shift
  scrolls and assert the ladder; turn a knob then click its title and assert
  the value snapped to default and the mixer saw it.
- Unit test in `slider.rs` for the notch math (ladder fractions, clamping,
  step quantization) if the existing test layout allows.

## Out of scope

- Sliders outside the DJ tab (they keep today's behavior).
- Double-click-on-knob reset, per-knob custom scroll speeds, wheel
  acceleration.
