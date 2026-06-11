# Makepad Studio AI Visibility Upgrade Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Studio's AI workflow observable in real time by showing step ownership, terminal activity, file touches, and recent execution transitions.

**Architecture:** Extend hub-side agent state with explicit visibility fields plus a bounded event ring, serialize those fields through the Studio protocol, and upgrade the desktop Task Board / Live Activity renderers to consume the new data. Keep workflow semantics stable; improve observability first.

**Tech Stack:** Rust, Makepad Studio hub/desktop, SerJson/DeJson, markdown rendering

---

### Task 1: Add visibility fields and event structs

**Files:**
- Modify: `platform/studio/src/hub_protocol.rs`
- Modify: `studio/hub/src/ai_manager.rs`

- [ ] **Step 1: Add protocol structs for visibility events**
Define bounded event payload types in `platform/studio/src/hub_protocol.rs`:
```rust
#[derive(Clone, Debug, SerJson, DeJson)]
pub struct AiVisibilityEvent {
    pub kind: String,
    pub agent_id: Option<AiAgentId>,
    pub title: String,
    pub detail: String,
    pub timestamp: f64,
}
```
Also extend the existing mount/agent protocol structs with:
```rust
pub active_terminal_path: Option<String>,
pub active_terminal_title: Option<String>,
pub state_changed_at: f64,
pub workflow_step_name: Option<String>,
pub workflow_step_status: Option<String>,
pub blocked_reason: Option<String>,
```
And add `visibility_events: Vec<AiVisibilityEvent>` to `AiMountState`.

- [ ] **Step 2: Extend hub-side structs**
In `studio/hub/src/ai_manager.rs`, add matching fields to `RunningAgent` and `MountAgents`.
Use a bounded vector/ring for mount events.

- [ ] **Step 3: Wire snapshot serialization**
Update `snapshot()` so every new field is serialized into `AiAgentSummary`, `AiAgentState`, and `AiMountState`.

- [ ] **Step 4: Verify hub compilation**
Run: `cargo check -p makepad-studio-hub`
Expected: exit 0, existing warnings allowed.

- [ ] **Step 5: Commit**
```bash
git add platform/studio/src/hub_protocol.rs studio/hub/src/ai_manager.rs
git commit -m "feat: add AI visibility state and event protocol fields"
```

---

### Task 2: Populate hub visibility state from workflow and terminal activity

**Files:**
- Modify: `studio/hub/src/ai_manager.rs`

- [ ] **Step 1: Add helper to push bounded mount events**
Add a helper like:
```rust
fn push_visibility_event(mount_state: &mut MountAgents, event: AiVisibilityEvent) {
    const MAX_VISIBILITY_EVENTS: usize = 64;
    mount_state.visibility_events.push(event);
    if mount_state.visibility_events.len() > MAX_VISIBILITY_EVENTS {
        let overflow = mount_state.visibility_events.len() - MAX_VISIBILITY_EVENTS;
        mount_state.visibility_events.drain(0..overflow);
    }
}
```

- [ ] **Step 2: Populate workflow ownership fields**
When `/workflow-name` activates, set on the owning agent:
- `workflow_step_name`
- `workflow_step_status`
- `current_action` such as `Executing workflow step 1`
- `state_changed_at`
Emit `step_activated` visibility event.

- [ ] **Step 3: Populate terminal fields**
When tracked terminal state changes, populate on the owning agent:
- `active_terminal_path`
- `active_terminal_title`
- `last_terminal_excerpt`
- `current_action` from terminal summary
- `blocked_reason` when waiting for input / needs attention
- `files_touched`
Emit events for terminal attached, needs input, done, file touched.

- [ ] **Step 4: Populate completion/failure transitions**
When agent or workflow step finishes/fails, update:
- `workflow_step_status`
- `blocked_reason`
- `current_action`
- `state_changed_at`
Emit `agent_done`, `agent_failed`, or `step_completed` as appropriate.

- [ ] **Step 5: Add focused hub tests**
Add tests covering:
- event ring bounds
- workflow activation populates owner fields
- terminal observation populates activity fields
- file touch updates agent files
- owner deletion clears workflow state and stops further workflow injection

- [ ] **Step 6: Verify**
Run:
```bash
cargo test -p makepad-studio-hub workflow
cargo check -p makepad-studio-hub
```
Expected: tests pass, check passes with existing warnings only.

- [ ] **Step 7: Commit**
```bash
git add studio/hub/src/ai_manager.rs
git commit -m "feat: populate AI visibility state from workflow and terminal activity"
```

---

### Task 3: Upgrade Task Board rendering

**Files:**
- Modify: `studio/desktop/src/ai_manager.rs`

- [ ] **Step 1: Render owning agent under active step**
Update `ai_task_board_markdown` to render the owning agent row beneath the active workflow step using the new fields:
- status marker
- title
- current action
- workflow step name/status when present
Preserve the existing nested agent rendering.

- [ ] **Step 2: Bound workflow rendering**
Keep the current workflow truncation/row cap behavior. Reuse `truncate_inline` for long names and preserve omitted-step count behavior.

- [ ] **Step 3: Add focused desktop tests**
Add/update tests for:
- active workflow with owner action shown
- bounded workflow rendering preserved
- nested subagent rows preserved

- [ ] **Step 4: Verify**
Run: `cargo test -p makepad-studio ai_task_board_tests`
Expected: pass.

- [ ] **Step 5: Commit**
```bash
git add studio/desktop/src/ai_manager.rs
git commit -m "feat: show workflow owner activity in Task Board"
```

---

### Task 4: Upgrade Live Activity rendering

**Files:**
- Modify: `studio/desktop/src/ai_manager.rs`

- [ ] **Step 1: Render mount visibility events**
Update `ai_live_activity_markdown` to render `visibility_events` newest-first in a dedicated `Recent Activity` section.
Format each row with a short human-readable line using `kind`, `title`, and `detail`.

- [ ] **Step 2: Add agent detail strip section**
Add an `Agent Details` section for the most relevant active/pending agent showing:
- terminal path/title
- current action
- last terminal excerpt
- touched files preview
- blocked reason
Keep the existing polished `live_markdown` content above this new section.

- [ ] **Step 3: Bound output sizes**
Use truncation and row limits so that:
- visibility event rows are bounded
- excerpts do not explode markdown size
- touched files preview is capped

- [ ] **Step 4: Add focused desktop tests**
Add/update tests for:
- event timeline rendering
- blocked reason rendering
- touched files truncation
- coexistence with existing `live_markdown`

- [ ] **Step 5: Verify**
Run:
```bash
cargo test -p makepad-studio ai_
cargo check -p makepad-studio
```
Expected: pass with existing warnings only.

- [ ] **Step 6: Commit**
```bash
git add studio/desktop/src/ai_manager.rs
git commit -m "feat: add recent AI activity timeline and agent detail rendering"
```

---

### Task 5: End-to-end Studio verification

**Files:**
- Create/Modify: `.studio/workflows/review-prs.md`
- Create/Modify: `.studio/skills/semantic-compression.md`

- [ ] **Step 1: Prepare workspace examples**
Ensure `.studio/workflows/review-prs.md` and `.studio/skills/semantic-compression.md` exist with simple content suitable for testing.

- [ ] **Step 2: Launch Studio through remote flow**
Run:
```bash
echo '{"ListBuilds":[]}' | target/release/cargo-makepad studio --studio=127.0.0.1:8001
echo '{"ClearBuild":{"build_id":[N]}}' | target/release/cargo-makepad studio --studio=127.0.0.1:8001
echo '{"RunItem":{"mount":"makepad","name":"makepad-studio"}}' | target/release/cargo-makepad studio --studio=127.0.0.1:8001
```
Expected: `BuildStarted` then `AppStarted`.

- [ ] **Step 3: Submit workflow command**
Use Studio remote click/type/return to submit:
```text
/review-prs 12
```
Then click the AI run button.

- [ ] **Step 4: Verify Task Board and Live Activity visually**
Capture screenshot(s) and verify:
- workflow step owner/action visible
- recent activity timeline visible
- terminal attach / file touch / step transition lines visible
- blocked/waiting state visible when present

- [ ] **Step 5: Verify owner deletion cleanup**
Delete the owning chat if feasible via UI or invoke the corresponding code path in test coverage; confirm stale workflow visibility disappears.

- [ ] **Step 6: Final verification**
Run:
```bash
cargo build --release -p cargo-makepad -p makepad-studio
```
Expected: pass with existing warnings only.

- [ ] **Step 7: Commit examples or cleanup**
If the `.studio` examples are intended to remain:
```bash
git add .studio/skills/semantic-compression.md .studio/workflows/review-prs.md
git commit -m "chore: add Studio workflow visibility examples"
```
If they are temporary test files, remove them instead and commit the cleanup.
