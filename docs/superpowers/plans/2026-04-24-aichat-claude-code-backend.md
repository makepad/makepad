# AI Chat Claude Code Backend Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a selectable `Claude Code` backend to `makepad-example-aichat` through `makepad_ai::Agent`, using Claude CLI headless stream-json mode for V1.

**Architecture:** Implement a new session-based `ClaudeCodeCliAgent` under `libs/makepad_ai/src/backends/` and keep tmux out of scope. The backend should be testable without invoking real Claude by splitting availability/command construction and stream-json parsing into pure helper functions. aichat only adds a backend enum variant, label, detection, and creation path.

**Tech Stack:** Rust 2021, `makepad-ai`, `makepad_micro_serde`, `std::process::Command`, `std::sync::mpsc`, `makepad_widgets::Cx`, existing `Agent`/`AgentEvent` abstractions, `cargo test`, `cargo check`.

---

## File Map

- `libs/makepad_ai/src/backends/claude_code_cli.rs`: new backend implementation, command builder, availability helper, stream-json parser, and unit tests.
- `libs/makepad_ai/src/backends/mod.rs`: module declaration and public re-export.
- `libs/makepad_ai/src/lib.rs`: no change expected unless public exports require adjustment.
- `libs/makepad_ai/Cargo.toml`: no new dependency expected; change only if unavoidable.
- `examples/aichat/src/main.rs`: add `BackendType::ClaudeCode`, dropdown label, availability detection, agent creation, status label, and tests.
- `examples/aichat/specs/aichat-claude-code-backend.spec.md`: source contract; do not weaken during implementation.
- `docs/research/aichat-claude-code-backend.md`: research note; update only if implementation discovers a materially different CLI behavior.

## Implementation Notes

- Do not implement tmux in this task.
- Do not call real Claude in tests.
- Keep tests deterministic with fake env maps, fake executable lookup, and fake stdout/stderr lines.
- Use `makepad_micro_serde` if JSON parsing is simple enough; otherwise start with narrow string extraction helpers instead of adding dependencies.
- V1 can spawn one `claude -p` process per prompt. This is simpler than a long-lived stream-json stdin protocol and satisfies the spec.
- The command must never include dangerous permission bypass flags unless a later explicit task changes the contract.
- The stream-json schema from Claude Code may evolve; parser should accept multiple plausible text shapes and ignore unknown lines rather than panic.

### Task 1: Add ClaudeCodeCliAgent Skeleton And Public Export

**Files:**
- Create: `libs/makepad_ai/src/backends/claude_code_cli.rs`
- Modify: `libs/makepad_ai/src/backends/mod.rs`

- [ ] **Step 1: Add a failing public export test**

Create `libs/makepad_ai/src/backends/claude_code_cli.rs` with a test module containing:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::Agent;

    #[test]
    fn claude_code_cli_public_export_compiles() {
        let agent = ClaudeCodeCliAgent::new();
        let _boxed: Box<dyn Agent> = Box::new(agent);
    }
}
```

This will not compile until `ClaudeCodeCliAgent` exists and implements `Agent`.

- [ ] **Step 2: Add module export**

Modify `libs/makepad_ai/src/backends/mod.rs`:

```rust
pub mod claude_code_cli;

pub use claude_code_cli::ClaudeCodeCliAgent;
```

- [ ] **Step 3: Implement minimal struct and Agent trait**

Add:

```rust
use crate::agent::*;
use crate::types::*;
use makepad_widgets::*;
use std::collections::{HashMap, VecDeque};

pub struct ClaudeCodeCliAgent {
    sessions: HashMap<LiveId, ClaudeCodeSession>,
    pending_events: VecDeque<AgentEvent>,
}

struct ClaudeCodeSession {
    ready: bool,
    config: SessionConfig,
    current_prompt: Option<PromptId>,
}

impl ClaudeCodeCliAgent {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            pending_events: VecDeque::new(),
        }
    }
}
```

Then implement `Agent` with:

- `create_session`: insert session, push `SessionReady`.
- `send_prompt`: create `PromptId`, store current prompt, for now push `PromptError` saying not implemented.
- `send_tool_result`: no-op.
- `cancel_prompt`: clear matching current prompt.
- `handle_event`: drain `pending_events`.
- `is_session_ready`: true if session exists and ready.

- [ ] **Step 4: Run focused compile test**

Run:

```bash
cargo test -p makepad-ai claude_code_cli_public_export_compiles -- --nocapture
```

Expected: PASS.

### Task 2: Availability And Command Builder

**Files:**
- Modify: `libs/makepad_ai/src/backends/claude_code_cli.rs`

- [ ] **Step 1: Add testable env/lookup helpers**

Add internal structs:

```rust
#[derive(Debug, Clone, Default)]
struct ClaudeCodeEnv {
    bin: Option<String>,
    model: Option<String>,
    permission_mode: Option<String>,
    path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ClaudeCommandSpec {
    program: String,
    args: Vec<String>,
}
```

Add helper signatures:

```rust
fn find_claude_binary<F>(env: &ClaudeCodeEnv, is_executable: F) -> Option<String>
where
    F: Fn(&str) -> bool;

fn build_claude_command(env: &ClaudeCodeEnv, config: &SessionConfig, prompt: &str) -> ClaudeCommandSpec;
```

- [ ] **Step 2: Write availability tests**

Add tests:

```rust
#[test]
fn claude_code_cli_detects_configured_binary() { ... }

#[test]
fn claude_code_cli_detects_path_binary() { ... }

#[test]
fn claude_code_cli_unavailable_without_binary() { ... }
```

Use fake closures for executable lookup. Do not touch real env or filesystem.

- [ ] **Step 3: Implement availability helpers**

Rules:

- If `env.bin` is Some and executable, return it.
- Else scan `env.path` path entries for `/claude`.
- Else return None.

Add public runtime method:

```rust
pub fn is_available() -> bool {
    let env = ClaudeCodeEnv::from_process_env();
    find_claude_binary(&env, |path| std::path::Path::new(path).is_file()).is_some()
}
```

- [ ] **Step 4: Write command-builder tests**

Add:

```rust
#[test]
fn claude_code_cli_builds_headless_command() { ... }

#[test]
fn claude_code_cli_builds_command_from_env_config() { ... }
```

Assertions:

- program path honors `CLAUDE_CODE_BIN`.
- args include `--print`.
- args include `--output-format stream-json`.
- args include `--include-partial-messages`.
- args include `--model <model>` only when configured.
- args include `--permission-mode <mode>` only for safe modes such as `default`, `acceptEdits`, `dontAsk`, `plan`, `auto`.
- args never include `--tmux`.
- args never include dangerous bypass flags.

- [ ] **Step 5: Implement command builder**

Build a per-prompt command spec:

```rust
ClaudeCommandSpec {
    program,
    args: vec![
        "--print".into(),
        "--output-format".into(),
        "stream-json".into(),
        "--include-partial-messages".into(),
        "--append-system-prompt".into(),
        system_prompt,
        prompt.into(),
    ],
}
```

Only include `--append-system-prompt` if session config has a non-empty system prompt.

- [ ] **Step 6: Run focused tests**

Run:

```bash
cargo test -p makepad-ai claude_code_cli_detects -- --nocapture
cargo test -p makepad-ai claude_code_cli_builds -- --nocapture
```

Expected: PASS.

### Task 3: Stream-JSON Parser And Error Mapping

**Files:**
- Modify: `libs/makepad_ai/src/backends/claude_code_cli.rs`

- [ ] **Step 1: Add parser event enum**

Add:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
enum ClaudeStreamEvent {
    Text(String),
    Done,
    Error(String),
    Ignored,
}
```

Add function:

```rust
fn parse_claude_stream_line(line: &str) -> ClaudeStreamEvent
```

- [ ] **Step 2: Write parser tests**

Add tests:

```rust
#[test]
fn claude_code_cli_stream_json_text_deltas() { ... }

#[test]
fn claude_code_cli_stream_json_turn_complete() { ... }

#[test]
fn claude_code_cli_errors_surface_as_prompt_error() { ... }
```

Use representative fake lines. Include at least these shapes:

```json
{"type":"assistant","message":{"content":[{"type":"text","text":"hello"}]}}
{"type":"content_block_delta","delta":{"type":"text_delta","text":" world"}}
{"type":"result","subtype":"success"}
{"type":"error","message":"boom"}
```

- [ ] **Step 3: Implement tolerant parsing**

Implementation can use simple string extraction helpers initially:

- If line contains `"type":"error"`, extract `"message"` if present and return `Error`.
- If line contains `"type":"result"` and `"success"`, return `Done`.
- If line contains `"text_delta"` and `"text":"..."`, return `Text`.
- If line contains `"type":"text"` and `"text":"..."`, return `Text`.
- Otherwise return `Ignored`.

Keep parsing non-panicking. Malformed JSON-like lines should return `Error` only if they look like failed output; otherwise `Ignored`.

- [ ] **Step 4: Map parser events to AgentEvent**

Add helper:

```rust
fn stream_event_to_agent_event(prompt_id: PromptId, event: ClaudeStreamEvent) -> Option<AgentEvent>
```

Mapping:

- `Text(t)` -> `AgentEvent::TextDelta { prompt_id, text: t }`
- `Done` -> `AgentEvent::TurnComplete { prompt_id, stop_reason: StopReason::EndTurn }`
- `Error(e)` -> `AgentEvent::PromptError { prompt_id, error: e }`
- `Ignored` -> None

- [ ] **Step 5: Run focused parser tests**

Run:

```bash
cargo test -p makepad-ai claude_code_cli_stream_json -- --nocapture
cargo test -p makepad-ai claude_code_cli_errors_surface_as_prompt_error -- --nocapture
```

Expected: PASS.

### Task 4: Spawn Claude CLI Per Prompt

**Files:**
- Modify: `libs/makepad_ai/src/backends/claude_code_cli.rs`

- [ ] **Step 1: Add process result plumbing**

Use a worker thread per prompt similar to `claude_acp.rs`:

- Spawn `Command::new(spec.program).args(spec.args)`.
- Set current directory from `SessionConfig.cwd` if present.
- Capture stdout/stderr.
- Read stdout lines.
- Convert lines to `ClaudeStreamEvent`.
- Send parsed events back through `mpsc`.
- On process exit with non-zero status, send `PromptError` with stderr.

- [ ] **Step 2: Store receiver on backend**

Extend `ClaudeCodeCliAgent`:

```rust
stdout_receiver: Option<std::sync::mpsc::Receiver<(PromptId, ClaudeStreamEvent)>>,
```

or keep a vector of receivers if multiple prompts can overlap. V1 may support one prompt at a time per backend; reject overlapping prompts with `PromptError`.

- [ ] **Step 3: Implement `send_prompt`**

Rules:

- Create `PromptId`.
- If session is missing, return prompt id and push `PromptError`.
- If binary unavailable, push `PromptError`.
- Build command from env/config/prompt.
- Spawn process worker.
- Store current prompt.
- Return prompt id.

- [ ] **Step 4: Implement `handle_event` receiver drain**

In `handle_event`, drain queued process events and map them to `AgentEvent`.

When `Done` or `Error` occurs:

- clear session current prompt.
- emit corresponding final event.

- [ ] **Step 5: Implement cancellation**

V1 minimum:

- Clear current prompt.
- If child kill handle is not practical yet, mark cancellation as best-effort and emit a `PromptError` saying cancelled.

Preferred:

- Store `Child` in worker control and kill it on cancel.

- [ ] **Step 6: Run makepad-ai tests**

Run:

```bash
cargo test -p makepad-ai claude_code_cli -- --nocapture
cargo test -p makepad-ai
```

Expected: PASS. No test should require real Claude auth/network.

### Task 5: Wire Backend Into aichat

**Files:**
- Modify: `examples/aichat/src/main.rs`

- [ ] **Step 1: Add BackendType variant and label**

Add:

```rust
ClaudeCode,
```

Update `ALL_BACKENDS` length and contents.

Update labels in `backend_dropdown`:

```rust
labels: ["Claude Code" "Claude Splash" ...]
```

Keep enum order and dropdown order consistent.

- [ ] **Step 2: Update BackendType methods**

Update:

- `status_label`: `Active: Claude Code`
- `system_prompt`: use the same non-Splash diagram-capable prompt as Moonshot/OpenAI.
- `to_index` / `from_index`: should work automatically through `ALL_BACKENDS`.

- [ ] **Step 3: Update detection and creation**

In `detect_available_backends()`:

```rust
if ClaudeCodeCliAgent::is_available() {
    available_backends.push(BackendType::ClaudeCode);
}
```

In `create_agent()`:

```rust
BackendType::ClaudeCode => ClaudeCodeCliAgent::is_available()
    .then(|| Box::new(ClaudeCodeCliAgent::new()) as Box<dyn Agent>),
```

- [ ] **Step 4: Add aichat tests**

Add tests:

```rust
#[test]
fn aichat_backend_type_includes_claude_code() { ... }

#[test]
fn aichat_create_claude_code_agent() { ... }
```

If direct availability hook injection is too invasive, make tests cover enum/label/index round-trip and public constructor type. Do not depend on real `claude`.

- [ ] **Step 5: Run aichat tests/check**

Run:

```bash
cargo test -p makepad-example-aichat aichat_backend_type_includes_claude_code -- --nocapture
cargo test -p makepad-example-aichat aichat_create_claude_code_agent -- --nocapture
cargo check -p makepad-example-aichat
```

Expected: PASS.

### Task 6: Final Verification And Manual Smoke

**Files:**
- Modify if needed: `docs/research/aichat-claude-code-backend.md`

- [ ] **Step 1: Run spec verification**

Run:

```bash
agent-spec parse examples/aichat/specs/aichat-claude-code-backend.spec.md
agent-spec lint examples/aichat/specs/aichat-claude-code-backend.spec.md --min-score 0.7
```

Expected: parse succeeds, lint has no issues.

- [ ] **Step 2: Run Rust verification**

Run:

```bash
cargo test -p makepad-ai
cargo test -p makepad-example-aichat
cargo check -p makepad-example-aichat
```

Expected: PASS.

- [ ] **Step 3: Manual CLI availability smoke**

If `claude` is installed, run:

```bash
claude --version
```

Expected: prints a version and exits.

Do not run a real prompt unless the user explicitly asks, because that can use auth/network/quota.

- [ ] **Step 4: Manual GUI smoke**

Run:

```bash
RUST_BACKTRACE=1 cargo run -p makepad-example-aichat
```

Expected:

- If `claude` is available, dropdown includes `Claude Code`.
- Selecting `Claude Code` changes status to `Active: Claude Code`.
- Existing backends still appear when their keys/tools are available.
- No startup abort, no script property errors.

## Execution Order

1. Task 1: public skeleton/export.
2. Task 2: availability and command builder.
3. Task 3: stream parser.
4. Task 4: process execution.
5. Task 5: aichat integration.
6. Task 6: full verification/manual smoke.

## Known Risks

- Claude Code stream-json line shape may differ from the fake fixtures. Keep parser tolerant and update research notes if real output differs.
- `makepad_micro_serde::JsonValue` may be enough, but narrow string extraction may be faster to implement. Avoid new dependencies unless unavoidable.
- A per-prompt process is simpler but may be slower than a persistent process. This is acceptable for V1.
- Cancellation may require a stronger process handle design. If robust cancellation grows scope, ship a safe best-effort V1 and document the gap.

