//! Persistent host policy for entries into one script VM.
use crate::{ScriptIp, ScriptObject, ScriptValue};
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Limit {
    Fuel,
    CallFrames,
    OperandStack,
    StringBytes,
    RetainedHeap,
    Allocation,
    NativeReentry,
    Time,
    Interrupted,
    EqualityWork,
}

impl fmt::Display for Limit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Fuel => "instruction fuel",
            Self::CallFrames => "call frames",
            Self::OperandStack => "operand stack",
            Self::StringBytes => "string bytes",
            Self::RetainedHeap => "retained heap bytes",
            Self::Allocation => "entry allocation bytes",
            Self::NativeReentry => "native re-entry",
            Self::Time => "execution time",
            Self::Interrupted => "execution interrupted",
            Self::EqualityWork => "equality work",
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub route: String,
    pub receiver_type: String,
    pub property: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Poison {
    Limit(Limit),
    Error(String),
    Refusal(Refusal),
}

impl fmt::Display for Poison {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Limit(limit) => write!(f, "Limit reached: {limit}"),
            Self::Error(message) => f.write_str(message),
            Self::Refusal(r) => write!(f, "{}: {}.{}: {}", r.route, r.receiver_type, r.property, r.message),
        }
    }
}

impl Poison {
    pub(crate) fn as_script_error(&self) -> ScriptValue {
        match self {
            Self::Limit(_) => ScriptValue::script_err_limit(ScriptIp::UNKNOWN),
            Self::Error(_) => ScriptValue::script_err_unexpected(ScriptIp::UNKNOWN),
            Self::Refusal(_) => ScriptValue::script_err_not_allowed(ScriptIp::UNKNOWN),
        }
    }
}

/// Opaque until the host registers apply rules.
#[derive(Debug, Default)]
pub struct AdmissionTable {
    _private: (),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EntryUsage {
    pub fuel: u64,
    pub bytes: usize,
}

/// The host configures this before handing it to the VM. Once installed it is read-only
/// to callers. A failed VM retains its first reason and the accounting for its last entry.
#[derive(Debug)]
pub struct ScriptVmPolicy {
    pub fuel: Option<u64>,
    pub call_frames: Option<usize>,
    pub stack_values: Option<usize>,
    pub string_bytes: Option<usize>,
    pub retained_heap_bytes: Option<usize>,
    pub allocation_bytes: Option<usize>,
    pub allow_debug_output: bool,
    pub bail_on_uncaught_error: bool,
    // Cached at installation, so the stock interpreter takes one branch per entry.
    pub(crate) active: bool,
    pub(crate) sealed: bool,
    pub(crate) poisoned: Option<Poison>,
    pub(crate) last_entry: EntryUsage,
    pub(crate) total: EntryUsage,
    pub admission_table: AdmissionTable,
    admitted_object: Option<ScriptObject>,
}

impl Default for ScriptVmPolicy {
    fn default() -> Self {
        Self {
            fuel: None, call_frames: None, stack_values: None, string_bytes: None,
            retained_heap_bytes: None, allocation_bytes: None, allow_debug_output: true,
            bail_on_uncaught_error: false, active: false, sealed: false, poisoned: None,
            last_entry: EntryUsage::default(), total: EntryUsage::default(),
            admission_table: AdmissionTable::default(), admitted_object: None,
        }
    }
}

impl ScriptVmPolicy {
    /// The allowance historically requested by Splash bodies and widget callbacks.
    pub const CALLBACK_FUEL: u64 = 200_000;

    pub fn untrusted() -> Self {
        Self {
            fuel: Some(Self::CALLBACK_FUEL),
            call_frames: Some(1024),
            stack_values: Some(1 << 16),
            string_bytes: Some(1 << 20),
            retained_heap_bytes: Some(64 << 20),
            allocation_bytes: Some(8 << 20),
            allow_debug_output: false,
            bail_on_uncaught_error: true,
            ..Self::default()
        }
    }

    pub fn sealed(&self) -> bool { self.sealed }
    pub fn poisoned(&self) -> Option<&Poison> { self.poisoned.as_ref() }
    pub fn last_entry(&self) -> EntryUsage { self.last_entry }
    pub fn total(&self) -> EntryUsage { self.total }
    pub fn clear_admitted_object(&mut self) { self.admitted_object = None; }

    pub(crate) fn has_limits(&self) -> bool {
        self.fuel.is_some() || self.call_frames.is_some() || self.stack_values.is_some()
            || self.string_bytes.is_some() || self.retained_heap_bytes.is_some()
            || self.allocation_bytes.is_some() || !self.allow_debug_output || self.bail_on_uncaught_error
    }
}
