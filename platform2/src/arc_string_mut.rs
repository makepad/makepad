use crate::makepad_script::*;

/// A string type that can hold either a shared `Arc<String>` from the script heap
/// or a mutable owned `String`. This allows for efficient string handling when
/// receiving strings from scripts (no copy) while also supporting mutation.
#[derive(Clone)]
pub enum ArcStringMut {
    Rc(ScriptRcString),
    String(String),
}

impl Default for ArcStringMut {
    fn default() -> Self {
        Self::String(String::new())
    }
}

impl std::fmt::Debug for ArcStringMut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.as_ref())
    }
}

impl ArcStringMut {
    pub fn as_rc(&self) -> ScriptRcString {
        match self {
            Self::Rc(rc) => rc.clone(),
            Self::String(s) => ScriptRcString::new(s.clone()),
        }
    }

    pub fn as_mut(&mut self) -> &mut String {
        match self {
            Self::Rc(rc) => {
                *self = Self::String(rc.0.to_string());
                self.as_mut()
            }
            Self::String(s) => s,
        }
    }

    pub fn as_mut_empty(&mut self) -> &mut String {
        match self {
            Self::Rc(_) => {
                *self = Self::String(String::new());
                self.as_mut()
            }
            Self::String(s) => {
                s.clear();
                s
            }
        }
    }

    pub fn set(&mut self, v: &str) {
        match self {
            Self::Rc(_) => {
                *self = Self::String(v.to_string());
            }
            Self::String(s) => {
                s.clear();
                s.push_str(v);
            }
        }
    }

    pub fn as_ref(&self) -> &str {
        match self {
            Self::Rc(rc) => &rc.0,
            Self::String(s) => s,
        }
    }
}

impl ScriptHook for ArcStringMut {}
impl ScriptNew for ArcStringMut {
    fn script_new(_vm: &mut ScriptVm) -> Self {
        Default::default()
    }
    fn script_type_check(_heap: &ScriptHeap, value: ScriptValue) -> bool {
        value.is_string_like()
    }
}

impl ScriptApply for ArcStringMut {
    fn script_apply(
        &mut self,
        vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        value: ScriptValue,
    ) {
        // Convert to owned String using the heap's cast method
        let mut s = String::new();
        vm.bx.heap.cast_to_string(value, &mut s);
        *self = ArcStringMut::String(s);
    }

    fn script_to_value(&self, vm: &mut ScriptVm) -> ScriptValue {
        let s = self.as_ref();
        if let Some(val) = ScriptValue::from_inline_string(s) {
            return val;
        }
        vm.bx.heap.new_string_from_str(s).into()
    }
}
