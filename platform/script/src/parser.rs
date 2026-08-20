use crate::makepad_error_log::*;
use crate::makepad_live_id::live_id::*;
use crate::makepad_live_id::makepad_live_id_macros::*;
use crate::opcode::*;
use crate::tokenizer::*;
use crate::value::*;

macro_rules! error {
    ($self:expr, $tokenizer:expr, $($arg:tt)*) => {
        $self.report_error($tokenizer, format!("{} (from: {}:{})", format!($($arg)*), file!(), line!()))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum State {
    BeginStmt {
        last_was_sep: bool,
    },
    BeginExpr {
        required: bool,
    },
    EndExpr,
    EndStmt {
        last: u32,
    },

    EscapedId,

    ForIdent {
        idents: u32,
        index: u32,
    },
    ForBody {
        idents: u32,
        index: u32,
    },
    ForExpr {
        code_start: u32,
    },
    ForBlock {
        code_start: u32,
    },
    Loop {
        index: u32,
    },
    While {
        index: u32,
    },
    WhileTest {
        code_start: u32,
    },

    IfTest {
        index: u32,
    },
    IfTrueExpr {
        if_start: u32,
    },
    IfTrueBlock {
        if_start: u32,
        last_was_sep: bool,
    },
    IfMaybeElse {
        if_start: u32,
        was_block: bool,
    },
    IfElse {
        else_start: u32,
    },
    IfElseExpr {
        else_start: u32,
    },
    IfElseBlock {
        else_start: u32,
        last_was_sep: bool,
    },

    OkTest {
        index: u32,
    },
    OkTestBlock {
        ok_start: u32,
        last_was_sep: bool,
    },
    OkTestExpr {
        ok_start: u32,
    },

    TryTest {
        index: u32,
    },
    TryTestBlock {
        try_start: u32,
        last_was_sep: bool,
    },
    TryTestExpr {
        try_start: u32,
    },
    TryErrBlockOrExpr,
    TryErrBlock {
        err_start: u32,
        last_was_sep: bool,
    },
    TryErrExpr {
        err_start: u32,
    },
    TryOk {
        was_block: bool,
    },
    TryOkBlockOrExpr,
    TryOkBlock {
        ok_start: u32,
        last_was_sep: bool,
    },
    TryOkExpr {
        ok_start: u32,
    },

    FnMaybeLet {
        index: u32,
    },
    FnLetMaybeArgs,
    FnArgList {
        lambda: bool,
    },
    FnArgMaybeType {
        lambda: bool,
        index: u32,
    },
    FnArgType {
        lambda: bool,
        index: u32,
    },
    FnArgTypeAssign {
        lambda: bool,
        index: u32,
    },

    FnBody {
        lambda: bool,
    },
    FnReturnType {
        lambda: bool,
    },
    FnBodyTyped {
        lambda: bool,
    },
    EndFnBlock {
        fn_slot: u32,
        last_was_sep: bool,
        index: u32,
    },
    EndFnExpr {
        fn_slot: u32,
        index: u32,
    },
    EmitFnArgTyped {
        index: u32,
    },
    EmitFnArgDyn {
        index: u32,
    },

    EmitUnary {
        what_op: LiveId,
        index: u32,
    },
    EmitSplat {
        index: u32,
    },
    EmitOp {
        what_op: LiveId,
        index: u32,
        slot_assign: Option<LiveId>,
    },
    EmitFieldAssign {
        what_op: LiveId,
        index: u32,
    },
    EmitIndexAssign {
        what_op: LiveId,
        index: u32,
    },

    EndBare,
    EndBareSquare,
    EndProto,
    EndProtoInherit,
    EndScopeInherit,
    EndFieldInherit,
    EndIndexInherit,
    EndRound,

    CallMaybeDo {
        is_method: bool,
        index: u32,
    },
    EmitCallFromDo {
        is_method: bool,
        index: u32,
    },
    EndCall {
        is_method: bool,
        index: u32,
    },
    ArrayIndex,

    EmitReturn {
        index: u32,
        code_len_before: u32,
    },
    EmitBreak {
        index: u32,
    },
    EmitContinue {
        index: u32,
    },

    Use {
        index: u32,
    },
    Let {
        index: u32,
    },
    LetDynOrTyped {
        index: u32,
        name: LiveId,
    },
    LetType {
        index: u32,
    },
    LetTypedAssign {
        index: u32,
    },
    EmitLetDyn {
        index: u32,
        name: LiveId,
    },
    EmitLetTyped {
        index: u32,
    },

    // Destructuring patterns
    // We collect binding ids in the code stream, then after RHS we emit extraction opcodes
    // Defaults are stored separately and ?= is emitted after extraction
    LetArrayDestruct {
        index: u32,
        count: u32,
        ids_start: u32,
    }, // parsing [x, y, ...] pattern
    LetArrayDestructEl {
        index: u32,
        count: u32,
        binding_id: LiveId,
        ids_start: u32,
    }, // after identifier, maybe = default
    LetArrayDestructDefault {
        index: u32,
        count: u32,
        binding_id: LiveId,
        ids_start: u32,
        default_start: u32,
    }, // parsing default expr
    LetArrayDestructRhs {
        index: u32,
        count: u32,
        ids_start: u32,
        defaults_start: u32,
    }, // after ], before =
    EmitLetArrayDestruct {
        index: u32,
        count: u32,
        ids_start: u32,
        defaults_start: u32,
    }, // after RHS, emit opcodes

    // Nested object pattern inside array: let [{x}] = ...
    LetArrayDestructNestedObject {
        index: u32,
        outer_count: u32,
        outer_ids_start: u32,
        nested_count: u32,
    },
    LetArrayDestructNestedObjectEl {
        index: u32,
        outer_count: u32,
        outer_ids_start: u32,
        nested_count: u32,
        binding_id: LiveId,
    },

    // Nested array pattern inside array: let [[x]] = ...
    LetArrayDestructNestedArray {
        index: u32,
        outer_count: u32,
        outer_ids_start: u32,
        nested_count: u32,
    },
    LetArrayDestructNestedArrayEl {
        index: u32,
        outer_count: u32,
        outer_ids_start: u32,
        nested_count: u32,
        binding_id: LiveId,
    },

    LetObjectDestruct {
        index: u32,
        count: u32,
        ids_start: u32,
    }, // parsing {x, y, ...} pattern
    LetObjectDestructEl {
        index: u32,
        count: u32,
        binding_id: LiveId,
        ids_start: u32,
    }, // after identifier, maybe = default
    LetObjectDestructDefault {
        index: u32,
        count: u32,
        binding_id: LiveId,
        ids_start: u32,
        default_start: u32,
    }, // parsing default expr
    LetObjectDestructRhs {
        index: u32,
        count: u32,
        ids_start: u32,
        defaults_start: u32,
    }, // after }, before =
    EmitLetObjectDestruct {
        index: u32,
        count: u32,
        ids_start: u32,
        defaults_start: u32,
    }, // after RHS, emit opcodes

    // Nested array pattern inside object: let {a: [x]} = ...
    LetObjectDestructNestedArray {
        index: u32,
        outer_count: u32,
        outer_ids_start: u32,
        key: LiveId,
        nested_count: u32,
    },
    LetObjectDestructNestedArrayEl {
        index: u32,
        outer_count: u32,
        outer_ids_start: u32,
        key: LiveId,
        nested_count: u32,
        binding_id: LiveId,
    },

    // Nested object pattern inside object: let {a: {x}} = ...
    LetObjectDestructNestedObject {
        index: u32,
        outer_count: u32,
        outer_ids_start: u32,
        key: LiveId,
        nested_count: u32,
    },
    LetObjectDestructNestedObjectEl {
        index: u32,
        outer_count: u32,
        outer_ids_start: u32,
        key: LiveId,
        nested_count: u32,
        binding_id: LiveId,
    },
    Var {
        index: u32,
    },
    VarDynOrTyped {
        index: u32,
    },
    VarType {
        index: u32,
    },
    VarTypedAssign {
        index: u32,
    },
    EmitVarDyn {
        index: u32,
    },
    EmitVarTyped {
        index: u32,
    },

    MatchSubject {
        temp_id: LiveId,
        index: u32,
    },
    MatchBlock {
        temp_id: LiveId,
        index: u32,
    },
    MatchArmPattern {
        temp_id: LiveId,
        first: bool,
        prev_else_start: u32,
        index: u32,
    },
    MatchArmArrow {
        temp_id: LiveId,
        prev_else_start: u32,
        index: u32,
    },
    MatchArmBody {
        temp_id: LiveId,
        if_start: u32,
        prev_else_start: u32,
        index: u32,
    },
    MatchArmBlock {
        temp_id: LiveId,
        if_start: u32,
        prev_else_start: u32,
        last_was_sep: bool,
        index: u32,
    },
    MatchWildcardArrow {
        prev_else_start: u32,
        index: u32,
    },
    MatchWildcardBody {
        prev_else_start: u32,
        index: u32,
    },
    MatchWildcardBlock {
        prev_else_start: u32,
        last_was_sep: bool,
        index: u32,
    },
    MatchMaybeArm {
        temp_id: LiveId,
        if_start: u32,
        prev_else_start: u32,
        index: u32,
    },
    MatchWildcardEnd {
        prev_else_start: u32,
        index: u32,
    },

    // Short-circuit evaluation - patches TEST opcode jump after second operand
    ShortCircuitEnd {
        test_slot: u32,
    },
    // Short-circuit ?= - emits ASSIGN after RHS, then patches jump
    ShortCircuitAssignEnd {
        test_slot: u32,
        index: u32,
    },
}

impl State {
    /// Whether this state is an unclosed `( )` / `[ ]` context (grouping, call
    /// args, array literal, or index). Inside these, newlines are insignificant
    /// and expressions continue across them (Python/Swift/JS-style).
    fn is_round_square_container(&self) -> bool {
        matches!(
            self,
            State::EndRound | State::EndCall { .. } | State::EndBareSquare | State::ArrayIndex
        )
    }

    /// Whether this state is an unclosed `{ }` / statement-list context, where
    /// newlines DO delimit statements (function/match/block bodies, object and
    /// bare blocks).
    fn is_statement_container(&self) -> bool {
        matches!(
            self,
            State::EndProto
                | State::EndProtoInherit
                | State::EndScopeInherit
                | State::EndFieldInherit
                | State::EndIndexInherit
                | State::EndBare
                | State::EndFnBlock { .. }
                | State::EndFnExpr { .. }
                | State::MatchArmBody { .. }
                | State::MatchArmBlock { .. }
                | State::MatchWildcardBody { .. }
                | State::MatchWildcardBlock { .. }
        )
    }

    fn is_short_circuit_op(op: LiveId) -> bool {
        op == id!(&&) || op == id!(||) || op == id!(|?)
    }

    fn short_circuit_opcode(op: LiveId) -> Opcode {
        match op {
            id!(&&) => Opcode::LOGIC_AND_TEST,
            id!(||) => Opcode::LOGIC_OR_TEST,
            id!(|?) => Opcode::NIL_OR_TEST,
            _ => Opcode::NOP,
        }
    }
}

// we have a stack, and we have operations
// operators:
/*
Order list from highest prio to lowest
1 Identifierpath
2 Method calls
3 Field expression
4 Functioncall, array index
5 ?
6 unary - ! * borrow
7 as
8 * /  %
9 + -
10 << >>
11 &
12 ^
13 |
14 == != < > <= >=
15 &&
16 ||
17 = += -= *= /= %=
18 &= |= ^= <<= >>=
19 return break
*/

impl State {
    fn operator_order(op: LiveId) -> usize {
        match op {
            id!(.) => 3,
            id!(.?) => 3,
            id!(*) | id!(/) | id!(%) => 8,
            id!(+) | id!(-) => 9,
            id!(<<) | id!(>>) => 10,
            id!(&) => 11,
            id!(^) => 12,
            id!(|) => 13,
            id!(-:) => 14,
            id!(++) => 14,
            id!(===) | id!(!==) | id!(==) | id!(!=) | id!(<) | id!(>) | id!(<=) | id!(>=) => 15,
            id!(is) => 15,
            id!(&&) => 16,
            id!(||) | id!(|?) => 17,
            id!(..) => 18,
            id!(:)
            | id!(:=)
            | id!(=)
            | id!(>:)
            | id!(<:)
            | id!(^:)
            | id!(+:)
            | id!(+=)
            | id!(-=)
            | id!(*=)
            | id!(/=)
            | id!(%=) => 19,
            id!(&=) | id!(|=) | id!(^=) | id!(<<=) | id!(>>=) | id!(?=) => 20,
            _ => 0,
        }
    }

    fn is_assign_operator(op: LiveId) -> bool {
        match op {
            id!(=)
            | id!(:)
            | id!(:=)
            | id!(+=)
            | id!(<:)
            | id!(+:)
            | id!(+=)
            | id!(-=)
            | id!(*=)
            | id!(/=)
            | id!(%=)
            | id!(&=)
            | id!(|=)
            | id!(^=)
            | id!(<<=)
            | id!(>>=)
            | id!(?=) => true,
            _ => false,
        }
    }

    fn operator_supports_inline_number(op: LiveId) -> bool {
        match op {
            id!(*)
            | id!(/)
            | id!(%)
            | id!(+)
            | id!(-)
            | id!(<<)
            | id!(>>)
            | id!(&)
            | id!(^)
            | id!(|)
            | id!(<)
            | id!(>)
            | id!(<=)
            | id!(>=) => true,
            _ => false,
        }
    }

    fn operator_to_field_assign(op: LiveId) -> ScriptValue {
        match op {
            id!(=) | id!(:) => Opcode::ASSIGN_FIELD,
            id!(+=) => Opcode::ASSIGN_FIELD_ADD,
            id!(-=) => Opcode::ASSIGN_FIELD_SUB,
            id!(*=) => Opcode::ASSIGN_FIELD_MUL,
            id!(/=) => Opcode::ASSIGN_FIELD_DIV,
            id!(%=) => Opcode::ASSIGN_FIELD_MOD,
            id!(&=) => Opcode::ASSIGN_FIELD_AND,
            id!(|=) => Opcode::ASSIGN_FIELD_OR,
            id!(^=) => Opcode::ASSIGN_FIELD_XOR,
            id!(<<=) => Opcode::ASSIGN_FIELD_SHL,
            id!(>>=) => Opcode::ASSIGN_FIELD_SHR,
            id!(?=) => Opcode::ASSIGN_FIELD_IFNIL,
            _ => Opcode::NOP,
        }
        .into()
    }

    fn operator_to_index_assign(op: LiveId) -> ScriptValue {
        match op {
            id!(=) => Opcode::ASSIGN_INDEX,
            id!(+=) => Opcode::ASSIGN_INDEX_ADD,
            id!(-=) => Opcode::ASSIGN_INDEX_SUB,
            id!(*=) => Opcode::ASSIGN_INDEX_MUL,
            id!(/=) => Opcode::ASSIGN_INDEX_DIV,
            id!(%=) => Opcode::ASSIGN_INDEX_MOD,
            id!(&=) => Opcode::ASSIGN_INDEX_AND,
            id!(|=) => Opcode::ASSIGN_INDEX_OR,
            id!(^=) => Opcode::ASSIGN_INDEX_XOR,
            id!(<<=) => Opcode::ASSIGN_INDEX_SHL,
            id!(>>=) => Opcode::ASSIGN_INDEX_SHR,
            id!(?=) => Opcode::ASSIGN_INDEX_IFNIL,
            _ => Opcode::NOP,
        }
        .into()
    }

    fn operator_to_unary(op: LiveId) -> ScriptValue {
        match op {
            id!(~) => Opcode::LOG,
            id!(!) => Opcode::NOT,
            id!(-) => Opcode::NEG,
            id!(+) => Opcode::NOP,
            _ => Opcode::NOP,
        }
        .into()
    }

    fn operator_to_opcode(op: LiveId) -> ScriptValue {
        match op {
            id!(*) => Opcode::MUL,
            id!(/) => Opcode::DIV,
            id!(%) => Opcode::MOD,
            id!(+) => Opcode::ADD,
            id!(-) => Opcode::SUB,
            id!(<<) => Opcode::SHL,
            id!(>>) => Opcode::SHR,
            id!(&) => Opcode::AND,
            id!(^) => Opcode::XOR,
            id!(|) => Opcode::OR,
            id!(++) => Opcode::CONCAT,
            id!(is) => Opcode::IS,
            id!(==) => Opcode::EQ,
            id!(!=) => Opcode::NEQ,
            id!(<) => Opcode::LT,
            id!(>) => Opcode::GT,
            id!(<=) => Opcode::LEQ,
            id!(>=) => Opcode::GEQ,
            id!(===) => Opcode::SHALLOW_EQ,
            id!(!==) => Opcode::SHALLOW_NEQ,

            // &&, ||, |? are handled specially for short-circuit evaluation
            id!(&&) => Opcode::LOGIC_AND_TEST,
            id!(||) => Opcode::LOGIC_OR_TEST,
            id!(|?) => Opcode::NIL_OR_TEST,
            id!(:) => Opcode::ASSIGN_ME,
            id!(:=) => Opcode::ASSIGN_ME_VEC,
            id!(<:) => Opcode::ASSIGN_ME_BEFORE,
            id!(>:) => Opcode::ASSIGN_ME_AFTER,
            id!(^:) => Opcode::ASSIGN_ME_BEGIN,
            id!(=) => Opcode::ASSIGN,
            id!(+=) => Opcode::ASSIGN_ADD,
            id!(-=) => Opcode::ASSIGN_SUB,
            id!(*=) => Opcode::ASSIGN_MUL,
            id!(/=) => Opcode::ASSIGN_DIV,
            id!(%=) => Opcode::ASSIGN_MOD,
            id!(&=) => Opcode::ASSIGN_AND,
            id!(|=) => Opcode::ASSIGN_OR,
            id!(^=) => Opcode::ASSIGN_XOR,
            id!(<<=) => Opcode::ASSIGN_SHL,
            id!(>>=) => Opcode::ASSIGN_SHR,
            id!(?=) => Opcode::ASSIGN_IFNIL,
            id!(..) => Opcode::RANGE,
            id!(.) => Opcode::FIELD,
            id!(.?) => Opcode::FIELD_NIL,
            id!(me.) => Opcode::ME_FIELD,
            id!(?) => Opcode::RETURN_IF_ERR,
            _ => Opcode::NOP,
        }
        .into()
    }

    fn is_heq_prio(&self, other: State) -> bool {
        match self {
            Self::EmitOp { what_op: op1, .. } => match other {
                Self::EmitOp { what_op: op2, .. } => {
                    if Self::is_assign_operator(*op1) && Self::is_assign_operator(op2) {
                        return false;
                    }
                    Self::operator_order(*op1) <= Self::operator_order(op2)
                }
                _ => false,
            },
            _ => false,
        }
    }
}

/// Represents a nested destructuring pattern (one level deep for now)
#[derive(Clone, Debug)]
pub enum NestedPattern {
    /// Nested object pattern like {x, y} - stores binding ids
    Object(Vec<LiveId>),
    /// Nested array pattern like [x, y] - stores binding ids
    Array(Vec<LiveId>),
}

/// Per-fn-body slot resolver context (see SLOTS_PLAN.md). The parser logs
/// candidate positions while emitting the fully dynamic opcode stream, and
/// only at body close — if the body stayed eligible — rewrites the logged
/// positions in place with slot opcodes (every rewrite is 1:1 in stream
/// shape). Disqualification simply means "don't rewrite".
#[derive(Clone, Debug)]
pub(crate) struct SlotCtx {
    /// false => body uses dynamic scopes only (closure inside, use, scope,
    /// try/ok, typed fn). No rewrites at close.
    eligible: bool,
    /// index of this body's SLOTS_FRAME placeholder opcode
    slots_frame_at: u32,
    /// slotted name -> (slot index, loop depth where defined)
    names: Vec<(LiveId, u32, u32)>,
    /// names that must stay dynamic (shadowed, var/typed/destructured,
    /// targeted by unsupported assign ops, loop variables)
    poisoned: Vec<LiveId>,
    /// [id] value positions to become PUSH_SLOT: (position, name, slot).
    /// Slots are resolved at log time — a name sealed at loop exit (or
    /// reused in a sibling loop with a fresh slot) keeps correct candidates.
    reads: Vec<(u32, LiveId, u32)>,
    /// LET_DYN positions to become LET_SLOT: (position, name, slot)
    lets: Vec<(u32, LiveId, u32)>,
    /// assign reduce positions to become STORE_SLOT / ASSIGN_SLOT_*
    assigns: Vec<(u32, LiveId, u32, Opcode)>,
    /// enclosing loop kinds, innermost last (true = for, false = while/loop).
    /// for-loops reset their iteration scope, so lets inside them can be
    /// slotted; while/loop bodies keep shadow chains and stay dynamic.
    loop_kinds: Vec<bool>,
}

pub struct ScriptParser {
    pub index: u32,
    pub opcodes: Vec<ScriptValue>,
    pub source_map: Vec<Option<u32>>,
    pub had_error: bool,
    /// The formatted messages behind `had_error`, bounded. A host that
    /// installs a captured-error sink gets these routed into it after each
    /// streaming parse (vm::eval_with_append_source), so a structural parse
    /// error FAILS a game eval instead of logging into the void while the
    /// recovered parse runs something else entirely — `let loop = [...]`
    /// recovered into an infinite empty loop and burned the whole
    /// instruction budget before this existed.
    pub parse_errors: Vec<String>,

    state: Vec<State>,
    pub file: String,
    pub line_offset: usize,
    pub col_offset: usize,

    // Temporary storage for destructuring defaults (binding_id, value_code, value_map)
    pub(crate) destruct_defaults: Vec<(LiveId, Vec<ScriptValue>, Vec<Option<u32>>)>,

    /// Code position most recently patched as a forward-jump target — a
    /// short-circuit (&&, ||, |?) landing site or a branch join (the position
    /// every arm of an if/elif/else, match, or try/err jumps to when it
    /// finishes). `set_pop_to_me` must not fuse the POP_TO_ME flag onto the
    /// opcode just before this position: the fused commit would sit inside a
    /// region some path jumps over, silently losing the value (a taken true
    /// branch skipped the flag fused onto the else branch's tail — the
    /// "on_render emits nothing" family of bugs). A standalone POP_TO_ME
    /// emitted AT this position is the jump target itself, so every path
    /// executes the commit.
    last_jump_target: u32,

    // Storage for nested patterns during parsing
    // Each entry is (pattern_info). The index into this vec is encoded in the ids list.
    nested_patterns: Vec<NestedPattern>,

    /// Open fn-body slot contexts, innermost last. Root script code never
    /// has a context (root statements are always dynamic).
    slot_ctxs: Vec<SlotCtx>,
    /// Slot-index -> name tables of finalized slot bodies, keyed by the
    /// body's SLOTS_FRAME opcode index. Consumers that walk the bytecode
    /// symbolically (the shader compiler) use this to translate PUSH_SLOT
    /// back to the variable name. Eligible bodies cannot nest (a nested fn
    /// disqualifies its parent), so nearest-preceding lookup is sound.
    pub(crate) slot_frames: Vec<(u32, Vec<LiveId>)>,
}

impl Default for ScriptParser {
    fn default() -> Self {
        Self {
            index: 0,
            opcodes: Default::default(),
            source_map: Default::default(),
            had_error: false,
            parse_errors: Default::default(),
            state: vec![State::BeginStmt {
                last_was_sep: false,
            }],
            file: String::new(),
            line_offset: 0,
            col_offset: 0,
            destruct_defaults: Default::default(),
            nested_patterns: Default::default(),
            slot_ctxs: Default::default(),
            slot_frames: Default::default(),
            last_jump_target: u32::MAX,
        }
    }
}

/// Snapshot of parser state that can be restored for incremental parsing.
/// Captures the state before auto-close so we can undo those synthetic closings
/// when more source arrives.
pub struct ParserCheckpoint {
    pub opcodes_len: usize,
    pub source_map_len: usize,
    pub token_index: u32,
    state: Vec<State>,
    destruct_defaults_len: usize,
    nested_patterns_len: usize,
    /// The last opcode before the checkpoint, saved because auto-close's
    /// set_pop_to_me() mutates it in place. Must be restored on continuation.
    last_opcode: Option<ScriptValue>,
    /// Open slot contexts at checkpoint time; restored wholesale so slot
    /// candidates logged after the checkpoint die with the restore.
    slot_ctxs: Vec<SlotCtx>,
    slot_frames_len: usize,
}

impl ScriptParser {
    pub fn report_error(&mut self, tokenizer: &ScriptTokenizer, msg: String) {
        self.had_error = true;
        let (line, col) = tokenizer
            .token_index_to_row_col(self.index)
            .unwrap_or((0, 0));
        if self.parse_errors.len() < 16 {
            self.parse_errors.push(format!(
                "{}:{}:{}: {}",
                self.file,
                line as usize + self.line_offset + 1,
                col as usize + self.col_offset + 1,
                msg
            ));
        }
        log_with_level(
            &self.file,
            line as u32 + self.line_offset as u32,
            col as u32 + self.col_offset as u32,
            line as u32 + self.line_offset as u32,
            col as u32 + self.col_offset as u32,
            msg,
            LogLevel::Error,
        );
    }

    /// Identifiers that may never be bound by `let`/`var`, function arguments,
    /// `for` variables or destructuring patterns. Binding one is a silent trap:
    /// reads still resolve to the keyword (`me` stays the implicit self, `nil`
    /// stays nil), so the "variable" can never be read back. Hard parse error
    /// instead. `_` stays allowed as the discard binding. `self` is reserved
    /// for shader code and future use even though script does not evaluate it.
    fn is_reserved_binding(id: LiveId) -> bool {
        id == id!(me)
            || id == id!(scope)
            || id == id!(self)
            || id == id!(nil)
            || id == id!(true)
            || id == id!(false)
            || id == id!(ok)
            || id == id!(let)
            || id == id!(var)
            || id == id!(mut)
            || id == id!(fn)
            || id == id!(if)
            || id == id!(elif)
            || id == id!(else)
            || id == id!(for)
            || id == id!(in)
            || id == id!(while)
            || id == id!(loop)
            || id == id!(match)
            || id == id!(return)
            || id == id!(break)
            || id == id!(continue)
            || id == id!(and)
            || id == id!(or)
            || id == id!(is)
            || id == id!(do)
            || id == id!(try)
            || id == id!(use)
    }

    // ===== slot resolver helpers (see SLOTS_PLAN.md) =====

    /// Open a slot context for a fn body and emit its SLOTS_FRAME
    /// placeholder. Any enclosing open body now contains a closure that may
    /// capture its scope — disqualify all of them.
    fn slot_body_open(&mut self, eligible: bool) {
        for ctx in self.slot_ctxs.iter_mut() {
            ctx.eligible = false;
        }
        let slots_frame_at = self.code_len();
        self.push_code_none(ScriptValue::from_opcode_args(
            Opcode::SLOTS_FRAME,
            OpcodeArgs::from_u32(0),
        ));
        self.slot_ctxs.push(SlotCtx {
            eligible,
            slots_frame_at,
            names: Vec::new(),
            poisoned: Vec::new(),
            reads: Vec::new(),
            lets: Vec::new(),
            assigns: Vec::new(),
            loop_kinds: Vec::new(),
        });
    }

    /// Close the innermost body: rewrite logged candidates with slot ops if
    /// the body stayed eligible. All rewrites are 1:1 in stream shape.
    fn slot_body_close(&mut self) {
        let Some(ctx) = self.slot_ctxs.pop() else {
            return;
        };
        if !ctx.eligible || ctx.names.is_empty() {
            // Nothing will be rewritten: remove the SLOTS_FRAME placeholder
            // so dynamic bodies pay zero dispatch. Safe: every intra-body
            // jump is a relative distance between two points that shift
            // uniformly, and every enclosing patch position lies before the
            // placeholder and is resolved against post-removal lengths.
            self.opcodes.remove(ctx.slots_frame_at as usize);
            self.source_map.remove(ctx.slots_frame_at as usize);
            return;
        }
        let ok = |name: &LiveId| !ctx.poisoned.contains(name);
        for (at, name, slot) in &ctx.reads {
            if ok(name) {
                let slot = *slot;
                self.opcodes[*at as usize] =
                    ScriptValue::from_opcode_args(Opcode::PUSH_SLOT, OpcodeArgs::from_u32(slot));
            }
        }
        for (at, name, slot) in &ctx.lets {
            if ok(name) {
                let slot = *slot;
                // preserve flag bits (pop_to_me/need_nil set post-emission)
                let flags = self.opcodes[*at as usize].raw() as u32
                    & (OpcodeArgs::POP_TO_ME_FLAG | OpcodeArgs::NEED_NIL_FLAG);
                self.opcodes[*at as usize] = ScriptValue::from_opcode_args(
                    Opcode::LET_SLOT,
                    OpcodeArgs(OpcodeArgs::from_u32(slot).raw() | flags),
                );
            }
        }
        for (at, name, slot, op) in &ctx.assigns {
            if ok(name) {
                let slot = *slot;
                let slot_op = match *op {
                    Opcode::ASSIGN => Opcode::STORE_SLOT,
                    Opcode::ASSIGN_ADD => Opcode::ASSIGN_SLOT_ADD,
                    Opcode::ASSIGN_SUB => Opcode::ASSIGN_SLOT_SUB,
                    Opcode::ASSIGN_MUL => Opcode::ASSIGN_SLOT_MUL,
                    Opcode::ASSIGN_DIV => Opcode::ASSIGN_SLOT_DIV,
                    Opcode::ASSIGN_MOD => Opcode::ASSIGN_SLOT_MOD,
                    _ => continue,
                };
                // preserve flag bits (pop_to_me) from the dynamic opcode
                let flags = self.opcodes[*at as usize].raw() as u32
                    & (OpcodeArgs::POP_TO_ME_FLAG | OpcodeArgs::NEED_NIL_FLAG);
                self.opcodes[*at as usize] = ScriptValue::from_opcode_args(
                    slot_op,
                    OpcodeArgs(OpcodeArgs::from_u32(slot).raw() | flags),
                );
            }
        }
        // patch the frame size to the high-water slot count (sealed loop
        // names shrink ctx.names but keep their allocated slots)
        let frame = ctx
            .lets
            .iter()
            .map(|(_, _, s)| *s + 1)
            .max()
            .unwrap_or(0)
            .max(ctx.names.iter().map(|(_, s, _)| *s + 1).max().unwrap_or(0));
        self.set_opcode_args(ctx.slots_frame_at, OpcodeArgs::from_u32(frame));
        // name table for symbolic bytecode consumers (shader compiler)
        let mut names = vec![LiveId(0); frame as usize];
        for (at_, name, slot) in &ctx.lets {
            let _ = at_;
            names[*slot as usize] = *name;
        }
        self.slot_frames.push((ctx.slots_frame_at, names));
    }

    fn slot_ctx(&mut self) -> Option<&mut SlotCtx> {
        self.slot_ctxs.last_mut()
    }

    /// A name that must stay dynamic in the innermost body.
    fn slot_poison(&mut self, name: LiveId) {
        if let Some(ctx) = self.slot_ctxs.last_mut() {
            if !ctx.poisoned.contains(&name) {
                ctx.poisoned.push(name);
            }
        }
    }

    /// Mark the innermost body ineligible (dynamic scope visibility needed).
    fn slot_disqualify_body(&mut self) {
        if let Some(ctx) = self.slot_ctxs.last_mut() {
            ctx.eligible = false;
        }
    }

    /// Log a bare identifier in value position (about to be emitted at
    /// code_len). Only names already slotted in the innermost body qualify —
    /// reads before the `let` stay dynamic, which is correct: the fn scope
    /// map never contains a slotted name, so those reads resolve to outer
    /// scopes exactly like the dynamic form.
    fn slot_note_read(&mut self, name: LiveId) {
        // never in field-name position: `a.x` parses x with EmitOp{.} below
        if matches!(
            self.state.last(),
            Some(State::EmitOp {
                what_op: id!(.) | id!(.?) | id!(me.),
                ..
            })
        ) {
            return;
        }
        let at = self.code_len();
        if let Some(ctx) = self.slot_ctxs.last_mut() {
            if let Some((_, slot, _)) = ctx.names.iter().find(|(n, _, _)| *n == name) {
                let slot = *slot;
                ctx.reads.push((at, name, slot));
            }
        }
    }

    /// `let name = expr` about to emit LET_DYN at code_len.
    fn slot_note_let(&mut self, name: LiveId) {
        if Self::is_reserved_binding(name) || name == id!(_) {
            return;
        }
        let at = self.code_len();
        if let Some(ctx) = self.slot_ctxs.last_mut() {
            let known = ctx.names.iter().any(|(n, _, _)| *n == name);
            if known {
                // re-let shadows (fn-level or per-iteration) — stay dynamic
                if !ctx.poisoned.contains(&name) {
                    ctx.poisoned.push(name);
                }
                return;
            }
            // inside a while/loop body the iteration shadow chain persists —
            // only for-loop bodies (scope reset per iteration) can slot lets
            if ctx.loop_kinds.last() == Some(&false) {
                return;
            }
            let slot = ctx
                .lets
                .iter()
                .map(|(_, _, s)| *s + 1)
                .max()
                .unwrap_or(0)
                .max(ctx.names.iter().map(|(_, s, _)| *s + 1).max().unwrap_or(0));
            let depth = ctx.loop_kinds.len() as u32;
            ctx.names.push((name, slot, depth));
            ctx.lets.push((at, name, slot));
        }
    }

    /// EndExpr saw an assign-family operator right after a bare id we logged.
    /// Returns Some(name) when the id was un-logged and the caller should
    /// route the assignment to a slot op at reduce time.
    fn slot_take_assign_target(&mut self, op: LiveId) -> Option<LiveId> {
        let last_at = self.code_len().checked_sub(1)?;
        let ctx = self.slot_ctxs.last_mut()?;
        let (at, name, _) = *ctx.reads.last()?;
        if at != last_at {
            return None;
        }
        // the last emitted value must still be that raw id
        if self.opcodes.last().map(|c| c.as_id()) != Some(Some(name)) {
            return None;
        }
        ctx.reads.pop();
        match op {
            // supported scope assigns -> slot rewrite at reduce
            x if x == id!(=)
                || x == id!(+=)
                || x == id!(-=)
                || x == id!(*=)
                || x == id!(/=)
                || x == id!(%=) =>
            {
                Some(name)
            }
            // object-key family: the id is a key, not a variable
            x if x == id!(:)
                || x == id!(:=)
                || x == id!(<:)
                || x == id!(>:)
                || x == id!(^:) =>
            {
                None
            }
            // unsupported scope assigns: the name must stay fully dynamic
            _ => {
                if !ctx.poisoned.contains(&name) {
                    ctx.poisoned.push(name);
                }
                None
            }
        }
    }

    /// An assign-family EmitOp with a slot target reduced at code_len.
    fn slot_note_assign(&mut self, name: LiveId, opcode: Opcode) {
        let at = self.code_len();
        if let Some(ctx) = self.slot_ctxs.last_mut() {
            if let Some((_, slot, _)) = ctx.names.iter().find(|(n, _, _)| *n == name) {
                let slot = *slot;
                ctx.assigns.push((at, name, slot, opcode));
            }
        }
    }

    fn slot_loop_enter(&mut self, is_for: bool) {
        if let Some(ctx) = self.slot_ctxs.last_mut() {
            ctx.loop_kinds.push(is_for);
        }
    }

    fn slot_loop_exit(&mut self) {
        if let Some(ctx) = self.slot_ctxs.last_mut() {
            ctx.loop_kinds.pop();
            // seal names defined inside the exited loop: reads after the
            // loop must resolve dynamically (outer scope or not-found),
            // not to the leftover slot value. Their in-loop candidates
            // stay logged and still rewrite.
            let depth = ctx.loop_kinds.len() as u32;
            ctx.names.retain(|(_, _, d)| *d <= depth);
        }
    }

    fn code_len(&self) -> u32 {
        self.opcodes.len() as _
    }

    fn code_last(&self) -> Option<&ScriptValue> {
        self.opcodes.last()
    }

    fn pop_code(&mut self) {
        self.opcodes.pop();
        self.source_map.pop();
    }

    fn push_code(&mut self, code: ScriptValue, index: u32) {
        self.opcodes.push(code);
        self.source_map.push(Some(index));
    }

    fn push_code_none(&mut self, code: ScriptValue) {
        self.opcodes.push(code);
        self.source_map.push(None);
    }

    /// Whether the parser is currently inside an unclosed `( )` / `[ ]` context
    /// (grouping, call args, array literal, or index), where newlines do not
    /// delimit statements. Scans the state stack for the nearest enclosing
    /// container: round/square = inside brackets, curly/block = statement list.
    fn inside_round_square_bracket(&self) -> bool {
        for state in self.state.iter().rev() {
            if state.is_round_square_container() {
                return true;
            }
            if state.is_statement_container() {
                return false;
            }
        }
        false
    }

    fn set_pop_to_me(&mut self) {
        let Some(code) = self.opcodes.last() else {
            return;
        };
        let fuse = match code.as_opcode() {
            // RETURN can't carry the flag, and fusing onto a jump-target
            // boundary would let a taken jump skip the commit (see the
            // last_jump_target field docs).
            Some((opcode, _args)) => {
                opcode != Opcode::RETURN && self.code_len() != self.last_jump_target
            }
            // A raw value slot can't carry the flag.
            None => false,
        };
        if fuse {
            self.opcodes.last_mut().unwrap().set_opcode_args_pop_to_me();
        } else {
            self.push_code(Opcode::POP_TO_ME.into(), self.index)
        }
    }

    fn has_pop_to_me(&self) -> bool {
        if let Some(code) = self.opcodes.last() {
            if let Some((opcode, args)) = code.as_opcode() {
                if opcode == Opcode::POP_TO_ME {
                    return true;
                }
                return args.0 & OpcodeArgs::POP_TO_ME_FLAG != 0;
            }
        }
        false
    }

    fn clear_pop_to_me(&mut self) {
        if let Some(code) = self.opcodes.last_mut() {
            if let Some((opcode, _)) = code.as_opcode() {
                if opcode == Opcode::POP_TO_ME {
                    self.pop_code();
                    return;
                }
            }
            code.clear_opcode_args_pop_to_me();
        }
    }

    fn set_opcode_args(&mut self, index: u32, args: OpcodeArgs) {
        self.opcodes[index as usize].set_opcode_args(args);
    }

    fn parse_step(
        &mut self,
        tokenizer: &ScriptTokenizer,
        tok: ScriptToken,
        values: &[ScriptValue],
    ) -> u32 {
        let op = tok.operator();
        let sep = tok.separator();
        let id = tok.identifier();
        match self.state.pop().unwrap() {
            State::ForIdent { idents, index } => {
                // we push k and v
                // allow optional parens: `for (x, y) in z` is identical to `for x, y in z`
                if tok.is_open_round() || tok.is_close_round() {
                    self.state.push(State::ForIdent { idents, index });
                    return 1;
                }
                if id.not_empty() {
                    if id == id!(in) {
                        // alright we move on to parsing the range expr
                        self.state.push(State::ForBody { idents, index });
                        self.state.push(State::BeginExpr { required: true });
                        return 1;
                    } else if idents < 3 {
                        if Self::is_reserved_binding(id) {
                            error!(
                                self,
                                tokenizer,
                                "'{}' is reserved and cannot be a for-loop variable",
                                id
                            );
                        }
                        // loop variables shadow per iteration — never a slot
                        self.slot_poison(id);
                        self.push_code(id.into(), self.index);
                        self.state.push(State::ForIdent {
                            idents: idents + 1,
                            index,
                        });
                        return 1;
                    } else {
                        error!(self, tokenizer, "Too many identifiers in for");
                        return 0;
                    }
                }
                if sep == id!(,) {
                    // eat the commas
                    self.state.push(State::ForIdent { idents, index });
                    return 1;
                }
                error!(self, tokenizer, "Unexpected state in parsing for");
            }
            State::ForBody { idents, index } => {
                // alright lets emit a for instruction

                let code_start = self.code_len();
                if idents == 1 {
                    self.slot_loop_enter(true);
                    self.push_code(Opcode::FOR_1.into(), index);
                } else if idents == 2 {
                    self.slot_loop_enter(true);
                    self.push_code(Opcode::FOR_2.into(), index);
                } else if idents == 3 {
                    self.slot_loop_enter(true);
                    self.push_code(Opcode::FOR_3.into(), index);
                } else {
                    error!(
                        self,
                        tokenizer, "Wrong number of identifiers for for loop {idents}"
                    );
                    return 0;
                }
                if tok.is_open_curly() {
                    self.state.push(State::ForBlock { code_start });
                    self.state.push(State::BeginStmt {
                        last_was_sep: false,
                    });
                    return 1;
                } else {
                    self.state.push(State::ForExpr { code_start });
                    self.state.push(State::BeginExpr { required: true });
                }
            }
            State::Loop { index } => {
                let code_start = self.code_len();
                self.slot_loop_enter(false);
                self.push_code(Opcode::LOOP.into(), index);
                if tok.is_open_curly() {
                    self.state.push(State::ForBlock { code_start });
                    self.state.push(State::BeginStmt {
                        last_was_sep: false,
                    });
                    return 1;
                } else {
                    self.state.push(State::ForExpr { code_start });
                    self.state.push(State::BeginExpr { required: true });
                }
            }
            State::While { index } => {
                let code_start = self.code_len();
                self.slot_loop_enter(false);
                self.push_code(Opcode::LOOP.into(), index);
                self.state.push(State::WhileTest { code_start });
                self.state.push(State::BeginExpr { required: true });
            }
            State::WhileTest { code_start } => {
                self.push_code(Opcode::BREAKIFNOT.into(), self.index);
                if tok.is_open_curly() {
                    self.state.push(State::ForBlock { code_start });
                    self.state.push(State::BeginStmt {
                        last_was_sep: false,
                    });
                    return 1;
                } else {
                    self.state.push(State::ForExpr { code_start });
                    self.state.push(State::BeginExpr { required: true });
                }
            }
            // Match expression desugaring:
            // match x { 1 => true, 2 => {false} }
            // becomes:
            // let $mN = x
            // if $mN == 1 true else if $mN == 2 {false}
            State::MatchSubject { temp_id, index } => {
                // Subject expression has been parsed
                // Order is now: [temp_id, subject_value] - emit LET_DYN
                self.push_code(Opcode::LET_DYN.into(), index);
                self.state.push(State::MatchBlock { temp_id, index });
            }
            State::MatchBlock { temp_id, index } => {
                if tok.is_open_curly() {
                    self.state.push(State::MatchArmPattern {
                        temp_id,
                        first: true,
                        prev_else_start: 0,
                        index,
                    });
                    return 1;
                } else {
                    error!(self, tokenizer, "Expected {{ after match expression");
                }
            }
            State::MatchArmPattern {
                temp_id,
                first: _,
                prev_else_start,
                index,
            } => {
                if tok.is_close_curly() {
                    // Empty match - patch any pending IF_ELSE and push nil
                    if prev_else_start > 0 {
                        self.set_opcode_args(
                            prev_else_start,
                            OpcodeArgs::from_u32(self.code_len() as u32 - prev_else_start),
                        );
                    }
                    self.push_code(NIL, index);
                    self.state.push(State::EndExpr);
                    return 1;
                }
                // Check for wildcard pattern `_`
                if id == id!(_) {
                    // Wildcard pattern - just expect => and body
                    self.state.push(State::MatchWildcardArrow {
                        prev_else_start,
                        index,
                    });
                    return 1;
                }
                // Emit temp_id to push the match subject value
                self.push_code(ScriptValue::from_id(temp_id), index);
                // Parse the pattern expression
                self.state.push(State::MatchArmArrow {
                    temp_id,
                    prev_else_start,
                    index,
                });
                self.state.push(State::BeginExpr { required: true });
            }
            State::MatchArmArrow {
                temp_id,
                prev_else_start,
                index,
            } => {
                // Pattern expression done, emit EQ and IF_TEST
                self.push_code(Opcode::EQ.into(), index);
                let if_start = self.code_len();
                self.push_code(Opcode::IF_TEST.into(), index);
                // Expect =>
                if op == id!(=>) {
                    // Parse arm body - check for block or expression
                    self.state.push(State::MatchArmBody {
                        temp_id,
                        if_start,
                        prev_else_start,
                        index,
                    });
                    return 1;
                } else {
                    error!(
                        self,
                        tokenizer, "Expected => after match pattern, got {:?}", tok
                    );
                }
            }
            State::MatchArmBody {
                temp_id,
                if_start,
                prev_else_start,
                index,
            } => {
                // Check if arm body is a block or expression (like IfTest does)
                if tok.is_open_curly() {
                    // Block body - use BeginStmt like if blocks
                    self.state.push(State::MatchArmBlock {
                        temp_id,
                        if_start,
                        prev_else_start,
                        last_was_sep: false,
                        index,
                    });
                    self.state.push(State::BeginStmt {
                        last_was_sep: false,
                    });
                    return 1;
                }
                // Expression body
                self.state.push(State::MatchMaybeArm {
                    temp_id,
                    if_start,
                    prev_else_start,
                    index,
                });
                self.state.push(State::BeginExpr { required: true });
            }
            State::MatchArmBlock {
                temp_id,
                if_start,
                prev_else_start,
                last_was_sep,
                index,
            } => {
                // Block body done, expect }
                if tok.is_close_curly() {
                    if !last_was_sep && self.has_pop_to_me() {
                        self.clear_pop_to_me();
                    }
                    self.state.push(State::MatchMaybeArm {
                        temp_id,
                        if_start,
                        prev_else_start,
                        index,
                    });
                    return 1;
                } else {
                    error!(self, tokenizer, "Expected }} in match arm block");
                }
            }
            State::MatchWildcardArrow {
                prev_else_start,
                index,
            } => {
                // Wildcard pattern done, expect =>
                if op == id!(=>) {
                    // Parse wildcard arm body - check for block or expression
                    self.state.push(State::MatchWildcardBody {
                        prev_else_start,
                        index,
                    });
                    return 1;
                } else {
                    error!(
                        self,
                        tokenizer, "Expected => after _ wildcard pattern, got {:?}", tok
                    );
                }
            }
            State::MatchWildcardBody {
                prev_else_start,
                index,
            } => {
                // Check if wildcard body is a block or expression
                if tok.is_open_curly() {
                    // Block body
                    self.state.push(State::MatchWildcardBlock {
                        prev_else_start,
                        last_was_sep: false,
                        index,
                    });
                    self.state.push(State::BeginStmt {
                        last_was_sep: false,
                    });
                    return 1;
                }
                // Expression body
                self.state.push(State::MatchWildcardEnd {
                    prev_else_start,
                    index,
                });
                self.state.push(State::BeginExpr { required: true });
            }
            State::MatchWildcardBlock {
                prev_else_start,
                last_was_sep,
                index,
            } => {
                // Wildcard block body done, expect }
                if tok.is_close_curly() {
                    if !last_was_sep && self.has_pop_to_me() {
                        self.clear_pop_to_me();
                    }
                    self.state.push(State::MatchWildcardEnd {
                        prev_else_start,
                        index,
                    });
                    return 1;
                } else {
                    error!(self, tokenizer, "Expected }} in wildcard arm block");
                }
            }
            State::MatchWildcardEnd {
                prev_else_start,
                index: _,
            } => {
                // Wildcard body done - patch any pending IF_ELSE to jump here
                if prev_else_start > 0 {
                    self.last_jump_target = self.code_len();
                    self.set_opcode_args(
                        prev_else_start,
                        OpcodeArgs::from_u32(self.code_len() as u32 - prev_else_start),
                    );
                }
                // Expect }
                if tok.is_close_curly() {
                    self.state.push(State::EndExpr);
                    return 1;
                } else {
                    error!(
                        self,
                        tokenizer, "Expected }} after wildcard arm (no more arms allowed after _)"
                    );
                }
            }
            State::MatchMaybeArm {
                temp_id,
                if_start,
                prev_else_start,
                index,
            } => {
                // Arm body parsed. Now emit IF_ELSE and patch IF_TEST.

                // First, patch any previous arm's IF_ELSE to jump here (to current position)
                if prev_else_start > 0 {
                    self.last_jump_target = self.code_len();
                    self.set_opcode_args(
                        prev_else_start,
                        OpcodeArgs::from_u32(self.code_len() as u32 - prev_else_start),
                    );
                }

                if tok.is_close_curly() {
                    // End of match - no more arms, patch IF_TEST with need_nil flag
                    self.last_jump_target = self.code_len();
                    self.set_opcode_args(
                        if_start,
                        OpcodeArgs::from_u32(self.code_len() as u32 - if_start).set_need_nil(),
                    );
                    self.state.push(State::EndExpr);
                    return 1;
                }

                // More arms coming - emit IF_ELSE to skip remaining arms after this arm's body
                let else_start = self.code_len();
                self.push_code(Opcode::IF_ELSE.into(), index);

                // Patch IF_TEST to jump here (after arm body and IF_ELSE) to next arm
                self.set_opcode_args(
                    if_start,
                    OpcodeArgs::from_u32(self.code_len() as u32 - if_start),
                );

                // Check for wildcard or regular arm
                if id == id!(_) {
                    // Next arm is wildcard - it's the final else body
                    self.state.push(State::MatchWildcardArrow {
                        prev_else_start: else_start,
                        index,
                    });
                    return 1;
                }

                // Continue with next regular arm, passing this arm's else_start
                self.state.push(State::MatchArmPattern {
                    temp_id,
                    first: false,
                    prev_else_start: else_start,
                    index,
                });
            }
            State::ForExpr { code_start } => {
                self.set_pop_to_me();
                //self.push_code_none(Opcode::POP_TO_ME.into());
                self.slot_loop_exit();
                self.push_code_none(Opcode::FOR_END.into());
                let jump_to = (self.code_len() - code_start) as _;
                self.set_opcode_args(code_start, OpcodeArgs::from_u32(jump_to));
                return 0;
            }
            State::ForBlock { code_start } => {
                if tok.is_close_curly() {
                    self.slot_loop_exit();
                self.push_code_none(Opcode::FOR_END.into());
                    let jump_to = (self.code_len() - code_start) as _;
                    self.set_opcode_args(code_start, OpcodeArgs::from_u32(jump_to));
                    return 1;
                } else {
                    error!(self, tokenizer, "Expected }} not found in for");
                    return 0;
                }
            }
            State::Use { index } => {
                if let Some(code) = self.opcodes.last() {
                    if let Some((Opcode::FIELD, _)) = code.as_opcode() {
                        self.pop_code();
                        {self.slot_disqualify_body(); self.push_code(Opcode::USE.into(), index)}
                    } else {
                        error!(self, tokenizer, "Error use expected field operation")
                    }
                }
            }
            State::Let { index } => {
                if id == id!(mut) {
                    // "let mut" is treated as "var"
                    self.state.push(State::Var { index });
                    return 1;
                } else if tok.is_open_square() {
                    // Array destructuring: let [x, y] = ...
                    let ids_start = self.code_len();
                    self.state.push(State::LetArrayDestruct {
                        index,
                        count: 0,
                        ids_start,
                    });
                    return 1;
                } else if tok.is_open_curly() {
                    // Object destructuring: let {x, y} = ...
                    let ids_start = self.code_len();
                    self.state.push(State::LetObjectDestruct {
                        index,
                        count: 0,
                        ids_start,
                    });
                    return 1;
                } else if id.not_empty() {
                    if Self::is_reserved_binding(id) {
                        error!(
                            self,
                            tokenizer,
                            "'{}' is reserved and cannot be a variable name — 'me' and 'scope' are the implicit references; pick another name",
                            id
                        );
                    }
                    // lets expect an assignment expression
                    // push the id on to the stack
                    self.push_code(id.into(), self.index);
                    self.state.push(State::LetDynOrTyped { index, name: id });
                    return 1;
                } else {
                    // unknown
                    error!(self, tokenizer, "Let expected identifier");
                }
            }
            State::LetDynOrTyped { index, name } => {
                if op == id!(=) {
                    // assignment following
                    self.state.push(State::EmitLetDyn { index, name });
                    self.state.push(State::BeginExpr { required: true });
                    return 1;
                } else if op == id!(:) {
                    // typed let: stays dynamic; the name must too
                    self.slot_poison(name);
                    self.state.push(State::LetType { index });
                    return 1;
                } else {
                    // `let x` without value: stays dynamic; the name must too
                    self.slot_poison(name);
                    self.push_code(
                        ScriptValue::from_opcode_args(Opcode::LET_DYN, OpcodeArgs::NIL),
                        index,
                    );
                }
            }
            State::LetType { index } => {
                if id.not_empty() {
                    // lets expect an assignment expression
                    // push the id on to the stack
                    self.push_code(id.into(), self.index);
                    self.state.push(State::LetTypedAssign { index });
                    return 1;
                } else {
                    // unknown
                    error!(self, tokenizer, "Let type expected");
                }
            }
            State::LetTypedAssign { index } => {
                if op == id!(=) {
                    // assignment following
                    self.state.push(State::EmitLetTyped { index });
                    self.state.push(State::BeginExpr { required: true });
                    return 1;
                } else {
                    self.push_code(
                        ScriptValue::from_opcode_args(Opcode::LET_TYPED, OpcodeArgs::NIL),
                        index,
                    );
                }
            }
            State::EmitLetDyn { index, name } => {
                self.slot_note_let(name);
                self.push_code(Opcode::LET_DYN.into(), index);
            }
            State::EmitLetTyped { index } => {
                self.push_code(Opcode::LET_TYPED.into(), index);
            }

            // ====== Array Destructuring: let [x, y = default, ...] = expr ======
            // Layout during parsing: [ids..., defaults_with_ifnil..., rhs]
            // Final generated code: [rhs, id_0, EXTRACT(0), id_1, EXTRACT(1), ..., DROP, defaults_with_ifnil]
            State::LetArrayDestruct {
                index,
                count,
                ids_start,
            } => {
                if sep == id!(,) || sep == id!(;) {
                    self.state.push(State::LetArrayDestruct {
                        index,
                        count,
                        ids_start,
                    });
                    return 1;
                }
                if id.not_empty() {
                    // Just push the identifier
                    self.push_code(id.into(), self.index);
                    self.state.push(State::LetArrayDestructEl {
                        index,
                        count: count + 1,
                        binding_id: id,
                        ids_start,
                    });
                    return 1;
                } else if tok.is_open_curly() {
                    // Nested object pattern: let [{x, y}] = ...
                    // Push a marker that will be replaced with the nested pattern index
                    self.push_code(Opcode::NOP.into(), self.index); // placeholder
                    self.state.push(State::LetArrayDestructNestedObject {
                        index,
                        outer_count: count + 1,
                        outer_ids_start: ids_start,
                        nested_count: 0,
                    });
                    return 1;
                } else if tok.is_open_square() {
                    // Nested array pattern: let [[x, y]] = ...
                    self.push_code(Opcode::NOP.into(), self.index); // placeholder
                    self.state.push(State::LetArrayDestructNestedArray {
                        index,
                        outer_count: count + 1,
                        outer_ids_start: ids_start,
                        nested_count: 0,
                    });
                    return 1;
                } else if tok.is_close_square() {
                    let defaults_start = self.code_len();
                    self.state.push(State::LetArrayDestructRhs {
                        index,
                        count,
                        ids_start,
                        defaults_start,
                    });
                    return 1;
                } else {
                    error!(
                        self,
                        tokenizer, "Expected identifier in array destructuring pattern"
                    );
                }
            }
            State::LetArrayDestructEl {
                index,
                count,
                binding_id,
                ids_start,
            } => {
                if op == id!(=) {
                    // Default value follows - parse it
                    let default_start = self.code_len();
                    self.state.push(State::LetArrayDestructDefault {
                        index,
                        count,
                        binding_id,
                        ids_start,
                        default_start,
                    });
                    self.state.push(State::BeginExpr { required: true });
                    return 1;
                } else if sep == id!(,) || sep == id!(;) {
                    self.state.push(State::LetArrayDestruct {
                        index,
                        count,
                        ids_start,
                    });
                    return 1;
                } else if id.not_empty() {
                    self.push_code(id.into(), self.index);
                    self.state.push(State::LetArrayDestructEl {
                        index,
                        count: count + 1,
                        binding_id: id,
                        ids_start,
                    });
                    return 1;
                } else if tok.is_close_square() {
                    let defaults_start = self.code_len();
                    self.state.push(State::LetArrayDestructRhs {
                        index,
                        count,
                        ids_start,
                        defaults_start,
                    });
                    return 1;
                } else {
                    error!(
                        self,
                        tokenizer, "Expected '=' or identifier in array destructuring pattern"
                    );
                }
            }
            State::LetArrayDestructDefault {
                index,
                count,
                binding_id,
                ids_start,
                default_start,
            } => {
                // Default was parsed (value is in code stream at default_start..current)
                // Store it for later emission, don't emit inline
                let default_start = default_start as usize;
                let value_code: Vec<_> = self.opcodes[default_start..].to_vec();
                let value_map: Vec<_> = self.source_map[default_start..].to_vec();
                self.opcodes.truncate(default_start);
                self.source_map.truncate(default_start);

                // Store the default for later
                self.destruct_defaults
                    .push((binding_id, value_code, value_map));

                if sep == id!(,) || sep == id!(;) {
                    self.state.push(State::LetArrayDestruct {
                        index,
                        count,
                        ids_start,
                    });
                    return 1;
                } else if id.not_empty() {
                    self.push_code(id.into(), self.index);
                    self.state.push(State::LetArrayDestructEl {
                        index,
                        count: count + 1,
                        binding_id: id,
                        ids_start,
                    });
                    return 1;
                } else if tok.is_close_square() {
                    let defaults_start = self.code_len();
                    self.state.push(State::LetArrayDestructRhs {
                        index,
                        count,
                        ids_start,
                        defaults_start,
                    });
                    return 1;
                } else {
                    self.state.push(State::LetArrayDestruct {
                        index,
                        count,
                        ids_start,
                    });
                    return 0;
                }
            }
            // Nested object pattern inside array: let [{x, y}] = ...
            State::LetArrayDestructNestedObject {
                index,
                outer_count,
                outer_ids_start,
                nested_count,
            } => {
                if sep == id!(,) || sep == id!(;) {
                    self.state.push(State::LetArrayDestructNestedObject {
                        index,
                        outer_count,
                        outer_ids_start,
                        nested_count,
                    });
                    return 1;
                }
                if id.not_empty() {
                    self.state.push(State::LetArrayDestructNestedObjectEl {
                        index,
                        outer_count,
                        outer_ids_start,
                        nested_count: nested_count + 1,
                        binding_id: id,
                    });
                    return 1;
                } else if tok.is_close_curly() {
                    // Nested pattern complete - bindings were collected in NestedObjectEl
                    if let Some(NestedPattern::Object(ref bindings)) = self.nested_patterns.last() {
                        if bindings.len() == nested_count as usize {
                            // Pattern is complete, update the placeholder with the pattern index
                            let pattern_idx = self.nested_patterns.len() - 1;
                            let placeholder_pos =
                                outer_ids_start as usize + outer_count as usize - 1;
                            // Encode the nested pattern as a special marker: NOP with args = pattern_index + 1 (so 0 means "not nested")
                            self.opcodes[placeholder_pos] = ScriptValue::from_opcode_args(
                                Opcode::NOP,
                                OpcodeArgs::from_u32((pattern_idx + 1) as u32),
                            );
                        }
                    }
                    self.state.push(State::LetArrayDestruct {
                        index,
                        count: outer_count,
                        ids_start: outer_ids_start,
                    });
                    return 1;
                } else {
                    error!(
                        self,
                        tokenizer, "Expected identifier in nested object pattern"
                    );
                }
            }
            State::LetArrayDestructNestedObjectEl {
                index,
                outer_count,
                outer_ids_start,
                nested_count,
                binding_id,
            } => {
                // Store the binding in the nested pattern
                if nested_count == 1 {
                    // First binding - create new pattern
                    self.nested_patterns
                        .push(NestedPattern::Object(vec![binding_id]));
                } else {
                    // Add to existing pattern
                    if let Some(NestedPattern::Object(ref mut bindings)) =
                        self.nested_patterns.last_mut()
                    {
                        bindings.push(binding_id);
                    }
                }

                if sep == id!(,) || sep == id!(;) {
                    self.state.push(State::LetArrayDestructNestedObject {
                        index,
                        outer_count,
                        outer_ids_start,
                        nested_count,
                    });
                    return 1;
                } else if id.not_empty() {
                    self.state.push(State::LetArrayDestructNestedObjectEl {
                        index,
                        outer_count,
                        outer_ids_start,
                        nested_count: nested_count + 1,
                        binding_id: id,
                    });
                    return 1;
                } else if tok.is_close_curly() {
                    // Pattern complete - update placeholder
                    let pattern_idx = self.nested_patterns.len() - 1;
                    let placeholder_pos = outer_ids_start as usize + outer_count as usize - 1;
                    self.opcodes[placeholder_pos] = ScriptValue::from_opcode_args(
                        Opcode::NOP,
                        OpcodeArgs::from_u32((pattern_idx + 1) as u32),
                    );
                    self.state.push(State::LetArrayDestruct {
                        index,
                        count: outer_count,
                        ids_start: outer_ids_start,
                    });
                    return 1;
                } else {
                    self.state.push(State::LetArrayDestructNestedObject {
                        index,
                        outer_count,
                        outer_ids_start,
                        nested_count,
                    });
                    return 0;
                }
            }

            // Nested array pattern inside array: let [[x, y]] = ...
            State::LetArrayDestructNestedArray {
                index,
                outer_count,
                outer_ids_start,
                nested_count,
            } => {
                if sep == id!(,) || sep == id!(;) {
                    self.state.push(State::LetArrayDestructNestedArray {
                        index,
                        outer_count,
                        outer_ids_start,
                        nested_count,
                    });
                    return 1;
                }
                if id.not_empty() {
                    self.state.push(State::LetArrayDestructNestedArrayEl {
                        index,
                        outer_count,
                        outer_ids_start,
                        nested_count: nested_count + 1,
                        binding_id: id,
                    });
                    return 1;
                } else if tok.is_close_square() {
                    // Nested pattern complete
                    if let Some(NestedPattern::Array(ref bindings)) = self.nested_patterns.last() {
                        if bindings.len() == nested_count as usize {
                            let pattern_idx = self.nested_patterns.len() - 1;
                            let placeholder_pos =
                                outer_ids_start as usize + outer_count as usize - 1;
                            self.opcodes[placeholder_pos] = ScriptValue::from_opcode_args(
                                Opcode::NOP,
                                OpcodeArgs::from_u32((pattern_idx + 1) as u32),
                            );
                        }
                    }
                    self.state.push(State::LetArrayDestruct {
                        index,
                        count: outer_count,
                        ids_start: outer_ids_start,
                    });
                    return 1;
                } else {
                    error!(
                        self,
                        tokenizer, "Expected identifier in nested array pattern"
                    );
                }
            }
            State::LetArrayDestructNestedArrayEl {
                index,
                outer_count,
                outer_ids_start,
                nested_count,
                binding_id,
            } => {
                // Store the binding
                if nested_count == 1 {
                    self.nested_patterns
                        .push(NestedPattern::Array(vec![binding_id]));
                } else {
                    if let Some(NestedPattern::Array(ref mut bindings)) =
                        self.nested_patterns.last_mut()
                    {
                        bindings.push(binding_id);
                    }
                }

                if sep == id!(,) || sep == id!(;) {
                    self.state.push(State::LetArrayDestructNestedArray {
                        index,
                        outer_count,
                        outer_ids_start,
                        nested_count,
                    });
                    return 1;
                } else if id.not_empty() {
                    self.state.push(State::LetArrayDestructNestedArrayEl {
                        index,
                        outer_count,
                        outer_ids_start,
                        nested_count: nested_count + 1,
                        binding_id: id,
                    });
                    return 1;
                } else if tok.is_close_square() {
                    let pattern_idx = self.nested_patterns.len() - 1;
                    let placeholder_pos = outer_ids_start as usize + outer_count as usize - 1;
                    self.opcodes[placeholder_pos] = ScriptValue::from_opcode_args(
                        Opcode::NOP,
                        OpcodeArgs::from_u32((pattern_idx + 1) as u32),
                    );
                    self.state.push(State::LetArrayDestruct {
                        index,
                        count: outer_count,
                        ids_start: outer_ids_start,
                    });
                    return 1;
                } else {
                    self.state.push(State::LetArrayDestructNestedArray {
                        index,
                        outer_count,
                        outer_ids_start,
                        nested_count,
                    });
                    return 0;
                }
            }

            State::LetArrayDestructRhs {
                index,
                count,
                ids_start,
                defaults_start,
            } => {
                if op == id!(=) {
                    self.state.push(State::EmitLetArrayDestruct {
                        index,
                        count,
                        ids_start,
                        defaults_start,
                    });
                    self.state.push(State::BeginExpr { required: true });
                    return 1;
                } else {
                    error!(self, tokenizer, "Expected '=' after destructuring pattern");
                }
            }
            State::EmitLetArrayDestruct {
                index,
                count,
                ids_start,
                defaults_start,
            } => {
                // Layout: [ids..., rhs_code], defaults stored in destruct_defaults
                // Generate: [rhs, id_0, EXTRACT(0), ..., DROP, defaults_code...]
                // For nested patterns: [rhs, DUP, idx, ARRAY_INDEX_NIL, nested_bindings..., DROP, ...]

                let ids_start = ids_start as usize;
                let defaults_start = defaults_start as usize;
                let count = count as usize;

                let ids: Vec<_> = self.opcodes[ids_start..ids_start + count].to_vec();
                let ids_map: Vec<_> = self.source_map[ids_start..ids_start + count].to_vec();
                let rhs: Vec<_> = self.opcodes[defaults_start..].to_vec();
                let rhs_map: Vec<_> = self.source_map[defaults_start..].to_vec();

                for id_code in &ids {
                    if let Some(bid) = id_code.as_id() {
                        if Self::is_reserved_binding(bid) {
                            error!(
                                self,
                                tokenizer,
                                "'{}' is reserved and cannot be bound in a destructuring pattern",
                                bid
                            );
                        }
                    }
                }
                let mut reserved_nested: Vec<LiveId> = Vec::new();
                for pattern in &self.nested_patterns {
                    let bindings = match pattern {
                        NestedPattern::Object(b) | NestedPattern::Array(b) => b,
                    };
                    for bid in bindings {
                        if Self::is_reserved_binding(*bid) {
                            reserved_nested.push(*bid);
                        }
                    }
                }
                for bid in reserved_nested {
                    error!(
                        self,
                        tokenizer,
                        "'{}' is reserved and cannot be bound in a destructuring pattern",
                        bid
                    );
                }

                self.opcodes.truncate(ids_start);
                self.source_map.truncate(ids_start);

                // RHS first
                self.opcodes.extend(rhs);
                self.source_map.extend(rhs_map);

                // Process each element - check for nested patterns
                for (i, (id_code, id_map)) in ids.into_iter().zip(ids_map).enumerate() {
                    // Check if this is a nested pattern marker (NOP with non-zero args)
                    if let Some((opcode, args)) = id_code.as_opcode() {
                        if opcode == Opcode::NOP && args.to_u32() > 0 {
                            // This is a nested pattern
                            let pattern_idx = (args.to_u32() - 1) as usize;
                            if let Some(pattern) = self.nested_patterns.get(pattern_idx).cloned() {
                                // Emit: DUP, index, ARRAY_INDEX_NIL, nested extraction, DROP
                                self.push_code(Opcode::DUP.into(), index);
                                self.push_code(i.into(), index);
                                self.push_code(Opcode::ARRAY_INDEX_NIL.into(), index);

                                match pattern {
                                    NestedPattern::Object(bindings) => {
                                        // For nested object: id, LET_DESTRUCT_OBJECT_EL for each binding
                                        for binding_id in bindings {
                                            self.slot_poison(binding_id);
                                            self.push_code(binding_id.into(), index);
                                            self.push_code(
                                                Opcode::LET_DESTRUCT_OBJECT_EL.into(),
                                                index,
                                            );
                                        }
                                    }
                                    NestedPattern::Array(bindings) => {
                                        // For nested array: id, LET_DESTRUCT_ARRAY_EL(j) for each binding
                                        for (j, binding_id) in bindings.into_iter().enumerate() {
                                            self.slot_poison(binding_id);
                                            self.push_code(binding_id.into(), index);
                                            self.push_code(
                                                ScriptValue::from_opcode_args(
                                                    Opcode::LET_DESTRUCT_ARRAY_EL,
                                                    OpcodeArgs::from_u32(j as u32),
                                                ),
                                                index,
                                            );
                                        }
                                    }
                                }

                                // DROP the nested source
                                self.push_code(Opcode::DROP.into(), index);
                                continue;
                            }
                        }
                    }

                    // Simple identifier - emit normally
                    if let Some(bound) = id_code.as_id() {
                        self.slot_poison(bound);
                    }
                    self.opcodes.push(id_code);
                    self.source_map.push(id_map);
                    self.push_code(
                        ScriptValue::from_opcode_args(
                            Opcode::LET_DESTRUCT_ARRAY_EL,
                            OpcodeArgs::from_u32(i as u32),
                        ),
                        index,
                    );
                }

                // DROP outer source
                self.push_code(Opcode::DROP.into(), index);

                // Emit stored defaults with lazy evaluation: [id, ASSIGN_IFNIL(jump), value, ASSIGN]
                // jump_dist = value_code.len() + 2 to skip past value_code AND the ASSIGN opcode
                let defaults = std::mem::take(&mut self.destruct_defaults);
                for (binding_id, value_code, value_map) in defaults {
                    let jump_dist = (value_code.len() + 2) as u32;
                    self.push_code(binding_id.into(), index);
                    self.push_code(
                        ScriptValue::from_opcode_args(
                            Opcode::ASSIGN_IFNIL,
                            OpcodeArgs::from_u32(jump_dist),
                        ),
                        index,
                    );
                    self.opcodes.extend(value_code);
                    self.source_map.extend(value_map);
                    self.push_code(Opcode::ASSIGN.into(), index);
                }

                // Clear nested patterns for next destructuring
                self.nested_patterns.clear();

                // This is a let statement, go directly to BeginStmt to avoid pop_to_me being set on DROP
                self.state.push(State::BeginStmt {
                    last_was_sep: false,
                });
                return 0;
            }

            // ====== Object Destructuring: let {x, y = default, ...} = expr ======
            State::LetObjectDestruct {
                index,
                count,
                ids_start,
            } => {
                if sep == id!(,) || sep == id!(;) {
                    self.state.push(State::LetObjectDestruct {
                        index,
                        count,
                        ids_start,
                    });
                    return 1;
                }
                if id.not_empty() {
                    self.push_code(id.into(), self.index);
                    self.state.push(State::LetObjectDestructEl {
                        index,
                        count: count + 1,
                        binding_id: id,
                        ids_start,
                    });
                    return 1;
                } else if tok.is_close_curly() {
                    let defaults_start = self.code_len();
                    self.state.push(State::LetObjectDestructRhs {
                        index,
                        count,
                        ids_start,
                        defaults_start,
                    });
                    return 1;
                } else {
                    error!(
                        self,
                        tokenizer, "Expected identifier in object destructuring pattern"
                    );
                }
            }
            State::LetObjectDestructEl {
                index,
                count,
                binding_id,
                ids_start,
            } => {
                if op == id!(=) {
                    // Default value follows - parse it
                    let default_start = self.code_len();
                    self.state.push(State::LetObjectDestructDefault {
                        index,
                        count,
                        binding_id,
                        ids_start,
                        default_start,
                    });
                    self.state.push(State::BeginExpr { required: true });
                    return 1;
                } else if sep == id!(,) || sep == id!(;) {
                    self.state.push(State::LetObjectDestruct {
                        index,
                        count,
                        ids_start,
                    });
                    return 1;
                } else if id.not_empty() {
                    self.push_code(id.into(), self.index);
                    self.state.push(State::LetObjectDestructEl {
                        index,
                        count: count + 1,
                        binding_id: id,
                        ids_start,
                    });
                    return 1;
                } else if tok.is_close_curly() {
                    let defaults_start = self.code_len();
                    self.state.push(State::LetObjectDestructRhs {
                        index,
                        count,
                        ids_start,
                        defaults_start,
                    });
                    return 1;
                } else {
                    error!(
                        self,
                        tokenizer, "Expected '=' or identifier in object destructuring pattern"
                    );
                }
            }
            State::LetObjectDestructDefault {
                index,
                count,
                binding_id,
                ids_start,
                default_start,
            } => {
                // Default was parsed - store it for later emission, don't emit inline
                let default_start = default_start as usize;
                let value_code: Vec<_> = self.opcodes[default_start..].to_vec();
                let value_map: Vec<_> = self.source_map[default_start..].to_vec();
                self.opcodes.truncate(default_start);
                self.source_map.truncate(default_start);

                // Store the default for later
                self.destruct_defaults
                    .push((binding_id, value_code, value_map));

                if sep == id!(,) || sep == id!(;) {
                    self.state.push(State::LetObjectDestruct {
                        index,
                        count,
                        ids_start,
                    });
                    return 1;
                } else if id.not_empty() {
                    self.push_code(id.into(), self.index);
                    self.state.push(State::LetObjectDestructEl {
                        index,
                        count: count + 1,
                        binding_id: id,
                        ids_start,
                    });
                    return 1;
                } else if tok.is_close_curly() {
                    let defaults_start = self.code_len();
                    self.state.push(State::LetObjectDestructRhs {
                        index,
                        count,
                        ids_start,
                        defaults_start,
                    });
                    return 1;
                } else {
                    self.state.push(State::LetObjectDestruct {
                        index,
                        count,
                        ids_start,
                    });
                    return 0;
                }
            }
            // Nested array pattern inside object: let {a: [x, y]} = ...
            State::LetObjectDestructNestedArray {
                index,
                outer_count,
                outer_ids_start,
                key,
                nested_count,
            } => {
                if sep == id!(,) || sep == id!(;) {
                    self.state.push(State::LetObjectDestructNestedArray {
                        index,
                        outer_count,
                        outer_ids_start,
                        key,
                        nested_count,
                    });
                    return 1;
                }
                if id.not_empty() {
                    self.state.push(State::LetObjectDestructNestedArrayEl {
                        index,
                        outer_count,
                        outer_ids_start,
                        key,
                        nested_count: nested_count + 1,
                        binding_id: id,
                    });
                    return 1;
                } else if tok.is_close_square() {
                    // Pattern complete
                    if let Some(NestedPattern::Array(ref bindings)) = self.nested_patterns.last() {
                        if bindings.len() == nested_count as usize {
                            let pattern_idx = self.nested_patterns.len() - 1;
                            let placeholder_pos =
                                outer_ids_start as usize + outer_count as usize - 1;
                            self.opcodes[placeholder_pos] = ScriptValue::from_opcode_args(
                                Opcode::NOP,
                                OpcodeArgs::from_u32((pattern_idx + 1) as u32),
                            );
                        }
                    }
                    self.state.push(State::LetObjectDestruct {
                        index,
                        count: outer_count,
                        ids_start: outer_ids_start,
                    });
                    return 1;
                } else {
                    error!(
                        self,
                        tokenizer, "Expected identifier in nested array pattern"
                    );
                }
            }
            State::LetObjectDestructNestedArrayEl {
                index,
                outer_count,
                outer_ids_start,
                key: _,
                nested_count,
                binding_id,
            } => {
                if nested_count == 1 {
                    self.nested_patterns
                        .push(NestedPattern::Array(vec![binding_id]));
                } else {
                    if let Some(NestedPattern::Array(ref mut bindings)) =
                        self.nested_patterns.last_mut()
                    {
                        bindings.push(binding_id);
                    }
                }

                if sep == id!(,) || sep == id!(;) {
                    self.state.push(State::LetObjectDestructNestedArray {
                        index,
                        outer_count,
                        outer_ids_start,
                        key: LiveId::empty(),
                        nested_count,
                    });
                    return 1;
                } else if id.not_empty() {
                    self.state.push(State::LetObjectDestructNestedArrayEl {
                        index,
                        outer_count,
                        outer_ids_start,
                        key: LiveId::empty(),
                        nested_count: nested_count + 1,
                        binding_id: id,
                    });
                    return 1;
                } else if tok.is_close_square() {
                    let pattern_idx = self.nested_patterns.len() - 1;
                    let placeholder_pos = outer_ids_start as usize + outer_count as usize - 1;
                    self.opcodes[placeholder_pos] = ScriptValue::from_opcode_args(
                        Opcode::NOP,
                        OpcodeArgs::from_u32((pattern_idx + 1) as u32),
                    );
                    self.state.push(State::LetObjectDestruct {
                        index,
                        count: outer_count,
                        ids_start: outer_ids_start,
                    });
                    return 1;
                } else {
                    self.state.push(State::LetObjectDestructNestedArray {
                        index,
                        outer_count,
                        outer_ids_start,
                        key: LiveId::empty(),
                        nested_count,
                    });
                    return 0;
                }
            }

            // Nested object pattern inside object: let {a: {x, y}} = ...
            State::LetObjectDestructNestedObject {
                index,
                outer_count,
                outer_ids_start,
                key,
                nested_count,
            } => {
                if sep == id!(,) || sep == id!(;) {
                    self.state.push(State::LetObjectDestructNestedObject {
                        index,
                        outer_count,
                        outer_ids_start,
                        key,
                        nested_count,
                    });
                    return 1;
                }
                if id.not_empty() {
                    self.state.push(State::LetObjectDestructNestedObjectEl {
                        index,
                        outer_count,
                        outer_ids_start,
                        key,
                        nested_count: nested_count + 1,
                        binding_id: id,
                    });
                    return 1;
                } else if tok.is_close_curly() {
                    // Pattern complete
                    if let Some(NestedPattern::Object(ref bindings)) = self.nested_patterns.last() {
                        if bindings.len() == nested_count as usize {
                            let pattern_idx = self.nested_patterns.len() - 1;
                            let placeholder_pos =
                                outer_ids_start as usize + outer_count as usize - 1;
                            self.opcodes[placeholder_pos] = ScriptValue::from_opcode_args(
                                Opcode::NOP,
                                OpcodeArgs::from_u32((pattern_idx + 1) as u32),
                            );
                        }
                    }
                    self.state.push(State::LetObjectDestruct {
                        index,
                        count: outer_count,
                        ids_start: outer_ids_start,
                    });
                    return 1;
                } else {
                    error!(
                        self,
                        tokenizer, "Expected identifier in nested object pattern"
                    );
                }
            }
            State::LetObjectDestructNestedObjectEl {
                index,
                outer_count,
                outer_ids_start,
                key: _,
                nested_count,
                binding_id,
            } => {
                if nested_count == 1 {
                    self.nested_patterns
                        .push(NestedPattern::Object(vec![binding_id]));
                } else {
                    if let Some(NestedPattern::Object(ref mut bindings)) =
                        self.nested_patterns.last_mut()
                    {
                        bindings.push(binding_id);
                    }
                }

                if sep == id!(,) || sep == id!(;) {
                    self.state.push(State::LetObjectDestructNestedObject {
                        index,
                        outer_count,
                        outer_ids_start,
                        key: LiveId::empty(),
                        nested_count,
                    });
                    return 1;
                } else if id.not_empty() {
                    self.state.push(State::LetObjectDestructNestedObjectEl {
                        index,
                        outer_count,
                        outer_ids_start,
                        key: LiveId::empty(),
                        nested_count: nested_count + 1,
                        binding_id: id,
                    });
                    return 1;
                } else if tok.is_close_curly() {
                    let pattern_idx = self.nested_patterns.len() - 1;
                    let placeholder_pos = outer_ids_start as usize + outer_count as usize - 1;
                    self.opcodes[placeholder_pos] = ScriptValue::from_opcode_args(
                        Opcode::NOP,
                        OpcodeArgs::from_u32((pattern_idx + 1) as u32),
                    );
                    self.state.push(State::LetObjectDestruct {
                        index,
                        count: outer_count,
                        ids_start: outer_ids_start,
                    });
                    return 1;
                } else {
                    self.state.push(State::LetObjectDestructNestedObject {
                        index,
                        outer_count,
                        outer_ids_start,
                        key: LiveId::empty(),
                        nested_count,
                    });
                    return 0;
                }
            }

            State::LetObjectDestructRhs {
                index,
                count,
                ids_start,
                defaults_start,
            } => {
                if op == id!(=) {
                    self.state.push(State::EmitLetObjectDestruct {
                        index,
                        count,
                        ids_start,
                        defaults_start,
                    });
                    self.state.push(State::BeginExpr { required: true });
                    return 1;
                } else {
                    error!(self, tokenizer, "Expected '=' after destructuring pattern");
                }
            }
            State::EmitLetObjectDestruct {
                index,
                count,
                ids_start,
                defaults_start,
            } => {
                // Layout: [ids..., rhs_code], defaults stored in destruct_defaults
                // Generate: [rhs, id_0, EXTRACT, ..., DROP, defaults_code...]

                let ids_start = ids_start as usize;
                let defaults_start = defaults_start as usize;
                let count = count as usize;

                let ids: Vec<_> = self.opcodes[ids_start..ids_start + count].to_vec();
                let ids_map: Vec<_> = self.source_map[ids_start..ids_start + count].to_vec();
                let rhs: Vec<_> = self.opcodes[defaults_start..].to_vec();
                let rhs_map: Vec<_> = self.source_map[defaults_start..].to_vec();

                for id_code in &ids {
                    if let Some(bid) = id_code.as_id() {
                        if Self::is_reserved_binding(bid) {
                            error!(
                                self,
                                tokenizer,
                                "'{}' is reserved and cannot be bound in a destructuring pattern",
                                bid
                            );
                        }
                    }
                }

                self.opcodes.truncate(ids_start);
                self.source_map.truncate(ids_start);

                // RHS first
                self.opcodes.extend(rhs);
                self.source_map.extend(rhs_map);

                // id, EXTRACT for each
                for (id_code, id_map) in ids.into_iter().zip(ids_map) {
                    if let Some(bound) = id_code.as_id() {
                        self.slot_poison(bound);
                    }
                    self.opcodes.push(id_code);
                    self.source_map.push(id_map);
                    self.push_code(Opcode::LET_DESTRUCT_OBJECT_EL.into(), index);
                }

                // DROP source
                self.push_code(Opcode::DROP.into(), index);

                // Emit stored defaults with lazy evaluation: [id, ASSIGN_IFNIL(jump), value, ASSIGN]
                // jump_dist = value_code.len() + 2 to skip past value_code AND the ASSIGN opcode
                let defaults = std::mem::take(&mut self.destruct_defaults);
                for (binding_id, value_code, value_map) in defaults {
                    let jump_dist = (value_code.len() + 2) as u32;
                    self.push_code(binding_id.into(), index);
                    self.push_code(
                        ScriptValue::from_opcode_args(
                            Opcode::ASSIGN_IFNIL,
                            OpcodeArgs::from_u32(jump_dist),
                        ),
                        index,
                    );
                    self.opcodes.extend(value_code);
                    self.source_map.extend(value_map);
                    self.push_code(Opcode::ASSIGN.into(), index);
                }

                // This is a let statement, go directly to BeginStmt to avoid pop_to_me being set on DROP
                self.state.push(State::BeginStmt {
                    last_was_sep: false,
                });
                return 0;
            }

            State::Var { index } => {
                if id.not_empty() {
                    if Self::is_reserved_binding(id) {
                        error!(
                            self,
                            tokenizer,
                            "'{}' is reserved and cannot be a variable name — 'me' and 'scope' are the implicit references; pick another name",
                            id
                        );
                    }
                    // var stays dynamic; a slotted name may not be re-bound
                    self.slot_poison(id);
                    // lets expect an assignment expression
                    // push the id on to the stack
                    self.push_code(id.into(), self.index);
                    self.state.push(State::VarDynOrTyped { index });
                    return 1;
                } else {
                    // unknown
                    error!(self, tokenizer, "Var expected identifier");
                }
            }
            State::VarDynOrTyped { index } => {
                if op == id!(=) {
                    // assignment following
                    self.state.push(State::EmitVarDyn { index });
                    self.state.push(State::BeginExpr { required: true });
                    return 1;
                } else if op == id!(:) {
                    // type following
                    self.state.push(State::VarType { index });
                    return 1;
                } else {
                    self.push_code(
                        ScriptValue::from_opcode_args(Opcode::VAR_DYN, OpcodeArgs::NIL),
                        index,
                    );
                }
            }
            State::VarType { index } => {
                if id.not_empty() {
                    // lets expect an assignment expression
                    // push the id on to the stack
                    self.push_code(id.into(), self.index);
                    self.state.push(State::VarTypedAssign { index });
                    return 1;
                } else {
                    // unknown
                    error!(self, tokenizer, "Var type expected");
                }
            }
            State::VarTypedAssign { index } => {
                if op == id!(=) {
                    // assignment following
                    self.state.push(State::EmitVarTyped { index });
                    self.state.push(State::BeginExpr { required: true });
                    return 1;
                } else {
                    self.push_code(
                        ScriptValue::from_opcode_args(Opcode::VAR_TYPED, OpcodeArgs::NIL),
                        index,
                    );
                }
            }
            State::EmitVarDyn { index } => {
                self.push_code(Opcode::VAR_DYN.into(), index);
            }
            State::EmitVarTyped { index } => {
                self.push_code(Opcode::VAR_TYPED.into(), index);
            }
            State::EndRound => {
                // we expect a ) here
                //self.code.push(Opcode::END_FRAG.into());
                if tok.is_close_round() {
                    self.state.push(State::EndExpr);
                    return 1;
                } else {
                    error!(self, tokenizer, "Expected )")
                }
            }
            State::EmitFnArgTyped { index } => {
                self.push_code(Opcode::FN_ARG_TYPED.into(), index);
            }
            State::EmitFnArgDyn { index } => {
                self.push_code(Opcode::FN_ARG_DYN.into(), index);
            }
            State::FnMaybeLet { index } => {
                if id.not_empty() {
                    // ok we did fn id — binds the name dynamically
                    self.slot_poison(id);
                    self.push_code(id.into(), self.index);
                    self.push_code(Opcode::FN_LET_ARGS.into(), self.index);
                    self.state.push(State::FnLetMaybeArgs);
                    return 1;
                } else if tok.is_open_curly() {
                    // zero args function
                    self.push_code(Opcode::FN_ARGS.into(), self.index);
                    let fn_slot = self.code_len();
                    self.push_code(Opcode::FN_BODY_DYN.into(), self.index);
                self.slot_body_open(true);
                    self.state.push(State::EndFnBlock {
                        fn_slot,
                        last_was_sep: false,
                        index: self.index,
                    });
                    self.state.push(State::BeginStmt {
                        last_was_sep: false,
                    });
                    return 1;
                }
                // immediate args
                else if tok.is_open_round() {
                    self.push_code(Opcode::FN_ARGS.into(), self.index);
                    self.state.push(State::FnArgList { lambda: false });
                    return 1;
                } else {
                    self.state.push(State::EmitFnArgDyn { index });
                    error!(self, tokenizer, "Argument type expected in function");
                }
            }
            State::FnLetMaybeArgs => {
                // no args
                if tok.is_open_curly() {
                    let fn_slot = self.code_len();
                    self.push_code(Opcode::FN_BODY_DYN.into(), self.index);
                self.slot_body_open(true);
                    self.state.push(State::EndFnBlock {
                        fn_slot,
                        last_was_sep: false,
                        index: self.index,
                    });
                    self.state.push(State::BeginStmt {
                        last_was_sep: false,
                    });
                    return 1;
                } else if tok.is_open_round() {
                    self.state.push(State::FnArgList { lambda: false });
                    return 1;
                } else {
                    error!(
                        self,
                        tokenizer, "Expected either {{ or ( in function definition"
                    );
                }
            }
            State::FnArgType { lambda, index } => {
                if id.not_empty() {
                    self.push_code(id.into(), self.index);
                    self.state.push(State::FnArgTypeAssign { lambda, index });
                    return 1;
                } else {
                    self.state.push(State::EmitFnArgDyn { index });
                    error!(self, tokenizer, "Argument type expected in function")
                }
            }
            State::FnArgTypeAssign { lambda, index } => {
                if !lambda && op == id!(=) {
                    // assignment following
                    self.state.push(State::EmitFnArgTyped { index });
                    self.state.push(State::BeginExpr { required: true });
                    return 1;
                } else {
                    self.push_code(
                        ScriptValue::from_opcode_args(Opcode::FN_ARG_TYPED, OpcodeArgs::NIL),
                        index,
                    );
                }
            }
            State::FnArgMaybeType { lambda, index } => {
                if !lambda && op == id!(=) {
                    // assignment following
                    self.state.push(State::EmitFnArgDyn { index });
                    self.state.push(State::BeginExpr { required: true });
                    return 1;
                }
                if op == id!(:) {
                    self.state.push(State::FnArgType { lambda, index });
                    return 1;
                }
                self.push_code(
                    ScriptValue::from_opcode_args(Opcode::FN_ARG_DYN, OpcodeArgs::NIL),
                    index,
                );
            }
            State::FnArgList { lambda } => {
                if id.not_empty() {
                    if Self::is_reserved_binding(id) {
                        error!(
                            self,
                            tokenizer,
                            "'{}' is reserved and cannot be an argument name",
                            id
                        );
                    }
                    // ident
                    self.push_code(id.into(), self.index);
                    self.state.push(State::FnArgList { lambda });
                    self.state.push(State::FnArgMaybeType {
                        lambda,
                        index: self.index,
                    });
                    return 1;
                }
                if lambda && op == id!(|) {
                    self.state.push(State::FnBody { lambda });
                    return 1;
                } else if !lambda && tok.is_close_round() {
                    self.state.push(State::FnBody { lambda });
                    return 1;
                }
                if sep == id!(,) {
                    self.state.push(State::FnArgList { lambda });
                    return 1;
                }
                // unexpected token, but just stay in the arg list mode
                error!(
                    self,
                    tokenizer, "Unexpected token in function argument list {:?}", tok
                );
                self.state.push(State::FnArgList { lambda });
                return 1;
            }
            State::FnBody { lambda } => {
                // Check for return type annotation with ->
                if op == id!(->) {
                    self.state.push(State::FnReturnType { lambda });
                    return 1;
                }
                let fn_slot = self.code_len() as _;
                self.push_code(Opcode::FN_BODY_DYN.into(), self.index);
                self.slot_body_open(true);
                if tok.is_open_curly() {
                    // function body
                    self.state.push(State::EndFnBlock {
                        fn_slot,
                        last_was_sep: false,
                        index: self.index,
                    });
                    self.state.push(State::BeginStmt {
                        last_was_sep: false,
                    });
                    return 1;
                } else if lambda {
                    // function body can be expression expression
                    self.state.push(State::EndFnExpr {
                        fn_slot,
                        index: self.index,
                    });
                    self.state.push(State::BeginExpr { required: true });
                } else {
                    error!(
                        self,
                        tokenizer, "Unexpected token in function definition, expected {{ {:?}", tok
                    );
                }
            }
            State::FnReturnType { lambda } => {
                if id.not_empty() {
                    // we have a return type identifier
                    self.push_code(id.into(), self.index);
                    self.state.push(State::FnBodyTyped { lambda });
                    return 1;
                } else {
                    error!(self, tokenizer, "Expected return type after ->");
                }
            }
            State::FnBodyTyped { lambda } => {
                let fn_slot = self.code_len() as _;
                self.push_code(Opcode::FN_BODY_TYPED.into(), self.index);
                self.slot_body_open(false);
                if tok.is_open_curly() {
                    // function body
                    self.state.push(State::EndFnBlock {
                        fn_slot,
                        last_was_sep: false,
                        index: self.index,
                    });
                    self.state.push(State::BeginStmt {
                        last_was_sep: false,
                    });
                    return 1;
                } else if lambda {
                    // function body can be expression expression
                    self.state.push(State::EndFnExpr {
                        fn_slot,
                        index: self.index,
                    });
                    self.state.push(State::BeginExpr { required: true });
                } else {
                    error!(
                        self,
                        tokenizer, "Unexpected token in function definition, expected {{ {:?}", tok
                    );
                }
            }
            State::EscapedId => {
                if id.not_empty() {
                    // ident
                    self.push_code(id.escape(), self.index);
                    return 1;
                } else {
                    error!(self, tokenizer, "Expected identifier after @");
                }
            }
            State::EndFnExpr { fn_slot, index } => {
                self.push_code(Opcode::RETURN.into(), index);
                self.slot_body_close();
                self.set_opcode_args(
                    fn_slot as _,
                    OpcodeArgs::from_u32(self.code_len() as u32 - fn_slot),
                );
            }
            State::EndFnBlock {
                fn_slot,
                last_was_sep,
                index,
            } => {
                /* if !last_was_sep && Some(&Opcode::POP_TO_ME.into()) == self.code_last(){
                    self.pop_code();
                    self.push_code(Opcode::RETURN.into(), index);
                }*/
                if !last_was_sep && self.has_pop_to_me() {
                    self.clear_pop_to_me();
                    self.push_code(Opcode::RETURN.into(), index);
                } else {
                    self.push_code(
                        ScriptValue::from_opcode_args(Opcode::RETURN, OpcodeArgs::NIL),
                        index,
                    );
                }
                self.slot_body_close();
                self.set_opcode_args(
                    fn_slot as _,
                    OpcodeArgs::from_u32(self.code_len() as u32 - fn_slot),
                );

                if tok.is_close_curly() {
                    return 1;
                } else {
                    error!(self, tokenizer, "Expected }} not found");
                    return 0;
                }
            }
            // alright we parsed a + b * c
            State::EmitFieldAssign { what_op, index } => {
                self.push_code(State::operator_to_field_assign(what_op), index);
            }
            State::EmitIndexAssign { what_op, index } => {
                self.push_code(State::operator_to_index_assign(what_op), index);
            }
            State::EmitOp {
                what_op,
                index,
                slot_assign,
            } => {
                if State::operator_supports_inline_number(what_op) {
                    if let Some(code) = self.code_last() {
                        if let Some(vf64) = code.as_f64() {
                            let num = vf64 as u64;
                            if vf64.fract() == 0.0 && num <= OpcodeArgs::MAX_U32 as u64 {
                                self.pop_code();
                                let mut value = State::operator_to_opcode(what_op);
                                value.set_opcode_args(OpcodeArgs::from_u32(num as u32));
                                self.push_code(value, index);
                                return 0;
                            }
                        }
                    }
                }
                // slot resolver: remember where this assign reduced so body
                // close can rewrite it to the slot form
                if let Some(name) = slot_assign {
                    if let Some((opcode, _)) = State::operator_to_opcode(what_op).as_opcode() {
                        self.slot_note_assign(name, opcode);
                    }
                }
                self.push_code(State::operator_to_opcode(what_op), index);
                return 0;
            }
            State::EmitUnary { what_op, index } => {
                self.push_code(State::operator_to_unary(what_op), index);
                return 0;
            }
            State::EmitSplat { index } => {
                self.push_code(Opcode::ME_SPLAT.into(), index);
                return 0;
            }
            State::ShortCircuitEnd { test_slot } => {
                // Patch the TEST opcode's jump to skip to current position (after second operand)
                self.set_opcode_args(test_slot, OpcodeArgs::from_u32(self.code_len() - test_slot));
                self.last_jump_target = self.code_len();
                return 0;
            }
            State::ShortCircuitAssignEnd { test_slot, index } => {
                // Emit ASSIGN after RHS, then patch the jump
                self.push_code(Opcode::ASSIGN.into(), index);
                self.set_opcode_args(test_slot, OpcodeArgs::from_u32(self.code_len() - test_slot));
                self.last_jump_target = self.code_len();
                return 0;
            }
            State::EmitReturn {
                index,
                code_len_before,
            } => {
                if self.code_len() as u32 == code_len_before {
                    // No expression was parsed after `return` — bare void return
                    self.push_code(
                        ScriptValue::from_opcode_args(Opcode::RETURN, OpcodeArgs::NIL),
                        index,
                    );
                } else {
                    // Expression was parsed — return with value
                    self.push_code(Opcode::RETURN.into(), index);
                }
                return 0;
            }
            State::EmitBreak { index } => {
                self.push_code(Opcode::BREAK.into(), index);
                return 0;
            }
            State::EmitContinue { index } => {
                self.push_code(Opcode::CONTINUE.into(), index);
                return 0;
            }
            State::EndBareSquare => {
                self.push_code(Opcode::END_ARRAY.into(), self.index);
                self.state.push(State::EndExpr);
                if tok.is_close_square() {
                    return 1;
                } else {
                    error!(self, tokenizer, "Expected ] not found");
                    return 0;
                }
            }
            State::EndBare => {
                self.push_code(Opcode::END_BARE.into(), self.index);
                self.state.push(State::EndExpr);
                if tok.is_close_curly() {
                    return 1;
                } else {
                    error!(self, tokenizer, "Expected }} not found");
                    return 0;
                }
            }
            // emit the create prototype instruction
            State::EndProto => {
                self.push_code(Opcode::END_PROTO.into(), self.index);
                self.state.push(State::EndExpr);
                if tok.is_close_curly() {
                    return 1;
                } else {
                    error!(self, tokenizer, "Expected }} not found");
                    return 0;
                }
            }
            // emit prototype instruction + proto inherit write (for +: operator)
            State::EndProtoInherit => {
                self.push_code(Opcode::END_PROTO.into(), self.index);
                self.push_code(Opcode::PROTO_INHERIT_WRITE.into(), self.index);
                self.state.push(State::EndExpr);
                if tok.is_close_curly() {
                    return 1;
                } else {
                    error!(self, tokenizer, "Expected }} not found");
                    return 0;
                }
            }
            // emit prototype instruction + scope inherit write (for value += {} operator)
            State::EndScopeInherit => {
                self.push_code(Opcode::END_PROTO.into(), self.index);
                self.push_code(Opcode::SCOPE_INHERIT_WRITE.into(), self.index);
                self.state.push(State::EndExpr);
                if tok.is_close_curly() {
                    return 1;
                } else {
                    error!(self, tokenizer, "Expected }} not found");
                    return 0;
                }
            }
            // emit prototype instruction + field inherit write (for obj.field += {} operator)
            State::EndFieldInherit => {
                self.push_code(Opcode::END_PROTO.into(), self.index);
                self.push_code(Opcode::FIELD_INHERIT_WRITE.into(), self.index);
                self.state.push(State::EndExpr);
                if tok.is_close_curly() {
                    return 1;
                } else {
                    error!(self, tokenizer, "Expected }} not found");
                    return 0;
                }
            }
            // emit prototype instruction + index inherit write (for obj[index] += {} operator)
            State::EndIndexInherit => {
                self.push_code(Opcode::END_PROTO.into(), self.index);
                self.push_code(Opcode::INDEX_INHERIT_WRITE.into(), self.index);
                self.state.push(State::EndExpr);
                if tok.is_close_curly() {
                    return 1;
                } else {
                    error!(self, tokenizer, "Expected }} not found");
                    return 0;
                }
            }
            State::EmitCallFromDo { is_method, index } => {
                self.set_pop_to_me();
                if is_method {
                    self.push_code(Opcode::METHOD_CALL_EXEC.into(), index);
                } else {
                    self.push_code(Opcode::CALL_EXEC.into(), index);
                }
                self.state.push(State::EndExpr);
            }
            State::CallMaybeDo { is_method, index } => {
                if id == id!(do) {
                    self.state.push(State::EmitCallFromDo { is_method, index });
                    self.state.push(State::BeginExpr { required: true });
                    return 1;
                } else {
                    if is_method {
                        self.push_code(Opcode::METHOD_CALL_EXEC.into(), index);
                    } else {
                        self.push_code(Opcode::CALL_EXEC.into(), index);
                    }
                    self.state.push(State::EndExpr);
                    return 0;
                }
            }
            State::EndCall { is_method, index } => {
                // expect )
                self.state.push(State::CallMaybeDo { is_method, index });
                if tok.is_close_round() {
                    return 1;
                } else {
                    error!(self, tokenizer, "Expected ) not found");
                    return 0;
                }
            }
            State::ArrayIndex => {
                self.push_code(Opcode::ARRAY_INDEX.into(), self.index);
                self.state.push(State::EndExpr);
                if tok.is_close_square() {
                    return 1;
                } else {
                    error!(self, tokenizer, "AT TOK {:?}", tok);
                    error!(self, tokenizer, "Expected ] not found");
                    return 0;
                }
            }

            State::TryTest { index } => {
                let try_start = self.code_len() as _;
                self.slot_disqualify_body();
                self.push_code(Opcode::TRY_TEST.into(), index);
                if tok.is_open_curly() {
                    self.state.push(State::TryTestBlock {
                        try_start,
                        last_was_sep: false,
                    });
                    self.state.push(State::BeginStmt {
                        last_was_sep: false,
                    });
                    return 1;
                }
                self.state.push(State::TryTestExpr { try_start });
                self.state.push(State::BeginExpr { required: true });
                return 0;
            }
            State::OkTest { index } => {
                let ok_start = self.code_len() as _;
                self.slot_disqualify_body();
                self.push_code(Opcode::OK_TEST.into(), index);
                if tok.is_open_curly() {
                    self.state.push(State::OkTestBlock {
                        ok_start,
                        last_was_sep: false,
                    });
                    self.state.push(State::BeginStmt {
                        last_was_sep: false,
                    });
                    return 1;
                }
                self.state.push(State::OkTestExpr { ok_start });
                self.state.push(State::BeginExpr { required: true });
                return 0;
            }
            State::OkTestExpr { ok_start } => {
                self.set_opcode_args(
                    ok_start,
                    OpcodeArgs::from_u32(self.code_len() as u32 - ok_start),
                );
                self.push_code(Opcode::OK_END.into(), self.index);
                return 0;
            }
            State::OkTestBlock {
                ok_start,
                last_was_sep,
            } => {
                self.set_opcode_args(
                    ok_start,
                    OpcodeArgs::from_u32(self.code_len() as u32 - ok_start),
                );
                if tok.is_close_curly() {
                    if !last_was_sep && self.has_pop_to_me() {
                        self.clear_pop_to_me();
                    }
                    self.push_code(Opcode::OK_END.into(), self.index);
                    self.state.push(State::EndExpr);
                    return 1;
                } else {
                    self.push_code(Opcode::OK_END.into(), self.index);
                    return 0;
                }
            }
            State::TryTestExpr { try_start } => {
                self.set_opcode_args(
                    try_start,
                    OpcodeArgs::from_u32(self.code_len() as u32 - try_start),
                );
                self.state.push(State::TryErrBlockOrExpr);
                return 0;
            }
            State::TryTestBlock {
                try_start,
                last_was_sep,
            } => {
                self.set_opcode_args(
                    try_start,
                    OpcodeArgs::from_u32(self.code_len() as u32 - try_start),
                );
                if tok.is_close_curly() {
                    if !last_was_sep && self.has_pop_to_me() {
                        self.clear_pop_to_me();
                    }
                    self.state.push(State::TryErrBlockOrExpr);
                    return 1;
                } else {
                    self.state.push(State::TryErrBlockOrExpr);
                    return 0;
                }
            }
            State::TryErrBlockOrExpr => {
                let err_start = self.code_len() as _;
                self.push_code(Opcode::TRY_ERR.into(), self.index);
                if tok.is_open_curly() {
                    self.state.push(State::TryErrBlock {
                        err_start,
                        last_was_sep: false,
                    });
                    self.state.push(State::BeginStmt {
                        last_was_sep: false,
                    });
                    return 1;
                }
                self.state.push(State::TryErrExpr { err_start });
                self.state.push(State::BeginExpr { required: true });
                return 0;
            }
            State::TryErrExpr { err_start } => {
                self.last_jump_target = self.code_len();
                self.set_opcode_args(
                    err_start,
                    OpcodeArgs::from_u32(self.code_len() as u32 - err_start),
                );
                self.state.push(State::TryOk { was_block: false });
            }
            State::TryErrBlock {
                err_start,
                last_was_sep,
            } => {
                self.last_jump_target = self.code_len();
                self.set_opcode_args(
                    err_start,
                    OpcodeArgs::from_u32(self.code_len() as u32 - err_start),
                );
                if tok.is_close_curly() {
                    if !last_was_sep && self.has_pop_to_me() {
                        self.clear_pop_to_me();
                    }
                    self.state.push(State::TryOk { was_block: true });
                    return 1;
                } else {
                    error!(self, tokenizer, "Expected }} not found");
                    self.state.push(State::TryOk { was_block: false });
                    return 0;
                }
            }
            State::TryOk { was_block } => {
                if id == id!(ok) {
                    self.state.push(State::TryOkBlockOrExpr);
                    return 1;
                }
                if was_block {
                    self.state.push(State::EndExpr)
                }
                return 0;
            }
            State::TryOkBlockOrExpr => {
                let ok_start = self.code_len() as _;
                self.push_code(Opcode::TRY_OK.into(), self.index);
                if tok.is_open_curly() {
                    self.state.push(State::TryOkBlock {
                        ok_start,
                        last_was_sep: false,
                    });
                    self.state.push(State::BeginStmt {
                        last_was_sep: false,
                    });
                    return 1;
                }
                self.state.push(State::TryOkExpr { ok_start });
                self.state.push(State::BeginExpr { required: true });
                return 0;
            }
            State::TryOkExpr { ok_start } => {
                self.set_opcode_args(
                    ok_start,
                    OpcodeArgs::from_u32(self.code_len() as u32 - ok_start),
                );
            }
            State::TryOkBlock {
                ok_start,
                last_was_sep,
            } => {
                self.set_opcode_args(
                    ok_start,
                    OpcodeArgs::from_u32(self.code_len() as u32 - ok_start),
                );
                if tok.is_close_curly() {
                    if !last_was_sep && self.has_pop_to_me() {
                        self.clear_pop_to_me();
                    }
                    self.state.push(State::EndExpr);
                    return 1;
                } else {
                    error!(self, tokenizer, "Expected }} not found");
                    return 0;
                }
            }
            State::IfTest { index } => {
                let if_start = self.code_len() as _;
                self.push_code(Opcode::IF_TEST.into(), index);
                if tok.is_open_curly() {
                    self.state.push(State::IfTrueBlock {
                        if_start,
                        last_was_sep: false,
                    });
                    self.state.push(State::BeginStmt {
                        last_was_sep: false,
                    });
                    return 1;
                }
                if id == id!(else) {
                    error!(self, tokenizer, "Unexpected else, use {{}} to disambiguate");
                    return 1;
                }
                self.state.push(State::IfTrueExpr { if_start });
                self.state.push(State::BeginExpr { required: true });
                return 0;
            }
            State::IfTrueExpr { if_start } => {
                self.state.push(State::IfMaybeElse {
                    if_start,
                    was_block: false,
                });
                return 0;
            }
            State::IfTrueBlock {
                if_start,
                last_was_sep,
            } => {
                if tok.is_close_curly() {
                    if !last_was_sep && self.has_pop_to_me() {
                        self.clear_pop_to_me();
                    }
                    self.state.push(State::IfMaybeElse {
                        if_start,
                        was_block: true,
                    });
                    return 1;
                } else {
                    self.state.push(State::EndExpr);
                    error!(self, tokenizer, "Expected }} not found");
                    return 0;
                }
            }
            State::IfMaybeElse {
                if_start,
                was_block,
            } => {
                if id == id!(elif) {
                    // Desugar `elif ...` as `else { if ... }`: the rest of the
                    // chain parses as a nested if, and IfElseExpr patches this
                    // arm's IF_ELSE to the chain's end once it completes. The
                    // old push of IfMaybeElse{if_start} left the IF_ELSE's
                    // jump at 0 — an infinite loop — and re-patched the
                    // already-patched IF_TEST at the chain's end.
                    let else_start = self.code_len() as u32;
                    self.push_code(Opcode::IF_ELSE.into(), self.index);
                    self.set_opcode_args(
                        if_start,
                        OpcodeArgs::from_u32(self.code_len() as u32 - if_start),
                    );

                    self.state.push(State::IfElseExpr { else_start });
                    self.state.push(State::IfTest { index: self.index });
                    self.state.push(State::BeginExpr { required: true });
                    return 1;
                }
                if id == id!(else) {
                    let else_start = self.code_len() as u32;
                    self.push_code(Opcode::IF_ELSE.into(), self.index);
                    self.set_opcode_args(
                        if_start,
                        OpcodeArgs::from_u32(self.code_len() as u32 - if_start),
                    );
                    self.state.push(State::IfElse { else_start });
                    return 1;
                }
                self.last_jump_target = self.code_len();
                self.set_opcode_args(
                    if_start,
                    OpcodeArgs::from_u32(self.code_len() as u32 - if_start).set_need_nil(),
                );
                // self.push_code_none(NIL);
                if was_block {
                    // allow expression to chain
                    self.state.push(State::EndExpr)
                }
            }
            State::IfElse { else_start } => {
                if tok.is_open_curly() {
                    self.state.push(State::IfElseBlock {
                        else_start,
                        last_was_sep: false,
                    });
                    self.state.push(State::BeginStmt {
                        last_was_sep: false,
                    });
                    return 1;
                }
                self.state.push(State::IfElseExpr { else_start });
                self.state.push(State::BeginExpr { required: true });
                return 0;
            }
            State::IfElseExpr { else_start } => {
                self.last_jump_target = self.code_len();
                self.set_opcode_args(
                    else_start,
                    OpcodeArgs::from_u32(self.code_len() as u32 - else_start),
                );
                return 0;
            }
            State::IfElseBlock {
                else_start,
                last_was_sep,
            } => {
                if tok.is_close_curly() {
                    if !last_was_sep && self.has_pop_to_me() {
                        self.clear_pop_to_me();
                    }
                    self.last_jump_target = self.code_len();
                    self.set_opcode_args(
                        else_start,
                        OpcodeArgs::from_u32(self.code_len() as u32 - else_start),
                    );
                    self.state.push(State::EndExpr);
                    return 1;
                } else {
                    self.state.push(State::EndExpr);
                    error!(self, tokenizer, "Expected }} not found");
                    return 0;
                }
            }
            State::BeginExpr { required } => {
                if !required
                    && matches!(
                        self.state.last(),
                        Some(State::EmitReturn { .. } | State::EmitBreak { .. })
                    )
                    && (id == id!(let)
                        || id == id!(var)
                        || id == id!(use)
                        || id == id!(return)
                        || id == id!(break)
                        || id == id!(continue))
                {
                    return 0;
                }
                if let Some(index) = tok.as_rust_value() {
                    self.push_code(values[index as usize], self.index);
                    self.state.push(State::EndExpr);
                    return 1;
                }
                if tok.is_open_curly() {
                    // Check if there's a pending +: operator for proto-inherit
                    if let Some(State::EmitOp {
                        what_op: id!(+:), ..
                    }) = self.state.last()
                    {
                        self.state.pop();
                        // Proto-inherit operator: field +: { ... }
                        // Emit PROTO_INHERIT_READ to read field and push proto value
                        self.push_code(Opcode::PROTO_INHERIT_READ.into(), self.index);
                        self.push_code(Opcode::BEGIN_PROTO.into(), self.index);
                        self.state.push(State::EndProtoInherit);
                        self.state.push(State::BeginStmt {
                            last_was_sep: false,
                        });
                        return 1;
                    }
                    // Check if there's a pending += operator for scope-inherit
                    if let Some(State::EmitOp {
                        what_op: id!(+=),
                        slot_assign,
                        ..
                    }) = self.state.last()
                    {
                        // += turned out to be scope-inherit: the target reads
                        // through the scope chain, so the name must stay dynamic
                        if let Some(name) = *slot_assign {
                            self.slot_poison(name);
                        }
                        self.state.pop();
                        // Scope-inherit operator: value += { ... }
                        // Emit SCOPE_INHERIT_READ to read variable and push proto value
                        self.push_code(Opcode::SCOPE_INHERIT_READ.into(), self.index);
                        self.push_code(Opcode::BEGIN_PROTO.into(), self.index);
                        self.state.push(State::EndScopeInherit);
                        self.state.push(State::BeginStmt {
                            last_was_sep: false,
                        });
                        return 1;
                    }
                    // Check if there's a pending field += for field-inherit
                    if let Some(State::EmitFieldAssign {
                        what_op: id!(+=), ..
                    }) = self.state.last()
                    {
                        self.state.pop();
                        // Field-inherit operator: obj.field += { ... }
                        // Emit FIELD_INHERIT_READ to read field and push proto value
                        self.push_code(Opcode::FIELD_INHERIT_READ.into(), self.index);
                        self.push_code(Opcode::BEGIN_PROTO.into(), self.index);
                        self.state.push(State::EndFieldInherit);
                        self.state.push(State::BeginStmt {
                            last_was_sep: false,
                        });
                        return 1;
                    }
                    // Check if there's a pending index += for index-inherit
                    if let Some(State::EmitIndexAssign {
                        what_op: id!(+=), ..
                    }) = self.state.last()
                    {
                        self.state.pop();
                        // Index-inherit operator: obj[index] += { ... }
                        // Emit INDEX_INHERIT_READ to read index and push proto value
                        self.push_code(Opcode::INDEX_INHERIT_READ.into(), self.index);
                        self.push_code(Opcode::BEGIN_PROTO.into(), self.index);
                        self.state.push(State::EndIndexInherit);
                        self.state.push(State::BeginStmt {
                            last_was_sep: false,
                        });
                        return 1;
                    }
                    self.push_code(Opcode::BEGIN_BARE.into(), self.index);
                    self.state.push(State::EndBare);
                    self.state.push(State::BeginStmt {
                        last_was_sep: false,
                    });
                    return 1;
                }
                if tok.is_open_square() {
                    self.push_code(Opcode::BEGIN_ARRAY.into(), self.index);
                    self.state.push(State::EndBareSquare);
                    self.state.push(State::BeginStmt {
                        last_was_sep: false,
                    });
                    return 1;
                }
                if tok.is_open_round() {
                    //self.code.push(Opcode::BEGIN_FRAG.into());
                    self.state.push(State::EndRound);
                    self.state.push(State::BeginExpr { required: true });
                    return 1;
                }
                if let Some(v) = tok.as_f64() {
                    self.push_code(ScriptValue::from_f64(v), self.index);
                    self.state.push(State::EndExpr);
                    return 1;
                }
                if let Some(v) = tok.as_u40() {
                    self.push_code(ScriptValue::from_u40(v), self.index);
                    self.state.push(State::EndExpr);
                    return 1;
                }
                if let Some(v) = tok.as_f32() {
                    self.push_code(ScriptValue::from_f32(v), self.index);
                    self.state.push(State::EndExpr);
                    return 1;
                }
                if let Some(v) = tok.as_u32() {
                    self.push_code(ScriptValue::from_u32(v), self.index);
                    self.state.push(State::EndExpr);
                    return 1;
                }
                if let Some(v) = tok.as_i32() {
                    self.push_code(ScriptValue::from_i32(v), self.index);
                    self.state.push(State::EndExpr);
                    return 1;
                }
                if let Some(v) = tok.as_f16() {
                    self.push_code(ScriptValue::from_f16(v), self.index);
                    self.state.push(State::EndExpr);
                    return 1;
                }
                if id == id!(if) {
                    // do if as an expression
                    self.state.push(State::IfTest { index: self.index });
                    self.state.push(State::BeginExpr { required: true });
                    return 1;
                }
                if id == id!(try) {
                    // do if as an expression
                    self.state.push(State::TryTest { index: self.index });
                    return 1;
                }
                if id == id!(ok) {
                    // do if as an expression
                    self.state.push(State::OkTest { index: self.index });
                    return 1;
                }
                if id == id!(for) {
                    self.state.push(State::ForIdent {
                        idents: 0,
                        index: self.index,
                    });
                    return 1;
                }
                if id == id!(loop) {
                    self.state.push(State::Loop { index: self.index });
                    return 1;
                }
                if id == id!(while) {
                    self.state.push(State::While { index: self.index });
                    return 1;
                }
                if id == id!(match) {
                    // Generate temp variable name based on code position
                    // Avoid double-underscore identifiers in generated GLSL locals.
                    // The shader backend prefixes locals with `l_`, so `_match_*` would
                    // become `l__match_*`, which is reserved in GLSL ES.
                    let temp_name = format!("match_{}", self.code_len());
                    // Use from_str_with_lut to register the temp name in the LUT for lookup
                    let temp_id = LiveId::from_str_with_lut(&temp_name)
                        .unwrap_or_else(|_| LiveId::from_str(&temp_name));
                    // Emit temp_id FIRST, then parse subject expression, then LET_DYN
                    self.push_code(ScriptValue::from_id(temp_id), self.index);
                    self.state.push(State::MatchSubject {
                        temp_id,
                        index: self.index,
                    });
                    self.state.push(State::BeginExpr { required: true });
                    return 1;
                }
                // NOTE on `use x.*` semantics: a glob import copies the names
                // that exist in the target namespace AT THIS POINT in
                // evaluation. Names registered later — including later in the
                // SAME script_mod! block (e.g. `mod.widgets.Foo = ...` below a
                // `use mod.widgets.*`) — are NOT visible through the import
                // and must be referenced fully qualified (`mod.widgets.Foo`).
                // The runtime reports such misses as "variable X not found in
                // scope" with suggestions.
                if id == id!(use) {
                    self.state.push(State::Use { index: self.index });
                    self.state.push(State::BeginExpr { required: true });
                    return 1;
                }
                if id == id!(fn) {
                    self.state.push(State::FnMaybeLet { index: self.index });
                    return 1;
                }
                if id == id!(let) {
                    // we have to have an identifier after let
                    self.state.push(State::Let { index: self.index });
                    return 1;
                }
                if id == id!(var) {
                    // we have to have an identifier after var
                    self.state.push(State::Var { index: self.index });
                    return 1;
                }
                if id == id!(return) {
                    let code_len_before = self.code_len() as u32;
                    self.state.push(State::EmitReturn {
                        index: self.index,
                        code_len_before,
                    });
                    self.state.push(State::BeginExpr { required: false });
                    return 1;
                }
                if id == id!(break) {
                    self.state.push(State::EmitBreak { index: self.index });
                    self.state.push(State::BeginExpr { required: false });
                    return 1;
                }
                if id == id!(continue) {
                    self.state.push(State::EmitContinue { index: self.index });
                    return 1;
                }
                if id == id!(true) {
                    self.push_code(ScriptValue::from_bool(true), self.index);
                    self.state.push(State::EndExpr);
                    return 1;
                }
                if id == id!(false) {
                    self.push_code(ScriptValue::from_bool(false), self.index);
                    self.state.push(State::EndExpr);
                    return 1;
                }
                if id == id!(me) {
                    self.push_code(Opcode::ME.into(), self.index);
                    self.state.push(State::EndExpr);
                    return 1;
                }
                if id == id!(scope) {
                    self.slot_disqualify_body();
                    self.push_code(Opcode::SCOPE.into(), self.index);
                    self.state.push(State::EndExpr);
                    return 1;
                }
                if id == id!(nil) {
                    self.push_code(NIL, self.index);
                    self.state.push(State::EndExpr);
                    return 1;
                }
                if id.not_empty() {
                    self.slot_note_read(id);
                    self.push_code(ScriptValue::from_id(id), self.index);
                    self.state.push(State::EndExpr);
                    return 1;
                }
                if let Some(v) = tok.as_color() {
                    self.push_code(ScriptValue::from_color(v), self.index);
                    self.state.push(State::EndExpr);
                    return 1;
                }
                if let Some(value) = tok.as_string() {
                    self.push_code(value, self.index);
                    self.state.push(State::EndExpr);
                    return 1;
                }
                if op == id!(*) {
                    self.push_code(ScriptValue::from_id(id!(*)), self.index);
                    self.state.push(State::EndExpr);
                    return 1;
                }
                if op == id!(-) || op == id!(+) || op == id!(!) || op == id!(~) {
                    self.state.push(State::EmitUnary {
                        what_op: op,
                        index: self.index,
                    });
                    self.state.push(State::BeginExpr { required: true });
                    return 1;
                }
                if op == id!(@) {
                    self.state.push(State::EndExpr);
                    self.state.push(State::EscapedId);
                    return 1;
                }
                if op == id!(||) {
                    self.push_code(Opcode::FN_ARGS.into(), self.index);
                    self.state.push(State::FnBody { lambda: true });
                    return 1;
                }
                if op == id!(|) {
                    self.push_code(Opcode::FN_ARGS.into(), self.index);
                    self.state.push(State::FnArgList { lambda: true });
                    return 1;
                }
                if op == id!(.) {
                    self.state.push(State::EmitOp {
                        what_op: id!(me.),
                        index: self.index,
                        slot_assign: None,
                    });
                    self.state.push(State::BeginExpr { required: true });
                    return 1;
                }
                if op == id!(..) {
                    // Prefix .. is the splat operator
                    self.state.push(State::EmitSplat { index: self.index });
                    self.state.push(State::BeginExpr { required: true });
                    return 1;
                }
                if !required && (sep == id!(;) || sep == id!(,)) {
                    // self.push_code(NIL, self.index);
                }
                if required {
                    error!(
                        self,
                        tokenizer, "Expected expression after {:?} found {:?}", self.state, tok
                    );
                    self.push_code_none(NIL);
                }
            }

            State::EndExpr => {
                // Newline-delimited statements: a *postfix* `(` (call), `[`
                // (index), or `{` (proto instantiation) on a new line does NOT
                // glue onto the just-completed expression; it begins a new
                // statement. Without this, `let h = x % 7` followed by
                // `(h + 6) % 7` on the next line parses as calling the number 7
                // with `(h + 6)` -- the calendar-app bug -- and a line-leading
                // bare-object statement `{v: x}` after a value line compiled as
                // instantiating that value as a prototype, silently mangling
                // both statements. These three are the footgun class: each can
                // be BOTH a postfix operator on the previous value AND the
                // start of a fresh statement, so greedy gluing is a silent
                // surprise.
                //
                // Infix operators and `.` are deliberately NOT diverted: they
                // cannot begin a statement, so a leading one is unambiguously a
                // continuation -- and makepad's shader DSL relies on exactly that
                // to break long math across lines (`let c = a * k\n + (b) * j`).
                // To continue an expression across lines, lead the new line with an
                // operator (or trail the previous line with one).
                //
                // Suppressed inside `( )`/`[ ]`, where newlines are insignificant
                // and even a `(`/`[` is part of the surrounding expression.
                if tokenizer.token_preceded_by_newline(self.index)
                    && (tok.is_open_round() || tok.is_open_square() || tok.is_open_curly())
                    && !self.inside_round_square_bracket()
                {
                    return 0;
                }
                if op == id!(~) {
                    return 0;
                }
                if op == id!(?) {
                    // we have a post op return if err
                    if let Some(State::EmitOp { what_op, index, .. }) = self.state.last() {
                        if *what_op == id!(.) || *what_op == id!(.?) {
                            self.push_code(State::operator_to_opcode(*what_op), *index);
                            self.state.pop();
                        }
                    }
                    self.push_code(State::operator_to_opcode(id!(?)), self.index);
                    return 1;
                }
                // named operators
                let op = if id == id!(is) {
                    id!(is)
                } else if id == id!(and) {
                    id!(&&)
                } else if id == id!(or) {
                    id!(||)
                } else {
                    op
                };

                // Handle short-circuit operators (&&, ||, |?) specially
                // These need the TEST opcode emitted BEFORE the second operand
                if State::is_short_circuit_op(op) {
                    // Emit any pending unary operators first - they bind tighter than all binary ops
                    loop {
                        if let Some(State::EmitUnary { what_op, index }) = self.state.last() {
                            let (what_op, index) = (*what_op, *index);
                            self.state.pop();
                            self.push_code(State::operator_to_unary(what_op), index);
                        } else {
                            break;
                        }
                    }

                    let op_order = State::operator_order(op);

                    // First, process any pending EmitOp with higher or equal precedence
                    // (this ensures proper operator precedence)
                    while let Some(last) = self.state.last() {
                        if let State::EmitOp {
                            what_op,
                            index,
                            slot_assign,
                        } = last
                        {
                            if State::operator_order(*what_op) <= op_order {
                                let what_op = *what_op;
                                let index = *index;
                                // defensive: a drained assign loses its slot
                                // marker — keep the name fully dynamic
                                if let Some(name) = *slot_assign {
                                    self.slot_poison(name);
                                }
                                self.state.pop();
                                self.push_code(State::operator_to_opcode(what_op), index);
                            } else {
                                break;
                            }
                        } else if let State::ShortCircuitEnd { test_slot } = last {
                            // Patch any higher-precedence short-circuit ops
                            let test_slot = *test_slot;
                            self.state.pop();
                            self.set_opcode_args(
                                test_slot,
                                OpcodeArgs::from_u32(self.code_len() - test_slot),
                            );
                            self.last_jump_target = self.code_len();
                        } else {
                            break;
                        }
                    }

                    // Emit the TEST opcode with placeholder jump
                    let test_slot = self.code_len();
                    self.push_code(State::short_circuit_opcode(op).into(), self.index);

                    // Push state to patch the jump after second operand is parsed
                    self.state.push(State::ShortCircuitEnd { test_slot });
                    self.state.push(State::BeginExpr { required: true });
                    return 1;
                }

                // Handle ?= with lazy evaluation for SIMPLE variable assignments only
                // Field/index assignments (x.f?=, x[i]?=) use their own opcodes with eager evaluation
                // Emits: [id, ASSIGN_IFNIL(jump), rhs_code, ASSIGN]
                if op == id!(?=) {
                    // Only use lazy eval for simple variable assignment (last code is an identifier)
                    // For field/index assignments, fall through to normal operator handling
                    let is_simple_var_assign = self.code_last().map(|c| c.is_id()).unwrap_or(false)
                        && self
                            .state
                            .last()
                            .map(|s| {
                                !matches!(
                                    s,
                                    State::EmitOp {
                                        what_op: id!(.) | id!(.?),
                                        ..
                                    }
                                )
                            })
                            .unwrap_or(true)
                        && self.code_last() != Some(&Opcode::ARRAY_INDEX.into());

                    if is_simple_var_assign {
                        // Emit any pending unary operators first
                        loop {
                            if let Some(State::EmitUnary { what_op, index }) = self.state.last() {
                                let (what_op, index) = (*what_op, *index);
                                self.state.pop();
                                self.push_code(State::operator_to_unary(what_op), index);
                            } else {
                                break;
                            }
                        }

                        // Emit ASSIGN_IFNIL with placeholder jump
                        let test_slot = self.code_len();
                        self.push_code(Opcode::ASSIGN_IFNIL.into(), self.index);

                        // Push state to emit ASSIGN and patch jump after RHS is parsed
                        self.state.push(State::ShortCircuitAssignEnd {
                            test_slot,
                            index: self.index,
                        });
                        self.state.push(State::BeginExpr { required: true });
                        return 1;
                    }
                    // else: fall through to normal operator handling for field/index ?=
                }

                if State::operator_order(op) != 0 {
                    // Emit any pending unary operators, but only if this binary op has lower
                    // precedence than unary (order >= 6). Field access (order 3) has higher
                    // precedence than unary, so -x.y should parse as -(x.y), not (-x).y
                    if State::operator_order(op) >= 6 {
                        loop {
                            if let Some(State::EmitUnary { what_op, index }) = self.state.last() {
                                let (what_op, index) = (*what_op, *index);
                                self.state.pop();
                                self.push_code(State::operator_to_unary(what_op), index);
                            } else {
                                break;
                            }
                        }
                    }

                    // Slot resolver: an assign operator directly after a bare
                    // id we logged makes that id an assignment target, not a
                    // read. Detect BEFORE the index/field diversions below —
                    // their guards (ARRAY_INDEX / dot-op below) are mutually
                    // exclusive with "last emitted is that id".
                    let slot_assign = if State::is_assign_operator(op) {
                        self.slot_take_assign_target(op)
                    } else {
                        None
                    };
                    let next_state = State::EmitOp {
                        what_op: op,
                        index: self.index,
                        slot_assign,
                    };
                    // check if we have a ..[] =
                    if Some(&Opcode::ARRAY_INDEX.into()) == self.code_last() {
                        if State::is_assign_operator(op) {
                            self.pop_code();
                            self.state.push(State::EmitIndexAssign {
                                what_op: op,
                                index: self.index,
                            });
                            self.state.push(State::BeginExpr { required: true });
                            return 1;
                        }
                    }
                    // check if we need to generate proto_field ops
                    if let Some(last) = self.state.pop() {
                        if let State::EmitOp {
                            what_op: id!(.) | id!(.?),
                            ..
                        } = last
                        {
                            if State::is_assign_operator(op) {
                                // For : operator with field chain like field.sub: value
                                // transform to me.field.sub = value
                                if op == id!(:) {
                                    // Find the start of the field chain by walking backwards
                                    // The chain structure is: [id, id, (FIELD, id)*]
                                    // e.g. field.sub -> [id(field), id(sub)]
                                    // e.g. field.sub.sub2 -> [id(field), id(sub), FIELD, id(sub2)]
                                    let mut chain_start = self.opcodes.len();

                                    // Walk backwards through ids and FIELDs
                                    while chain_start > 0 {
                                        let prev = chain_start - 1;
                                        if self.opcodes[prev].is_id()
                                            || self.opcodes[prev] == Opcode::FIELD.into()
                                        {
                                            chain_start = prev;
                                        } else {
                                            break;
                                        }
                                    }

                                    // Now insert ME at chain_start
                                    self.opcodes.insert(chain_start, Opcode::ME.into());
                                    self.source_map.insert(chain_start, Some(self.index));

                                    // Insert PROTO_FIELD after the first id (which is now at chain_start + 1)
                                    // The first id is at chain_start + 1, so PROTO_FIELD goes at chain_start + 2
                                    self.opcodes
                                        .insert(chain_start + 2, Opcode::PROTO_FIELD.into());
                                    self.source_map.insert(chain_start + 2, Some(self.index));
                                }

                                // Patch remaining FIELD to PROTO_FIELD
                                for pair in self.opcodes.rchunks_mut(2) {
                                    if pair[0].is_id() && pair[1] == Opcode::FIELD.into() {
                                        pair[1] = Opcode::PROTO_FIELD.into()
                                    } else if pair[1].is_id() && pair[0] == Opcode::FIELD.into() {
                                        pair[0] = Opcode::PROTO_FIELD.into()
                                    } else {
                                        break;
                                    }
                                }
                                self.state.push(State::EmitFieldAssign {
                                    what_op: op,
                                    index: self.index,
                                });
                                self.state.push(State::BeginExpr { required: true });
                                return 1;
                            }
                        }
                        if last.is_heq_prio(next_state) {
                            // Push `last` back first
                            self.state.push(last);

                            // Find the correct position to insert the new operator.
                            // It should be inserted below ALL EmitOp states that have higher or equal priority.
                            // This ensures correct operator precedence for expressions like t.x*t.y + t.z*t.w
                            let op_order = State::operator_order(op);
                            let is_assign = State::is_assign_operator(op);
                            let mut insert_pos = self.state.len();
                            for i in (0..self.state.len()).rev() {
                                if let State::EmitOp { what_op, .. } = &self.state[i] {
                                    // If both are assignment operators, don't treat as heq_prio (right-to-left associativity)
                                    let pending_is_assign = State::is_assign_operator(*what_op);
                                    if pending_is_assign && is_assign {
                                        break;
                                    }
                                    if State::operator_order(*what_op) <= op_order {
                                        insert_pos = i;
                                    } else {
                                        break;
                                    }
                                } else {
                                    break;
                                }
                            }

                            // Insert new operator and BeginExpr at the found position
                            self.state
                                .insert(insert_pos, State::BeginExpr { required: true });
                            self.state.insert(
                                insert_pos,
                                State::EmitOp {
                                    what_op: op,
                                    index: self.index,
                                    slot_assign,
                                },
                            );
                            return 1;
                        } else {
                            self.state.push(last);
                        }
                    }
                    self.state.push(State::EmitOp {
                        what_op: op,
                        index: self.index,
                        slot_assign,
                    });
                    self.state.push(State::BeginExpr { required: true });
                    return 1;
                }

                if tok.is_open_curly() {
                    for state in self.state.iter().rev() {
                        if let State::EmitOp { .. } = state {
                        } else if let State::EmitUnary { .. } = state {
                        } else if let State::ShortCircuitEnd { .. } = state {
                        } else if let State::IfTest { .. } = state {
                            return 0;
                        } else if let State::TryTestExpr { .. } = state {
                            return 0;
                        } else if let State::WhileTest { .. } = state {
                            return 0;
                        } else if let State::ForBody { .. } = state {
                            return 0;
                        } else if let State::MatchSubject { .. } = state {
                            return 0;
                        } else {
                            break;
                        }
                    }
                    if let Some(last) = self.state.pop() {
                        if let State::EmitOp {
                            what_op: id!(.),
                            index,
                            ..
                        } = last
                        {
                            self.push_code(State::operator_to_opcode(id!(.)), index);
                        } else if let State::EmitOp {
                            what_op: id!(.?),
                            index,
                            ..
                        } = last
                        {
                            self.push_code(State::operator_to_opcode(id!(.?)), index);
                        } else if let State::EmitOp {
                            what_op: id!(+:), ..
                        } = last
                        {
                            // Proto-inherit operator: field +: Proto { ... }
                            // Emit PROTO_INHERIT_READ to read field and push proto value
                            self.push_code(Opcode::PROTO_INHERIT_READ.into(), self.index);
                            self.push_code(Opcode::BEGIN_PROTO.into(), self.index);
                            self.state.push(State::EndProtoInherit);
                            self.state.push(State::BeginStmt {
                                last_was_sep: false,
                            });
                            return 1;
                        } else if let State::EmitOp {
                            what_op: id!(+=),
                            slot_assign,
                            ..
                        } = last
                        {
                            // scope-inherit: target name must stay dynamic
                            if let Some(name) = slot_assign {
                                self.slot_poison(name);
                            }
                            // Scope-inherit operator: value += Proto { ... }
                            // Emit SCOPE_INHERIT_READ to read variable and push proto value
                            self.push_code(Opcode::SCOPE_INHERIT_READ.into(), self.index);
                            self.push_code(Opcode::BEGIN_PROTO.into(), self.index);
                            self.state.push(State::EndScopeInherit);
                            self.state.push(State::BeginStmt {
                                last_was_sep: false,
                            });
                            return 1;
                        } else if let State::EmitFieldAssign {
                            what_op: id!(+=), ..
                        } = last
                        {
                            // Field-inherit operator: obj.field += Proto { ... }
                            // Emit FIELD_INHERIT_READ to read field and push proto value
                            self.push_code(Opcode::FIELD_INHERIT_READ.into(), self.index);
                            self.push_code(Opcode::BEGIN_PROTO.into(), self.index);
                            self.state.push(State::EndFieldInherit);
                            self.state.push(State::BeginStmt {
                                last_was_sep: false,
                            });
                            return 1;
                        } else if let State::EmitIndexAssign {
                            what_op: id!(+=), ..
                        } = last
                        {
                            // Index-inherit operator: obj[index] += Proto { ... }
                            // Emit INDEX_INHERIT_READ to read index and push proto value
                            self.push_code(Opcode::INDEX_INHERIT_READ.into(), self.index);
                            self.push_code(Opcode::BEGIN_PROTO.into(), self.index);
                            self.state.push(State::EndIndexInherit);
                            self.state.push(State::BeginStmt {
                                last_was_sep: false,
                            });
                            return 1;
                        } else {
                            self.state.push(last);
                        }
                    }
                    // `{...}{...}` is NOT object-inherits-from-object. If the value we just
                    // finished is a bare object literal (last opcode END_BARE), a following
                    // `{` begins a NEW value separated by an implicit ("magic") comma - not a
                    // proto-inherit. EndExpr was already popped, so returning 0 without
                    // consuming `{` lets the enclosing collection/argument/block loop (where
                    // commas are optional) parse it as the next element. This makes comma-less
                    // `[{a} {b}]`, `f({a} {b})` and `{ {a} {b} }` parse as separate items
                    // instead of silently collapsing into one. `Ident{...}` instantiation ends
                    // in END_PROTO / an id (not END_BARE), so it is unaffected and still protos.
                    if self.code_last() == Some(&Opcode::END_BARE.into()) {
                        return 0;
                    }
                    self.push_code(Opcode::BEGIN_PROTO.into(), self.index);
                    self.state.push(State::EndProto);
                    self.state.push(State::BeginStmt {
                        last_was_sep: false,
                    });
                    return 1;
                }
                if tok.is_open_round() {
                    // (A `(` starting a new line was already diverted to a new
                    // statement by the newline-continuation guard at the top of
                    // EndExpr, so here it is always a same-line call.)
                    if let Some(last) = self.state.pop() {
                        if let State::EmitOp {
                            what_op: id!(.) | id!(.?),
                            ..
                        } = last
                        {
                            self.push_code(Opcode::METHOD_CALL_ARGS.into(), self.index);
                            self.state.push(State::EndCall {
                                is_method: true,
                                index: self.index,
                            });
                            self.state.push(State::BeginStmt {
                                last_was_sep: false,
                            });
                        } else {
                            self.state.push(last);
                            self.push_code(Opcode::CALL_ARGS.into(), self.index);
                            self.state.push(State::EndCall {
                                is_method: false,
                                index: self.index,
                            });
                            self.state.push(State::BeginStmt {
                                last_was_sep: false,
                            });
                        }
                    }
                    return 1;
                }
                if tok.is_open_square() {
                    if let Some(last) = self.state.pop() {
                        if let State::EmitOp {
                            what_op: id!(.),
                            index,
                            ..
                        } = last
                        {
                            self.push_code(State::operator_to_opcode(id!(.)), index);
                        } else if let State::EmitOp {
                            what_op: id!(.?),
                            index,
                            ..
                        } = last
                        {
                            self.push_code(State::operator_to_opcode(id!(.?)), index);
                        } else {
                            self.state.push(last);
                        }
                    }
                    self.state.push(State::ArrayIndex);
                    self.state.push(State::BeginExpr { required: true });
                    return 1;
                }
                return 0;
            }
            State::BeginStmt { last_was_sep } => {
                if sep == id!(;) || sep == id!(,) {
                    // just eat it
                    // we can pop all operator emits
                    self.state.push(State::BeginStmt { last_was_sep: true });
                    // we should also force a 'nil' in if/else/fn calls just like Rust
                    return 1;
                }
                if tok.is_close_round() || tok.is_close_curly() || tok.is_close_square() {
                    if last_was_sep {
                        if let Some(State::TryTestBlock { last_was_sep, .. }) =
                            self.state.last_mut()
                        {
                            *last_was_sep = true
                        }
                        if let Some(State::TryErrBlock { last_was_sep, .. }) = self.state.last_mut()
                        {
                            *last_was_sep = true
                        }
                        if let Some(State::TryOkBlock { last_was_sep, .. }) = self.state.last_mut()
                        {
                            *last_was_sep = true
                        }
                        if let Some(State::IfTrueBlock { last_was_sep, .. }) = self.state.last_mut()
                        {
                            *last_was_sep = true
                        }
                        if let Some(State::IfElseBlock { last_was_sep, .. }) = self.state.last_mut()
                        {
                            *last_was_sep = true
                        }
                        if let Some(State::EndFnBlock { last_was_sep, .. }) = self.state.last_mut()
                        {
                            *last_was_sep = true
                        }
                    }
                    // pop and let the stack handle it
                    return 0;
                }
                // lets do an expression statement as fallthrough
                self.state.push(State::EndStmt { last: self.index });
                self.state.push(State::BeginExpr { required: false });
                return 0;
            }
            State::EndStmt { last } => {
                if last == self.index {
                    error!(
                        self,
                        tokenizer, "Parser stuck on character {:?}, skipping", tok
                    );
                    self.state.push(State::BeginStmt {
                        last_was_sep: false,
                    });
                    return 1;
                }
                // in a function call we need the

                if let Some(code) = self.opcodes.last_mut() {
                    if let Some((opcode, _)) = code.as_opcode() {
                        if opcode == Opcode::FOR_END {
                            //code.set_opcode_is_statement();
                            self.state.push(State::BeginStmt {
                                last_was_sep: false,
                            });
                            return 0;
                        }
                        if opcode == Opcode::ASSIGN_ME || opcode == Opcode::ASSIGN_ME_VEC {
                            //code.set_opcode_is_statement();
                            self.state.push(State::BeginStmt {
                                last_was_sep: false,
                            });
                            return 0;
                        }
                        if opcode == Opcode::BREAK || opcode == Opcode::CONTINUE {
                            //code.set_opcode_is_statement();
                            self.state.push(State::BeginStmt {
                                last_was_sep: false,
                            });
                            return 0;
                        }
                        if opcode == Opcode::ME_SPLAT {
                            // ME_SPLAT already handles merging into me, no pop_to_me needed
                            self.state.push(State::BeginStmt {
                                last_was_sep: false,
                            });
                            return 0;
                        }
                        if code.is_let_opcode() {
                            self.state.push(State::BeginStmt {
                                last_was_sep: false,
                            });
                            return 0;
                        }
                    }
                }
                // otherwise pop to me
                self.set_pop_to_me();
                //self.push_code_none(Opcode::POP_TO_ME.into());
                self.state.push(State::BeginStmt {
                    last_was_sep: false,
                });
                return 0;
            }
        }
        0
    }

    pub fn parse(
        &mut self,
        tokenizer: &ScriptTokenizer,
        file: &str,
        offsets: (usize, usize),
        values: &[ScriptValue],
    ) {
        self.file = file.to_string();
        self.line_offset = offsets.0;
        self.col_offset = offsets.1;
        // wait for the tokens to be consumed
        let mut steps_zero = 0;
        let tokens = &tokenizer.tokens;
        while self.index < tokens.len() as u32 && self.state.len() > 0 {
            let tok = if let Some(tok) = tokens.get(self.index as usize) {
                tok.token.clone()
            } else {
                ScriptToken::StreamEnd
            };

            let step = self.parse_step(tokenizer, tok, values);
            if step == 0 {
                steps_zero += 1;
            } else {
                steps_zero = 0;
            }
            // println!("{:?} {:?}", self.code, self.state);
            if steps_zero > 1000 {
                error!(
                    self,
                    tokenizer,
                    "Parser stuck {:?} {} {:?}",
                    self.state,
                    step,
                    tokens[self.index as usize]
                );
                break;
            }
            self.index += step;
        }

        // Auto-close any unclosed proto/object states left on the stack
        // This happens when input is truncated mid-object (e.g. streaming)
        let mut let_closed = false;
        let last_index = self.index.saturating_sub(1);
        // set_pop_to_me() reads self.index for source map entries, so point it
        // at the last valid token rather than one-past-end.
        self.index = last_index;
        while self.state.len() > 0 {
            match self.state.pop().unwrap() {
                State::EndProto => {
                    self.push_code(Opcode::END_PROTO.into(), last_index);
                }
                State::EndProtoInherit => {
                    self.push_code(Opcode::END_PROTO.into(), last_index);
                    self.push_code(Opcode::PROTO_INHERIT_WRITE.into(), last_index);
                }
                State::EndScopeInherit => {
                    self.push_code(Opcode::END_PROTO.into(), last_index);
                    self.push_code(Opcode::SCOPE_INHERIT_WRITE.into(), last_index);
                }
                State::EndFieldInherit => {
                    self.push_code(Opcode::END_PROTO.into(), last_index);
                    self.push_code(Opcode::FIELD_INHERIT_WRITE.into(), last_index);
                }
                State::EndIndexInherit => {
                    self.push_code(Opcode::END_PROTO.into(), last_index);
                    self.push_code(Opcode::INDEX_INHERIT_WRITE.into(), last_index);
                }
                State::EndStmt { .. } => {
                    // The EndStmt of an auto-closed `let` must NOT mark a
                    // statement value: LET consumed it, the final RETURN
                    // would pop an empty stack.
                    if !let_closed {
                        self.set_pop_to_me();
                    }
                    let_closed = false;
                }
                State::EmitOp {
                    what_op,
                    index,
                    slot_assign,
                } => {
                    if let Some(name) = slot_assign {
                        self.slot_poison(name);
                    }
                    self.push_code(State::operator_to_opcode(what_op), index);
                }
                State::EmitUnary { what_op, index } => {
                    self.push_code(State::operator_to_unary(what_op), index);
                }
                State::CallMaybeDo { is_method, index } => {
                    // A call as the final tokens of the source: no next token
                    // arrived to rule out a trailing `do` block, so resolve it
                    // as a plain call here.
                    if is_method {
                        self.push_code(Opcode::METHOD_CALL_EXEC.into(), index);
                    } else {
                        self.push_code(Opcode::CALL_EXEC.into(), index);
                    }
                }
                State::ShortCircuitEnd { test_slot } => {
                    // A short-circuit op at end of source: patch its jump so a
                    // taken test doesn't land on a stale zero offset.
                    self.set_opcode_args(test_slot, OpcodeArgs::from_u32(self.code_len() - test_slot));
                    self.last_jump_target = self.code_len();
                }
                // A fn/lambda body still open at end of source: close it the
                // same way the live states do — without this, the body's
                // jump-over stays 0 (FN_BODY_DYN re-runs, finds its me gone,
                // and execution FALLS INTO the body at definition time) and
                // any pending `let` below never binds.
                State::EndFnExpr { fn_slot, index } => {
                    self.push_code(Opcode::RETURN.into(), index);
                    self.set_opcode_args(
                        fn_slot as _,
                        OpcodeArgs::from_u32(self.code_len() as u32 - fn_slot),
                    );
                }
                State::EndFnBlock {
                    fn_slot,
                    last_was_sep,
                    index,
                } => {
                    if !last_was_sep && self.has_pop_to_me() {
                        self.clear_pop_to_me();
                        self.push_code(Opcode::RETURN.into(), index);
                    } else {
                        self.push_code(
                            ScriptValue::from_opcode_args(Opcode::RETURN, OpcodeArgs::NIL),
                            index,
                        );
                    }
                    self.set_opcode_args(
                        fn_slot as _,
                        OpcodeArgs::from_u32(self.code_len() as u32 - fn_slot),
                    );
                }
                State::EmitLetDyn { index, name } => {
                    self.slot_note_let(name);
                    self.push_code(Opcode::LET_DYN.into(), index);
                    // A let statement leaves no value: without this, the
                    // trailing RETURN pops the value LET just consumed.
                    self.clear_pop_to_me();
                    let_closed = true;
                }
                State::EmitLetTyped { index } => {
                    self.push_code(Opcode::LET_TYPED.into(), index);
                    self.clear_pop_to_me();
                    let_closed = true;
                }
                _ => {
                    // Other states (EndExpr, BeginStmt, etc.) - just drop them
                }
            }
        }

        // Handle the last value as the script's return value, similar to function blocks
        if self.has_pop_to_me() {
            self.clear_pop_to_me();
            self.push_code(Opcode::RETURN.into(), self.index.saturating_sub(1));
        } else if self.opcodes.len() > 0 {
            // Check if last opcode already returns or is a statement that doesn't produce a value
            let needs_nil_return = if let Some(code) = self.code_last() {
                if let Some((opcode, _)) = code.as_opcode() {
                    opcode != Opcode::RETURN
                } else {
                    true
                }
            } else {
                true
            };
            if needs_nil_return {
                self.push_code(
                    ScriptValue::from_opcode_args(Opcode::RETURN, OpcodeArgs::NIL),
                    self.index.saturating_sub(1),
                );
            }
        }

        //println!("{:?}", self.opcodes)
    }

    /// Save parser state before auto-close, returning a checkpoint that can be
    /// used to restore state for continuation when more source arrives.
    /// Call this after the main parse loop but BEFORE auto-close runs.
    pub fn save_checkpoint(&self) -> ParserCheckpoint {
        ParserCheckpoint {
            opcodes_len: self.opcodes.len(),
            source_map_len: self.source_map.len(),
            token_index: self.index,
            state: self.state.clone(),
            destruct_defaults_len: self.destruct_defaults.len(),
            nested_patterns_len: self.nested_patterns.len(),
            last_opcode: self.opcodes.last().copied(),
            slot_ctxs: self.slot_ctxs.clone(),
            slot_frames_len: self.slot_frames.len(),
        }
    }

    /// Restore parser state from a checkpoint, undoing any auto-close opcodes
    /// that were appended after the checkpoint was taken.
    pub fn restore_checkpoint(&mut self, cp: ParserCheckpoint) {
        self.opcodes.truncate(cp.opcodes_len);
        self.source_map.truncate(cp.source_map_len);
        // Restore the last opcode in case auto-close mutated its POP_TO_ME flag
        if let Some(saved) = cp.last_opcode {
            if let Some(last) = self.opcodes.last_mut() {
                *last = saved;
            }
        }
        self.index = cp.token_index;
        self.state = cp.state;
        self.destruct_defaults.truncate(cp.destruct_defaults_len);
        self.nested_patterns.truncate(cp.nested_patterns_len);
        self.slot_ctxs = cp.slot_ctxs;
        self.slot_frames.truncate(cp.slot_frames_len);
    }

    /// Parse tokens incrementally: run the main parse loop, then save a checkpoint,
    /// then auto-close for execution. Returns the checkpoint for later restoration.
    ///
    /// `unfinished_string`: if the tokenizer's last token is `StringUnfinished`,
    /// pass the interned value here so the parser emits the real partial string
    /// into opcodes (for incremental UI rendering). The tokenizer token is NOT
    /// modified, preserving its state machine for the next `tokenize()` call.
    pub fn parse_streaming(
        &mut self,
        tokenizer: &ScriptTokenizer,
        file: &str,
        offsets: (usize, usize),
        values: &[ScriptValue],
        unfinished_string: Option<ScriptValue>,
    ) -> ParserCheckpoint {
        self.file = file.to_string();
        self.line_offset = offsets.0;
        self.col_offset = offsets.1;
        let mut steps_zero = 0;
        let tokens = &tokenizer.tokens;
        let max_token_index = if tokens.is_empty() {
            0
        } else {
            (tokens.len() - 1) as u32
        };
        while self.index < tokens.len() as u32 && self.state.len() > 0 {
            let tok = if let Some(tok) = tokens.get(self.index as usize) {
                tok.token.clone()
            } else {
                ScriptToken::StreamEnd
            };
            // When we hit StringUnfinished: save checkpoint BEFORE it so the
            // parser re-processes this token next time (with updated content).
            // Substitute the interned value for the current execution, then
            // consume remaining tokens and auto-close.
            if let ScriptToken::StringUnfinished = &tok {
                let checkpoint_before = self.save_checkpoint();
                let tok = if let Some(v) = unfinished_string {
                    ScriptToken::String(v)
                } else {
                    tok
                };
                let step = self.parse_step(tokenizer, tok, values);
                self.index += step;
                // Continue parsing any remaining tokens after the string
                while self.index < tokens.len() as u32 && self.state.len() > 0 {
                    let tok2 = if let Some(tok2) = tokens.get(self.index as usize) {
                        tok2.token.clone()
                    } else {
                        ScriptToken::StreamEnd
                    };
                    let step2 = self.parse_step(tokenizer, tok2, values);
                    self.index += step2;
                }
                return self.auto_close(checkpoint_before, max_token_index);
            }

            let step = self.parse_step(tokenizer, tok, values);
            if step == 0 {
                steps_zero += 1;
            } else {
                steps_zero = 0;
            }
            if steps_zero > 1000 {
                error!(
                    self,
                    tokenizer,
                    "Parser stuck {:?} {} {:?}",
                    self.state,
                    step,
                    tokens[self.index as usize]
                );
                break;
            }
            self.index += step;
        }

        // Save checkpoint BEFORE auto-close
        let checkpoint = self.save_checkpoint();
        self.auto_close(checkpoint, max_token_index)
    }

    /// Auto-close any open states for execution, then append a RETURN.
    /// `max_token_index` is the last valid token index in the tokenizer,
    /// used to clamp source map entries for synthetic opcodes.
    /// Returns the checkpoint that was passed in (taken before auto-close).
    fn auto_close(
        &mut self,
        checkpoint: ParserCheckpoint,
        max_token_index: u32,
    ) -> ParserCheckpoint {
        // Use the last consumed token index for synthetic auto-close source map entries,
        // same as the regular parse() method. Clamp to max_token_index for safety.
        let last_index = self.index.saturating_sub(1).min(max_token_index);
        let mut let_closed = false;
        // set_pop_to_me() reads self.index for source map entries, so point it
        // at the last valid token rather than one-past-end.
        self.index = last_index;
        while self.state.len() > 0 {
            match self.state.pop().unwrap() {
                State::EndProto => {
                    self.push_code(Opcode::END_PROTO.into(), last_index);
                }
                State::EndProtoInherit => {
                    self.push_code(Opcode::END_PROTO.into(), last_index);
                    self.push_code(Opcode::PROTO_INHERIT_WRITE.into(), last_index);
                }
                State::EndScopeInherit => {
                    self.push_code(Opcode::END_PROTO.into(), last_index);
                    self.push_code(Opcode::SCOPE_INHERIT_WRITE.into(), last_index);
                }
                State::EndFieldInherit => {
                    self.push_code(Opcode::END_PROTO.into(), last_index);
                    self.push_code(Opcode::FIELD_INHERIT_WRITE.into(), last_index);
                }
                State::EndIndexInherit => {
                    self.push_code(Opcode::END_PROTO.into(), last_index);
                    self.push_code(Opcode::INDEX_INHERIT_WRITE.into(), last_index);
                }
                State::EndStmt { .. } => {
                    // The EndStmt of an auto-closed `let` must NOT mark a
                    // statement value: LET consumed it, the final RETURN
                    // would pop an empty stack.
                    if !let_closed {
                        self.set_pop_to_me();
                    }
                    let_closed = false;
                }
                State::EmitOp {
                    what_op,
                    index,
                    slot_assign,
                } => {
                    if let Some(name) = slot_assign {
                        self.slot_poison(name);
                    }
                    self.push_code(State::operator_to_opcode(what_op), index);
                }
                State::EmitUnary { what_op, index } => {
                    self.push_code(State::operator_to_unary(what_op), index);
                }
                State::CallMaybeDo { is_method, index } => {
                    // A call as the final tokens of the source: no next token
                    // arrived to rule out a trailing `do` block, so resolve it
                    // as a plain call here (a later streamed append restores
                    // the checkpoint and re-parses if a `do` does arrive).
                    if is_method {
                        self.push_code(Opcode::METHOD_CALL_EXEC.into(), index);
                    } else {
                        self.push_code(Opcode::CALL_EXEC.into(), index);
                    }
                }
                State::ShortCircuitEnd { test_slot } => {
                    // A short-circuit op at end of source: patch its jump so a
                    // taken test doesn't land on a stale zero offset.
                    self.set_opcode_args(test_slot, OpcodeArgs::from_u32(self.code_len() - test_slot));
                    self.last_jump_target = self.code_len();
                }
                // A fn/lambda body still open at end of source: close it the
                // same way the live states do — without this, the body's
                // jump-over stays 0 (FN_BODY_DYN re-runs, finds its me gone,
                // and execution FALLS INTO the body at definition time) and
                // any pending `let` below never binds.
                State::EndFnExpr { fn_slot, index } => {
                    self.push_code(Opcode::RETURN.into(), index);
                    self.set_opcode_args(
                        fn_slot as _,
                        OpcodeArgs::from_u32(self.code_len() as u32 - fn_slot),
                    );
                }
                State::EndFnBlock {
                    fn_slot,
                    last_was_sep,
                    index,
                } => {
                    if !last_was_sep && self.has_pop_to_me() {
                        self.clear_pop_to_me();
                        self.push_code(Opcode::RETURN.into(), index);
                    } else {
                        self.push_code(
                            ScriptValue::from_opcode_args(Opcode::RETURN, OpcodeArgs::NIL),
                            index,
                        );
                    }
                    self.set_opcode_args(
                        fn_slot as _,
                        OpcodeArgs::from_u32(self.code_len() as u32 - fn_slot),
                    );
                }
                State::EmitLetDyn { index, name } => {
                    self.slot_note_let(name);
                    self.push_code(Opcode::LET_DYN.into(), index);
                    // A let statement leaves no value: without this, the
                    // trailing RETURN pops the value LET just consumed.
                    self.clear_pop_to_me();
                    let_closed = true;
                }
                State::EmitLetTyped { index } => {
                    self.push_code(Opcode::LET_TYPED.into(), index);
                    self.clear_pop_to_me();
                    let_closed = true;
                }
                _ => {}
            }
        }

        if self.has_pop_to_me() {
            self.clear_pop_to_me();
            self.push_code(Opcode::RETURN.into(), last_index);
        } else if self.opcodes.len() > 0 {
            let needs_nil_return = if let Some(code) = self.code_last() {
                if let Some((opcode, _)) = code.as_opcode() {
                    opcode != Opcode::RETURN
                } else {
                    true
                }
            } else {
                true
            };
            if needs_nil_return {
                self.push_code(
                    ScriptValue::from_opcode_args(Opcode::RETURN, OpcodeArgs::NIL),
                    last_index,
                );
            }
        }

        checkpoint
    }

    pub fn dump_opcodes(&self) {
        println!("=== OPCODES ({} total) ===", self.opcodes.len());
        for (i, op) in self.opcodes.iter().enumerate() {
            println!("{:3}: {:?}", i, op);
        }
        println!("=== END OPCODES ===");
    }
}
