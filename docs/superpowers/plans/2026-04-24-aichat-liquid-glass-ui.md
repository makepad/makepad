# AI Chat Liquid Glass UI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the `AI Chat Liquid Glass UI` spec for `makepad-example-aichat`: dark emerald glass shell, unified toolbar, polished sidebar, readable output cards, horizontal block scrolling, integrated composer, and runtime opacity control.

**Architecture:** Keep the app as a Makepad-native UI in `examples/aichat/src/main.rs`; do not introduce a web/CSS layer or a new UI framework. Reuse the existing `GlassPanel`, `DropDown`, `SliderMinimalFlat`, `ScrollXView`, Markdown, CodeView, and DiagramView paths. Add contract-style tests first where feasible, then converge the DSL and Rust state glue around stable ids so visual behavior remains inspectable.

**Tech Stack:** Rust, Makepad 2.0 `script_mod!`, `makepad-widgets`, `GlassPanel` SDF shader, Markdown/CodeView/DiagramView widgets, `cargo test`, `cargo check`, manual GUI run.

---

## File Map

- `examples/aichat/src/main.rs`: primary implementation surface. Owns the app DSL, theme/font overrides, layout tree, opacity runtime updates, backend selector wiring, composer actions, and contract tests.
- `examples/aichat/specs/aichat-liquid-glass-ui.spec.md`: source contract for this plan. Do not weaken it during implementation.
- `examples/aichat/README.md`: optional user-facing note for the new UI and opacity control if the example README exists or is created.
- `widgets/src/markdown.rs`: inspect only unless horizontal code/diagram block dispatch requires a widget-level fix.
- `widgets/src/glass_panel.rs`: inspect only; avoid changing the global `GlassPanel` shader unless app-local styling cannot satisfy the target.

## Implementation Notes

- Use `makepad:makepad-2.0-design-judgment` first, then co-load `makepad-2.0-layout`, `makepad-2.0-widgets`, `makepad-2.0-theme`, `makepad-2.0-events`, and `makepad-2.0-shaders` as needed.
- Preserve user-visible behavior already working: backend selection, send/cancel/clear, Enter-to-send, Escape-to-cancel, Markdown rendering, code blocks, diagram blocks, and chat persistence.
- Treat Makepad layout as a turtle layout. Every `View`/`GlassPanel` in a `Fit` chain needs explicit `height: Fit`; scroll containers need known `Fill` parents.
- Keep ids stable where Rust uses them: `main_window`, `app_shell`, `sidebar`, `main_area`, `top_bar`, `backend_dropdown`, `opacity_slider`, `opacity_value`, `chat_list`, `composer`, `input`, `cancel_button`, `clear_button`, `send_button`, `status_label`, `empty_state`.
- Default opacity should be `0.90` unless visual testing shows `0.88` reads better. The supported runtime range should be tightened to approximately `0.72..0.98`; below that the target wallpaper readability breaks.

### Task 1: Add Contract Tests For Stable UI State

**Files:**
- Modify: `examples/aichat/src/main.rs`

- [ ] **Step 1: Add test helpers for constants and opacity mapping**

Add a small pure helper near `DEFAULT_GLASS_OPACITY` so opacity behavior can be tested without launching a window:

```rust
const DEFAULT_GLASS_OPACITY: f64 = 0.90;
const MIN_GLASS_OPACITY: f64 = 0.72;
const MAX_GLASS_OPACITY: f64 = 0.98;

#[derive(Debug, Clone, Copy, PartialEq)]
struct GlassOpacity {
    app: f32,
    sidebar: f32,
    main: f32,
    composer: f32,
}

fn glass_opacity_values(opacity: f64) -> GlassOpacity {
    let opacity = opacity.clamp(MIN_GLASS_OPACITY, MAX_GLASS_OPACITY);
    GlassOpacity {
        app: opacity as f32,
        sidebar: (opacity + 0.06).min(0.98) as f32,
        main: (opacity + 0.04).min(0.97) as f32,
        composer: (opacity + 0.03).min(0.98) as f32,
    }
}
```

- [ ] **Step 2: Write failing tests**

Add tests in `examples/aichat/src/main.rs`:

```rust
#[cfg(test)]
mod liquid_glass_contract_tests {
    use super::*;

    #[test]
    fn aichat_glass_opacity_slider_contract() {
        assert!((DEFAULT_GLASS_OPACITY - 0.90).abs() < f64::EPSILON);
        let values = glass_opacity_values(DEFAULT_GLASS_OPACITY);
        assert!((0.85..=0.92).contains(&(values.app as f64)));
        assert!(values.sidebar >= values.app);
        assert!(values.main >= values.app);
        assert!(values.composer >= values.app);
    }

    #[test]
    fn aichat_liquid_glass_shell_contract() {
        let low = glass_opacity_values(0.0);
        let high = glass_opacity_values(2.0);
        assert!((low.app - MIN_GLASS_OPACITY as f32).abs() < f32::EPSILON);
        assert!((high.app - MAX_GLASS_OPACITY as f32).abs() < f32::EPSILON);
    }
}
```

- [ ] **Step 3: Run tests to verify current gap**

Run:

```bash
cargo test -p makepad-example-aichat aichat_glass_opacity_slider_contract -- --nocapture
cargo test -p makepad-example-aichat aichat_liquid_glass_shell_contract -- --nocapture
```

Expected: FAIL until constants/helper are added or updated.

- [ ] **Step 4: Wire `apply_glass_opacity` through the helper**

Replace inline opacity math in `apply_glass_opacity` with:

```rust
let opacity = opacity.clamp(MIN_GLASS_OPACITY, MAX_GLASS_OPACITY);
let values = glass_opacity_values(opacity);
```

Then apply `values.app`, `values.sidebar`, `values.main`, and `values.composer` to the existing ids.

- [ ] **Step 5: Re-run focused tests**

Run:

```bash
cargo test -p makepad-example-aichat aichat_glass_opacity_slider_contract -- --nocapture
cargo test -p makepad-example-aichat aichat_liquid_glass_shell_contract -- --nocapture
```

Expected: PASS.

### Task 2: Consolidate Liquid-Glass Design Tokens And Base Components

**Files:**
- Modify: `examples/aichat/src/main.rs`

- [ ] **Step 1: Introduce app-local color/style aliases**

Inside `script_mod!`, replace scattered hardcoded color choices with a compact token block near the existing gradients:

```rust
let ai_ink = #x06130F
let ai_panel = #x06251D
let ai_panel_deep = #x031510
let ai_cream = #xF3E3C7
let ai_cream_dim = #xCDBF9FAA
let ai_cyan = #x72E4FF
let ai_cyan_soft = #x72E4FF55
let ai_gold = #xF6BE63
let ai_gold_soft = #xF6BE6388
```

Expected: no functional change yet.

- [ ] **Step 2: Add reusable app-local control templates**

Add templates in `script_mod!` after `ChatSceneVector`:

```rust
let ToolbarLabel = Label {
    draw_text.color: ai_cream_dim
    draw_text.text_style.font_size: 11
}

let PillButton = ButtonFlat {
    height: 34
    padding: Inset{left: 14 right: 14 top: 0 bottom: 0}
    draw_text +: {
        color: ai_cream
        text_style +: { font_size: 11 }
    }
    draw_bg +: {
        color: #x0B241CCC
        color_hover: #x12362ADD
        border_color: #x72E4FF30
        border_size: 1.0
        border_radius: 17.0
    }
}

let IconButton = ButtonFlat {
    width: 36
    height: 36
    padding: 0
    draw_text +: {
        color: ai_cream
        text_style +: { font_size: 15 }
    }
    draw_bg +: {
        color: #x0B241CAA
        color_hover: #x174335DD
        border_color: #x72E4FF26
        border_size: 1.0
        border_radius: 18.0
    }
}
```

- [ ] **Step 3: Run compile check**

Run:

```bash
cargo check -p makepad-example-aichat
```

Expected: PASS. Fix DSL syntax errors before continuing.

### Task 3: Polish Shell, Sidebar, And Top Toolbar

**Files:**
- Modify: `examples/aichat/src/main.rs`

- [ ] **Step 1: Update shell opacity and border styling**

In `app_shell`, set:

```rust
draw_bg +: {
    tint_color: #x06251D
    tint_alpha: 0.90
    border_color: ai_cyan
    border_alpha: 0.58
    border_width: 1.2
    corner_radius: 30.0
    specular_strength: 0.30
    noise_strength: 0.010
    use_scene_blur: 1.0
    blur_amount: 0.10
}
```

Expected visual result: rounded emerald shell, visible cyan edging, no extra inner neon rectangles.

- [ ] **Step 2: Rebuild sidebar as a target nav rail**

Keep `sidebar` fixed width around `300`. Update header to include gear mark plus title:

```rust
sidebar_header := View {
    width: Fill
    height: Fit
    flow: Down
    spacing: 8
    margin: Inset{top: 4 bottom: 18}
    View {
        width: Fill
        height: Fit
        flow: Right
        spacing: 10
        Label { text: "⚙" ... }
        Label { text: "AI Chat" ... }
    }
    Label { text: "Diagram workspace" ... }
}
```

Convert `nav_new` to the active capsule using cyan fill/glow-like border:

```rust
draw_bg +: {
    color: #x0B6B67AA
    color_hover: #x108E88CC
    border_color: #x72E4FF66
    border_size: 1.0
    border_radius: 10.0
}
```

Add a trailing sparkle in the text if a separate right marker is too expensive:

```rust
text: "▣  新对话                         ✦"
```

- [ ] **Step 3: Convert top controls into a unified toolbar**

Keep `top_bar` as `flow: Right`, but normalize heights:

```rust
top_bar := View {
    width: Fill
    height: 40
    flow: Right
    align: Align{y: 0.5}
}
```

Backend selector: height `34`, width `168`, border radius `17`, cyan border low alpha, dark emerald fill.

Glass slider: width `170`, height `26`, value color `ai_gold`, handle `ai_gold`, inactive rail `#x72E4FF55`.

Opacity label: fixed width `40`, aligned with slider:

```rust
opacity_value := Label {
    width: 40
    text: "90%"
    draw_text.color: ai_cream_dim
    draw_text.text_style.font_size: 11
}
```

- [ ] **Step 4: Re-run compile**

Run:

```bash
cargo check -p makepad-example-aichat
```

Expected: PASS.

### Task 4: Make Output Cards Readable And Horizontally Scrollable

**Files:**
- Modify: `examples/aichat/src/main.rs`
- Inspect: `widgets/src/markdown.rs`

- [ ] **Step 1: Tighten chat card surfaces**

In `ChatList.User` and `ChatList.Assistant`, increase surface opacity and border contrast:

```rust
draw_bg +: {
    color: #x0B2A22E6
    radius: 12.0
}
```

For code blocks, keep a deeper card:

```rust
editor +: {
    height: Fit
    draw_bg +: { color: #x031510EE }
}
```

Expected: text remains readable over detailed wallpaper.

- [ ] **Step 2: Ensure code blocks and diagram blocks scroll horizontally**

Wrap both `code_block` and `diagram_block` in `ScrollXView` for User and Assistant templates:

```rust
code_block := ScrollXView {
    width: Fill
    height: Fit
    flow: Right
    code_view := CodeView {
        width: Fit
        keep_cursor_at_end: true
        editor +: { height: Fit }
    }
}
```

Keep `diagram_block` with inner id `diagram_view`:

```rust
diagram_block := ScrollXView {
    width: Fill
    height: Fit
    flow: Right
    diagram_view := DiagramView {
        width: Fit
        height: Fit
    }
}
```

- [ ] **Step 3: Run compile**

Run:

```bash
cargo check -p makepad-example-aichat
```

Expected: PASS.

- [ ] **Step 4: Manual content test**

Run the app and send/paste a wide block:

```text
请输出一个很宽的 state diagram，并且给出一段很长的 Rust 代码块，要求代码行长度超过 180 字符。
```

Expected: wide code/diagram scroll inside their cards; window does not need to be widened to inspect the full content.

### Task 5: Rebuild Composer As One Integrated Panel

**Files:**
- Modify: `examples/aichat/src/main.rs`

- [ ] **Step 1: Resize composer to target proportions**

Change composer width from `Fill{min: 520 max: 880}` to a main-column proportional cap:

```rust
composer := GlassPanel {
    width: Fill{min: 620 max: 1040}
    height: Fit
    padding: Inset{left: 18 top: 14 right: 14 bottom: 12}
    ...
}
```

Expected: composer reads as the main input surface, not a small floating bubble.

- [ ] **Step 2: Convert quick actions to real icon buttons**

Replace the loose `Label` quick actions with `IconButton` instances:

```rust
attach_button := IconButton { text: "+" }
mention_button := IconButton { text: "@" }
tools_button := IconButton { text: "▣" }
```

Keep the permission/status label low contrast:

```rust
Label {
    text: "默认权限"
    draw_text.color: ai_cream_dim
    draw_text.text_style.font_size: 11
}
```

- [ ] **Step 3: Normalize right action group**

Update `clear_button` to use `PillButton` and `send_button` to use warm gold:

```rust
clear_button := PillButton {
    text: "Clear"
    width: 72
}

send_button := ButtonFlat {
    text: "↑"
    width: 44
    height: 44
    padding: 0
    draw_text +: {
        color: ai_ink
        text_style +: { font_size: 20 }
    }
    draw_bg +: {
        color: ai_gold
        color_hover: #xFFD98B
        border_color: #xFFFFFF00
        border_size: 0.0
        border_radius: 22.0
    }
}
```

- [ ] **Step 4: Run focused interaction tests**

Run:

```bash
cargo test -p makepad-example-aichat
cargo check -p makepad-example-aichat
```

Expected: PASS. Existing send/cancel/clear ids still work.

### Task 6: Runtime Verification And Manual Visual Pass

**Files:**
- Modify if needed: `examples/aichat/src/main.rs`
- Optional: `examples/aichat/README.md`

- [ ] **Step 1: Run formatting and tests**

Run:

```bash
cargo fmt -p makepad-example-aichat
cargo test -p makepad-example-aichat
cargo check -p makepad-example-aichat
```

Expected: all pass.

- [ ] **Step 2: Run the GUI example**

Run:

```bash
RUST_BACKTRACE=1 cargo run -p makepad-example-aichat
```

Expected: app starts without abort/segfault.

- [ ] **Step 3: Manual visual checklist**

Verify against the target screenshot:

- Outer shell is rounded and visibly cyan-edged.
- No native red/yellow/green traffic lights.
- Window can still be dragged from the top safe drag strip.
- Sidebar active item is visibly selected.
- Top toolbar controls share one baseline and consistent heights.
- Glass slider updates the percentage and panel opacity.
- Output text is readable over bright/detailed wallpaper.
- Code and diagram blocks scroll horizontally inside their own cards.
- Composer is a single integrated bottom panel; `Clear` and send are aligned and not pasted to the edge.
- Network failure text remains visible and composer stays usable.

- [ ] **Step 4: Optional README note**

If `examples/aichat/README.md` is present or created, document:

```markdown
## Liquid Glass UI

The example uses a borderless transparent Makepad window with a dark emerald
glass shell. Use the `Glass` slider in the top toolbar to adjust panel opacity.
Lower values show more wallpaper; higher values improve text contrast.
```

- [ ] **Step 5: Final full verification**

Run:

```bash
agent-spec parse examples/aichat/specs/aichat-liquid-glass-ui.spec.md
agent-spec lint examples/aichat/specs/aichat-liquid-glass-ui.spec.md --min-score 0.7
cargo test -p makepad-example-aichat
cargo check -p makepad-example-aichat
```

Expected: spec parse/lint passes with no issues; tests/check pass.

## Execution Order

1. Task 1 first: establishes testable opacity/runtime contract.
2. Task 2 second: reduces color/layout drift before visual edits.
3. Task 3 third: fixes the visible top/sidebar/shell issues.
4. Task 4 fourth: protects content readability and scrolling.
5. Task 5 fifth: fixes the composer proportion and actions.
6. Task 6 last: full verification and manual visual pass.

## Rollback Guidance

- If a DSL edit causes startup abort or shader conversion errors, revert only the last style block changed and re-run `cargo check -p makepad-example-aichat`.
- Do not revert unrelated user edits in `examples/aichat/src/main.rs`; this file is already dirty.
- If the `SliderMinimalFlat` styling cannot produce the target rail/knob, keep the existing slider behavior and defer a custom slider shader to a separate task.
- If horizontal `CodeView` scrolling conflicts with Markdown dispatch, keep diagram horizontal scrolling and document code scrolling as the remaining gap rather than rewriting `widgets/src/markdown.rs` in the same pass.
