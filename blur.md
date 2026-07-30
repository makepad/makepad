# Numbered Overlays And Multi-Level Gauss Blur

## Goal

Replace the current single overlay channel with numbered overlay levels, then add two Gauss capture levels:

- Gauss level 0: captures the base window scene before any overlay.
- Gauss level 1: captures the base scene plus overlay level 0.

This lets glass on overlay level 0 blur/lens the app background, while glass on overlay level 1 can blur/lens the already-composited level-0 UI. It also keeps the rule clear: a glass surface samples only completed lower levels, never itself or peers in the same level.

## Current State

The current overlay path is singular:

- `draw/src/cx_2d.rs` stores one `overlay_id`, one `overlay_pass_id`, and `overlay_draw_depth`.
- `draw/src/overlay.rs` owns one overlay root `DrawList`.
- `draw/src/draw_list_2d.rs::begin_overlay_reuse()` appends a widget draw list to that one overlay root.
- `widgets/src/window.rs` owns one `overlay: Overlay`.
- `widgets/src/gauss_view.rs::request_window_gauss()` only succeeds while `cx.is_drawing_overlay()` is true.
- `Window::begin()` chooses either the normal pass or the `gauss_scene` capture pass, then points the one overlay root at the final window pass.

That means all overlay widgets are in one ordering bucket. A glass popup can sample the base app scene, but another glass layer above it cannot sample the popup as part of its blurred backing. Overlapping glass in the same overlay level also cannot produce nested blur because all glass is sampling the same lower snapshot.

## Proposed Model

Introduce ordered overlay levels:

```rust
pub type OverlayLevel = u8;

pub const OVERLAY_LEVEL_BASE: OverlayLevel = 0;
pub const OVERLAY_LEVEL_FLOATING: OverlayLevel = 1;
pub const OVERLAY_LEVEL_TOP: OverlayLevel = 2;
```

Level order is strict:

1. Main scene
2. Overlay level 0
3. Overlay level 1
4. Overlay level 2 and higher

Within a level, existing append order and `begin_overlay_last` behavior still apply. Across levels, numeric level decides ordering.

## Public API Shape

Keep existing calls as level-0 compatibility:

```rust
draw_list.begin_overlay_reuse(cx);      // means level 0
draw_list.begin_overlay_last(cx);       // means level 0, last within level 0
```

Add explicit level APIs:

```rust
draw_list.begin_overlay_level_reuse(cx, level);
draw_list.begin_overlay_level_last(cx, level);
```

Widgets that own overlay draw lists should gain a live property:

```rust
#[live(0)]
overlay_level: u8,
```

Then `Tooltip`, `PopupNotification`, `PopupMenu`, `Modal`, dock ghost overlays, and the new glass layer can choose where they belong without custom render code.

## Render Context Changes

Replace the single overlay target fields in `Cx2d` with a small overlay target table plus current overlay draw state:

```rust
pub struct OverlayTarget {
    pub draw_list_id: DrawListId,
    pub pass_id: Option<DrawPassId>,
}

pub struct Cx2d<'a, 'b> {
    overlay_targets: SmallVec<[Option<OverlayTarget>; 4]>,
    current_overlay_level: Option<OverlayLevel>,
    overlay_draw_depth: usize,
    ...
}
```

Required helpers:

```rust
pub fn is_drawing_overlay(&self) -> bool;
pub fn current_overlay_level(&self) -> Option<OverlayLevel>;
pub fn overlay_target(&self, level: OverlayLevel) -> Option<&OverlayTarget>;
```

`DrawList2d::begin_overlay_level_*` should:

- Look up the target for the requested level.
- Set this draw list's pass to the target pass, falling back to the current pass if no explicit pass exists.
- Store this draw list under that level's overlay root.
- Set `current_overlay_level` while drawing.
- Increment/decrement `overlay_draw_depth` as today.

## Overlay Stack

Replace or wrap `Overlay` with an `OverlayStack` owned by `Window`:

```rust
pub struct OverlayStack {
    levels: Vec<Overlay>,
}
```

Responsibilities:

- Allocate one root overlay draw list per level.
- During window begin, publish all active overlay targets into `Cx2d`.
- During window end, append each overlay root in ascending level order.
- Flush stale sublists per level, using the same redraw-id cleanup as today's single `Overlay::end()`.

Backward compatibility can be kept by making `Overlay` a thin level-0 wrapper or by preserving `Overlay::begin()` as `OverlayStack::begin_level(0)`.

## Two Gauss Capture Levels

Extend the per-window Gauss state from one request/snapshot to two indexed capture states:

```rust
pub const WINDOW_GAUSS_LEVELS: usize = 2;

struct GaussCaptureState {
    requested_last_frame: bool,
    requested_this_frame: bool,
    capture_active: bool,
    snapshot: Option<GaussBlurSnapshot>,
}

struct GaussWindowEntry {
    generation: u64,
    levels: [GaussCaptureState; WINDOW_GAUSS_LEVELS],
}
```

Add APIs:

```rust
pub fn request_window_gauss_level(cx: &mut Cx2d, level: usize) -> Option<GaussBlurSnapshot>;
pub fn request_window_gauss(cx: &mut Cx2d) -> Option<GaussBlurSnapshot>;
```

Default behavior:

- If drawing overlay level 0, `request_window_gauss()` requests Gauss level 0.
- If drawing overlay level 1 or higher, `request_window_gauss()` requests Gauss level 1.
- Explicit `request_window_gauss_level()` exists for advanced widgets and tests.

This mapping keeps existing `GaussRoundedView` source-compatible while allowing higher overlays to sample a later scene.

## Window Render Order

When no Gauss is requested, rendering can remain almost identical:

1. Draw main scene to the window pass.
2. Draw overlay roots in numeric order.

When only Gauss level 0 is requested:

1. Draw main scene into `gauss_stack[0].scene`.
2. Build blur chain 0.
3. Draw scene 0 texture to the window pass.
4. Draw overlay level 0+ to the window pass. Level-0 glass samples snapshot 0.

When Gauss level 1 is requested:

1. Draw main scene into `gauss_stack[0].scene`.
2. Build blur chain 0.
3. Draw/copy scene 0 into `gauss_stack[1].scene`.
4. Draw overlay level 0 into `gauss_stack[1].scene`. Level-0 glass samples snapshot 0.
5. Build blur chain 1 from `gauss_stack[1].scene`.
6. Draw scene 1 texture to the window pass.
7. Draw overlay level 1+ to the window pass. Level-1 glass samples snapshot 1.

This avoids feedback loops:

- Level 0 never samples itself.
- Level 1 samples base plus completed level 0.
- Same-level glass overlap is still not nested blur, by design.

## Gauss Stack Ownership

`Window` currently owns one `GaussStack`. Change that to two stacks:

```rust
gauss_stacks: [GaussStack; WINDOW_GAUSS_LEVELS],
```

To reduce duplicated code:

- Keep `GaussStack` as the render-texture/mip-chain owner.
- Add a helper that renders one stack from an input draw phase.
- Reuse existing `DrawGaussDownsample`, `DrawGaussUpsample`, and `DrawGaussScene`.

The second stack needs a way to start with the previous level's scene texture. A simple first implementation can draw `gauss_stack[0].scene_texture` into `gauss_stack[1].scene` with `DrawGaussScene`, then draw overlay level 0 over it.

## Widget Migration

Add `overlay_level` to:

- `PopupNotification`
- `Tooltip`
- `PopupMenu`
- `Modal`
- Dock drag ghost overlay
- `mod.widgets.glass.Layer`

Defaults:

- Existing behavior stays level 0.
- Menus/tooltips that must float over glass app chrome can opt into level 1.
- Debug/drag/cursor-like overlays can use level 2 or `begin_overlay_level_last()`.

## Compatibility Rules

- Existing apps keep working because `begin_overlay_reuse()` maps to level 0.
- Existing `GaussRoundedView` keeps working because `request_window_gauss()` maps from current overlay level.
- Non-overlay Gauss requests should still return `None`.
- Existing single-level popup demos should look unchanged.

## Tests And Validation

Add focused tests or demos:

1. Base scene only: no overlay, no Gauss request, same as current render.
2. Level-0 glass over detailed background: samples Gauss level 0.
3. Level-1 glass popup over level-0 glass nav: samples Gauss level 1 and visibly blurs/lenses the nav.
4. Same-level overlapping glass: does not recursively blur peers.
5. `begin_overlay_last()` remains last only within its level.
6. Stale overlay draw lists are removed independently per level.

Runtime validation should use Studio release runs and screenshots, because this is a UI/render-stack change.

## Implementation Phases

1. Add numbered overlay plumbing to `Cx2d`, `DrawList2d`, and `OverlayStack`, keeping old APIs as level-0 wrappers.
2. Migrate `Window` from one `Overlay` to `OverlayStack` with one level enabled first.
3. Add `overlay_level` fields to overlay widgets and verify old demos are unchanged.
4. Refactor Gauss window global state to indexed capture states.
5. Add two `GaussStack`s to `Window` and implement level-1 capture/composite.
6. Update `request_window_gauss()` to choose a snapshot from the current overlay level.
7. Build a small demo with level-0 app chrome and level-1 popup glass to prove nested blur.

## Open Decisions

- Maximum overlay levels: fixed small array versus dynamic vector. A fixed small array is simpler and likely enough initially.
- Whether level 2 should get a third Gauss capture later. For now it should render above level 1 while sampling level-1's snapshot if it uses glass.
- Whether pass compositing should draw full scene textures at each capture level or draw only overlay deltas. Full scene textures are simpler and safer first; delta compositing can be optimized later.
