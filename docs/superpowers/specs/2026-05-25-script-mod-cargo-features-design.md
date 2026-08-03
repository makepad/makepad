# `script_mod!` Cargo-Feature Gating — Design

**Status:** Draft for review
**Date:** 2026-05-25
**Branch:** `dev`

## Problem

The `script_mod!` proc-macro (defined at `platform/script/derive/src/script.rs:9`) embeds a Makepad-DSL block verbatim into the compiled binary. Today the body is captured as a single whitespace-preserved string and stored as `ScriptMod { code: String, … }` (`platform/script/src/vm.rs:43`). There is no way to vary the DSL based on Cargo features, so projects that conditionally include UI panels or platform-specific widgets must duplicate whole `script_mod!` blocks behind `#[cfg]` modules — which doesn't even compose cleanly because the cargo_makepad watcher does string-level scanning for `script_mod!` invocations (`tools/cargo_makepad/src/wasm/compile.rs:1247`).

We want a way to write **a single `script_mod!` block** whose contents include `#[cfg(feature = "…")]`-gated sections that select different DSL at compile time, while keeping the live-reload (`platform/src/live_reload.rs`) pipeline working.

## Goals

1. Inside a single `script_mod! { … }` block, allow top-level `#[cfg(...)]` attributes that gate the next statement.
2. Support `feature = "x"` plus the combinators `not(...)`, `any(...)`, `all(...)`. Reject other predicates with a clear `compile_error!`.
3. Keep hot-reload (`live_reload.rs`) working for cfg-gated blocks, including across feature flips at rebuild time.
4. Zero behaviour change and zero overhead for `script_mod!` blocks that contain no `#[cfg]` attributes (the common case).

## Non-goals

- Nested `#[cfg]` inside DSL struct bodies (e.g. inside `Root { … }`). v1 only handles attributes at the *top level* of the macro body.
- Predicates other than feature flags (`target_os`, `target_arch`, `debug_assertions`, …). Rejected at the macro with a helpful message.
- Hot-reload that survives structural changes (adding/removing a `#[cfg]` attribute). Such edits require a rebuild; hot-reload skips the affected site with a clear message.
- A custom cfg evaluator inside the proc-macro. We let rustc do the evaluation via emitted `#[cfg(...)]` Rust attributes.

## Surface syntax

Inside `script_mod! { … }`, a `#[cfg(...)]` attribute at the top level applies to **the next statement**, which is one of:

**Form is decided by the very first non-whitespace token after the attribute's closing `]`:**

1. **Brace-grouped block** — leading token is `{`. `#[cfg(...)] { … multiple DSL statements … }`. The outer braces are stripped from the emitted DSL; the contents are included only when the cfg evaluates true. Use this form whenever the gated content spans more than one logical line, or when grouping multiple statements is desired.
2. **Single statement** — any other leading token. The cfg's scope runs from that first token to whichever of these comes first:
   - the closing `}` of the first top-level `{ … }` group encountered (e.g. `Name = … { … }`, `name(args) do #(...) { … }`), or
   - the next newline at depth zero — i.e. a newline that is not inside any `(`, `[`, or `{` — when no brace group appears before that newline. This handles single-line `use mod.foo.*` and `let X = 36.0` statements.

   **Rejection rule (the contract):** any single-statement cfg scope that consumes zero brace groups *and* spans more than one source line is rejected with a `compile_error!` instructing the user to wrap the gated content in `{ … }`. This is the rule the proc-macro implements. (A smarter heuristic — peek at the next fragment to detect a split `use` path — was considered but deferred; the conservative rule is sufficient and unambiguous.)

`#[cfg(...)]` is recognised **only at the top level** of the macro body. Anywhere else (nested inside a brace, paren, or bracket group) it is passed through as plain DSL tokens, which the DSL parser will reject — no silent failure.

**Only outer attributes are recognised.** Inner attributes `#![cfg(...)]` are not supported; the macro emits `compile_error!("script_mod! does not accept inner attributes; use #[cfg(...)] not #![cfg(...)]")`. `#[cfg_attr(...)]` is not supported and produces `compile_error!("script_mod! does not support cfg_attr; use #[cfg(...)] directly")`. Other attribute shapes (`#[doc = …]`, `#[derive(…)]`, etc.) at the top level are rejected with `compile_error!("script_mod! only accepts #[cfg(...)] attributes")`.

Allowed cfg predicates: `feature = "<lit>"`, `not(<expr>)`, `any(<expr>, …)`, `all(<expr>, …)`. Anything else is a compile error.

**Statement-separation note:** two single-statement `use` lines on the same source line, separated only by spaces, will both be swallowed into a single cfg scope under the rules above. This is uncommon and the workaround is obvious (put them on separate lines, or use the brace-grouped form). We accept this edge case rather than introducing per-DSL-statement tokenisation in the proc-macro.

### Example

```rust
script_mod! {
    use mod.prelude.widgets.*

    #[cfg(feature = "ai")]
    use mod.ai_widgets.*

    #[cfg(feature = "ai")]
    load_all_resources() do #(App::script_component(vm)) {
        ui: Root { main_window := AppUI { ai_panel := AiPanel {} } }
    }

    #[cfg(not(feature = "ai"))]
    load_all_resources() do #(App::script_component(vm)) {
        ui: Root { main_window := AppUI {} }
    }
}
```

## Architecture

Three components change. Boundaries are deliberately small so the no-cfg path stays trivial.

```
┌────────────────────────────────────────────┐
│ platform/script/derive/src/script.rs       │   proc-macro
│  - parse top-level cfg attrs               │
│  - validate predicates                     │
│  - emit cfg-aware code/values/cfg_fragments│
└────────────────────┬───────────────────────┘
                     │ generated Rust code (uses rustc's #[cfg])
                     ▼
┌────────────────────────────────────────────┐
│ platform/script/src/vm.rs                  │   data model
│  - ScriptMod gains `cfg_fragments: Vec<bool>`│
└────────────────────┬───────────────────────┘
                     │ at compiled-site init
                     ▼
┌────────────────────────────────────────────┐
│ platform/src/live_reload.rs                │   hot-reload
│  - extractor parses top-level cfg attrs    │
│  - filters fragments using site.cfg_fragments│
└────────────────────────────────────────────┘
```

`tools/cargo_makepad/src/wasm/compile.rs` is **unchanged**. Its extractor only identifies presence of `script_mod!` and detects body changes; it doesn't need to evaluate cfg.

## Component design

### 1. Proc-macro (`platform/script/derive/src/script.rs`)

`script_impl` is upgraded as follows.

**Step 1 — parse fragments.** Walk the top-level token stream once, producing a `Vec<Fragment>`:

```rust
enum Fragment {
    Unconditional { tokens: TokenStream, span: Span },
    Conditional   { cfg_expr: TokenStream, tokens: TokenStream, span: Span },
}
```

When the walker sees `# [ cfg ( … ) ]` at depth zero (a `Punct '#'` followed by a bracketed group whose first ident is `cfg` followed by a parenthesised group), it captures `cfg_expr` (the contents of the parens) and switches to scope-collection mode for the next statement. Statement boundaries follow the surface-syntax rules above:
- If the next non-whitespace token starts a brace group → consume it, strip its outer braces, that's the conditional body.
- Otherwise: consume tokens until either (a) the close of the first top-level brace group is consumed, or (b) the next token's span-start line differs from the previous token's span-end line and we are at depth zero (newline boundary).

The walker maintains `last_span_end_line` while collecting so the newline check is local.

**Step 2 — validate cfg predicates.** For each `Conditional`, walk `cfg_expr` once and ensure every leaf is `feature = "<string-literal>"`, every combinator is `not`/`any`/`all`, and parentheses are balanced. On violation, emit `compile_error!("script_mod! only supports cfg(feature = \"…\"), not(...), any(...), all(...); got <token>")` at the offending span.

**Step 3 — emit the builder.** Two emission modes, chosen by whether the parsed body contains any `Conditional`:

- **No-conditional mode (common case).** Emit byte-identical output to today's macro: static `code: <literal>`, flat `values:` list with hardcoded `#(N)` indices baked into the literal, and `cfg_fragments: Vec::new()`. Zero runtime overhead, zero behavioural drift.
- **With-conditional mode.** Every fragment — *including unconditionals* — uses the runtime builder pattern below, with placeholder indices written via `write!` against the live `__values.len()`. Mixing static literals with runtime push_str for unconditionals after an excluded conditional would silently desynchronise `#(N)` references from the actual `__values` index (a real bug — the static literal would bake in an index that assumes the excluded conditional contributed its values, but it didn't). To avoid that, with-conditional mode is uniform: every fragment becomes runtime-built.

In both modes the output is

```rust
ScriptMod {
    cargo_manifest_path: …,
    module_path: …,
    file: …,
    line: …,
    column: …,
    code: __code,
    values: __values,
    cfg_fragments: __cfg_fragments,
}
```

where `__code`, `__values`, `__cfg_fragments` come from a preceding `let` block:

```rust
let (__code, __values, __cfg_fragments) = {
    let mut __code = String::new();
    let mut __values: Vec<ScriptValue> = Vec::new();
    let mut __cfg_fragments: Vec<bool> = Vec::new();

    // For each Fragment, in source order (with-conditional mode):
    //
    //   Unconditional fragment with chunks [text..., placeholder(value_expr), text..., …]:
    //     for each chunk in the fragment:
    //       text chunk:        __code.push_str(<lit-with-no-#(…)-placeholders>);
    //       placeholder chunk: let __i = __values.len();
    //                          __values.push({ <value_expr> }.script_to_value(vm));
    //                          write!(&mut __code, "#({})", __i).unwrap();
    //
    //   Conditional fragment:
    //     __cfg_fragments.push(cfg!(<cfg_expr>));   // outside the guard
    //     #[cfg(<cfg_expr>)]
    //     {
    //         // same chunked emission as unconditional
    //     }

    (__code, __values, __cfg_fragments)
};
```

`cfg!(<cfg_expr>)` is emitted **outside** the `#[cfg(...)]` guard so `cfg_fragments.len()` always matches the number of conditional fragments in source — even for conditionals that resolve false. The hot-reload extractor relies on this invariant.

**Why every fragment is chunked in with-conditional mode** — including unconditionals: the alternative (static `#(N)` literals for unconditionals) breaks if a conditional between two unconditionals is excluded by rustc, because the static `#(N)` baked into the second unconditional would assume the conditional contributed its values when it actually contributed none. Uniformly emitting `write!(__code, "#({})", __values.len())` for *every* placeholder, in every fragment, keeps indices tight and consistent regardless of which conditionals are excluded.

In no-conditional mode the macro emits today's exact shape (string literal `code:`, flat `values:` with hardcoded indices, `cfg_fragments: Vec::new()`), so existing call sites pay zero overhead and the only diff in compiled output is the empty trailing field.

### 2. Data model (`platform/script/src/vm.rs`)

```rust
#[derive(Default, Debug)]
pub struct ScriptMod {
    pub cargo_manifest_path: String,
    pub module_path: String,
    pub file: String,
    pub line: usize,
    pub column: usize,
    pub code: String,
    pub values: Vec<ScriptValue>,
    /// One bool per top-level `#[cfg(...)]`-gated fragment in source order.
    /// `true` iff the fragment was selected by rustc in this build.
    /// Empty for blocks with no cfg attributes (the common case).
    pub cfg_fragments: Vec<bool>,
}
```

`#[derive(Default)]` still applies. No other consumer reads `cfg_fragments` except `live_reload.rs`.

### 3. Hot-reload extractor (`platform/src/live_reload.rs`)

The extractor (`extract_script_mods_from_rust_file` / `normalize_script_mod_body`, line 559) currently produces a single `code: String` per script_mod site. It is extended to:

1. While normalising, also recognise top-level outer `#[cfg(...)]` attributes. Recognition rule (depth-zero only, applied by the existing byte-scanner which already tracks string/comment/raw-string/group state):
   - The current byte must be `#` and not inside a string, char, raw string, or comment.
   - The next non-whitespace, non-comment byte must be `[` (an inner attribute `#![cfg(...)]` — i.e. `#` followed by `!` then `[` — is **not** recognised; it is treated as plain DSL tokens and will be rejected by the proc-macro at compile time, so we don't expect to see it in any compiled site).
   - Inside the `[ … ]` group, ignoring leading whitespace and comments, the next identifier must be `cfg` (so `#[doc = …]`, `#[derive(…)]`, etc. are *not* matched).
   - After `cfg`, ignoring whitespace and comments, the next token must be `(`.
   - The `(…)` group must close cleanly; the outer `]` must follow (whitespace/comments allowed in between).

   The extractor and proc-macro both use this rule. Pseudocode for the extractor lives alongside the existing `skip_ws_and_comments` helper.
2. After matching an attribute, the extractor parses the following statement using the **same form-disambiguation rule as the proc-macro**: leading `{` → brace-grouped block (outer braces stripped from the captured body); else → single statement bounded by either the close of the first encountered top-level brace group or by a depth-zero newline. Multi-line single-statement scopes are rejected with a structured error (count-mismatch fallback at hot-reload time, plus the proc-macro would have rejected it at build time).
3. Collect fragments as it goes:
   ```rust
   enum ExtractedFragment<'a> {
       Unconditional(&'a str),
       Conditional { body: &'a str },  // outer braces already stripped if present
   }
   ```
4. Return both the verbatim concatenated body (legacy `code` field for blocks without cfg fragments) and the structured fragment list (new field) from `ExtractedScriptMod`.

`apply_extracted_script_mod_overrides` (`platform/src/live_reload.rs:200`) is then updated:

- **No-cfg site** (`site.cfg_fragments` is empty) → use today's verbatim comparison and storage path, unchanged.
- **Cfg-aware site** (`site.cfg_fragments` non-empty):
  - **Count mismatch** between extracted `Conditional` fragments and `site.cfg_fragments.len()` → log `"hot reload could not match cfg fragments for <file>: file has N cfg fragments, compiled binary has M — rebuild required"` and skip this site (do not modify `next_overrides` for it).
  - **Counts match** → compute `extracted_filtered` by concatenating every `Unconditional` fragment plus each `Conditional` fragment whose `site.cfg_fragments[i]` is `true`. The two comparison checks today (`extracted.code == current_effective` and `extracted.code == site.original_code`) become `extracted_filtered == current_effective` and `extracted_filtered == site.original_code`. **Stored value:** when the override is accepted, `next_overrides[site.key]` receives `extracted_filtered` (i.e. the cfg-resolved DSL the VM would parse), **not** the raw fragmented source. This way the VM never sees `#[cfg(...)]` syntax. The raw fragmented source is not persisted anywhere on the site — toggling features at runtime requires a rebuild, which the design already mandates.

We deliberately do **not** persist the cfg-expression text in `ScriptMod` — only the resulting boolean. This is enough for safe filtering and keeps the metadata minimal.

## Data flow

```
┌────────────────────────┐                ┌────────────────────────┐
│ user source .rs        │                │ user source .rs        │
│ with #[cfg] fragments  │                │ (post-edit)            │
└────────┬───────────────┘                └────────┬───────────────┘
         │ rustc + proc-macro                       │ filesystem watcher
         │                                          │
         ▼                                          ▼
┌────────────────────────┐                ┌────────────────────────┐
│ ScriptMod {            │                │ live_reload extractor  │
│   code: rustc-selected │                │  - parses top cfg attrs│
│         DSL,           │                │  - emits fragment list │
│   values: matching vec,│                └────────┬───────────────┘
│   cfg_fragments: bools │                         │
│ }                      │                         │
└────────┬───────────────┘                         │
         │ runs in VM                              │
         ▼                                         ▼
   vm.eval(...)                          apply_extracted_script_mod_overrides
                                          - uses site.cfg_fragments
                                          - filters & compares
                                          - on count mismatch: skip + warn
```

## Error handling

| Error condition | Detected at | Behaviour |
|---|---|---|
| Predicate other than `feature = "…"` / `not` / `any` / `all` | proc-macro | `compile_error!("script_mod! only supports cfg(feature = \"…\"), not(...), any(...), all(...); got <token>")` at the offending span |
| Malformed cfg (no `=`, no string literal, unbalanced parens) | proc-macro | `compile_error!("malformed cfg in script_mod!: …")` at the cfg attribute |
| `#[cfg(...)]` not followed by any statement (e.g. at end of body) | proc-macro | `compile_error!("script_mod! cfg attribute has no following item")` |
| `#[cfg(...)]` appears inside a nested group | proc-macro does not recognise; tokens pass through verbatim | DSL parser produces its own error at the `#` token |
| `#![cfg(...)]` inner attribute at top of body | proc-macro | `compile_error!("script_mod! does not accept inner attributes; use #[cfg(...)] not #![cfg(...)]")` |
| `#[cfg_attr(...)]` at top of body | proc-macro | `compile_error!("script_mod! does not support cfg_attr; use #[cfg(...)] directly")` |
| Top-level attribute that is not `cfg` (e.g. `#[doc = …]`, `#[derive(…)]`) | proc-macro | `compile_error!("script_mod! only accepts #[cfg(...)] attributes")` |
| Single-statement cfg scope spans multiple source lines without a brace group | proc-macro | `compile_error!("script_mod! single-statement #[cfg(...)] cannot span multiple lines without a brace group — wrap the gated content in { … }")` |
| Hot-reload file has different cfg-fragment count than compiled binary | `apply_extracted_script_mod_overrides` | log explicit "rebuild required" message, skip the site (does not affect other sites or terminate hot-reload) |

No new runtime panics. No silent fallbacks.

## Testing

**Proc-macro unit tests** (in `platform/script/derive/src/script.rs` or a sibling `tests/` file):

1. Body with no `#[cfg]` → emitted code is byte-identical to today's macro output (regression guard).
2. Single `#[cfg(feature = "x")]` over a `use` line → two fragments, `cfg_fragments` length 1, generated code references `cfg!(feature = "x")`.
3. `#[cfg(feature = "x")] { multi: Block { … } let Other = 1.0 }` → outer braces stripped in emitted DSL, single cfg fragment.
4. `#[cfg(not(any(feature = "a", feature = "b")))]` → accepted, compiled `cfg!` preserves the predicate.
5. `#[cfg(target_os = "linux")]` → expansion produces `compile_error!` with the listed message.
6. `#[cfg(feature = "x")]` followed by `name(...) do #(arg) { … }` → placeholder index inside the conditional is emitted via runtime `write!(__code, "…#({})…", __values.len())`; runtime index matches `__values` length.
7. **Placeholder-renumbering integrity.** Body:

   ```rust
   script_mod! {
       before := Foo { val: #(a_expr) }
       #[cfg(feature = "x")]
       middle := Bar { val: #(b_expr) }
       after := Baz { val: #(c_expr) }
   }
   ```

   With feature `x` **off**, expect `__values.len() == 2`, runtime values `[a_expr, c_expr]` (in that order), and the emitted `code` contains exactly two placeholder occurrences `#(0)` and `#(1)` with no `#(2)` ghost.

   With feature `x` **on**, expect `__values.len() == 3`, runtime values `[a_expr, b_expr, c_expr]`, and the emitted `code` contains `#(0)`, `#(1)`, `#(2)` in source order. Asserts that unconditional fragments after a conditional renumber correctly under both branches.
8. Multiple consecutive conditionals with no separating unconditional → each becomes its own fragment; `cfg_fragments` length equals the count.
9. Cfg attribute at end of body with no following item → `compile_error!`.
10. `#![cfg(feature="x")]` inner attribute → `compile_error!` per the error table.
11. `#[cfg_attr(feature="x", something)]` → `compile_error!` per the error table.
12. `#[doc = "..."]` at top of body → `compile_error!` per the error table.
13. `#[cfg(feature="x")]` followed by a `use` continuing across a newline (no brace group) → `compile_error!` per the error table.
14. `#[cfg]` substring inside a string literal or DSL value (`code: "..."` or similar) → tokeniser already produces a single string token; no cfg detection triggers (regression guard).
15. **All-false conditional sweep.** A body containing only unconditional and conditional fragments where every conditional resolves false (e.g. `#[cfg(feature = "never_enabled")]`) — `cfg_fragments` is non-empty and all `false`; the with-conditional emission path is still taken (not the no-cfg fast path); emitted `code` contains only the unconditional fragments with correctly renumbered placeholders.

**Hot-reload extractor unit tests** (in `live_reload.rs`, beside existing tests at line 1021+):

16. File with `[Unconditional, Conditional]` where compiled `cfg_fragments = [true]` → filtered code equals the verbatim body.
17. Same shape with `cfg_fragments = [false]` → filtered code omits the conditional body.
18. File grows from 1 cfg fragment to 2 → count-mismatch error, site skipped, sentinel log message present.
19. File loses its only cfg fragment → same count-mismatch error.
20. Cfg-gated brace block (`#[cfg(…)] { … }`) → outer braces stripped from the extracted body so it matches macro output.
21. `#[cfg]` lookalike that is actually inside a string literal or comment → not detected as a cfg attribute (existing skip_non_code_segment handles strings/comments).
22. **Lexical parity with proc-macro:** parametrised test fed identical bodies with awkward spacing (`#  [  cfg  (  feature  =  "x"  )  ]`, comment between `#` and `[`, comment inside the cfg parens) — extractor and proc-macro must agree on whether the attribute is recognised, and on the same `cfg_fragments.len()` count.
23. `#![cfg(...)]` inner-attribute in the file → not recognised as a top-level cfg attribute by the extractor (matches the proc-macro's rejection so a compiled site never persists one in `cfg_fragments`).

**Integration check:**

24. Build `studio/desktop` twice with `--features=` that toggle a sample feature and confirm the produced `ScriptMod.code` strings differ as expected; both binaries start and run their DSL successfully. Also confirm `cfg_fragments` differs (booleans flip) between the two builds.

## Rollout / migration

- Existing `script_mod!` invocations are untouched in source and produce byte-identical output. No call-site changes needed.
- The added `cfg_fragments` field defaults to an empty `Vec` for any pre-existing `ScriptMod::default()` construction (e.g. `script_impl`'s empty-input branch).
- Hot-reload behaves identically for blocks that contain no cfg attributes.

## Open questions

None remaining after spec-review. Surface syntax, cfg-grammar subset, placeholder-renumbering strategy, extractor-vs-macro lexical parity, override-storage semantics, and rebuild-required behaviour on structural cfg edits are all resolved above. Implementation plan to follow.

## Notes for the planner

- The duplication of cfg expression between `cfg!(<expr>)` (runtime bool that drives `cfg_fragments`) and `#[cfg(<expr>)] { … }` (compile-time gate over the actual emitted code) is intentional. `cfg!` produces a runtime value the extractor needs; the attribute drives what rustc actually compiles. They must reference the same expression because the spec relies on `cfg_fragments.len()` matching the number of conditional fragments regardless of which ones were selected.
- `cfg_fragments: Vec<bool>` is deliberately a `Vec`, not a packed bitfield — typical counts per `script_mod!` block are very small (<10) and the readability win outweighs the bytes.
