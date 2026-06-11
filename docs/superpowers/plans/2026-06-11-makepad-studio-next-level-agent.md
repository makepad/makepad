# Next-Level Agent Workflows, Skills, and Visibility Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement workflows, skills, and advanced real-time visibility (Task Board & Swarm Activity) inside Makepad Studio, driven by Git-trackable files in a `.studio/` folder.

**Architecture:** Extend the hub's `AiManager` state to parse and execute Markdown-based workflows and skills from `.studio/`. Enrich `RunningAgent` with real-time visibility tracking (current action, terminal output line, files touched). Update the desktop client's markdown formatting functions to render the checklist progress and subagent activity.

**Tech Stack:** Rust, Makepad framework, UI Script, Markdown, SerJson/DeJson

---

### Task 1: Add State Structs & Protocol Summaries

**Files:**
- Modify: `platform/studio/src/hub_protocol.rs:180-230`
- Modify: `studio/hub/src/ai_manager.rs:80-140`

- [ ] **Step 1: Define Workflow structs in hub_protocol.rs**
Add the new structs to `platform/studio/src/hub_protocol.rs` and serialize them:
```rust
#[derive(Clone, Debug, SerJson, DeJson)]
pub struct WorkflowStepState {
    pub name: String,
    pub status: String,
}

#[derive(Clone, Debug, SerJson, DeJson)]
pub struct ActiveWorkflowState {
    pub name: String,
    pub current_step: usize,
    pub steps: Vec<WorkflowStepState>,
}
```

- [ ] **Step 2: Extend AiAgentSummary and AiAgentState in hub_protocol.rs**
Add `current_action`, `last_terminal_excerpt`, and `files_touched` fields to `AiAgentSummary` and `AiAgentState`. Also add `active_workflow: Option<ActiveWorkflowState>` to `AiMountState`.

- [ ] **Step 3: Extend RunningAgent and MountAgents in ai_manager.rs**
Add these same fields to the internal structs in `studio/hub/src/ai_manager.rs`.

- [ ] **Step 4: Verify Compilation**
Run: `cargo check -p makepad-studio-protocol -p makepad-studio-hub`
Expected: Process exits with `0`.

- [ ] **Step 5: Commit state upgrades**
```bash
git add platform/studio/src/hub_protocol.rs studio/hub/src/ai_manager.rs
git commit -m "feat: add workflow and detailed subagent tracking states to protocol and hub"
```

---

### Task 2: Implement Skills & Workflows Parser in hub

**Files:**
- Modify: `studio/hub/src/ai_manager.rs`

- [ ] **Step 1: Write parser for Skill markdown files**
Add helper methods in `AiManager` to load `.studio/skills/*.md`, extract frontmatter using regex or simple parsing (extracting YAML values between `---`), and load the body text.

- [ ] **Step 2: Write parser for Workflow markdown files**
Add parser to read `.studio/workflows/*.md` and extract the title and steps under the `## Steps` header.

- [ ] **Step 3: Integrate parsers into AiManager**
Call these parsers when initializing the mount workspace.

- [ ] **Step 4: Verify check**
Run: `cargo check -p makepad-studio-hub`
Expected: Success.

- [ ] **Step 5: Commit parsers**
```bash
git add studio/hub/src/ai_manager.rs
git commit -m "feat: implement markdown skills and workflows parsers in studio hub"
```

---

### Task 3: Implement Dynamic Prompt Injection & Execution Loop

**Files:**
- Modify: `studio/hub/src/ai_manager.rs`

- [ ] **Step 1: Inject Skills into Model Request System Prompt**
Update the model prompt construction in `AiManager::start_model_request` to load matching skills from `.studio/skills/*.md` and append them to the initial system prompt.

- [ ] **Step 2: Inject Workflow step focus**
If a workflow is active, append the current step name and goal description to the system prompt of the manager agent.

- [ ] **Step 3: Implement step transition**
When a task finishes or the manager calls a completion action, automatically progress the `current_step` in `ActiveWorkflowState` and trigger the next model request.

- [ ] **Step 4: Verify check**
Run: `cargo check -p makepad-studio-hub`
Expected: Success.

- [ ] **Step 5: Commit execution engine**
```bash
git add studio/hub/src/ai_manager.rs
git commit -m "feat: implement dynamic skill prompt injection and workflow execution loop"
```

---

### Task 4: Upgrade UI Swarm & Task Board rendering

**Files:**
- Modify: `studio/desktop/src/ai_manager.rs:1130-1280`

- [ ] **Step 1: Update ai_task_board_markdown to render active workflows**
In `studio/desktop/src/ai_manager.rs`, modify `ai_task_board_markdown` to check `state.active_workflow` and render the parsed steps checklist (showing checkmarks for finished steps, spinner/arrow for active step, and fanned-out subagents underneath).

- [ ] **Step 2: Update ai_live_activity_markdown to show detailed actions**
Update `ai_live_activity_markdown` to show the subagents' `current_action`, `last_terminal_excerpt`, and `files_touched`.

- [ ] **Step 3: Build & verify**
Run: `cargo build --release -p makepad-studio`
Expected: Process exits with `0`.

- [ ] **Step 4: Commit rendering upgrades**
```bash
git add studio/desktop/src/ai_manager.rs
git commit -m "feat: upgrade Task Board and Live Activity markdown rendering in desktop client"
```

---

### Task 5: End-to-End Visual Verification

- [ ] **Step 1: Setup test workflow and skills**
Create `.studio/skills/semantic-compression.md` and `.studio/workflows/review-prs.md` in the workspace directory.
- [ ] **Step 2: Run Makepad Studio**
Launch Makepad Studio and type `/review-prs` in the AI input.
- [ ] **Step 3: Verify Task Board & Swarm Activity**
Verify that the Task Board fold opens, displays the list of steps correctly, and that subagents appear under the active step with real-time status updates.
