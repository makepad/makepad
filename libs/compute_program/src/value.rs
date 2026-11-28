//! Plain values between the app and a program: parameters, messages, `value` outputs and the
//! result of `run`. They hold no VM references, so they cross threads (and later the wire).
use makepad_script::*;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Nil,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Value>),
    /// Fields in order.
    Object(Vec<(String, Value)>),
}

impl Value {
    pub fn object(fields: &[(&str, Value)]) -> Value {
        Value::Object(fields.iter().map(|(k, v)| (k.to_string(), v.clone())).collect())
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Object(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_number(&self) -> Option<f64> {
        match self {
            Value::Number(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }

    /// The numbers of an array value (None if any item is not a number).
    pub fn numbers(&self) -> Option<Vec<f64>> {
        self.as_array()?.iter().map(|v| v.as_number()).collect()
    }
}

impl From<f64> for Value {
    fn from(v: f64) -> Value {
        Value::Number(v)
    }
}

impl From<bool> for Value {
    fn from(v: bool) -> Value {
        Value::Bool(v)
    }
}

impl From<&str> for Value {
    fn from(v: &str) -> Value {
        Value::String(v.to_string())
    }
}

/// The most values one conversion from the VM makes, and its deepest nesting: a program's
/// `value` or result is data for the app, not a way to fill its memory.
pub(crate) const MAX_ITEMS: usize = 1 << 20;
const MAX_DEPTH: usize = 32;

/// `v` as a plain value; Err when it is larger or deeper than [`MAX_ITEMS`] / 32 levels.
/// A fn, or any other VM-only value, becomes Nil.
pub(crate) fn from_script(vm: &ScriptVm, v: ScriptValue) -> Result<Value, String> {
    let mut items = 0;
    convert(vm, v, 0, &mut items)
}

fn convert(vm: &ScriptVm, v: ScriptValue, depth: usize, items: &mut usize) -> Result<Value, String> {
    *items += 1;
    if *items > MAX_ITEMS {
        return Err(format!("a value of more than {MAX_ITEMS} items"));
    }
    if depth > MAX_DEPTH {
        return Err(format!("a value nested more than {MAX_DEPTH} levels"));
    }
    if v.is_nil() {
        return Ok(Value::Nil);
    }
    if let Some(b) = v.as_bool() {
        return Ok(Value::Bool(b));
    }
    if let Some(n) = v.as_number() {
        return Ok(Value::Number(n));
    }
    if let Some(s) = vm.heap().string_with(v, |_, s| s.to_string()) {
        return Ok(Value::String(s));
    }
    if let Some(id) = v.as_id() {
        return Ok(Value::String(id.to_string()));
    }
    if let Some(a) = v.as_array() {
        let n = vm.heap().array_len(a);
        let mut out = Vec::with_capacity(n.min(MAX_ITEMS));
        for i in 0..n {
            out.push(convert(vm, vm.heap().array_index(a, i, NoTrap), depth + 1, items)?);
        }
        return Ok(Value::Array(out));
    }
    if let Some(o) = v.as_object() {
        if vm.is_fn(o) {
            return Ok(Value::Nil);
        }
        let mut out = Vec::new();
        for e in vm.heap().map_entries_ordered(o) {
            let Some(k) = e.key.as_id() else { continue };
            out.push((k.to_string(), convert(vm, e.value, depth + 1, items)?));
        }
        return Ok(Value::Object(out));
    }
    Ok(Value::Nil)
}

/// `v` made in the VM.
pub(crate) fn to_script(vm: &mut ScriptVm, v: &Value) -> ScriptValue {
    match v {
        Value::Nil => NIL,
        Value::Bool(b) => ScriptValue::from_bool(*b),
        Value::Number(n) => ScriptValue::from_f64(*n),
        Value::String(s) => vm.new_string(s),
        Value::Array(items) => {
            let a = vm.heap_mut().new_array();
            if vm.heap().allocation_error_pending() { return NIL; }
            for item in items {
                let x = to_script(vm, item);
                if vm.heap().allocation_error_pending() { return NIL; }
                vm.array_push_trap(a, x);
                if vm.heap().allocation_error_pending() { return NIL; }
            }
            a.into()
        }
        Value::Object(fields) => {
            let o = vm.new_object();
            if vm.heap().allocation_error_pending() { return NIL; }
            for (k, item) in fields {
                let x = to_script(vm, item);
                if vm.heap().allocation_error_pending() { return NIL; }
                vm.set_value_def(o, key_id(k), x);
                if vm.heap().allocation_error_pending() { return NIL; }
            }
            o.into()
        }
    }
}

/// A field name as the VM's id, readable back by name.
pub(crate) fn key_id(name: &str) -> LiveId {
    LiveId::from_str_with_lut(name).unwrap_or_else(|_| LiveId::from_str(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_script_stops_on_first_refusal() {
        let mut host = ScriptVmHost::new((), ());
        let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
        vm.capture_errors();
        let inputs = [
            Value::Array(vec![Value::from("too_long"), Value::Array(vec![]), Value::Object(vec![])]),
            Value::object(&[("a", Value::from("too_long")), ("b", Value::Array(vec![]))]),
        ];
        let observed = inputs.map(|input| {
            for _ in 0..2 {
                vm.heap_mut().new_array();
                vm.new_object();
            }
            vm.gc();
            vm.heap_mut().set_max_string_bytes(Some(4));
            assert!(!vm.heap().allocation_error_pending(), "each conversion must start with a clean heap");
            let before = vm.heap().gc_live_counts();
            // Closing the scope delivers the refusal before the next case reuses the VM.
            let (result, _) = vm.with_heap_allocation_limit(usize::MAX, |vm| to_script(vm, &input));
            let after = vm.heap().gc_live_counts();
            assert!(vm.heap_mut().take_string_limit_exceeded(), "the first string must hit the limit");
            vm.heap_mut().set_max_string_bytes(None);
            (result.is_nil(), after.arrays - before.arrays, after.objects - before.objects)
        });
        assert_eq!(observed, [(true, 1, 0), (true, 0, 1)],
            "refused array and object conversions must return NIL and allocate only their root");
    }
}
