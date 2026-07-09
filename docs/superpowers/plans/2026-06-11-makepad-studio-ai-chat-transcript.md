# Makepad Studio AI Chat Transcript Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the AI chat transcript visually separate user and assistant turns with left-aligned IDE-style cards while preserving existing markdown content and compact activity behavior.

**Architecture:** Keep the existing `ai_chat_markdown(agent)` path and `Markdown` widget for this iteration. Change only the generated markdown shape for main user/assistant/system/error messages, using quote/card blocks with role labels instead of bare `### User` / `### Assistant` headings. Preserve compact tool/waiting/thinking behavior already covered by tests.

**Tech Stack:** Rust, Makepad `script_mod!`, existing `Markdown` widget, `cargo test -p makepad-studio`, `cargo check -p makepad-studio`.

---

## Files

- Modify: `studio/desktop/src/ai_manager.rs`
  - Update chat markdown generation helpers.
  - Add regression tests for card-style user/assistant rendering.
- Optionally modify: `studio/desktop/src/app_ui.rs`
  - Only if the generated quote-card markdown needs slightly stronger visual styling through existing `AiChatMarkdown` properties.
  - Do not build a custom native message list in this iteration.

---

### Task 1: Add transcript card regression tests

**Files:**
- Modify: `studio/desktop/src/ai_manager.rs`

- [ ] **Step 1: Read the current test area**

Read `studio/desktop/src/ai_manager.rs` around the existing `ai_chat_markdown_*` tests.

- [ ] **Step 2: Add failing test for role card markdown**

Add this test near the other `ai_chat_markdown` tests:

```rust
#[test]
fn ai_chat_markdown_renders_user_and_assistant_as_cards() {
    let agent = test_agent_state(
        AiAgentId(1),
        "ready",
        false,
        vec![
            AiMessage {
                role: AiMessageRole::User,
                text: "What enhancement should we make?".to_string(),
            },
            AiMessage {
                role: AiMessageRole::Assistant,
                text: "Use transcript cards for clearer separation.".to_string(),
            },
        ],
    );

    let markdown = ai_chat_markdown(&agent);
    assert!(markdown.contains("> **User**"));
    assert!(markdown.contains("> What enhancement should we make?"));
    assert!(markdown.contains("> **Assistant**"));
    assert!(markdown.contains("> Use transcript cards for clearer separation."));
    assert!(!markdown.contains("### User"));
    assert!(!markdown.contains("### Assistant"));
}
```

- [ ] **Step 3: Add failing test that activity stays between cards**

Add this test near `ai_chat_markdown_groups_activity_before_assistant`:

```rust
#[test]
fn ai_chat_markdown_keeps_tool_activity_between_cards() {
    let agent = test_agent_state(
        AiAgentId(1),
        "ready",
        false,
        vec![
            AiMessage {
                role: AiMessageRole::User,
                text: "Inspect the terminal.".to_string(),
            },
            AiMessage {
                role: AiMessageRole::ToolResult,
                text: "`read_terminal` result\n```text\n{}\n```".to_string(),
            },
            AiMessage {
                role: AiMessageRole::Assistant,
                text: "Terminal is idle.".to_string(),
            },
        ],
    );

    let markdown = ai_chat_markdown(&agent);
    let user = markdown.find("> **User**").unwrap();
    let tools = markdown.find("> **Tools**").unwrap();
    let assistant = markdown.find("> **Assistant**").unwrap();
    assert!(user < tools);
    assert!(tools < assistant);
    assert!(markdown.contains("Read terminal"));
}
```

- [ ] **Step 4: Run tests to verify they fail**

Run:

```bash
cargo test -p makepad-studio ai_chat_markdown_
```

Expected: the new role-card assertions fail because current markdown still contains `### User` and `### Assistant` headings.

- [ ] **Step 5: Commit failing tests only if following strict checkpoint workflow**

Normally do not commit failing tests. Continue to Task 2 in the same working tree.

---

### Task 2: Render main chat turns as card-style quote blocks

**Files:**
- Modify: `studio/desktop/src/ai_manager.rs`

- [ ] **Step 1: Add role label helper**

Replace or supersede `ai_main_message_heading` with a role label helper. Use this exact shape:

```rust
fn ai_main_message_label(message: &AiMessage) -> &'static str {
    match message.role {
        AiMessageRole::User => "User",
        AiMessageRole::Assistant => "Assistant",
        AiMessageRole::System => "System",
        AiMessageRole::Error => "Error",
        AiMessageRole::Thinking => "Thinking",
        AiMessageRole::ToolCall | AiMessageRole::ToolResult => "Tool",
    }
}
```

If no callsites need `ai_main_message_heading` after this change, delete `ai_main_message_heading`.

- [ ] **Step 2: Add card append helper**

Add this helper near `ai_main_message_markdown_body`:

```rust
fn append_main_message_markdown(markdown: &mut String, message: &AiMessage, body: &str) {
    if !markdown.is_empty() {
        markdown.push_str("\n\n");
    }
    markdown.push_str("> **");
    markdown.push_str(ai_main_message_label(message));
    markdown.push_str("**");
    for line in body.lines() {
        markdown.push_str("\n> ");
        markdown.push_str(line);
    }
}
```

This intentionally uses markdown quote blocks because `AiChatMarkdown` already gives quote blocks a separate background and accent strip.

- [ ] **Step 3: Update `ai_chat_markdown` main-message branch**

Inside `ai_chat_markdown`, replace this existing sequence:

```rust
let heading = ai_main_message_heading(message);
let body = ai_main_message_markdown_body(message);
if body.is_empty() {
    continue;
}
if !markdown.is_empty() {
    markdown.push_str("\n\n");
}
markdown.push_str(heading);
markdown.push_str("\n\n");
markdown.push_str(&body);
```

with:

```rust
let body = ai_main_message_markdown_body(message);
if body.is_empty() {
    continue;
}
append_main_message_markdown(&mut markdown, message, &body);
```

- [ ] **Step 4: Run focused tests**

Run:

```bash
cargo test -p makepad-studio ai_chat_markdown_
```

Expected: all `ai_chat_markdown_` tests pass except any older tests that still assert `### User` / `### Assistant`.

- [ ] **Step 5: Update old expectations only where behavior changed intentionally**

In `ai_chat_markdown_groups_activity_before_assistant`, replace these assertions:

```rust
assert!(markdown.contains("### User"));
assert!(markdown.contains("### Assistant"));
```

with:

```rust
assert!(markdown.contains("> **User**"));
assert!(markdown.contains("> **Assistant**"));
```

Do not loosen assertions for tool compaction.

- [ ] **Step 6: Run full AI markdown tests**

Run:

```bash
cargo test -p makepad-studio ai_
```

Expected: all AI-related desktop tests pass.

- [ ] **Step 7: Commit production and test change**

Run:

```bash
git add studio/desktop/src/ai_manager.rs
git commit -m "feat: render AI chat turns as transcript cards"
```

---

### Task 3: Tune existing markdown card visual style only if needed

**Files:**
- Modify if needed: `studio/desktop/src/app_ui.rs:21-39`
- Test: visual Studio run if this task changes UI styling.

- [ ] **Step 1: Inspect current `AiChatMarkdown` styling**

Read `studio/desktop/src/app_ui.rs` around `let AiChatMarkdown = Markdown { ... }`.

- [ ] **Step 2: Decide if styling is needed**

If quote blocks already create clear cards, skip this task and do not edit `app_ui.rs`.

If the quote/card blocks are still too flat, make only these small style changes:

```rust
quote_layout: Layout {
    flow: Flow.Right {wrap: true}
    padding: Inset {left: 10.0 right: 10.0 top: 7.0 bottom: 8.0}
}
draw_block +: {
    quote_bg_color: theme.color_bg_highlight * 1.08
    quote_fg_color: theme.color_label_inner_inactive
    code_color: theme.color_bg_highlight
}
```

Keep `paragraph_spacing` at `9.0` unless runtime verification shows cards are too crowded.

- [ ] **Step 3: Run desktop check if edited**

Run:

```bash
cargo check -p makepad-studio
```

Expected: command exits 0. Existing warnings are acceptable.

- [ ] **Step 4: Commit styling change if edited**

Run:

```bash
git add studio/desktop/src/app_ui.rs
git commit -m "style: tune AI transcript card spacing"
```

Skip this commit if no styling edit was needed.

---

### Task 4: Verify behavior and finish

**Files:**
- No expected source edits.

- [ ] **Step 1: Run targeted tests**

Run:

```bash
cargo test -p makepad-studio ai_
```

Expected: all AI-related desktop tests pass.

- [ ] **Step 2: Run desktop compile check**

Run:

```bash
cargo check -p makepad-studio
```

Expected: command exits 0. Existing warnings are acceptable.

- [ ] **Step 3: Runtime UI verification if styling changed**

If `app_ui.rs` changed, verify through Studio remote per `AGENTS.md`:

1. Use an existing Studio instance at `127.0.0.1:8001`.
2. Start one persistent bridge process:

```bash
target/release/cargo-makepad studio --studio=127.0.0.1:8001
```

3. Send `ListBuilds`.
4. If an older `makepad-studio` build exists, send `ClearBuild` for that build id.
5. Send:

```json
{"RunItem":{"mount":"makepad","name":"makepad-studio"}}
```

6. Wait for `BuildStarted` and `AppStarted`.
7. Open/inspect the AI panel and confirm:
   - user and assistant turns are visually separated,
   - activity summaries remain compact,
   - Task Board and Live Activity still render.

If only `ai_manager.rs` changed, tests plus compile check are sufficient.

- [ ] **Step 4: Confirm clean tree**

Run:

```bash
git status --short
```

Expected: no output.

---

## Self-review

Spec coverage:
- Card-style left-aligned turns: Task 2.
- Role labels preserved without bare headings: Task 1 and Task 2.
- Activity rows remain compact and between turns: Task 1 and Task 2.
- Existing observation/event hiding preserved: Task 4 runs full AI tests.
- Optional visual styling: Task 3.

Placeholder scan: no TBD/TODO placeholders remain.

Type consistency:
- Uses existing `AiMessage`, `AiMessageRole`, `AiAgentState`, and `ai_chat_markdown` names.
- New helper names are local to `studio/desktop/src/ai_manager.rs`.
