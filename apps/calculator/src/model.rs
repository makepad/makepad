//! Calculator document, commands, and layout classification. Pure Rust.

use crate::engine::{
    close_unmatched_parens, display_expression, error_message, evaluate, format_number,
    last_token_is_number, preview as engine_preview, root_repeat,
    roundtrip_literal, tokenize, trailing_function_opener, trailing_operand_span, TokenKind,
    MAX_SOURCE_BYTES,
};
use makepad_strict_json::{self as json, Value};
use std::collections::VecDeque;

pub const MAX_HISTORY: usize = 200;
pub const MAX_DOCUMENT_BYTES: usize = 512 * 1024;
pub const LAYOUT_COMPACT_MAX: f64 = 700.0;
pub const LAYOUT_SHORT_MAX: f64 = 480.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AngleMode {
    Degrees,
    Radians,
}

impl AngleMode {
    pub fn as_str(self) -> &'static str {
        match self {
            AngleMode::Degrees => "deg",
            AngleMode::Radians => "rad",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "deg" => Some(AngleMode::Degrees),
            "rad" => Some(AngleMode::Radians),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Subtract,
    Multiply,
    Divide,
    Power,
}

impl BinaryOp {
    fn as_str(self) -> &'static str {
        match self {
            BinaryOp::Add => "add",
            BinaryOp::Subtract => "subtract",
            BinaryOp::Multiply => "multiply",
            BinaryOp::Divide => "divide",
            BinaryOp::Power => "power",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "add" => Some(BinaryOp::Add),
            "subtract" => Some(BinaryOp::Subtract),
            "multiply" => Some(BinaryOp::Multiply),
            "divide" => Some(BinaryOp::Divide),
            "power" => Some(BinaryOp::Power),
            _ => None,
        }
    }

    pub fn glyph(self) -> char {
        match self {
            BinaryOp::Add => '+',
            BinaryOp::Subtract => '-',
            BinaryOp::Multiply => '*',
            BinaryOp::Divide => '/',
            BinaryOp::Power => '^',
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Function {
    Reciprocal,
    Sqrt,
    Cbrt,
    Exp,
    Pow10,
    Ln,
    Log10,
    Sin,
    Cos,
    Tan,
}

impl Function {
    pub fn name(self) -> &'static str {
        match self {
            Function::Reciprocal => "inv",
            Function::Sqrt => "sqrt",
            Function::Cbrt => "cbrt",
            Function::Exp => "exp",
            Function::Pow10 => "pow10",
            Function::Ln => "ln",
            Function::Log10 => "log",
            Function::Sin => "sin",
            Function::Cos => "cos",
            Function::Tan => "tan",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Constant {
    E,
    Pi,
}

impl Constant {
    pub fn ident(self) -> &'static str {
        match self {
            Constant::E => "e",
            Constant::Pi => "pi",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Editing,
    Result,
    Error,
}

impl Phase {
    fn as_str(self) -> &'static str {
        match self {
            Phase::Editing => "editing",
            Phase::Result => "result",
            Phase::Error => "error",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "editing" => Some(Phase::Editing),
            "result" => Some(Phase::Result),
            "error" => Some(Phase::Error),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Repeat {
    pub op: BinaryOp,
    pub rhs: f64,
    pub relative_percent: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    pub source: String,
    pub phase: Phase,
    pub value: f64,
    pub repeat: Option<Repeat>,
    pub reuse_repeat: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HistoryEntry {
    pub id: u64,
    pub source: String,
    pub value: f64,
    pub angle: AngleMode,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CalculatorDoc {
    pub version: u32,
    pub angle: AngleMode,
    pub memory: Option<f64>,
    pub session: Session,
    pub next_id: u64,
    pub history: VecDeque<HistoryEntry>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorCode {
    Incomplete,
    Syntax,
    UnknownName,
    DivisionByZero,
    Domain,
    Overflow,
    Limit,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::Incomplete => "incomplete",
            ErrorCode::Syntax => "syntax",
            ErrorCode::UnknownName => "unknown_name",
            ErrorCode::DivisionByZero => "division_by_zero",
            ErrorCode::Domain => "domain",
            ErrorCode::Overflow => "overflow",
            ErrorCode::Limit => "limit",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvalError {
    pub code: ErrorCode,
    pub byte_offset: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Preview {
    Value(f64),
    Incomplete,
    Error(EvalError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    Calculator,
    History,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UiState {
    pub route: Route,
    pub selected_history: Option<u64>,
    pub clear_is_ac: bool,
    pub last_error: Option<EvalError>,
    pub storage_status: Option<String>,
    pub input_status: Option<String>,
    pub save_failed: bool,
    pub load_failed: bool,
    pub awaiting_reset: bool,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            route: Route::Calculator,
            selected_history: None,
            clear_is_ac: true,
            last_error: None,
            storage_status: None,
            input_status: None,
            save_failed: false,
            load_failed: false,
            awaiting_reset: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WidthClass {
    Compact,
    Wide,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayoutClass {
    pub width: WidthClass,
    pub short: bool,
}

impl LayoutClass {
    pub fn scientific(self) -> bool {
        self.width == WidthClass::Wide
    }

    pub fn history_tape(self) -> bool {
        self.width == WidthClass::Wide && !self.short
    }
}

pub fn classify_layout(width: f64, height: f64) -> LayoutClass {
    LayoutClass {
        width: if width < LAYOUT_COMPACT_MAX {
            WidthClass::Compact
        } else {
            WidthClass::Wide
        },
        short: height < LAYOUT_SHORT_MAX,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KeyMetrics {
    pub width: f64,
    pub height: f64,
    pub h_gap: f64,
    pub v_gap: f64,
    pub sci_basic_gap: f64,
    pub radius: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayoutMetrics {
    pub class: LayoutClass,
    pub inset: f64,
    pub tape_width: f64,
    pub col_sep: f64,
    pub key: KeyMetrics,
    pub show_tape: bool,
    pub show_scientific: bool,
    pub show_short_bar: bool,
    pub show_toolbar: bool,
    pub show_display: bool,
    pub result_pt: f64,
    pub expr_pt: f64,
    pub digit_pt: f64,
    pub op_pt: f64,
    pub sci_pt: f64,
    pub util_pt: f64,
}

impl LayoutMetrics {
    pub fn for_size(width: f64, height: f64) -> Self {
        let class = classify_layout(width, height);
        match (class.width, class.short) {
            (WidthClass::Wide, true) => Self::short_scientific(width, height),
            (WidthClass::Wide, false) => Self::wide(width, height),
            (WidthClass::Compact, _) => Self::compact(width, height),
        }
    }

    fn wide(width: f64, height: f64) -> Self {
        let (inset, tape, col_sep, h_gap, sci_basic) = if width < 1100.0 {
            (12.0, 180.0, 12.0, 4.0, 8.0)
        } else {
            (20.0, 280.0, 20.0, 8.0, 16.0)
        };
        let workspace = (width - inset * 2.0 - tape - col_sep).max(0.0);
        let inner_gaps = 7.0 * h_gap + sci_basic;
        let key_w = ((workspace - inner_gaps) / 9.0).max(44.0);
        let toolbar = 48.0;
        let display = 128.0;
        let gap = 16.0;
        let available = (height - inset * 2.0 - toolbar - display - gap).max(44.0);
        let v_gap = 10.0;
        let key_h = ((available - 4.0 * v_gap) / 5.0).clamp(44.0, 72.0);
        Self {
            class: LayoutClass {
                width: WidthClass::Wide,
                short: false,
            },
            inset,
            tape_width: tape,
            col_sep,
            key: KeyMetrics {
                width: key_w,
                height: key_h,
                h_gap,
                v_gap,
                sci_basic_gap: sci_basic,
                radius: 20.0,
            },
            show_tape: true,
            show_scientific: true,
            show_short_bar: false,
            show_toolbar: true,
            show_display: true,
            result_pt: 80.0,
            expr_pt: 22.0,
            digit_pt: 32.0,
            op_pt: 34.0,
            sci_pt: 18.0,
            util_pt: 24.0,
        }
    }

    fn compact(width: f64, height: f64) -> Self {
        let key_w = (width - 32.0 - 30.0) / 4.0;
        let top = 8.0 + 44.0 + (147.0 - 52.0) + 136.0;
        let bottom = 16.0;
        let available = (height - top - bottom).max(44.0);
        let v_gap = 10.0;
        let key_h = ((available - 4.0 * v_gap) / 5.0).clamp(44.0, 85.0);
        Self {
            class: LayoutClass {
                width: WidthClass::Compact,
                short: height < LAYOUT_SHORT_MAX,
            },
            inset: 16.0,
            tape_width: 0.0,
            col_sep: 0.0,
            key: KeyMetrics {
                width: key_w,
                height: key_h,
                h_gap: 10.0,
                v_gap,
                sci_basic_gap: 0.0,
                radius: key_h * 0.5,
            },
            show_tape: false,
            show_scientific: false,
            show_short_bar: false,
            show_toolbar: true,
            show_display: true,
            result_pt: 88.0,
            expr_pt: 20.0,
            digit_pt: 32.0,
            op_pt: 34.0,
            sci_pt: 18.0,
            util_pt: 26.0,
        }
    }

    fn short_scientific(width: f64, _height: f64) -> Self {
        let inner = (width - 16.0).max(0.0);
        let h_gap = 4.0;
        let sci_basic = 8.0;
        let inner_gaps = 8.0 * h_gap + (sci_basic - h_gap);
        let key_w = ((inner - inner_gaps) / 9.0).max(1.0);
        Self {
            class: LayoutClass {
                width: WidthClass::Wide,
                short: true,
            },
            inset: 8.0,
            tape_width: 0.0,
            col_sep: 0.0,
            key: KeyMetrics {
                width: key_w,
                height: 44.0,
                h_gap,
                v_gap: 4.0,
                sci_basic_gap: sci_basic,
                radius: 22.0,
            },
            show_tape: false,
            show_scientific: true,
            show_short_bar: true,
            show_toolbar: false,
            show_display: false,
            result_pt: 28.0,
            expr_pt: 16.0,
            digit_pt: 24.0,
            op_pt: 26.0,
            sci_pt: 16.0,
            util_pt: 18.0,
        }
    }

    pub fn keys_meet_target(&self) -> bool {
        self.key.width + 1e-9 >= 44.0 && self.key.height + 1e-9 >= 44.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Digit(u8),
    Decimal,
    Op(BinaryOp),
    Sign,
    Percent,
    Ee,
    Func(Function),
    Square,
    Cube,
    Factorial,
    Power,
    LParen,
    RParen,
    Backspace,
    Clear,
    AllClear,
    Equals,
    MemoryClear,
    MemoryAdd,
    MemorySub,
    MemoryRecall,
    AngleToggle,
    Copy,
    RecallHistory(u64),
    ClearHistory,
    OpenHistory,
    CloseHistory,
    RetryStorage,
    ResetStorage,
    Constant(Constant),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ApplyOut {
    pub changed: bool,
    pub copied: Option<String>,
    pub open_history: bool,
    pub close_history: bool,
}

impl ApplyOut {
    fn silent() -> Self {
        Self {
            changed: false,
            copied: None,
            open_history: false,
            close_history: false,
        }
    }

    fn changed() -> Self {
        Self {
            changed: true,
            copied: None,
            open_history: false,
            close_history: false,
        }
    }
}

fn json_number(v: f64) -> Value {
    if v.fract() == 0.0 && v.abs() < (i64::MAX as f64) && (v as i64) as f64 == v {
        Value::Int(v as i64)
    } else {
        Value::F64(v)
    }
}

fn read_number(v: &Value) -> Option<f64> {
    match v {
        Value::F64(f) if f.is_finite() => Some(*f),
        Value::Int(i) => Some(*i as f64),
        _ => None,
    }
}

impl CalculatorDoc {
    pub fn to_json(&self) -> Value {
        json::obj(vec![
            ("version", Value::Int(self.version as i64)),
            ("angle", json::s(self.angle.as_str())),
            (
                "memory",
                match self.memory {
                    Some(m) => json_number(m),
                    None => Value::Null,
                },
            ),
            ("session", self.session.to_json()),
            ("next_id", Value::Int(self.next_id as i64)),
            (
                "history",
                Value::Arr(self.history.iter().map(HistoryEntry::to_json).collect()),
            ),
        ])
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        self.to_json().to_json().into_bytes()
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, &'static str> {
        if bytes.len() > MAX_DOCUMENT_BYTES {
            return Err("document too large");
        }
        let value = json::parse(bytes).map_err(|_| "invalid json")?;
        Self::from_value(&value)
    }

    fn from_value(v: &Value) -> Result<Self, &'static str> {
        let version = v.get("version").and_then(Value::as_i64).ok_or("version")?;
        if version != 1 {
            return Err("unknown version");
        }
        let angle = v
            .get("angle")
            .and_then(Value::as_str)
            .and_then(AngleMode::parse)
            .ok_or("angle")?;
        let memory = match v.get("memory") {
            Some(Value::Null) | None => None,
            Some(n) => Some(read_number(n).ok_or("memory")?),
        };
        let session = Session::from_value(v.get("session").ok_or("session")?)?;
        let next_id = v
            .get("next_id")
            .and_then(Value::as_u64)
            .filter(|n| *n > 0 && *n <= i64::MAX as u64)
            .ok_or("next_id")?;
        let history_v = v.get("history").and_then(Value::as_arr).ok_or("history")?;
        if history_v.len() > MAX_HISTORY {
            return Err("history too long");
        }
        let mut history = VecDeque::new();
        let mut seen = std::collections::BTreeSet::new();
        for item in history_v {
            let entry = HistoryEntry::from_value(item)?;
            if !seen.insert(entry.id) {
                return Err("duplicate id");
            }
            if entry.source.len() > MAX_SOURCE_BYTES {
                return Err("expression too long");
            }
            history.push_back(entry);
        }
        if history.iter().any(|entry| entry.id >= next_id) {
            return Err("next_id must exceed history ids");
        }
        Ok(Self {
            version: 1,
            angle,
            memory,
            session,
            next_id,
            history,
        })
    }
}

impl Session {
    fn to_json(&self) -> Value {
        json::obj(vec![
            ("source", json::s(&self.source)),
            ("phase", json::s(self.phase.as_str())),
            ("value", json_number(self.value)),
            (
                "repeat",
                match self.repeat {
                    Some(r) => json::obj(vec![
                        ("op", json::s(r.op.as_str())),
                        ("rhs", json_number(r.rhs)),
                        ("relative_percent", Value::Bool(r.relative_percent)),
                    ]),
                    None => Value::Null,
                },
            ),
            ("reuse_repeat", Value::Bool(self.reuse_repeat)),
        ])
    }

    fn from_value(v: &Value) -> Result<Self, &'static str> {
        let source = v
            .get("source")
            .and_then(Value::as_str)
            .ok_or("source")?
            .to_string();
        if source.len() > MAX_SOURCE_BYTES {
            return Err("expression too long");
        }
        let phase = v
            .get("phase")
            .and_then(Value::as_str)
            .and_then(Phase::parse)
            .ok_or("phase")?;
        let value = v.get("value").and_then(read_number).ok_or("value")?;
        let repeat = match v.get("repeat") {
            Some(Value::Null) | None => None,
            Some(r) => {
                let op = r
                    .get("op")
                    .and_then(Value::as_str)
                    .and_then(BinaryOp::parse)
                    .ok_or("repeat.op")?;
                let rhs = r.get("rhs").and_then(read_number).ok_or("repeat.rhs")?;
                let relative_percent = r
                    .get("relative_percent")
                    .and_then(Value::as_bool)
                    .ok_or("repeat.relative_percent")?;
                Some(Repeat {
                    op,
                    rhs,
                    relative_percent,
                })
            }
        };
        let reuse_repeat = v
            .get("reuse_repeat")
            .and_then(Value::as_bool)
            .ok_or("reuse_repeat")?;
        Ok(Self {
            source,
            phase,
            value,
            repeat,
            reuse_repeat,
        })
    }
}

impl HistoryEntry {
    fn to_json(&self) -> Value {
        json::obj(vec![
            ("id", Value::Int(self.id as i64)),
            ("source", json::s(&self.source)),
            ("value", json_number(self.value)),
            ("angle", json::s(self.angle.as_str())),
        ])
    }

    fn from_value(v: &Value) -> Result<Self, &'static str> {
        let id = v
            .get("id")
            .and_then(Value::as_u64)
            .filter(|n| *n > 0 && *n <= i64::MAX as u64)
            .ok_or("id")?;
        let source = v
            .get("source")
            .and_then(Value::as_str)
            .ok_or("source")?
            .to_string();
        let value = v.get("value").and_then(read_number).ok_or("value")?;
        let angle = v
            .get("angle")
            .and_then(Value::as_str)
            .and_then(AngleMode::parse)
            .ok_or("angle")?;
        Ok(Self {
            id,
            source,
            value,
            angle,
        })
    }
}

pub fn apply(doc: &mut CalculatorDoc, ui: &mut UiState, cmd: Command) -> ApplyOut {
    if ui.load_failed {
        return ApplyOut::silent();
    }
    ui.input_status = None;
    let previous = doc.session.clone();
    let out = apply_inner(doc, ui, cmd);
    if doc.session.source.len() > MAX_SOURCE_BYTES
        || matches!(crate::engine::parse(&doc.session.source), Err(EvalError { code: ErrorCode::Limit, .. }))
    {
        doc.session = previous;
        rebuild_derived(doc, ui);
        ui.input_status = Some("Expression limit reached".into());
        return ApplyOut::silent();
    }
    out
}

fn apply_inner(doc: &mut CalculatorDoc, ui: &mut UiState, cmd: Command) -> ApplyOut {
    match cmd {
        Command::OpenHistory => {
            ui.route = Route::History;
            return ApplyOut {
                changed: false,
                copied: None,
                open_history: true,
                close_history: false,
            };
        }
        Command::CloseHistory => {
            ui.route = Route::Calculator;
            ui.selected_history = None;
            return ApplyOut {
                changed: false,
                copied: None,
                open_history: false,
                close_history: true,
            };
        }
        Command::RetryStorage | Command::ResetStorage => return ApplyOut::silent(),
        Command::Copy => {
            return ApplyOut {
                changed: false,
                copied: Some(display_result(doc, ui)),
                open_history: false,
                close_history: false,
            };
        }
        Command::ClearHistory => {
            if doc.history.is_empty() {
                return ApplyOut::silent();
            }
            doc.history.clear();
            ui.selected_history = None;
            return ApplyOut::changed();
        }
        Command::RecallHistory(id) => {
            let Some(entry) = doc.history.iter().find(|e| e.id == id).cloned() else {
                return ApplyOut::silent();
            };
            doc.session.source = entry.source;
            doc.session.value = entry.value;
            doc.session.phase = Phase::Result;
            doc.session.repeat = None;
            doc.session.reuse_repeat = false;
            doc.angle = entry.angle;
            ui.selected_history = Some(id);
            ui.last_error = None;
            ui.clear_is_ac = true;
            ui.route = Route::Calculator;
            return ApplyOut {
                changed: true,
                copied: None,
                open_history: false,
                close_history: true,
            };
        }
        _ => {}
    }

    match cmd {
        Command::AllClear => {
            all_clear(doc, ui);
            ApplyOut::changed()
        }
        Command::Clear => {
            if ui.clear_is_ac || doc.session.phase != Phase::Editing {
                all_clear(doc, ui);
            } else {
                clear_entry(doc, ui);
            }
            ApplyOut::changed()
        }
        Command::AngleToggle => {
            doc.angle = match doc.angle {
                AngleMode::Degrees => AngleMode::Radians,
                AngleMode::Radians => AngleMode::Degrees,
            };
            doc.session.repeat = None;
            doc.session.reuse_repeat = false;
            if doc.session.phase == Phase::Result {
                doc.session.phase = Phase::Editing;
            }
            refresh_preview(doc, ui);
            ApplyOut::changed()
        }
        Command::Equals => equals(doc, ui),
        Command::Backspace => {
            backspace(doc, ui);
            ApplyOut::changed()
        }
        Command::Digit(d) => {
            let before = doc.session.source.clone();
            input_digit(doc, ui, d);
            if doc.session.source == before {
                ApplyOut::silent()
            } else {
                ApplyOut::changed()
            }
        }
        Command::Decimal => {
            input_decimal(doc, ui);
            ApplyOut::changed()
        }
        Command::Ee => {
            input_ee(doc, ui);
            ApplyOut::changed()
        }
        Command::Sign => {
            toggle_sign(doc, ui);
            ApplyOut::changed()
        }
        Command::Op(op) => {
            input_op(doc, ui, op);
            ApplyOut::changed()
        }
        Command::Percent => {
            apply_postfix(doc, ui, "%");
            ApplyOut::changed()
        }
        Command::Factorial => {
            apply_postfix(doc, ui, "!");
            ApplyOut::changed()
        }
        Command::Square => {
            apply_postfix(doc, ui, "^2");
            ApplyOut::changed()
        }
        Command::Cube => {
            apply_postfix(doc, ui, "^3");
            ApplyOut::changed()
        }
        Command::Power => {
            input_op(doc, ui, BinaryOp::Power);
            ApplyOut::changed()
        }
        Command::LParen => {
            input_lparen(doc, ui);
            ApplyOut::changed()
        }
        Command::RParen => {
            input_rparen(doc, ui);
            ApplyOut::changed()
        }
        Command::Func(func) => {
            input_func(doc, ui, func);
            ApplyOut::changed()
        }
        Command::MemoryClear => {
            doc.memory = None;
            ApplyOut::changed()
        }
        Command::MemoryAdd => memory_add(doc, ui, 1.0),
        Command::MemorySub => memory_add(doc, ui, -1.0),
        Command::MemoryRecall => {
            let stored = doc.memory.unwrap_or(0.0);
            insert_literal(doc, ui, stored);
            ApplyOut::changed()
        }
        Command::Constant(c) => {
            insert_ident(doc, ui, c.ident());
            ApplyOut::changed()
        }
        _ => ApplyOut::silent(),
    }
}

fn insert_ident(doc: &mut CalculatorDoc, ui: &mut UiState, ident: &str) {
    if doc.session.phase == Phase::Result || doc.session.phase == Phase::Error {
        after_result_new_entry(doc, ui, false);
    }
    if doc.session.source == "0" || doc.session.source.is_empty() {
        doc.session.source = ident.to_string();
    } else {
        doc.session.source.push_str(ident);
    }
    ui.clear_is_ac = false;
    refresh_preview(doc, ui);
}

fn all_clear(doc: &mut CalculatorDoc, ui: &mut UiState) {
    doc.session = crate::seed::initial().session;
    ui.last_error = None;
    ui.clear_is_ac = true;
}

fn clear_entry(doc: &mut CalculatorDoc, ui: &mut UiState) {
    if let Some((start, end)) = trailing_operand_span(&doc.session.source) {
        let mut next = String::new();
        next.push_str(&doc.session.source[..start]);
        next.push('0');
        next.push_str(&doc.session.source[end..]);
        doc.session.source = next;
    } else if doc.session.source.is_empty() {
        doc.session.source = "0".into();
    } else {
        doc.session.source.push('0');
    }
    doc.session.phase = Phase::Editing;
    doc.session.repeat = None;
    doc.session.reuse_repeat = false;
    ui.clear_is_ac = true;
    refresh_preview(doc, ui);
}

fn begin_fresh_entry(doc: &mut CalculatorDoc, ui: &mut UiState) {
    doc.session.source.clear();
    doc.session.phase = Phase::Editing;
    doc.session.repeat = None;
    doc.session.reuse_repeat = false;
    ui.last_error = None;
    ui.clear_is_ac = false;
}

fn after_result_new_entry(doc: &mut CalculatorDoc, ui: &mut UiState, keep_repeat: bool) {
    doc.session.source.clear();
    doc.session.phase = Phase::Editing;
    if !keep_repeat {
        doc.session.repeat = None;
        doc.session.reuse_repeat = false;
    } else {
        doc.session.reuse_repeat = doc.session.repeat.is_some();
    }
    ui.last_error = None;
    ui.clear_is_ac = false;
}

fn input_digit(doc: &mut CalculatorDoc, ui: &mut UiState, d: u8) {
    match doc.session.phase {
        Phase::Result => after_result_new_entry(doc, ui, true),
        Phase::Error => begin_fresh_entry(doc, ui),
        Phase::Editing => {}
    }
    if d > 9 { return; }
    if last_numeric_span(&doc.session.source).is_some() {
        if !append_to_number(&mut doc.session.source, |lit| push_digit(lit, d)) {
            ui.input_status = Some("Entry limit: 16 significant digits, 3 exponent digits".into());
            return;
        }
    } else {
        if expects_operand(&doc.session.source) {
            if doc.session.source == "0" {
                doc.session.source = d.to_string();
            } else {
                doc.session.source.push(char::from(b'0' + d));
            }
        }
    }
    ui.clear_is_ac = false;
    refresh_preview(doc, ui);
}

fn push_digit(lit: &mut String, d: u8) -> bool {
    let ch = char::from(b'0' + d);
    if let Some(e) = lit.find(['e', 'E']) {
        let exp = &lit[e + 1..];
        let digits = exp.trim_start_matches(['+', '-']);
        if digits.len() >= 3 {
            return false;
        }
        if digits.is_empty() && (exp == "+" || exp == "-" || exp.is_empty()) {
            lit.push(ch);
            return true;
        }
        lit.push(ch);
        true
    } else {
        let body = lit.trim_start_matches('-');
        let sig: String = body.chars().filter(|c| c.is_ascii_digit()).collect();
        if sig.trim_start_matches('0').len() >= 16 {
            return false;
        }
        if body == "0" {
            lit.replace_range(lit.len() - 1.., &ch.to_string());
            return true;
        }
        lit.push(ch);
        true
    }
}

fn input_decimal(doc: &mut CalculatorDoc, ui: &mut UiState) {
    match doc.session.phase {
        Phase::Result => after_result_new_entry(doc, ui, true),
        Phase::Error => begin_fresh_entry(doc, ui),
        Phase::Editing => {}
    }
    if !append_to_number(&mut doc.session.source, |lit| {
        if lit.contains('.') || lit.contains(['e', 'E']) {
            false
        } else {
            if lit.is_empty() || lit == "-" {
                lit.push('0');
            }
            lit.push('.');
            true
        }
    }) {
        if expects_operand(&doc.session.source) {
            if doc.session.source.is_empty() || doc.session.source == "0" {
                doc.session.source = "0.".into();
            } else {
                doc.session.source.push_str("0.");
            }
        }
    }
    ui.clear_is_ac = false;
    refresh_preview(doc, ui);
}

fn input_ee(doc: &mut CalculatorDoc, ui: &mut UiState) {
    if doc.session.phase == Phase::Result {
        doc.session.source = roundtrip_literal(doc.session.value);
        doc.session.phase = Phase::Editing;
        doc.session.repeat = None;
        doc.session.reuse_repeat = false;
    }
    if doc.session.phase == Phase::Error {
        return;
    }
    let _ = append_to_number(&mut doc.session.source, |lit| {
        if lit.contains(['e', 'E']) {
            return false;
        }
        let body = lit.trim_start_matches('-');
        if body.is_empty() || body == "." {
            return false;
        }
        lit.push('e');
        true
    });
    ui.clear_is_ac = false;
    refresh_preview(doc, ui);
}

fn toggle_sign(doc: &mut CalculatorDoc, ui: &mut UiState) {
    if doc.session.phase == Phase::Error {
        return;
    }
    if doc.session.phase == Phase::Result {
        doc.session.value = -doc.session.value;
        if doc.session.value == 0.0 {
            doc.session.value = 0.0;
        }
        doc.session.source = roundtrip_literal(doc.session.value);
        ui.clear_is_ac = false;
        return;
    }
    if let Some((start, end)) = last_numeric_span(&doc.session.source) {
        let mut next = doc.session.source[start..end].to_string();
        if let Some(e) = next.find(['e', 'E']) {
            let mut exp = next[e + 1..].to_string();
            if let Some(rest) = exp.strip_prefix('-') {
                exp = rest.to_string();
            } else if let Some(rest) = exp.strip_prefix('+') {
                exp = format!("-{rest}");
            } else {
                exp = format!("-{exp}");
            }
            next.replace_range(e + 1.., &exp);
        } else if let Some(rest) = next.strip_prefix('-') {
            next = rest.to_string();
        } else {
            next.insert(0, '-');
        }
        let mut source = String::new();
        source.push_str(&doc.session.source[..start]);
        source.push_str(&next);
        source.push_str(&doc.session.source[end..]);
        doc.session.source = source;
    } else if let Some((start, end)) = trailing_operand_span(&doc.session.source) {
        let piece = &doc.session.source[start..end];
        let wrapped = if let Some(rest) = piece.strip_prefix('-') {
            rest.to_string()
        } else {
            format!("-({piece})")
        };
        let mut source = String::new();
        source.push_str(&doc.session.source[..start]);
        source.push_str(&wrapped);
        source.push_str(&doc.session.source[end..]);
        doc.session.source = source;
    } else if doc.session.source.is_empty() || doc.session.source == "0" {
        doc.session.source = "-0".into();
    }
    ui.clear_is_ac = false;
    refresh_preview(doc, ui);
}

fn input_op(doc: &mut CalculatorDoc, ui: &mut UiState, op: BinaryOp) {
    if doc.session.phase == Phase::Error {
        return;
    }
    if doc.session.phase == Phase::Result {
        doc.session.source = roundtrip_literal(doc.session.value);
        doc.session.phase = Phase::Editing;
    }
    doc.session.repeat = None;
    doc.session.reuse_repeat = false;
    replace_or_push_op(&mut doc.session.source, op);
    ui.clear_is_ac = false;
    refresh_preview(doc, ui);
}

fn replace_or_push_op(source: &mut String, op: BinaryOp) {
    let glyph = op.glyph();
    if let Ok(tokens) = tokenize(source) {
        if let Some(last) = tokens.last() {
            match last.kind {
                TokenKind::Plus | TokenKind::Minus | TokenKind::Star | TokenKind::Slash | TokenKind::Caret => {
                    if op == BinaryOp::Subtract && !matches!(last.kind, TokenKind::Minus) {
                        source.push('-');
                        return;
                    }
                    source.replace_range(last.start..last.end, std::str::from_utf8(&[glyph as u8]).unwrap_or("+"));
                    return;
                }
                _ => {}
            }
        }
    }
    if source.is_empty() {
        source.push('0');
    }
    source.push(glyph);
}

fn apply_postfix(doc: &mut CalculatorDoc, ui: &mut UiState, suffix: &str) {
    if doc.session.phase == Phase::Error {
        return;
    }
    if doc.session.phase == Phase::Result {
        doc.session.source = roundtrip_literal(doc.session.value);
        doc.session.phase = Phase::Editing;
    }
    doc.session.repeat = None;
    doc.session.reuse_repeat = false;
    if let Some((start, end)) = trailing_operand_span(&doc.session.source) {
        let operand = &doc.session.source[start..end];
        let replacement = format!("({operand}){suffix}");
        doc.session.source.replace_range(start..end, &replacement);
    } else if expects_operand(&doc.session.source) && (doc.session.source.is_empty() || doc.session.source == "0") {
        doc.session.source = format!("0{suffix}");
    }
    ui.clear_is_ac = false;
    refresh_preview(doc, ui);
}

fn input_lparen(doc: &mut CalculatorDoc, ui: &mut UiState) {
    if doc.session.phase == Phase::Result {
        after_result_new_entry(doc, ui, false);
    }
    if doc.session.phase == Phase::Error {
        begin_fresh_entry(doc, ui);
    }
    if doc.session.source == "0" {
        doc.session.source = "(".into();
    } else {
        doc.session.source.push('(');
    }
    ui.clear_is_ac = false;
    refresh_preview(doc, ui);
}

fn input_rparen(doc: &mut CalculatorDoc, ui: &mut UiState) {
    if doc.session.phase != Phase::Editing {
        return;
    }
    let depth = paren_depth(&doc.session.source);
    if depth > 0 && trailing_operand_span(&doc.session.source).is_some() {
        doc.session.source.push(')');
        ui.clear_is_ac = false;
        refresh_preview(doc, ui);
    }
}

fn input_func(doc: &mut CalculatorDoc, ui: &mut UiState, func: Function) {
    if doc.session.phase == Phase::Error {
        begin_fresh_entry(doc, ui);
    }
    if doc.session.phase == Phase::Result {
        doc.session.source = roundtrip_literal(doc.session.value);
        doc.session.phase = Phase::Editing;
    }
    doc.session.repeat = None;
    doc.session.reuse_repeat = false;
    let name = func.name();
    if let Some((start, end)) = trailing_operand_span(&doc.session.source) {
        let piece = doc.session.source[start..end].to_string();
        let wrapped = format!("{name}({piece})");
        let mut source = String::new();
        source.push_str(&doc.session.source[..start]);
        source.push_str(&wrapped);
        source.push_str(&doc.session.source[end..]);
        doc.session.source = source;
    } else {
        if doc.session.source == "0" {
            doc.session.source.clear();
        }
        doc.session.source.push_str(name);
        doc.session.source.push('(');
    }
    ui.clear_is_ac = false;
    refresh_preview(doc, ui);
}

fn insert_literal(doc: &mut CalculatorDoc, ui: &mut UiState, value: f64) {
    if doc.session.phase == Phase::Result || doc.session.phase == Phase::Error {
        after_result_new_entry(doc, ui, false);
    }
    let lit = roundtrip_literal(value);
    if let Some((start, end)) = trailing_operand_span(&doc.session.source) {
        if last_token_is_number(&doc.session.source) {
            let mut source = String::new();
            source.push_str(&doc.session.source[..start]);
            source.push_str(&lit);
            source.push_str(&doc.session.source[end..]);
            doc.session.source = source;
        } else {
            doc.session.source.push_str(&lit);
        }
    } else if doc.session.source == "0" || doc.session.source.is_empty() {
        doc.session.source = lit;
    } else {
        doc.session.source.push_str(&lit);
    }
    ui.clear_is_ac = false;
    refresh_preview(doc, ui);
}

fn memory_add(doc: &mut CalculatorDoc, _ui: &mut UiState, sign: f64) -> ApplyOut {
    let Some(v) = current_valid_value(doc) else {
        return ApplyOut::silent();
    };
    let base = doc.memory.unwrap_or(0.0);
    let next = base + sign * v;
    if !next.is_finite() {
        return ApplyOut::silent();
    }
    doc.memory = Some(next);
    ApplyOut::changed()
}

fn current_valid_value(doc: &CalculatorDoc) -> Option<f64> {
    match doc.session.phase {
        Phase::Error => None,
        Phase::Result => Some(doc.session.value).filter(|v| v.is_finite()),
        Phase::Editing => match engine_preview(&doc.session.source, doc.angle) {
            Preview::Value(v) => Some(v),
            _ => Some(doc.session.value).filter(|v| v.is_finite()),
        },
    }
}

fn equals(doc: &mut CalculatorDoc, ui: &mut UiState) -> ApplyOut {
    if doc.session.phase == Phase::Error {
        return ApplyOut::silent();
    }
    if doc.session.reuse_repeat {
        if let Some(repeat) = doc.session.repeat {
            return match evaluate(&doc.session.source, doc.angle) {
                Ok(lhs) => apply_repeat(doc, ui, repeat, lhs),
                Err(error) => calculation_error(doc, ui, error),
            };
        }
    }
    if doc.session.phase == Phase::Result {
        if let Some(repeat) = doc.session.repeat {
            return apply_repeat(doc, ui, repeat, doc.session.value);
        }
        return ApplyOut::silent();
    }
    if doc.session.phase == Phase::Error {
        return ApplyOut::silent();
    }
    let mut source = doc.session.source.clone();
    if let Some(closed) = close_unmatched_parens(&source) {
        source = closed;
        doc.session.source = source.clone();
    }
    match evaluate(&source, doc.angle) {
        Ok(value) => {
            let repeat = root_repeat(&source, doc.angle).map(|(op, rhs, relative_percent)| Repeat {
                op,
                rhs,
                relative_percent,
            });
            commit_result(doc, ui, source, value, repeat)
        }
        Err(e) => {
            doc.session.phase = Phase::Error;
            ui.last_error = Some(e);
            ui.clear_is_ac = true;
            ApplyOut::changed()
        }
    }
}

fn calculation_error(doc: &mut CalculatorDoc, ui: &mut UiState, error: EvalError) -> ApplyOut {
    doc.session.phase = Phase::Error;
    ui.last_error = Some(error);
    ui.clear_is_ac = true;
    ApplyOut::changed()
}

fn commit_result(doc: &mut CalculatorDoc, ui: &mut UiState, source: String, value: f64, repeat: Option<Repeat>) -> ApplyOut {
    // Reserve i64::MAX as the exhausted next-id sentinel so every serialized
    // document has a valid next_id strictly greater than all allocated IDs.
    if doc.next_id >= i64::MAX as u64 || doc.history.iter().any(|e| e.id >= doc.next_id) {
        ui.input_status = Some("History ID limit reached".into());
        return ApplyOut::silent();
    }
    let value = normalize_zero(value);
    let id = doc.next_id;
    doc.next_id += 1;
    doc.history.push_front(HistoryEntry { id, source: source.clone(), value, angle: doc.angle });
    doc.history.truncate(MAX_HISTORY);
    doc.session.source = source;
    doc.session.value = value;
    doc.session.phase = Phase::Result;
    doc.session.repeat = repeat;
    doc.session.reuse_repeat = false;
    ui.last_error = None;
    ui.clear_is_ac = true;
    ApplyOut::changed()
}

fn apply_repeat(doc: &mut CalculatorDoc, ui: &mut UiState, repeat: Repeat, lhs: f64) -> ApplyOut {
    let rhs = roundtrip_literal(if repeat.relative_percent { repeat.rhs * 100.0 } else { repeat.rhs });
    let source = format!("({}){}({}){}", roundtrip_literal(lhs), repeat.op.glyph(), rhs,
        if repeat.relative_percent { "%" } else { "" });
    match evaluate(&source, doc.angle) {
        Ok(value) => commit_result(doc, ui, source, value, Some(repeat)),
        Err(error) => calculation_error(doc, ui, error),
    }
}

fn backspace(doc: &mut CalculatorDoc, ui: &mut UiState) {
    match doc.session.phase {
        Phase::Result => {
            doc.session.phase = Phase::Editing;
            doc.session.repeat = None;
            doc.session.reuse_repeat = false;
        }
        Phase::Error => {
            doc.session.phase = Phase::Editing;
            ui.last_error = None;
        }
        Phase::Editing => {}
    }
    if let Some((start, _)) = trailing_function_opener(&doc.session.source) {
        doc.session.source.truncate(start);
    } else if doc.session.source.chars().count() <= 1 {
        doc.session.source = "0".into();
        ui.clear_is_ac = true;
    } else {
        doc.session.source.pop();
        if doc.session.source.is_empty() {
            doc.session.source = "0".into();
            ui.clear_is_ac = true;
        }
    }
    refresh_preview(doc, ui);
}

fn refresh_preview(doc: &mut CalculatorDoc, ui: &mut UiState) {
    match engine_preview(&doc.session.source, doc.angle) {
        Preview::Value(v) => {
            doc.session.value = v;
            if doc.session.phase == Phase::Error {
                doc.session.phase = Phase::Editing;
            }
            ui.last_error = None;
        }
        Preview::Incomplete => {
            if doc.session.phase == Phase::Error {
                doc.session.phase = Phase::Editing;
                ui.last_error = None;
            }
        }
        Preview::Error(e) => {
            if !matches!(e.code, ErrorCode::Incomplete) {
                doc.session.phase = Phase::Error;
                ui.last_error = Some(e);
            }
        }
    }
}

fn expects_operand(source: &str) -> bool {
    if source.is_empty() || source == "0" {
        return true;
    }
    match tokenize(source) {
        Ok(tokens) => match tokens.last().map(|t| &t.kind) {
            Some(
                TokenKind::Plus
                | TokenKind::Minus
                | TokenKind::Star
                | TokenKind::Slash
                | TokenKind::Caret
                | TokenKind::LParen,
            )
            | None => true,
            _ => false,
        },
        Err(_) => true,
    }
}

fn paren_depth(source: &str) -> i32 {
    let mut d = 0i32;
    for c in source.chars() {
        match c {
            '(' => d += 1,
            ')' => d -= 1,
            _ => {}
        }
    }
    d
}

fn last_numeric_span(source: &str) -> Option<(usize, usize)> {
    if last_token_is_number(source) {
        return trailing_operand_span(source);
    }
    let bytes = source.as_bytes();
    if bytes.is_empty() {
        return None;
    }
    let mut i = bytes.len();
    while i > 0 && bytes[i - 1].is_ascii_digit() {
        i -= 1;
    }
    if i > 0 && (bytes[i - 1] == b'+' || bytes[i - 1] == b'-') && i > 1 && (bytes[i - 2] == b'e' || bytes[i - 2] == b'E')
    {
        i -= 1;
    }
    if i > 0 && (bytes[i - 1] == b'e' || bytes[i - 1] == b'E') {
        i -= 1;
        if i > 0 && bytes[i - 1] == b'.' {
            i -= 1;
        }
        while i > 0 && bytes[i - 1].is_ascii_digit() {
            i -= 1;
        }
        if i > 0 && bytes[i - 1] == b'.' {
            i -= 1;
            while i > 0 && bytes[i - 1].is_ascii_digit() {
                i -= 1;
            }
        }
        if i > 0 && bytes[i - 1] == b'-' {
            let before = if i > 1 { bytes[i - 2] } else { b' ' };
            if !before.is_ascii_digit() && before != b'.' && before != b')' {
                i -= 1;
            }
        }
        return Some((i, source.len()));
    }
    None
}

fn append_to_number(source: &mut String, f: impl FnOnce(&mut String) -> bool) -> bool {
    let Some((start, end)) = last_numeric_span(source) else {
        return false;
    };
    let mut lit = source[start..end].to_string();
    if !f(&mut lit) {
        return false;
    }
    let mut next = String::new();
    next.push_str(&source[..start]);
    next.push_str(&lit);
    next.push_str(&source[end..]);
    *source = next;
    true
}

fn normalize_zero(v: f64) -> f64 {
    if v == 0.0 {
        0.0
    } else {
        v
    }
}

pub fn display_expression_line(doc: &CalculatorDoc) -> String {
    if doc.session.phase == Phase::Editing && (doc.session.source == "0" || doc.session.source.is_empty())
    {
        String::new()
    } else {
        display_expression(&doc.session.source)
    }
}

pub fn display_result(doc: &CalculatorDoc, _ui: &UiState) -> String {
    if doc.session.phase == Phase::Error {
        return "Error".into();
    }
    if doc.session.phase == Phase::Editing {
        if let Some((start, end)) = last_numeric_span(&doc.session.source) {
            // Preserve spelling only when the entire expression is a literal.
            // Completed sums/products show their evaluated preview.
            if start == 0 && end == doc.session.source.len() {
                if let Some(formatted) = format_literal_for_display(&doc.session.source) {
                    return formatted;
                }
            }
        }
    }
    format_number(doc.session.value)
}

/// Restore transient display state after reading a persisted session.
pub fn rebuild_derived(doc: &mut CalculatorDoc, ui: &mut UiState) {
    ui.last_error = None;
    ui.input_status = None;
    if doc.session.phase != Phase::Result {
        match crate::engine::evaluate(&doc.session.source, doc.angle) {
            Ok(value) => {
                doc.session.value = value;
                doc.session.phase = Phase::Editing;
            }
            Err(error) if error.code == ErrorCode::Incomplete && doc.session.phase == Phase::Editing => {}
            Err(error) => {
                doc.session.phase = Phase::Error;
                ui.last_error = Some(error);
            }
        }
    }
    ui.clear_is_ac = doc.session.phase != Phase::Editing || doc.session.source == "0";
}

fn format_literal_for_display(lit: &str) -> Option<String> {
    let (sign, body) = if let Some(body) = lit.strip_prefix('-').or_else(|| lit.strip_prefix('−')) {
        ("-", body)
    } else if let Some(body) = lit.strip_prefix('+') {
        ("+", body)
    } else {
        ("", lit)
    };
    let mantissa = body.split(['e', 'E']).next()?;
    let (int, frac) = match mantissa.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (mantissa, None),
    };
    // Token spans include unary sign chains and intervening whitespace.
    // Those are expressions, not literal spellings to group as digits.
    if !int.bytes().all(|b| b.is_ascii_digit())
        || frac.is_some_and(|f| !f.bytes().all(|b| b.is_ascii_digit()))
        || (int.is_empty() && frac.unwrap_or_default().is_empty())
    {
        return None;
    }
    if body.contains(['e', 'E']) {
        return Some(format!("{sign}{body}"));
    }
    let int = if int.is_empty() { "0" } else { int };
    let grouped = group_int(int);
    Some(match frac {
        Some(f) => format!("{sign}{grouped}.{f}"),
        None => format!("{sign}{grouped}"),
    })
}

fn group_int(int_digits: &str) -> String {
    let mut out = String::new();
    let len = int_digits.chars().count();
    for (i, ch) in int_digits.chars().enumerate() {
        if i > 0 && (len - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

pub fn status_line(doc: &CalculatorDoc, ui: &UiState) -> String {
    if let Some(msg) = &ui.storage_status { return msg.clone(); }
    if let Some(msg) = &ui.input_status { return msg.clone(); }
    if let Some(err) = &ui.last_error { return error_message(err.code).to_string(); }
    let mut parts = Vec::new();
    parts.push(if doc.angle == AngleMode::Degrees {
        "Deg"
    } else {
        "Rad"
    });
    if doc.memory.is_some() {
        parts.push("M");
    }
    parts.join("  ")
}

pub fn font_size_for_pt(pt: f64) -> f64 {
    pt * 0.75
}

/// Apply a relative percent or binary repeat onto `lhs`.
pub fn apply_repeat_value(lhs: f64, repeat: Repeat) -> Result<f64, ErrorCode> {
    let v = if repeat.relative_percent {
        match repeat.op {
            BinaryOp::Add => lhs + lhs * repeat.rhs,
            BinaryOp::Subtract => lhs - lhs * repeat.rhs,
            _ => lhs,
        }
    } else {
        match repeat.op {
            BinaryOp::Add => lhs + repeat.rhs,
            BinaryOp::Subtract => lhs - repeat.rhs,
            BinaryOp::Multiply => lhs * repeat.rhs,
            BinaryOp::Divide => {
                if repeat.rhs == 0.0 {
                    return Err(ErrorCode::DivisionByZero);
                }
                lhs / repeat.rhs
            }
            BinaryOp::Power => lhs.powf(repeat.rhs),
        }
    };
    if v.is_finite() {
        Ok(v)
    } else {
        Err(ErrorCode::Overflow)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::seed;

    fn doc() -> (CalculatorDoc, UiState) {
        (seed::initial(), UiState::default())
    }

    fn run(cmds: &[Command]) -> CalculatorDoc {
        let (mut d, mut ui) = doc();
        for c in cmds {
            apply(&mut d, &mut ui, *c);
        }
        d
    }

    fn eq_value(cmds: &[Command]) -> f64 {
        run(cmds).session.value
    }

    fn close(a: f64, b: f64) {
        assert!((a - b).abs() <= 1e-12 * (1.0 + b.abs()) || a == b, "{a} != {b}");
    }

    fn digits(n: &str) -> Vec<Command> {
        n.chars()
            .map(|c| match c {
                '.' => Command::Decimal,
                d => Command::Digit(d.to_digit(10).unwrap() as u8),
            })
            .collect()
    }

    #[test]
    fn editing_pending_op_decimal_sign_functions_parens_c_and_error() {
        let mut d = run(&[Command::Digit(2), Command::Op(BinaryOp::Add), Command::Op(BinaryOp::Multiply)]);
        assert!(d.session.source.ends_with('*'));
        d = run(&[Command::Digit(1), Command::Decimal, Command::Decimal, Command::Digit(5)]);
        assert_eq!(d.session.source, "1.5");
        d = run(&[Command::Digit(1), Command::Ee, Command::Sign, Command::Digit(2)]);
        assert_eq!(d.session.source, "1e-2");
        d = run(&[Command::Digit(9), Command::Func(Function::Sqrt)]);
        assert_eq!(d.session.source, "sqrt(9)");
        d = run(&[
            Command::LParen,
            Command::Digit(2),
            Command::Op(BinaryOp::Add),
            Command::Digit(3),
            Command::Equals,
        ]);
        close(d.session.value, 5.0);
        let (mut d, mut ui) = doc();
        apply(&mut d, &mut ui, Command::Digit(8));
        apply(&mut d, &mut ui, Command::Clear);
        assert_eq!(d.session.source, "0");
        apply(&mut d, &mut ui, Command::Digit(4));
        apply(&mut d, &mut ui, Command::Op(BinaryOp::Add));
        apply(&mut d, &mut ui, Command::Digit(1));
        apply(&mut d, &mut ui, Command::AllClear);
        assert_eq!(d.session.source, "0");
        assert!(d.memory.is_none());
        let (mut d, mut ui) = doc();
        apply(&mut d, &mut ui, Command::Digit(1));
        apply(&mut d, &mut ui, Command::Op(BinaryOp::Divide));
        apply(&mut d, &mut ui, Command::Digit(0));
        apply(&mut d, &mut ui, Command::Equals);
        assert_eq!(d.session.phase, Phase::Error);
        apply(&mut d, &mut ui, Command::Digit(2));
        assert_eq!(d.session.source, "2");
        assert_eq!(d.session.phase, Phase::Editing);
        let (mut d, mut ui) = doc();
        apply(&mut d, &mut ui, Command::Digit(1));
        apply(&mut d, &mut ui, Command::Op(BinaryOp::Divide));
        apply(&mut d, &mut ui, Command::Digit(0));
        apply(&mut d, &mut ui, Command::Equals);
        apply(&mut d, &mut ui, Command::Backspace);
        assert_eq!(d.session.phase, Phase::Editing);
        assert_eq!(d.session.source, "1/");
    }

    #[test]
    fn repeat_equals_percent_and_new_number() {
        let mut cmds = digits("2");
        cmds.extend([
            Command::Op(BinaryOp::Add),
            Command::Digit(3),
            Command::Op(BinaryOp::Multiply),
            Command::Digit(4),
            Command::Equals,
            Command::Equals,
        ]);
        close(eq_value(&cmds), 26.0);

        let mut cmds = digits("100");
        cmds.extend([
            Command::Op(BinaryOp::Add),
            Command::Digit(1),
            Command::Digit(5),
            Command::Percent,
            Command::Equals,
            Command::Equals,
        ]);
        close(eq_value(&cmds), 132.25);

        let mut cmds = digits("100");
        cmds.extend([
            Command::Op(BinaryOp::Add),
            Command::Digit(1),
            Command::Digit(5),
            Command::Percent,
            Command::Equals,
            Command::Digit(1),
            Command::Digit(5),
            Command::Digit(0),
            Command::Equals,
        ]);
        close(eq_value(&cmds), 172.5);

        let (mut d, mut ui) = doc();
        apply(&mut d, &mut ui, Command::Digit(2));
        apply(&mut d, &mut ui, Command::Func(Function::Sqrt));
        apply(&mut d, &mut ui, Command::Equals);
        let v = d.session.value;
        apply(&mut d, &mut ui, Command::Equals);
        close(d.session.value, v);
    }

    #[test]
    fn memory_empty_add_zero_clear_ac_and_overflow() {
        let (mut d, mut ui) = doc();
        apply(&mut d, &mut ui, Command::MemoryRecall);
        close(d.session.value, 0.0);
        apply(&mut d, &mut ui, Command::Digit(5));
        apply(&mut d, &mut ui, Command::MemoryAdd);
        assert_eq!(d.memory, Some(5.0));
        apply(&mut d, &mut ui, Command::MemorySub);
        assert_eq!(d.memory, Some(0.0));
        apply(&mut d, &mut ui, Command::MemoryClear);
        assert!(d.memory.is_none());
        apply(&mut d, &mut ui, Command::AllClear);
        apply(&mut d, &mut ui, Command::Digit(3));
        apply(&mut d, &mut ui, Command::MemoryAdd);
        apply(&mut d, &mut ui, Command::AllClear);
        assert_eq!(d.memory, Some(3.0));
        d.memory = Some(f64::MAX);
        d.session.value = f64::MAX;
        d.session.phase = Phase::Result;
        let before = d.memory;
        apply(&mut d, &mut ui, Command::MemoryAdd);
        assert_eq!(d.memory, before);
    }

    #[test]
    fn history_appends_only_on_equals_recalls_and_evicts() {
        let (mut d, mut ui) = doc();
        assert!(d.history.is_empty());
        apply(&mut d, &mut ui, Command::Digit(2));
        apply(&mut d, &mut ui, Command::Op(BinaryOp::Add));
        apply(&mut d, &mut ui, Command::Digit(2));
        assert!(d.history.is_empty());
        apply(&mut d, &mut ui, Command::Equals);
        assert_eq!(d.history.len(), 1);
        close(d.history[0].value, 4.0);
        let id = d.history[0].id;
        apply(&mut d, &mut ui, Command::AllClear);
        apply(&mut d, &mut ui, Command::RecallHistory(id));
        close(d.session.value, 4.0);
        assert_eq!(d.angle, AngleMode::Degrees);
        apply(&mut d, &mut ui, Command::RecallHistory(999));
        apply(&mut d, &mut ui, Command::ClearHistory);
        assert!(d.history.is_empty());
        for _ in 0..201 {
            apply(&mut d, &mut ui, Command::Digit(1));
            apply(&mut d, &mut ui, Command::Op(BinaryOp::Add));
            apply(&mut d, &mut ui, Command::Digit(1));
            apply(&mut d, &mut ui, Command::Equals);
        }
        assert_eq!(d.history.len(), 200);
    }

    #[test]
    fn json_round_trip_and_corrupt_documents() {
        let (mut d, mut ui) = doc();
        apply(&mut d, &mut ui, Command::Digit(2));
        apply(&mut d, &mut ui, Command::Op(BinaryOp::Add));
        apply(&mut d, &mut ui, Command::Digit(3));
        apply(&mut d, &mut ui, Command::Equals);
        apply(&mut d, &mut ui, Command::MemoryAdd);
        let bytes = d.to_bytes();
        let back = CalculatorDoc::from_bytes(&bytes).unwrap();
        assert_eq!(back.session.source, d.session.source);
        close(back.session.value, d.session.value);
        assert_eq!(back.memory, Some(5.0));
        assert!(CalculatorDoc::from_bytes(b"{").is_err());
        assert!(CalculatorDoc::from_bytes(br#"{"version":2,"angle":"deg","memory":null,"session":{"source":"0","phase":"editing","value":0,"repeat":null,"reuse_repeat":false},"next_id":1,"history":[]}"#).is_err());
        assert!(CalculatorDoc::from_bytes(br#"{"version":1,"angle":"deg","memory":null,"session":{"source":"0","phase":"editing","value":"x","repeat":null,"reuse_repeat":false},"next_id":1,"history":[]}"#).is_err());
        let inf = br#"{"version":1,"angle":"deg","memory":1e1000,"session":{"source":"0","phase":"editing","value":0,"repeat":null,"reuse_repeat":false},"next_id":1,"history":[]}"#;
        assert!(CalculatorDoc::from_bytes(inf).is_err());
    }

    #[test]
    fn layout_thresholds_and_key_targets() {
        let c402 = LayoutMetrics::for_size(402.0, 780.0);
        assert_eq!(c402.class.width, WidthClass::Compact);
        assert!(!c402.show_scientific && !c402.show_tape);
        assert!(c402.keys_meet_target());
        let c699 = LayoutMetrics::for_size(699.0, 800.0);
        assert_eq!(c699.class.width, WidthClass::Compact);
        let w700 = LayoutMetrics::for_size(700.0, 800.0);
        assert_eq!(w700.class.width, WidthClass::Wide);
        assert!(w700.show_tape && w700.show_scientific);
        assert!(w700.key.width > 49.0 && w700.key.width < 51.0);
        assert!(w700.keys_meet_target());
        let w1240 = LayoutMetrics::for_size(1240.0, 800.0);
        assert_eq!(w1240.class.width, WidthClass::Wide);
        assert!((w1240.key.width - 92.0).abs() < 0.01);
        let short = LayoutMetrics::for_size(874.0, 300.0);
        assert!(short.class.short && short.class.scientific());
        assert!(!short.show_tape && short.show_short_bar && short.show_scientific);
        assert!((short.key.height - 44.0).abs() < 0.01);
        assert!(short.keys_meet_target());
        let keypad = 5.0 * short.key.height + 4.0 * short.key.v_gap;
        let remaining = 300.0 - 4.0 - 8.0 - 44.0 - 8.0;
        assert!((keypad - 236.0).abs() < 1e-9);
        assert!(keypad <= remaining + 1e-9);
        let w1100 = LayoutMetrics::for_size(1100.0, 800.0);
        let workspace = 1100.0 - w1100.inset * 2.0 - w1100.tape_width - w1100.col_sep;
        let fitted = 9.0 * w1100.key.width + 7.0 * w1100.key.h_gap + w1100.key.sci_basic_gap;
        assert!(fitted <= workspace + 1e-6, "keypad {fitted} overflows workspace {workspace}");
        assert!(w1100.keys_meet_target());
        let (mut d, mut ui) = doc();
        apply(&mut d, &mut ui, Command::Digit(7));
        let before = d.session.clone();
        let _ = LayoutMetrics::for_size(402.0, 780.0);
        let _ = LayoutMetrics::for_size(1240.0, 800.0);
        assert_eq!(d.session, before);
    }

    #[test]
    fn eval_leaves_the_document_untouched() {
        let (mut d, mut ui) = doc();
        apply(&mut d, &mut ui, Command::Digit(9));
        let snapshot = d.clone();
        let _ = crate::engine::evaluate("200+10%", d.angle);
        assert_eq!(d, snapshot);
    }
    #[test]
    fn scientific_buttons_apply_to_complete_signed_operands() {
        let negative_root = run(&[Command::Digit(9), Command::Sign, Command::Func(Function::Sqrt), Command::Equals]);
        assert_eq!(negative_root.session.source, "sqrt(-9)");
        assert_eq!(negative_root.session.phase, Phase::Error);
        for (command, value) in [(Command::Square, 4.0), (Command::Cube, -8.0), (Command::Percent, -0.02), (Command::Func(Function::Reciprocal), -0.5)] {
            let d = run(&[Command::Digit(2), Command::Sign, command, Command::Equals]);
            assert_eq!(d.session.value, value, "{command:?}");
        }
        assert_eq!(run(&[Command::Digit(2), Command::Sign, Command::Factorial]).session.phase, Phase::Error);
        assert_eq!(run(&[Command::Digit(8), Command::Sign, Command::Func(Function::Cbrt), Command::Equals]).session.value, -2.0);
        let d = run(&[Command::Digit(7), Command::Op(BinaryOp::Add), Command::Digit(2), Command::Sign, Command::Square, Command::Equals]);
        assert_eq!(d.session.value, 11.0);
    }

    #[test]
    fn signed_literal_spelling_and_complete_expression_preview() {
        let (mut d, mut ui) = doc();
        for command in [Command::Digit(9), Command::Sign] { apply(&mut d, &mut ui, command); }
        assert_eq!(display_result(&d, &ui), "-9");
        apply(&mut d, &mut ui, Command::Sign);
        assert_eq!(display_result(&d, &ui), "9");
        apply(&mut d, &mut ui, Command::AllClear);
        for command in digits("0.00") { apply(&mut d, &mut ui, command); }
        apply(&mut d, &mut ui, Command::Sign);
        assert_eq!(display_result(&d, &ui), "-0.00");
        apply(&mut d, &mut ui, Command::AllClear);
        for command in [Command::Digit(2), Command::Op(BinaryOp::Add), Command::Digit(3)] { apply(&mut d, &mut ui, command); }
        assert_eq!(display_result(&d, &ui), "5");
        assert!(d.history.is_empty());
    }

    #[test]
    fn unicode_signed_literals_and_unary_expressions_display_without_byte_corruption() {
        let (mut d, mut ui) = doc();
        for (source, expected) in [("−9", "-9"), ("−1234.00", "-1,234.00"), ("−.50", "-0.50"), ("−0.00", "-0.00"), ("−1.20e-3", "-1.20e-3"), ("+1234", "+1,234"), ("−−9", "9"), ("-−9", "9"), ("− 9", "-9"), ("−9+3", "-6")] {
            d.session.source = source.into();
            d.session.phase = Phase::Editing;
            rebuild_derived(&mut d, &mut ui);
            assert_eq!(display_result(&d, &ui), expected, "{source}");
            assert_eq!(apply(&mut d, &mut ui, Command::Copy).copied.as_deref(), Some(expected));
            let mut restored = CalculatorDoc::from_bytes(&d.to_bytes()).unwrap();
            rebuild_derived(&mut restored, &mut ui);
            assert_eq!(display_result(&restored, &ui), expected, "restored {source}");
        }
    }

    #[test]
    fn repeat_commits_history_and_rejects_incomplete_new_number() {
        let (mut d, mut ui) = doc();
        for command in [Command::Digit(2), Command::Op(BinaryOp::Add), Command::Digit(3), Command::Equals, Command::Equals] { apply(&mut d, &mut ui, command); }
        assert_eq!(d.session.value, 8.0);
        assert_eq!(d.history.len(), 2);
        assert_eq!(evaluate(&d.history[0].source, d.angle), Ok(8.0));
        let count = d.history.len();
        for command in [Command::Digit(4), Command::Ee, Command::Equals] { apply(&mut d, &mut ui, command); }
        assert_eq!(d.session.source, "4e");
        assert_eq!(d.session.phase, Phase::Error);
        assert_eq!(ui.last_error.as_ref().unwrap().code, ErrorCode::Incomplete);
        assert_eq!(d.history.len(), count);
    }

    #[test]
    fn operand_start_minus_preserves_power_and_binary_replacement() {
        let d = run(&[Command::Digit(2), Command::Op(BinaryOp::Power), Command::Op(BinaryOp::Subtract), Command::Digit(3), Command::Equals]);
        assert_eq!(d.session.source, "2^-3");
        assert_eq!(d.session.value, 0.125);
        let d = run(&[Command::Digit(2), Command::Op(BinaryOp::Add), Command::Op(BinaryOp::Multiply), Command::Digit(3), Command::Equals]);
        assert_eq!(d.session.value, 6.0);
    }

    #[test]
    fn stale_history_ids_and_signed_id_exhaustion_are_guarded() {
        let (mut d, mut ui) = doc();
        apply(&mut d, &mut ui, Command::Equals);
        d.next_id = 1;
        assert!(CalculatorDoc::from_bytes(&d.to_bytes()).is_err());
        d.next_id = i64::MAX as u64 - 1;
        apply(&mut d, &mut ui, Command::Digit(2));
        apply(&mut d, &mut ui, Command::Equals);
        assert_eq!(d.history[0].id, i64::MAX as u64 - 1);
        assert_eq!(CalculatorDoc::from_bytes(&d.to_bytes()).unwrap(), d);
        apply(&mut d, &mut ui, Command::Digit(3));
        let before = d.clone();
        apply(&mut d, &mut ui, Command::Equals);
        assert_eq!(d, before);
        assert!(status_line(&d, &ui).contains("ID limit"));
    }

    #[test]
    fn leading_zeros_do_not_spend_significant_digits_and_limits_are_visible() {
        let (mut d, mut ui) = doc();
        for command in digits("0.0000000000000012") { apply(&mut d, &mut ui, command); }
        assert_eq!(d.session.source, "0.0000000000000012");
        apply(&mut d, &mut ui, Command::AllClear);
        for command in digits("1234567890123456") { apply(&mut d, &mut ui, command); }
        apply(&mut d, &mut ui, Command::Digit(7));
        assert_eq!(d.session.source, "1234567890123456");
        assert!(status_line(&d, &ui).contains("Entry limit"));
        apply(&mut d, &mut ui, Command::AllClear);
        for command in [Command::Digit(1), Command::Ee, Command::Sign, Command::Digit(1), Command::Digit(0), Command::Digit(0), Command::Digit(1)] { apply(&mut d, &mut ui, command); }
        assert_eq!(d.session.source, "1e-100");
        assert!(status_line(&d, &ui).contains("Entry limit"));
    }

    #[test]
    fn clear_entry_all_clear_repeat_reset_and_recall_precision() {
        let (mut d, mut ui) = doc();
        d.angle = AngleMode::Radians;
        for command in [Command::Digit(1), Command::Op(BinaryOp::Divide), Command::Digit(3), Command::Equals, Command::MemoryAdd] { apply(&mut d, &mut ui, command); }
        let entry = d.history[0].clone();
        for command in [Command::Digit(2), Command::Op(BinaryOp::Add), Command::Digit(3), Command::Clear] { apply(&mut d, &mut ui, command); }
        assert_eq!(d.session.source, "2+0");
        assert!(ui.clear_is_ac && d.session.repeat.is_none());
        apply(&mut d, &mut ui, Command::Clear);
        assert_eq!(d.session.source, "0");
        assert_eq!(d.memory, Some(1.0 / 3.0));
        assert_eq!(d.history.len(), 1);
        assert_eq!(d.angle, AngleMode::Radians);
        apply(&mut d, &mut ui, Command::AngleToggle);
        apply(&mut d, &mut ui, Command::RecallHistory(entry.id));
        assert_eq!(d.angle, AngleMode::Radians);
        assert_eq!(d.session.value.to_bits(), entry.value.to_bits());
        assert!(d.session.repeat.is_none());
        apply(&mut d, &mut ui, Command::Op(BinaryOp::Multiply));
        apply(&mut d, &mut ui, Command::Digit(3));
        apply(&mut d, &mut ui, Command::Equals);
        assert_eq!(d.session.value, 1.0);
        apply(&mut d, &mut ui, Command::AllClear);
        assert!(d.session.repeat.is_none() && !d.session.reuse_repeat);
    }

}
