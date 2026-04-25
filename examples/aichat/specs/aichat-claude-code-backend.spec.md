spec: task
name: "AI Chat Claude Code Backend"
tags: [makepad, aichat, claude-code, backend, agent]
---

## Intent

Add Claude Code as a selectable `makepad-example-aichat` backend through the existing
`makepad_ai::Agent` abstraction. The first implementation must use Claude CLI's
headless stream-json mode rather than terminal automation, so aichat can receive
structured streaming text without depending on terminal screen scraping.

## Decisions

- Backend name: `Claude Code`.
- V1 backend mode: headless Claude CLI using `claude --print --output-format stream-json`.
- V1 may spawn one Claude process per prompt; a long-lived process is not required.
- V1 must not use `tmux`; tmux control is explicitly deferred to a separate backend.
- Availability checks `CLAUDE_CODE_BIN` first, then `claude` in `PATH`.
- Configuration environment variables:
  - `CLAUDE_CODE_BIN`
  - `CLAUDE_CODE_MODEL`
  - `CLAUDE_CODE_PERMISSION_MODE`
- Default permission mode is conservative; do not pass bypass/dangerous permission flags by default.
- The backend implements `makepad_ai::Agent`, not the lower-level `AiBackend`.
- The implementation lives under `libs/makepad_ai/src/backends/`.
- aichat integration is limited to adding a new `BackendType` variant, dropdown label, detection path, and creation path.

## Boundaries

### Allowed Changes

- libs/makepad_ai/src/backends/**
- libs/makepad_ai/src/agent.rs
- libs/makepad_ai/src/lib.rs
- libs/makepad_ai/Cargo.toml
- examples/aichat/src/main.rs
- examples/aichat/specs/**
- docs/research/aichat-claude-code-backend.md
- docs/superpowers/plans/**

### Forbidden

- Do not implement tmux control in this task.
- Do not require tmux to be installed for `Claude Code` backend availability.
- Do not add a webview, shell UI embedding, or terminal emulator widget.
- Do not change existing `ClaudeAcp`, `ClaudeApi`, `Gemini`, `OpenAi`, or `Moonshot` behavior.
- Do not pass `--dangerously-skip-permissions`, `--allow-dangerously-skip-permissions`, or `--permission-mode bypassPermissions` by default.
- Do not make live tests require real Claude network/auth.

### Out of Scope

- Interactive Claude Code TUI control.
- Claude Code `--tmux --worktree` integration.
- Tool-use bridging from Claude Code into Makepad tools.
- Multi-agent Claude Code workflows.
- Full terminal rendering inside aichat.
- UI redesign unrelated to adding the backend selector and status label.

## Acceptance Criteria

Scenario: Claude Code availability detects configured binary
  Test:
    Package: makepad-ai
    Filter: claude_code_cli_detects_configured_binary
  Level: unit
  Test Double: fake environment and fake executable lookup
  Given `CLAUDE_CODE_BIN` points to an executable fake Claude binary
  When `ClaudeCodeCliAgent::is_available()` is evaluated through the test hook
  Then the backend is reported as available
  And no tmux executable is required

Scenario: Claude Code availability falls back to PATH
  Test:
    Package: makepad-ai
    Filter: claude_code_cli_detects_path_binary
  Level: unit
  Test Double: fake environment and fake PATH lookup
  Given `CLAUDE_CODE_BIN` is unset
  And a fake `claude` executable is discoverable in the injected test PATH
  When availability is evaluated
  Then the backend is reported as available

Scenario: Claude Code unavailable when no CLI exists
  Test:
    Package: makepad-ai
    Filter: claude_code_cli_unavailable_without_binary
  Level: unit
  Test Double: fake environment and fake PATH lookup
  Given `CLAUDE_CODE_BIN` is unset
  And the injected test PATH contains no `claude`
  When availability is evaluated
  Then the backend is not reported as available

Scenario: Claude Code command uses headless stream-json mode
  Test:
    Package: makepad-ai
    Filter: claude_code_cli_builds_headless_command
  Given a session config with cwd, system prompt, and model
  When the backend builds the Claude command
  Then the command contains `--print`
  And the command contains `--output-format stream-json`
  And the command does not contain `--tmux`
  And the command does not contain dangerous bypass permission flags

Scenario: Claude Code command honors safe environment configuration
  Test:
    Package: makepad-ai
    Filter: claude_code_cli_builds_command_from_env_config
  Level: unit
  Test Double: fake environment map
  Given `CLAUDE_CODE_BIN` is `/tmp/fake-claude`
  And `CLAUDE_CODE_MODEL` is `sonnet`
  And `CLAUDE_CODE_PERMISSION_MODE` is `default`
  When the backend builds the Claude command
  Then the executable path is `/tmp/fake-claude`
  And the command contains `--model sonnet`
  And the command contains `--permission-mode default`
  And the command still contains `--print`
  And the command still contains `--output-format stream-json`

Scenario: Claude Code stream-json text becomes AgentEvent deltas
  Test:
    Package: makepad-ai
    Filter: claude_code_cli_stream_json_text_deltas
  Level: unit
  Test Double: fake stdout JSON lines
  Given fake Claude stdout emits stream-json partial text chunks
  When the backend parser consumes the lines
  Then each text chunk becomes an `AgentEvent::TextDelta`
  And the prompt id is preserved

Scenario: Claude Code stream completion becomes turn complete
  Test:
    Package: makepad-ai
    Filter: claude_code_cli_stream_json_turn_complete
  Level: unit
  Test Double: fake stdout JSON lines
  Given fake Claude stdout emits a final success event
  When the backend parser consumes the line
  Then the backend emits `AgentEvent::TurnComplete`
  And the stop reason is mapped to `StopReason::EndTurn`

Scenario: Claude Code process or JSON failure is surfaced
  Test:
    Package: makepad-ai
    Filter: claude_code_cli_errors_surface_as_prompt_error
  Level: unit
  Test Double: fake stderr / malformed stdout
  Given the Claude process exits non-zero or emits malformed stream-json
  When the backend handles the result
  Then the backend emits `AgentEvent::PromptError`
  And the aichat session remains usable for a later prompt

Scenario: aichat dropdown exposes Claude Code when available
  Test:
    Package: makepad-example-aichat
    Filter: aichat_backend_type_includes_claude_code
  Given `ClaudeCodeCliAgent::is_available()` is true through a test hook
  When available backends are detected
  Then `BackendType::ClaudeCode` appears in the backend list
  And the dropdown labels include `Claude Code`
  And existing backend indices still round-trip through `to_index` and `from_index`

Scenario: aichat creates a Claude Code Agent without changing other backends
  Test:
    Package: makepad-example-aichat
    Filter: aichat_create_claude_code_agent
  Given the `Claude Code` backend is selected
  When aichat creates an agent
  Then the returned agent implements `makepad_ai::Agent`
  And `ClaudeAcp`, `ClaudeApi`, `Gemini`, `OpenAi`, and `Moonshot` creation behavior is unchanged

Scenario: Claude Code backend module is exported through makepad-ai
  Test:
    Package: makepad-ai
    Filter: claude_code_cli_public_export_compiles
  Given `ClaudeCodeCliAgent` is implemented under `libs/makepad_ai/src/backends/`
  When a test imports it through the public `makepad_ai::backends` surface
  Then the import compiles
  And the type can be boxed as `Box<dyn makepad_ai::Agent>`
