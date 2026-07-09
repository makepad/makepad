# Spec: Makepad Studio AI Visibility Upgrade

## Goal
Make Makepad Studio's AI workflow debuggable in real time by clearly showing what the manager and subagents are doing, which workflow step owns the work, which terminal is active, what output was last seen, and which files were touched.

## Scope
This is a visibility-first upgrade. It should not fundamentally change workflow semantics. It should make the existing workflow engine legible and easier to debug.

## Primary UX Direction
Use a visibility-first design:
- **Task Board** shows current structured execution state.
- **Live Activity** shows recent transitions/events.
- A lightweight **agent detail strip** shows the currently relevant terminal/output/file context.

## Hub State Additions
Extend per-agent visibility state to include:
- `current_action`
- `active_terminal_path`
- `active_terminal_title`
- `last_terminal_excerpt`
- `files_touched`
- `state_changed_at`
- `workflow_step_name`
- `workflow_step_status`
- `blocked_reason` when waiting for input, blocked by error, or blocked on decision

Add a bounded per-mount event ring with recent events such as:
- `agent_started`
- `step_activated`
- `terminal_attached`
- `terminal_needs_input`
- `terminal_done`
- `file_touched`
- `agent_done`
- `agent_failed`

The event ring must be bounded to avoid unbounded memory growth.

## Desktop Rendering
### Task Board
Render workflow-first state:
- workflow name
- active step progress
- status for each step
- owning agent under the active step
- nested subagents beneath their owner
- each row shows status marker, title, and current action

### Live Activity
Render recent events newest-first:
- concise, human-readable activity lines
- examples: terminal attached, file touched, waiting for input, step activated, step completed

### Agent Detail Strip
Show short detail for the selected or most relevant active agent:
- active terminal path/title
- last output excerpt
- touched files preview
- blocked/waiting reason

## Debugging Outcomes
The design must make these failure modes obvious:
- workflow exists but no owner agent
- terminal never attached
- terminal is waiting for input
- file changes happened but step did not advance
- wrong agent advanced a workflow step
- deleted owner left stale workflow state

## Verification Scenario
Use a real Studio run with a `.studio/workflows/review-prs.md` workflow and execute `/review-prs 12`.
Verify:
- Task Board shows active step and owner
- Live Activity shows terminal attach, action, file touch, and step transition events
- waiting-for-input state is visible when applicable
- deleting owner clears workflow visibility
- unrelated chat does not inherit workflow state
