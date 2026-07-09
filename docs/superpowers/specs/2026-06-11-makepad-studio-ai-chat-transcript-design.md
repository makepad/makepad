# Makepad Studio AI Chat Transcript Design

## Problem

The AI chat transcript currently renders user and assistant turns as plain Markdown headings (`User`, `Assistant`) followed by body text. In the Studio AI panel this makes adjacent turns blend together, especially when assistant messages are long. The transcript also shares visual language with Task Board and Live Activity markdown, so the main conversation does not read as a distinct chat surface.

Recent compaction work moved terminal observations/events out of chat and shortened tool/thinking noise. The remaining issue is visual separation of actual conversation turns.

## Goal

Make user and assistant messages clearly distinguishable while keeping Studio's IDE-like tone and without inventing unsupported agent features.

## Non-goals

- No new agent behavior.
- No new workflow semantics.
- No message actions such as retry, copy, collapse, or inspect in this change.
- No right-aligned consumer-chat layout for now.
- No replacement of the Markdown renderer with a full custom rich text system in this change.

## Chosen Direction

Use a left-aligned IDE transcript card layout.

Each user and assistant turn renders as a distinct card-like block:

- All messages remain left-aligned.
- Role label moves into the card chrome instead of being a markdown heading in the message body.
- User and assistant cards use different but subtle surfaces/accent bars.
- Assistant cards keep enough width for readable technical text.
- User cards can be visually tighter but still left-aligned.
- Activity/tool/waiting rows remain compact separators between message cards.
- Terminal observations and task events stay out of chat and remain visible through Live Activity.

This preserves the IDE transcript feel while giving the eye strong boundaries between turns.

## Rendering Strategy

### Initial implementation path

Keep the existing `ai_chat_markdown(agent)` data path and existing `Markdown` widget for the first implementation. Encode message cards in generated markdown using quote/card blocks and role-specific labels.

The first version should avoid custom widget row state and PortalList complexity. It should produce better separation with low behavioral risk and remain easy to regression test.

Expected generated shape:

```markdown
> **User**
>
> What enhancement do you think we can do?

> **Assistant**
>
> A strong UI/UX enhancement would be...
```

The exact markdown may use small role markers or additional separators if needed by the current renderer, but it must keep the body content readable and avoid large empty fenced blocks.

### Follow-up path

If the markdown-card approach is not visually strong enough, evolve to a hybrid native row:

- native row/card container for each message
- embedded markdown only for message body
- compact native activity rows between messages

That is intentionally deferred until the simple card approach is evaluated in Studio.

## Visual Rules

### User turn

- Label: `User`
- Surface: slightly distinct from assistant, not bright
- Accent: stronger or cooler left accent
- Width: left aligned, can be visually compact but must not truncate normal prompts

### Assistant turn

- Label: `Assistant`
- Surface: subtle container background
- Accent: softer left accent
- Width: comfortable for long technical text
- Markdown body keeps paragraphs, lists, inline code, and code blocks

### Activity rows

- Tools remain compact inline summaries.
- Waiting remains visible but compact.
- Empty placeholder thinking remains hidden.
- Real thinking text may still render when useful, but should not dominate the transcript.

## Testing

Add regression tests around `ai_chat_markdown`:

1. User and assistant turns are rendered as card blocks, not `### User` / `### Assistant` headings.
2. User and assistant labels are still present.
3. Message body text is preserved.
4. Tool summaries still appear between message cards.
5. Hidden terminal observations/events remain hidden from chat.

## Runtime Verification

After implementation, verify in Studio with a real AI panel transcript:

1. Start Studio through the Studio remote flow if runtime/UI verification is needed.
2. Open the AI panel with existing conversation messages.
3. Confirm user and assistant turns have clear card boundaries.
4. Confirm activity summaries remain compact.
5. Confirm Task Board and Live Activity are unaffected.

## Acceptance Criteria

- Main chat no longer relies on bare `### User` / `### Assistant` markdown headings.
- User and assistant messages are visually separable in the AI panel.
- Existing markdown content in assistant replies remains readable.
- Tool/thinking/observation compaction behavior from previous fixes does not regress.
- Targeted tests and `cargo check -p makepad-studio` pass.
