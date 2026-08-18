# Slot resolver (phase 1c) — implementation spec

Goal: lexically resolved fn-body locals live in `thread.slots` (see 1a/1b
commits) instead of scope-object maps. Parser-side only; runtime opcodes
already exist (SLOTS_FRAME, PUSH_SLOT, LET_SLOT, STORE_SLOT,
ASSIGN_SLOT_ADD/SUB/MUL/DIV/MOD).

## Core architecture: log-then-rewrite (NOT substitute-at-emission)

The parser inspects emitted opcode shapes for grammar decisions (`?=` lazy
path checks `code_last().is_id()`; the `:` field-chain transform walks
backwards over `[id, FIELD]` patterns; inline-number fusion pops the last
f64). Therefore the resolver NEVER changes the stream mid-parse. It:

1. logs candidate positions while parsing (stream stays 100% dynamic),
2. at fn-body close, if the body is still eligible, rewrites logged
   positions in place. Every rewrite is 1:1 (same opcode-slot count):
   - read candidate `[id x]`            -> `PUSH_SLOT(slot)`
   - `LET_DYN` at logged let position   -> `LET_SLOT(slot)` (id slot stays,
     popped+discarded by the handler)
   - `ASSIGN` at logged reduce position -> `STORE_SLOT(slot)`
   - `ASSIGN_ADD/SUB/MUL/DIV/MOD` ditto -> `ASSIGN_SLOT_*(slot)`

Disqualification = don't rewrite. No patch-back ever.

## SlotCtx (one per fn body, stack in parser)

```
struct SlotCtx {
    eligible: bool,            // false => no rewrites at close
    slots_frame_at: u32,       // index of the SLOTS_FRAME placeholder
    names: Vec<(LiveId, u32)>, // slotted name -> slot index (linear scan fine)
    poisoned: Vec<LiveId>,     // names that must stay dynamic
    reads: Vec<(u32, LiveId)>, // [id] value positions to become PUSH_SLOT
    lets: Vec<(u32, LiveId)>,  // LET_DYN positions to become LET_SLOT
    assigns: Vec<(u32, LiveId, Opcode)>, // reduce positions -> slot variant
    loop_depth: u32,           // >0 => inside loop body (shadow scopes)
}
```
All Clone; `slot_ctxs: Vec<SlotCtx>` snapshotted into ParserCheckpoint and
restored wholesale (candidates created after a checkpoint die with the
restore; indices always < the truncation length).

## Hook points in parser.rs

- FN_BODY_DYN / FN_BODY_TYPED emission (4-5 sites): emit `SLOTS_FRAME(0)`
  placeholder right after, push SlotCtx { eligible: DYN only, ... }.
  Also: if any enclosing ctx is open, mark ALL enclosing ctxs
  eligible=false (a nested fn is a closure that may capture anything).
- EndFnBlock / EndFnExpr: pop ctx, finalize (see below). These are the ONLY
  pops — parser structure guarantees pairing. NOTE EndFnBlock/EndFnExpr
  emit RETURN and patch the FN_BODY jump; finalize runs before/after that
  patch, order irrelevant (rewrites don't change lengths).
- BeginExpr bare-identifier emission (~line 3500 `if id.not_empty()`):
  if innermost ctx exists && name in its `names` && not poisoned &&
  state.last() is NOT EmitOp{. | .? | me.} (field-name position):
  log read candidate (code_len before push, name). Stream still gets [id].
- State::Let -> LetDynOrTyped -> EmitLetDyn: thread `name: LiveId` through
  these states. At EmitLetDyn (only the `= expr` form): if ctx open:
  - loop_depth == 0 && name not in names && not poisoned && not reserved:
    alloc slot, insert names, log let candidate (position of LET_DYN).
  - name already in names (re-let) or loop_depth > 0 && name in names:
    poison(name).
  LET_DYN nil-form (`let x`), LET_TYPED, VAR_DYN/TYPED: never slotted; if
  name in names -> poison.
- EndExpr operator detection: when op is assign-family AND
  `code_last()` is an id equal to the innermost ctx's LAST logged read
  candidate at index code_len-1: remove that read candidate, then:
  - = += -= *= /= %=  : set `slot_target: Some((name, ..))` on the
    State::EmitOp being pushed (new field). At EmitOp reduce (2748), if
    slot_target set: log assign candidate (emit position, name, opcode).
    CAREFUL: the reduce inline-number fusion (pops last f64 into opargs)
    must not fire for slot-target ops (opargs needed for the slot index) —
    skip fusion when slot_target is set.
  - : := <: >: ^: (ASSIGN_ME family): candidate removed, nothing recorded
    (object key stays a raw id).
  - ?= &= |= ^= <<= >>=: poison(name).
  - ShortCircuitAssignEnd (ASSIGN after ?=-lazy): unreachable for slotted
    names since ?= poisons first (order: EndExpr sees ?= before that path).
- Loop tracking: FOR_1/FOR_2/FOR_3/LOOP emission -> loop_depth += 1;
  FOR_END emission -> loop_depth -= 1 (saturating). For-loop variable
  ids (ForIdent state): poison each.
- Body disqualifiers -> innermost ctx eligible=false: USE emission, SCOPE
  opcode emission, TRY_TEST / OK_TEST emission, destructuring lets
  (also poison the bound names), match temp lets are generated unique
  names (never in names, no action needed).

## Finalize at body close

If !eligible or names.is_empty(): leave SLOTS_FRAME(0) (harmless no-op),
done. Else:
- drop all candidates whose name is poisoned,
- rewrite reads/lets/assigns as above (indices are absolute; all within
  [slots_frame_at, close)),
- patch SLOTS_FRAME opargs = number of allocated slots (poisoned names
  keep their slot allocated-but-unused; frame slightly larger, harmless).

## Correctness invariants (why this is safe)

- A name is either fully slotted or fully dynamic within a body: poison
  drops every candidate for that name before any rewrite happens.
- Reads before the `let` are dynamic AND correct: the slot is invisible to
  the scope walk, and the fn-scope map never contains the name (its LET
  became LET_SLOT), so pre-let reads resolve to outer scopes exactly as
  the dynamic form does.
- Loop-body `let` of a slotted name would shadow per-iteration -> poison.
  For-loop variables shadow -> poison. `var`, re-`let`, destructure -> poison.
- Nested fn anywhere in a body -> body ineligible (closure capture).
  `use`, `scope`, try/ok -> ineligible (dynamic scope visibility).
- Runtime slot handlers mirror dynamic pop order / results / escape
  barrier exactly (see opcodes_slots.rs).
- Streaming: checkpoint snapshots slot_ctxs; auto-close runs the normal
  EndFnBlock path (verify!), so an auto-closed body finalizes and executes
  with slots; restore_checkpoint truncates opcodes and restores ctxs.

## v1 limitations (deliberate)

Args are NOT slotted yet (ARGS_TO_SLOTS unused) — arg reads stay dynamic.
Typed lets/fns stay dynamic. while/loop bodies: lets inside stay dynamic.
Next steps: args (v2), loop variables into LoopValues slots (v3).

## Status / handoff (end of 2026-08-11 session)

Committed on splash-slots (rik2-based, in the fast_splash worktree):
1a substrate, 1b opcodes, 1c-v1 resolver, 1c-v2 (this commit). All test
suites green (assert suite = 2 expected errors, lib/reload/std-headless ok).

Measured vs pre-slots engine (same bench file, interleaved x3):
scope_depth_read -17%, array_iter -9..14%, array_index -6%, while -4%,
field_rw ~0, fib/method_call ~+5% (was expected to go to ~0 after
placeholder elision — verify elision actually fires for block-body
lambdas), game_tick_40 +5% — RESOLVED 2026-08-11: it was
binary link-layout luck, not slot cost. Rebuilding the identical
pre-slots source produced a binary 4-5% slower on game_tick than the
original build; a 5-commit bisect (pre-slots..v2) measured statistically
indistinguishable (119-122us). Opcode dump (BENCH_DUMP=1 splash_bench)
verified: fib gets full placeholder elision, loop-body lets/reads/assigns
all rewrite correctly. game_tick doesn't IMPROVE yet because its time is
in field access, loop VARIABLES and args — the next two steps.

Next steps after that: args into slots (ARGS_TO_SLOTS is implemented
runtime-side, unused), loop VARIABLES into slots (LoopValues carries the
value_id — a slot variant skips the per-iteration map insert + enables
skipping iteration-scope reset entirely for fully-slotted loop bodies),
then operand encoding (SLOTS_PLAN phase 2).

Merge path: squash or keep granular onto rik2 (NEVER push dev), then
delete the worktree branch after Rik squash-merges rik2 -> dev.
