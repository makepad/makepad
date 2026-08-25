# DJ Knob Scroll + Title-Click Reset Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** In the vj app's DJ tab, the scroll wheel adjusts any hovered knob/fader (Shift fine, Ctrl coarse, Ctrl+Shift super coarse), and clicking the title above a knob resets it to its default.

**Architecture:** Scroll lives in the shared `Slider` widget as an opt-in `scroll_step` field (0 = off) that the DJ-tab DSL styles enable; the pure notch/ladder math is a unit-tested helper. Title-click reset is app-side: the `KnobLabel` DSL style is rebased from `Label` onto a flat `Button`, EQ/FILTER labels get instance names, and `main.rs` wires clicks to `SliderRef::reset_to_default` plus the same `decks.set_*` calls the knobs already use.

**Tech Stack:** Rust, Makepad widgets (`script_mod!` DSL — see AGENTS.md for syntax; `Name: value`, `+:` merges), vj app.

**Spec:** `docs/superpowers/specs/2026-08-25-dj-knob-scroll-title-reset-design.md`

## Global Constraints

- Modifier ladder per wheel notch, fraction of range: Shift 0.5%, none 2.5%, Ctrl 10%, Ctrl+Shift 25%. Base `scroll_step: 0.025`; multipliers ×0.2 / ×1 / ×4 / ×10 are widget constants.
- Scroll up = value up. Windows sends wheel-up as `scroll.y = -120` (one notch = 120 units).
- `scroll_step: 0.0` (the default) must leave every existing Slider untouched.
- Defaults come from each slider's DSL `default:` field (EQ/stems 1.0, FILTER 0.5) — never hardcode them in `main.rs`.
- Rustfmt is disabled repo-wide — match surrounding indentation by hand, never run `cargo fmt`.
- The repo builds with `cargo check -p <pkg>` / `cargo build --release -p <pkg>`. The vj package is `makepad-vj`; the widgets package is `makepad-widgets`.

---

### Task 1: Scroll handling in the Slider widget

**Files:**
- Modify: `widgets/src/slider.rs` (struct ~line 1390, `impl Slider` ~line 1489, `handle_event` ~line 1579, `impl SliderRef` ~line 1677)

**Interfaces:**
- Produces: `Slider.scroll_step: f64` live field (DSL-settable); `pub(crate) fn wheel_value_delta(scroll: Vec2d, modifiers: &KeyModifiers, scroll_step: f64) -> f64`; `Slider::reset_to_default(&mut self, cx)`; `SliderRef::reset_to_default(&self, cx) -> Option<f64>` (returns the new external value); scroll emits `SliderAction::Slide(v)` then `SliderAction::EndSlide(v)` exactly like a drag ending.

- [ ] **Step 1: Write the failing unit tests** — at the bottom of `widgets/src/slider.rs`:

```rust
#[cfg(test)]
mod wheel_tests {
    use super::*;

    fn mods(control: bool, shift: bool) -> KeyModifiers {
        KeyModifiers { control, shift, alt: false, logo: false }
    }

    #[test]
    fn wheel_ladder() {
        // One Windows notch is scroll.y = -120 (wheel up) -> value moves UP.
        let up = Vec2d { x: 0.0, y: -120.0 };
        assert!((wheel_value_delta(up, &mods(false, false), 0.025) - 0.025).abs() < 1e-12);
        assert!((wheel_value_delta(up, &mods(false, true), 0.025) - 0.005).abs() < 1e-12);
        assert!((wheel_value_delta(up, &mods(true, false), 0.025) - 0.10).abs() < 1e-12);
        assert!((wheel_value_delta(up, &mods(true, true), 0.025) - 0.25).abs() < 1e-12);
    }

    #[test]
    fn wheel_down_decreases() {
        let down = Vec2d { x: 0.0, y: 120.0 };
        assert!((wheel_value_delta(down, &mods(false, false), 0.025) + 0.025).abs() < 1e-12);
    }

    #[test]
    fn horizontal_axis_fallback() {
        // Tilt wheels / horizontal trackpad gestures land on x when y is 0.
        let tilt = Vec2d { x: -120.0, y: 0.0 };
        assert!((wheel_value_delta(tilt, &mods(false, false), 0.025) - 0.025).abs() < 1e-12);
    }

    #[test]
    fn zero_step_disables() {
        let up = Vec2d { x: 0.0, y: -120.0 };
        assert_eq!(wheel_value_delta(up, &mods(true, true), 0.0), 0.0);
    }
}
```

If `KeyModifiers` has more fields than `{shift, control, alt, logo}`, extend `mods()` with `..Default::default()` instead of listing them.

- [ ] **Step 2: Run tests, verify they fail**

Run: `cargo test -p makepad-widgets wheel_` — expected: compile error, `wheel_value_delta` not found.

- [ ] **Step 3: Implement the helper, the field, and the event handling**

Add to the `Slider` live struct (next to `default: f64`, ~line 1390):

```rust
    /// Fraction of the value range one scroll-wheel notch moves while the
    /// pointer hovers this slider. 0.0 (the default) disables wheel input.
    #[live]
    scroll_step: f64,
```

Add near the top of the file (module level, after the imports):

```rust
/// Value delta for one scroll event: notch count (Windows wheels send 120
/// units per notch; trackpads send smaller deltas that accumulate over the
/// gesture) times the step fraction, scaled by the modifier ladder —
/// Shift fine (x0.2), plain (x1), Ctrl coarse (x4), Ctrl+Shift (x10).
/// Scroll up (negative y) raises the value; a zero step disables wheel input.
pub(crate) fn wheel_value_delta(
    scroll: Vec2d,
    modifiers: &KeyModifiers,
    scroll_step: f64,
) -> f64 {
    if scroll_step == 0.0 {
        return 0.0;
    }
    let axis = if scroll.y != 0.0 { -scroll.y } else { -scroll.x };
    let ladder = match (modifiers.control, modifiers.shift) {
        (true, true) => 10.0,
        (true, false) => 4.0,
        (false, true) => 0.2,
        (false, false) => 1.0,
    };
    (axis / 120.0) * scroll_step * ladder
}
```

Add to `impl Slider` (next to `set_value`, ~line 1493):

```rust
    /// Snap back to the DSL `default:` value, as a title-click reset does.
    pub fn reset_to_default(&mut self, cx: &mut Cx) {
        self.set_internal(self.default);
        self.update_text_input(cx);
        self.draw_bg.redraw(cx);
    }
```

Add to `impl SliderRef` (next to its `set_value`, ~line 1686):

```rust
    /// Reset to the DSL default and return the value now in effect, so the
    /// caller can push it into whatever the slider is bound to.
    pub fn reset_to_default(&self, cx: &mut Cx) -> Option<f64> {
        let mut inner = self.borrow_mut()?;
        inner.reset_to_default(cx);
        Some(inner.to_external())
    }
```

(`borrow_mut` returns `Option`; if the `?` form doesn't compile in this codebase's Ref idiom, use `if let Some(mut inner) = self.borrow_mut() { ... }` returning the value.)

In `handle_event`, inside `match event.hits(cx, self.draw_bg.area())` (~line 1579), add an arm after `Hit::FingerHoverOver`:

```rust
            Hit::FingerScroll(e) => {
                if self.scroll_step > 0.0 && !self.animator_in_state(cx, ids!(disabled.on)) {
                    let delta = wheel_value_delta(e.scroll, &e.modifiers, self.scroll_step);
                    if delta != 0.0 && self.dragging.is_none() {
                        self.relative_value = (self.relative_value + delta).max(0.0).min(1.0);
                        self.set_internal(self.to_external());
                        self.draw_bg.redraw(cx);
                        self.update_text_input(cx);
                        cx.widget_action(uid, SliderAction::Slide(self.to_external()));
                        cx.widget_action(uid, SliderAction::EndSlide(self.to_external()));
                    }
                }
            }
```

The `set_internal(self.to_external())` round-trip is the same quantization dragging uses (`step:` respected). Guarding on `dragging.is_none()` keeps a wheel event during an active drag from fighting the finger.

- [ ] **Step 4: Run the tests, verify they pass**

Run: `cargo test -p makepad-widgets wheel_` — expected: 4 passed.
Then: `cargo check -p makepad-widgets` — expected: clean (warnings at parity with before).

- [ ] **Step 5: Commit**

```bash
git add widgets/src/slider.rs
git commit -m "widgets: Slider takes the scroll wheel — opt-in scroll_step, modifier ladder, DSL-default reset helper"
```

---

### Task 2: Enable scroll on every DJ-tab control

**Files:**
- Modify: `apps/vj/src/music_view.rs` — the `MusicKnob` (~line 464), `MusicFader` (~line 526), `CrossFader` (~line 563) style blocks

**Interfaces:**
- Consumes: `scroll_step` live field from Task 1.
- Produces: every DJ-tab knob (EQ ×3, FILTER, stems ×4 per deck), the pitch and gain faders, and the crossfader respond to the wheel; nothing else in the app does.

- [ ] **Step 1: Add `scroll_step: 0.025` to the three styles**

In `MusicKnob` (covers EQ, FILTER, and `StemKnob` which inherits it), directly under `default: 1.0`:

```text
        scroll_step: 0.025
```

In `MusicFader` (covers `deck_x_pitch`, `deck_x_gain`), directly under `axis: DragAxis.Vertical`:

```text
        scroll_step: 0.025
```

In `CrossFader`, directly under `max: 1.0`:

```text
        scroll_step: 0.025
```

- [ ] **Step 2: Build**

Run: `cargo build --release -p makepad-vj` — expected: clean build.

- [ ] **Step 3: Smoke-test scroll via the remote protocol**

```bash
./target/release/makepad-vj --remote > /tmp/vj.log 2>&1 &
sleep 8 && PORT=$(grep -o 'listening on 127.0.0.1:[0-9]*' /tmp/vj.log | grep -o '[0-9]*$')
```

Open the DJ tab in the app window (or locate it via `curl http://127.0.0.1:$PORT/snap`), find a stem knob's rect in the snap JSON (widget id `deck_a_stem_drums`), then scroll one notch up over its center and read the value back:

```bash
curl -s "http://127.0.0.1:$PORT/snap" | jq '..|objects|select(.id?=="deck_a_stem_drums")'
curl -s -X POST "http://127.0.0.1:$PORT/m" -d '{"action":"scroll","x":CX,"y":CY,"dy":-120}'
curl -s "http://127.0.0.1:$PORT/snap" | jq '..|objects|select(.id?=="deck_a_stem_drums")'
```

Expected: the knob's value rises by 0.05 (2.5% of the 0–2 range). Repeat with `"mods":"shift"` / `"ctrl"` / `"ctrl shift"` (check `platform/src/remote.rs` `Params::mods` for the exact accepted spelling) and confirm 0.01 / 0.20 / 0.50 moves. Finish with `curl http://127.0.0.1:$PORT/gq`.

If the snap JSON does not expose a slider value, verify by eye in the window instead and note it in the commit message.

- [ ] **Step 4: Commit**

```bash
git add apps/vj/src/music_view.rs
git commit -m "vj: DJ-tab knobs and faders ride the scroll wheel"
```

---

### Task 3: Clickable knob titles

**Files:**
- Modify: `apps/vj/src/music_view.rs` — `KnobLabel` (~line 507), the EQ/FILTER label instances (~lines 827–844 deck A, ~928–944 deck B)
- Modify: `apps/vj/src/main.rs` — `MusicDeckRefs` (~line 2530), its constructor (~line 2565), `MusicDeckIds` (~line 2650) and both deck id tables (~lines 2701, 2754)

**Interfaces:**
- Consumes: nothing new.
- Produces: `KnobLabel` is a flat `Button` (emits `clicked`, hand cursor); new ids `deck_a_label_eq_low/mid/high`, `deck_a_label_filter` (same for deck B); `MusicDeckIds` gains `eq_labels: [&'static [LiveId]; 3]` (ordered low, mid, high — matching `eq_knobs`) and `filter_label: &'static [LiveId]`; `MusicDeckRefs` gains `eq_labels: Vec<ButtonRef>`, `filter_label: ButtonRef`, and `stem_labels` becomes `Vec<ButtonRef>`.

- [ ] **Step 1: Rebase `KnobLabel` onto a flat Button**

Replace the `KnobLabel` definition (keep the comment above it):

```text
    // A knob's legend: fills its stack, never widens it, never wraps.
    // A flat Button rather than a Label so a click on it resets its knob.
    let KnobLabel = Button{
        width: Fill
        height: Fit
        padding: Padding{left: 0 right: 0 top: 0 bottom: 0}
        margin: Margin{left: 0 right: 0 top: 0 bottom: 0}
        align: Align{x: 0.5, y: 0.0}
        draw_bg +: {
            color: #x00000000
            color_focus: #x00000000
            color_hover: #x00000000
            color_down: #x00000000
            border_size: 0.0
            border_radius: 0.0
        }
        draw_text +: {
            color: #xa6b1bd
            color_focus: #xa6b1bd
            color_hover: #xd6dee6
            color_down: #x8e9aa7
            text_style: theme.font_bold{font_size: 7}
        }
    }
```

Match the stock `Button`'s actual draw_bg/draw_text uniform names — check `widgets/src/button.rs`'s script block and the `MusicButton` style above (~line 415) for the exact set; drop any that don't exist. The visual target: identical to today's label at rest, slightly lighter on hover, hand cursor.

- [ ] **Step 2: Name the EQ and FILTER labels**

Deck A (~lines 827–844): `KnobLabel{text: "HIGH"}` → `deck_a_label_eq_high := KnobLabel{text: "HIGH"}`, same for `MID` → `deck_a_label_eq_mid`, `LOW` → `deck_a_label_eq_low`, `FILTER` → `deck_a_label_filter`. Deck B likewise with the `deck_b_` prefix (~lines 928–944; note deck B lists FILTER first, then LOW/MID/HIGH — names follow the text, not the position).

- [ ] **Step 3: Extend the id tables and refs in `main.rs`**

In `MusicDeckIds` (~line 2650) add, next to `stem_labels`:

```rust
    eq_labels: [&'static [LiveId]; 3],
    filter_label: &'static [LiveId],
```

In both deck tables (deck A ~line 2701, deck B ~line 2754), ordered like `eq_knobs` (low, mid, high):

```rust
                eq_labels: [
                    ids!(deck_a_label_eq_low),
                    ids!(deck_a_label_eq_mid),
                    ids!(deck_a_label_eq_high),
                ],
                filter_label: ids!(deck_a_label_filter),
```

(and the `deck_b_` versions). In `MusicDeckRefs` (~line 2530) change `stem_labels: Vec<LabelRef>` to `Vec<ButtonRef>` and add `eq_labels: Vec<ButtonRef>`, `filter_label: ButtonRef`; in the constructor (~line 2565) build them with `ui.button(cx, p)` instead of `ui.label(cx, p)`:

```rust
            stem_labels: ids.stem_labels.iter().map(|p| ui.button(cx, p)).collect(),
            eq_labels: ids.eq_labels.iter().map(|p| ui.button(cx, p)).collect(),
            filter_label: ui.button(cx, ids.filter_label),
```

`paint_stem_knob` (~line 14798) needs no change — it colors the legend through a generic `self.ui.widget(...)` + `draw_text` apply, which works on a Button.

- [ ] **Step 4: Build and check the tab still renders**

Run: `cargo build --release -p makepad-vj`, launch, confirm the DJ tab looks unchanged (labels same size/color, stem labels still take their stem colors) and label hover shows the hand cursor. If a label's layout shifted (Button padding), adjust the `padding`/`margin` zeros until the stack matches the old spacing.

- [ ] **Step 5: Commit**

```bash
git add apps/vj/src/music_view.rs apps/vj/src/main.rs
git commit -m "vj: knob titles are flat buttons with names — clickable, still painted like labels"
```

---

### Task 4: Title click resets the knob

**Files:**
- Modify: `apps/vj/src/main.rs` — the deck action handler (~line 16276, next to the existing `filter`/`eq_knobs`/`stem_knobs` blocks)

**Interfaces:**
- Consumes: `SliderRef::reset_to_default` (Task 1), `eq_labels`/`filter_label`/`stem_labels` refs (Task 3), existing `self.decks.set_eq / set_filter / set_stem` + `run_deck_cmds`.

- [ ] **Step 1: Wire the clicks**

In the handler that already contains `if let Some(value) = refs.filter.slided(actions)` (~line 16276), add alongside the matching blocks:

```rust
            if refs.filter_label.clicked(actions) {
                if let Some(value) = refs.filter.reset_to_default(cx) {
                    let cmds = self.decks.set_filter(deck, value as f32);
                    self.run_deck_cmds(cx, cmds);
                }
            }
            for (band, label) in refs.eq_labels.iter().enumerate() {
                if label.clicked(actions) {
                    if let Some(value) = refs.eq_knobs[band].reset_to_default(cx) {
                        let cmds = self.decks.set_eq(deck, band, value as f32);
                        self.run_deck_cmds(cx, cmds);
                    }
                }
            }
            for (stem, label) in refs.stem_labels.iter().enumerate() {
                if label.clicked(actions) {
                    if let Some(value) = refs.stem_knobs[stem].reset_to_default(cx) {
                        let cmds = self.decks.set_stem(deck, stem, value as f32);
                        self.run_deck_cmds(cx, cmds);
                    }
                }
            }
```

Ordering caveat: `eq_labels` was defined low/mid/high to match `eq_knobs` (Task 3) and `stem_labels` already matches `stem_knobs` (vocals, drums, bass, other) — both arrays index-pair label→knob 1:1; do not reorder either side.

- [ ] **Step 2: Build**

Run: `cargo build --release -p makepad-vj` — expected: clean.

- [ ] **Step 3: Verify end to end via the remote protocol**

Launch with `--remote` as in Task 2. Scroll `deck_a_stem_drums` a few notches away from 1.0, then click the center of `deck_a_label_drums` (rect from `/snap`):

```bash
curl -s -X POST "http://127.0.0.1:$PORT/m" -d '{"action":"click","x":LX,"y":LY}'
```

Expected: the knob snaps back to 1.0 (value in `/snap`, pointer at 12 o'clock-ish in a screenshot via `/g`). Repeat for `deck_a_label_filter` (default 0.5) after dragging it aside. Confirm audio follows if a track is playing (stem gain audibly returns). Finish with `/gq`.

- [ ] **Step 4: Commit**

```bash
git add apps/vj/src/main.rs
git commit -m "vj: click a knob title to snap the knob home"
```

---

### Task 5: Align the spec with the implemented scroll-consumption behavior

**Files:**
- Modify: `docs/superpowers/specs/2026-08-25-dj-knob-scroll-title-reset-design.md`

The spec says the widget "consumes the event (the containing view must not also scroll)". The codebase's established `Hit::FingerScroll` consumers (chart, browser, portal_list) handle without a consume flag, and the DJ tab has no enclosing scroll view — so Task 1 matched that pattern instead.

- [ ] **Step 1: Replace that bullet** with:

```text
  - Handles `Hit::FingerScroll` the same way the existing chart/browser
    widgets do (no consume flag exists in the Hit API); the DJ tab has no
    enclosing scroll view, so no scroll conflict arises.
```

- [ ] **Step 2: Commit**

```bash
git add docs/superpowers/specs/2026-08-25-dj-knob-scroll-title-reset-design.md
git commit -m "vj: spec follows the widget's real scroll-consumption pattern"
```
