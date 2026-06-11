# AI-Assisted Workflow Classifier Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement an AI-assisted workflow intent classifier in the Studio hub that dynamically routes natural language user prompts to the matching workspace workflow.

**Architecture:** Extend the hub's `AiManager` to intercept natural language prompts, query the active LLM provider with a oneshot non-streaming intent classification request listing available workflows, parse the JSON response, and either trigger the matched workflow or fallback to normal chat.

**Tech Stack:** Rust, Makepad Studio hub, JSON parsing, LLM prompt engineering

---

### Task 1: Add Classifier state map and struct

**Files:**
- Modify: `studio/hub/src/ai_manager.rs:100-150`

- [ ] **Step 1: Define PendingClassifier struct**
Add the struct representing an in-flight classification request:
```rust
#[derive(Clone, Debug)]
pub struct PendingClassifier {
    pub mount: String,
    pub agent_id: AiAgentId,
    pub original_prompt: String,
}
```

- [ ] **Step 2: Add pending_classifiers to AiManager**
Add `pending_classifiers: HashMap<LiveId, PendingClassifier>` (guarded by Mutex or under `AiManager`'s internal struct) to `AiManager`.
If `AiManager` has a `data` field (e.g., `self.data.lock().unwrap()`), insert the map there.

- [ ] **Step 3: Verify compilation**
Run: `cargo check -p makepad-studio-hub`
Expected: Success.

- [ ] **Step 4: Commit**
```bash
git add studio/hub/src/ai_manager.rs
git commit -m "feat: add PendingClassifier struct and map to AiManager state"
```

---

### Task 2: Implement dynamic classification prompt construction

**Files:**
- Modify: `studio/hub/src/ai_manager.rs`

- [ ] **Step 1: Write helper to build classification prompt**
Add a method `build_classifier_prompt(&self, prompt: &str, workflows: &[ParsedWorkflow]) -> String`:
```rust
fn build_classifier_prompt(&self, prompt: &str, workflows: &[ParsedWorkflow]) -> String {
    let mut catalog = String::new();
    for wf in workflows {
        catalog.push_str(&format!("- {}: {}\n", wf.name, wf.steps.first().map(|s| s.name.as_str()).unwrap_or("")));
    }
    format!(
        "You are an AI assistant routing user prompts to available workflows. \
        Available workflows:\n\
        {}\n\
        Given the user's prompt: \"{}\"\n\
        Determine if they want to trigger one of the workflows. \
        Return a JSON object in this format (no other text, no markdown fences):\n\
        {{\n  \"matched_workflow\": \"workflow-name\",\n  \"arguments\": \"extracted-args\"\n}}\n\
        If no workflow matches, return null.",
        catalog, prompt
    )
}
```

- [ ] **Step 2: Verify compilation**
Run: `cargo check -p makepad-studio-hub`

- [ ] **Step 3: Commit**
```bash
git add studio/hub/src/ai_manager.rs
git commit -m "feat: implement dynamic classification prompt construction"
```

---

### Task 3: Intercept prompts and send classification requests

**Files:**
- Modify: `studio/hub/src/ai_manager.rs`

- [ ] **Step 1: Update send_prompt to check and trigger classifier**
In `send_prompt`, if the prompt does not start with a slash `/`, and `workflows` list is non-empty:
- Generate a new `LiveId` for `request_id`.
- Store the state in `pending_classifiers`.
- Build the classifier prompt.
- Build the Http request with `stream: false` using the active provider backend and the classifier prompt.
- Send the `HubEvent::HttpRequest` and return.
- If workflows are empty or inactive, proceed directly to the normal prompt path.

- [ ] **Step 2: Verify check**
Run: `cargo check -p makepad-studio-hub`

- [ ] **Step 3: Commit**
```bash
git add studio/hub/src/ai_manager.rs
git commit -m "feat: intercept user prompts and trigger AI classification requests"
```

---

### Task 4: Handle classifier response and trigger workflow

**Files:**
- Modify: `studio/hub/src/ai_manager.rs`

- [ ] **Step 1: Handle classifier response in handle_http_response**
Update `handle_http_response` to check if `request_id` is in `pending_classifiers`:
- If yes, parse the body (JSON) to extract `matched_workflow` and `arguments`.
- If matched_workflow is Some:
  - Initialize the `ActiveWorkflowState` and set the owner.
  - Rewrite the prompt to the Step 1 execution instructions.
  - Proceed by calling the normal prompt acceptance path with the rewritten prompt.
- If matched_workflow is None (or parsing fails):
  - Fallback cleanly by sending the original user prompt to the normal prompt acceptance path.
- Remove the `request_id` from `pending_classifiers`.

- [ ] **Step 2: Add focused tests**
Add/update tests covering:
- classifier JSON parsing and fallback behavior
- successfully matched workflow activates the workflow and rewrites prompt

- [ ] **Step 3: Verify**
Run:
```bash
cargo test -p makepad-studio-hub workflow
cargo check -p makepad-studio-hub
```
Expected: all tests pass.

- [ ] **Step 4: Commit**
```bash
git add studio/hub/src/ai_manager.rs
git commit -m "feat: handle classifier HTTP response and trigger matched workflow"
```

---

### Task 5: E2E Verification

- [ ] **Step 1: Rebuild Studio**
Run: `cargo build --release -p cargo-makepad -p makepad-studio`
- [ ] **Step 2: Launch Studio and type natural language workflow request**
Launch Makepad Studio.
Type `review PR 12` in the AI input and click run.
- [ ] **Step 3: Verify workflow triggers**
Verify that the `Task Board` and `Live Activity` open and show the `review-prs` workflow progress, proving the classifier routed the prompt correctly.
