# Spec: Makepad Studio Next-Level Agent Workflows, Skills, and Visibility

## 1. Goal
Upgrade the Makepad Studio hub agent to support Git-trackable, customizable **Workflows** and **Skills** stored inside a `.studio/` workspace directory (similar to `oh-my-pi`), and significantly enhance **Visibility** into active tasks, subagents, and terminal actions in real-time.

## 2. Directory Layout & File Formats

Workflows and Skills are managed directly in the project workspace under `.studio/`:

```text
.studio/
  ├── skills/
  │     ├── semantic-compression.md
  │     └── code-standards.md
  └── workflows/
        └── review-prs.md
```

### 2.1 Skill Format (`.studio/skills/*.md`)
Contains a YAML frontmatter header followed by raw Markdown guidelines:
```markdown
---
name: semantic-compression
description: Guidelines for aggressive prompt compression.
---
# Semantic Compression Rules
- Delete articles.
- Keep only meaning-carrying content.
```

### 2.2 Workflow Format (`.studio/workflows/*.md`)
Structured steps under `## Steps`:
```markdown
# Review PRs Command
## Arguments
- `$ARGUMENTS`

## Steps
### 1. Resolve PR Set
Parse arguments and query github PRs.

### 2. Fan out subagents
Spawn one subagent per PR to review.
```

## 3. Architecture & State Upgrades

### 3.1 State Structs (`studio/hub/src/ai_manager.rs`)
Add active workflow and subagent metadata:

```rust
#[derive(Clone, Debug, SerJson, DeJson)]
pub struct WorkflowStepState {
    pub name: String,
    pub status: String, // "pending" | "active" | "done" | "failed"
}

#[derive(Clone, Debug, SerJson, DeJson)]
pub struct ActiveWorkflowState {
    pub name: String,
    pub current_step: usize,
    pub steps: Vec<WorkflowStepState>,
}
```

Extend `RunningAgent` and `MountAgents`:
```rust
struct RunningAgent {
    // ... existing fields ...
    current_action: Option<String>,
    last_terminal_excerpt: Option<String>,
    files_touched: Vec<String>,
}

struct MountAgents {
    // ... existing fields ...
    active_workflow: Option<ActiveWorkflowState>,
}
```

### 3.2 Protocol Upgrades (`platform/studio/src/hub_protocol.rs`)
Propagate the new status fields to `AiAgentSummary` and `AiAgentState`.

## 4. Workflow Execution & Skill Injection

### 4.1 Workflow Execution
- Triggers via `/workflow-name <arguments>` input.
- Initializes `ActiveWorkflowState` and guides the manager agent step-by-step.
- Prompts are dynamically adjusted to focus on the active step's goal.

### 4.2 Dynamic Skill Injection
- Prior to making LLM requests, read `.studio/skills/*.md`.
- Append matching skill content to the LLM `System` prompt based on agent role or active task.

## 5. Visual Board Rendering (`studio/desktop/src/ai_manager.rs`)

Rewrite rendering functions to display real-time progress:
- **Task Board**: Renders the checklist of workflow steps, active steps with progress spinners, and nested child subagents.
- **Live Activity**: Renders active subagents, their current action (e.g. `compiling...`), and their last terminal output line.
