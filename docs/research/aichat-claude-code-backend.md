# AI Chat Claude Code Backend Research

Date: 2026-04-24

## Goal

Investigate how to connect Claude Code to `makepad-example-aichat` as a backend,
with a specific request to consider controlling a Claude Code instance through
`tmux`.

## Local Findings

### Claude CLI

`claude` is installed locally:

```text
/Users/zhangalex/.local/bin/claude
```

Relevant `claude --help` capabilities observed:

- `-p`, `--print`: print response and exit.
- `--output-format stream-json`: stream output as JSON.
- `--input-format stream-json`: accept streaming JSON input.
- `--include-partial-messages`: include partial chunks in stream output.
- `--session-id <uuid>`: use a specific session id.
- `--continue`: continue most recent conversation in current directory.
- `--resume [value]`: resume by session id or picker.
- `--system-prompt <prompt>` and `--append-system-prompt <prompt>`.
- `--permission-mode <mode>`.
- `--tools <tools...>`.
- `--tmux`: create a tmux session for the worktree.
- `--worktree [name]`: create a new git worktree for the session.

### tmux

`tmux` is not currently available in PATH:

```text
command -v tmux
```

returned no executable path.

This means a tmux-driven backend is not immediately runnable on this machine
without installing tmux or relying on Claude Code's own `--tmux` behavior, which
itself requires `--worktree`.

### Existing aichat Backend Architecture

`examples/aichat/src/main.rs` uses `makepad_ai::Agent` as the common backend
abstraction.

Relevant files:

- `examples/aichat/src/main.rs`
- `libs/makepad_ai/src/agent.rs`
- `libs/makepad_ai/src/backends/claude_acp.rs`
- `libs/makepad_ai/src/lib.rs`

Current `BackendType` variants:

- `ClaudeCode`
- `ClaudeSplash`
- `ClaudeAcp`
- `ClaudeApi`
- `Gemini`
- `GeminiSplash`
- `OpenAi`
- `Moonshot`

The right integration shape is:

- Add a new backend implementation under `libs/makepad_ai/src/backends/`.
- Export it from `libs/makepad_ai/src/backends/mod.rs`.
- Add a new `BackendType` in `examples/aichat/src/main.rs`.
- Let the new backend implement `Agent`.

## Recommended Architecture

### Recommended V1: Headless Claude Code Backend

Prefer using Claude CLI's stream-json print mode first:

```bash
claude \
  --print \
  --output-format stream-json \
  --verbose \
  --include-partial-messages
```

Observed on Claude Code `2.1.119`:

- `--print --output-format stream-json` fails unless `--verbose` is also set.
- If `ANTHROPIC_API_KEY` is present but invalid, Claude Code uses it ahead of
  local login state and returns `Invalid API key`.
- `--include-partial-messages` emits `stream_event` lines containing
  `content_block_delta` / `text_delta`; final `assistant` summary lines repeat
  the full content and should be ignored to avoid duplicate text.

Why this is better than tmux for V1:

- Structured output is easier to parse than terminal screen state.
- Streaming text can map directly to `AgentEvent::TextDelta`.
- Process lifecycle is explicit.
- No terminal prompt detection.
- No copy-mode scraping.
- No dependency on tmux being installed.
- Easier to test with a fake process runner.

Suggested module:

```text
libs/makepad_ai/src/backends/claude_code_cli.rs
```

Suggested type:

```rust
pub struct ClaudeCodeCliAgent { ... }
```

Behavior:

- `is_available()` checks `CLAUDE_CODE_BIN` or `claude` in PATH.
- `create_session()` records cwd/system prompt/model/options and emits `SessionReady`.
- `send_prompt()` spawns or communicates with `claude`.
- Streaming JSON chunks become `AgentEvent::TextDelta`.
- Completion becomes `AgentEvent::TurnComplete`.
- CLI/process errors become `AgentEvent::PromptError`.

Open design choice:

- Either spawn one `claude -p` process per prompt with `--session-id`, or keep one
  long-lived stream-json process.
- Per-prompt process is simpler and more robust for V1.
- Long-lived stream-json is closer to an interactive session but needs more protocol work.

### Experimental V2: tmux-Controlled Claude Code Backend

A tmux backend would control an interactive Claude Code UI through terminal
automation.

Likely operations:

```bash
tmux new-session -d -s aichat-claude-code 'claude'
tmux send-keys -t aichat-claude-code '<prompt>' Enter
tmux capture-pane -pt aichat-claude-code
tmux send-keys -t aichat-claude-code C-c
tmux kill-session -t aichat-claude-code
```

Risks:

- tmux is not installed locally right now.
- Terminal UI output is not a stable protocol.
- Prompt-ready detection is brittle.
- Streaming deltas require diffing captured pane snapshots.
- ANSI escape handling is required.
- Copying multiline prompts safely requires paste-buffer or bracketed paste.
- Cancellation can leave Claude Code in a half-finished state.
- It is harder to test deterministically.

Conclusion:

tmux should not be the first implementation unless the explicit goal is to
drive the actual interactive Claude Code TUI. For a backend inside a chat UI,
the headless CLI stream-json mode is the better first target.

## Proposed Backend Contract

Add a new aichat backend label:

```text
Claude Code
```

Availability:

- Available if `CLAUDE_CODE_BIN` points to an executable, or `claude` exists in PATH.
- If using the tmux variant, also require `tmux` in PATH.

Session config:

- Use current repo cwd by default.
- Pass `system_prompt` through `--append-system-prompt` or `--system-prompt`.
- Use `--model` only if explicitly configured.
- Use conservative permission mode by default; do not bypass permissions silently.

Environment variables:

```text
CLAUDE_CODE_BIN=/Users/zhangalex/.local/bin/claude
CLAUDE_CODE_MODEL=sonnet
CLAUDE_CODE_PERMISSION_MODE=default
CLAUDE_CODE_USE_TMUX=0
```

V1 should ignore `CLAUDE_CODE_USE_TMUX=1` unless the tmux backend is explicitly implemented.

## Implementation Plan Sketch

### Phase 1: Headless Claude CLI Spike

- Add `ClaudeCodeCliAgent` under `libs/makepad_ai/src/backends/claude_code_cli.rs`.
- Implement a small process wrapper with stdin/stdout channels, similar to `claude_acp.rs`.
- Parse stream-json output enough to extract partial text and completion.
- Add tests using fake JSON lines, not a real Claude process.
- Add `BackendType::ClaudeCode` in aichat.
- Add it to dropdown labels and backend detection.

### Phase 2: Manual Verification

Run:

```bash
cargo test -p makepad-ai
cargo check -p makepad-example-aichat
RUST_BACKTRACE=1 cargo run -p makepad-example-aichat
```

Manual prompt:

```text
请用 markdown 简短解释 Rust 状态机，并给一个 state diagram。
```

Expected:

- Backend dropdown shows `Claude Code`.
- Selecting it creates a session.
- Sending a prompt streams text into the chat.
- Diagram fences still render through `DiagramView`.
- Prompt errors stay visible and do not break the UI.

### Phase 3: tmux Backend Only If Still Needed

Before implementing tmux:

- Install/verify `tmux`.
- Decide whether to use classic tmux or Claude Code `--tmux --worktree`.
- Define a deterministic prompt-ready marker.
- Define output scraping and ANSI cleanup.
- Add a kill/recover flow.

Suggested separate module:

```text
libs/makepad_ai/src/backends/claude_code_tmux.rs
```

Do not mix tmux and headless CLI logic in the same backend struct.
