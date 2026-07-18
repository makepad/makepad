# Makepad Web Phase 0 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the three confirmed Makepad Web correctness and lifetime defects before performance benchmarking.

**Architecture:** Correct the shared uniform slice lengths at their definitions, move the web render command vector from global `CxOs` scratch storage into each persistent `CxOsPass`, and run guarded ScriptVM collection at the existing web animation-frame safe point. Keep the current Rust-to-JavaScript protocol unchanged and use the repository's existing `include_str!` source-contract pattern for web-only lifecycle code that host tests cannot compile directly.

**Tech Stack:** Rust, Makepad platform, WebAssembly bridge, Cargo tests.

## Global Constraints

- Make the smallest root-cause changes; add no dependency, abstraction, JavaScript protocol change, or performance instrumentation.
- Preserve the untracked `LearningMakepad.md` and all unrelated user changes.
- Use release mode for every validation command.
- Do not launch a UI application outside Makepad Studio remote control.
- Each task must follow red-green TDD and receive both spec-compliance and code-quality review before the next task starts.

---

### Task 1: Correct uniform slice lengths

**Files:**
- Modify: `platform/src/draw_list.rs:379`
- Test: `platform/src/draw_list.rs`

**Interfaces:**
- Consumes: Existing `DrawCallUniforms::as_slice` and `DrawListUniforms::as_slice` callers on every renderer backend.
- Produces: Slice lengths expressed in `f32` elements: 4 for `DrawCallUniforms` and 24 for `DrawListUniforms`.

- [ ] **Step 1: Add the failing regression test after `DrawListUniforms::as_slice`**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::size_of;

    #[test]
    fn uniform_slices_match_struct_storage() {
        assert_eq!(
            DrawCallUniforms::default().as_slice().len(),
            size_of::<DrawCallUniforms>() >> 2,
        );
        assert_eq!(
            DrawListUniforms::default().as_slice().len(),
            size_of::<DrawListUniforms>() >> 2,
        );
    }
}
```

- [ ] **Step 2: Run the test and verify the expected failure**

Run: `rtk cargo test --release -p makepad-platform --lib uniform_slices_match_struct_storage`

Expected: FAIL because the current first slice length is 16 instead of 4.

- [ ] **Step 3: Correct both array lengths at their shared definitions**

```rust
impl DrawCallUniforms {
    pub fn as_slice(
        &self,
    ) -> &[f32; std::mem::size_of::<DrawCallUniforms>() >> 2] {
        unsafe { std::mem::transmute(self) }
    }
}

impl DrawListUniforms {
    pub fn as_slice(
        &self,
    ) -> &[f32; std::mem::size_of::<DrawListUniforms>() >> 2] {
        unsafe { std::mem::transmute(self) }
    }
}
```

- [ ] **Step 4: Run the focused and platform library tests**

Run: `rtk cargo test --release -p makepad-platform --lib uniform_slices_match_struct_storage`

Expected: PASS.

Run: `rtk cargo test --release -p makepad-platform --lib`

Expected: all tests PASS.

- [ ] **Step 5: Commit**

```bash
git add platform/src/draw_list.rs
git commit -m "fix(web): correct uniform slice lengths"
```

### Task 2: Make render command buffers pass-owned

**Files:**
- Create: `platform/tests/web_phase0.rs`
- Modify: `platform/src/os/web/web_render.rs:366`
- Modify: `platform/src/os/web/web.rs:1128`
- Modify: `platform/src/os/web/from_wasm.rs:321`

**Interfaces:**
- Consumes: Existing `FromWasmRenderCommandBuffer` pointer/length message and persistent `CxDrawPass::os` storage.
- Produces: One retained `Vec<u32>` per web draw pass, valid until JavaScript synchronously dispatches the completed pump.

- [ ] **Step 1: Add the failing host-runnable ownership contract**

```rust
const WEB_RENDER: &str = include_str!("../src/os/web/web_render.rs");
const WEB: &str = include_str!("../src/os/web/web.rs");

#[test]
fn web_render_command_buffers_are_pass_owned() {
    assert!(WEB_RENDER.contains(
        "pub struct CxOsPass {\n    pub(crate) render_cmd_buf: Vec<u32>,\n}"
    ));
    assert_eq!(WEB_RENDER.matches(".os.render_cmd_buf").count(), 6);
    assert!(!WEB.contains("pub(crate) render_cmd_buf: Vec<u32>"));
}
```

- [ ] **Step 2: Run the test and verify the expected failure**

Run: `rtk cargo test --release -p makepad-platform --test web_phase0 web_render_command_buffers_are_pass_owned`

Expected: FAIL because `CxOsPass` is empty and the command buffer is still on `CxOs`.

- [ ] **Step 3: Move storage into `CxOsPass` and use it in both producers**

In `web_render.rs`, define:

```rust
#[derive(Default, Clone, Debug)]
pub struct CxOsPass {
    pub(crate) render_cmd_buf: Vec<u32>,
}
```

In both `draw_pass_to_canvas` and `draw_pass_to_texture`, replace global buffer access with:

```rust
let mut cmd_buf =
    std::mem::take(&mut self.passes[draw_pass_id].os.render_cmd_buf);
cmd_buf.clear();
self.render_view(
    draw_pass_id,
    draw_list_id,
    &mut zbias,
    zbias_step,
    &mut cmd_buf,
);
self.passes[draw_pass_id].os.render_cmd_buf = cmd_buf;
let words = WasmPtrU32::new(&self.passes[draw_pass_id].os.render_cmd_buf);
self.os.from_wasm(FromWasmRenderCommandBuffer {
    words,
});
```

Delete `render_cmd_buf` from `CxOs` and its `Default` implementation in `web.rs`. Update the comment in `from_wasm.rs` to say the buffer is owned by `CxOsPass`.

- [ ] **Step 4: Run the ownership contract and platform tests**

Run: `rtk cargo test --release -p makepad-platform --test web_phase0 web_render_command_buffers_are_pass_owned`

Expected: PASS.

Run: `rtk cargo test --release -p makepad-platform --lib`

Expected: all tests PASS.

- [ ] **Step 5: Commit**

```bash
git add platform/tests/web_phase0.rs platform/src/os/web/web_render.rs platform/src/os/web/web.rs platform/src/os/web/from_wasm.rs
git commit -m "fix(web): retain render commands per pass"
```

### Task 3: Run guarded ScriptVM collection after web repaint

**Files:**
- Modify: `platform/tests/web_phase0.rs`
- Modify: `platform/src/os/web/web.rs:521`

**Interfaces:**
- Consumes: Existing `Cx::with_vm`, `ScriptHeap::needs_gc`, and `ScriptVm::gc` APIs.
- Produces: Native-backend-equivalent guarded collection at the web animation-frame safe point after repaint command generation.

- [ ] **Step 1: Append the failing lifecycle contract**

```rust
#[test]
fn web_animation_frame_runs_guarded_gc_after_repaint() {
    let frame = WEB
        .split("if let Some(time) = is_animation_frame {")
        .nth(1)
        .unwrap()
        .split("if network_responses.len() != 0")
        .next()
        .unwrap();
    let repaint = frame.find("self.handle_repaint(time);").unwrap();
    let guard = frame.find("if vm.heap().needs_gc()").unwrap();
    let collect = frame.find("vm.gc();").unwrap();
    assert!(repaint < guard && guard < collect);
}
```

- [ ] **Step 2: Run the test and verify the expected failure**

Run: `rtk cargo test --release -p makepad-platform --test web_phase0 web_animation_frame_runs_guarded_gc_after_repaint`

Expected: FAIL because the web animation-frame block has no guarded GC call.

- [ ] **Step 3: Add the minimal post-repaint guard**

Immediately after `self.handle_repaint(time);` in `process_to_wasm`:

```rust
self.with_vm(|vm| {
    if vm.heap().needs_gc() {
        vm.gc();
    }
});
```

- [ ] **Step 4: Run focused and complete Phase 0 tests**

Run: `rtk cargo test --release -p makepad-platform --test web_phase0 web_animation_frame_runs_guarded_gc_after_repaint`

Expected: PASS.

Run: `rtk cargo test --release -p makepad-platform --lib`

Expected: all tests PASS.

Run: `rtk cargo test --release -p makepad-platform --test web_phase0`

Expected: both web Phase 0 contracts PASS.

- [ ] **Step 5: Commit**

```bash
git add platform/tests/web_phase0.rs platform/src/os/web/web.rs
git commit -m "fix(web): collect script heap after repaint"
```

### Final verification

- [ ] Run `rtk cargo fmt --check`.
- [ ] Run `rtk cargo test --release -p makepad-platform --lib`.
- [ ] Run `rtk cargo test --release -p makepad-platform --test web_phase0`.
- [ ] Run `rtk cargo makepad wasm build -p makepad-example-splash --release` to compile the web backend without launching it.
- [ ] Inspect the complete branch diff for scope, then obtain a final spec-compliance and code-quality review.
