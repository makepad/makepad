use crate::array::*;
use crate::heap::*;
use crate::string::*;
use crate::value::*;
use std::fmt::Write;

impl ScriptHeap {
    // Strings

    pub fn string_mut_self_with<R, F: FnOnce(&mut Self, &str) -> R>(
        &mut self,
        value: ScriptValue,
        cb: F,
    ) -> Option<R> {
        if let Some(s) = value.as_string() {
            if let Some(s) = &self.strings[s] {
                let s = s.string.clone();
                let r = cb(self, &s.0);
                return Some(r);
            } else {
                return None;
            }
        }
        if let Some(r) = value.as_inline_string(|s| cb(self, s)) {
            return Some(r);
        }
        None
    }

    pub fn string_with<R, F: FnOnce(&Self, &str) -> R>(
        &self,
        value: ScriptValue,
        cb: F,
    ) -> Option<R> {
        if let Some(s) = value.as_string() {
            if let Some(s) = &self.strings[s] {
                let r = cb(self, &s.string.0);
                return Some(r);
            } else {
                return None;
            }
        }
        if let Some(r) = value.as_inline_string(|s| cb(self, s)) {
            return Some(r);
        }
        None
    }

    pub fn new_string_from_str(&mut self, value: &str) -> ScriptValue {
        if let Some(value) = ScriptValue::from_inline_string(value) {
            return value;
        }
        if let Some(index) = self.string_intern.get(value) {
            return (*index).into();
        }
        if !self.charge_string_payload(value.len(), "creating a string") {
            return NIL;
        }
        self.store_unique_string(value.to_owned()).into()
    }

    pub fn temp_string_with<R, F: FnOnce(&mut Self, &mut String) -> R>(&mut self, cb: F) -> R {
        let mut out = if let Some(s) = self.strings_reuse.pop() {
            s
        } else {
            String::new()
        };
        let r = cb(self, &mut out);
        out.clear();
        self.strings_reuse.push(out);
        r
    }

    pub fn new_string_with<F: FnOnce(&mut Self, &mut String)>(&mut self, cb: F) -> ScriptValue {
        if self.has_allocation_budget() {
            // A raw `&mut String` callback has no way to enforce a ceiling
            // before `push` reallocates. Sandboxed execution must use one of
            // the exact/upper-bound preflight helpers instead; trusted VMs
            // retain this unrestricted compatibility API.
            let _ = self.charge_allocation(
                usize::MAX,
                "building a string without an allocation preflight",
            );
            return NIL;
        }
        let mut out = if let Some(s) = self.strings_reuse.pop() {
            s
        } else {
            String::new()
        };
        out.clear();
        cb(self, &mut out);
        self.intern_or_store_string(out)
    }

    /// Materialize a script-scalable string only after its exact length (or a
    /// conservative upper bound) has fit in the current allocation budget.
    pub(crate) fn new_string_with_preflight<F: FnOnce(&mut Self, &mut String)>(
        &mut self,
        max_len: usize,
        operation: &'static str,
        cb: F,
    ) -> ScriptValue {
        if !self.charge_string_payload(max_len, operation) {
            return NIL;
        }
        let mut out = if let Some(s) = self.strings_reuse.pop() {
            s
        } else {
            String::new()
        };
        out.clear();
        if out.capacity() < max_len {
            out.reserve_exact(max_len);
        }
        cb(self, &mut out);
        debug_assert!(
            out.len() <= max_len,
            "preflighted string exceeded its declared bound"
        );
        self.intern_or_store_string_charged(out)
    }

    pub(crate) fn temp_string_with_preflight<R, F: FnOnce(&mut Self, &mut String) -> R>(
        &mut self,
        max_len: usize,
        operation: &'static str,
        cb: F,
    ) -> Option<R> {
        if !self.charge_allocation(max_len, operation) {
            return None;
        }
        let mut out = if let Some(s) = self.strings_reuse.pop() {
            s
        } else {
            String::new()
        };
        out.clear();
        if out.capacity() < max_len {
            out.reserve_exact(max_len);
        }
        let result = cb(self, &mut out);
        debug_assert!(out.len() <= max_len);
        out.clear();
        self.strings_reuse.push(out);
        Some(result)
    }

    /// Concatenate two script values with the exact output size preflighted
    /// before allocating the result. Script `+`, concat and compound assigns
    /// use this path, which closes the usual exponential string-doubling bomb.
    pub fn new_string_concat(&mut self, a: ScriptValue, b: ScriptValue) -> ScriptValue {
        let a_len = self.cast_to_string_len(a);
        let b_len = self.cast_to_string_len(b);
        let len = a_len.checked_add(b_len).unwrap_or(usize::MAX);
        if !self.charge_string_payload(len, "concatenating strings") {
            return NIL;
        }

        let mut out = if let Some(s) = self.strings_reuse.pop() {
            s
        } else {
            String::new()
        };
        out.clear();
        // The budget check bounds `len`, making this reserve safe even for a
        // hostile sparse/overflow-derived input length.
        if out.capacity() < len {
            out.reserve_exact(len);
        }
        self.cast_to_string(a, &mut out);
        self.cast_to_string(b, &mut out);
        self.intern_or_store_string_charged(out)
    }

    /// Takes an owned String and either interns it, reuses an existing interned value, or stores it as a new string.
    /// The String is consumed and may be returned to the reuse pool.
    pub fn intern_or_store_string(&mut self, mut out: String) -> ScriptValue {
        if let Some(v) = ScriptValue::from_inline_string(&out) {
            out.clear();
            self.strings_reuse.push(out);
            return v;
        }

        // check intern table
        if let Some(index) = self.string_intern.get(&out) {
            out.clear();
            self.strings_reuse.push(out);
            return (*index).into();
        }

        if !self.charge_string_payload(out.len(), "interning a string") {
            // Do not retain an over-budget buffer in the reuse pool: the
            // untrusted execution must release its transient allocation.
            return NIL;
        }
        self.store_unique_string(out).into()
    }

    fn charge_string_payload(&mut self, len: usize, operation: &'static str) -> bool {
        let bytes = len
            .checked_add(std::mem::size_of::<ScriptStringData>())
            .and_then(|bytes| bytes.checked_add(2 * std::mem::size_of::<usize>()))
            .unwrap_or(usize::MAX);
        self.charge_allocation(bytes, operation)
    }

    /// Store a string whose payload has already been charged. Inline and
    /// intern hits are still handled because concat preflights before it has
    /// materialized the bytes.
    fn intern_or_store_string_charged(&mut self, mut out: String) -> ScriptValue {
        if let Some(v) = ScriptValue::from_inline_string(&out) {
            out.clear();
            self.strings_reuse.push(out);
            return v;
        }
        if let Some(index) = self.string_intern.get(&out) {
            out.clear();
            self.strings_reuse.push(out);
            return (*index).into();
        }
        self.store_unique_string(out).into()
    }

    fn store_unique_string(&mut self, out: String) -> ScriptString {
        // fetch a free string
        if let Some(str) = self.strings_free.pop() {
            // str already has the correct generation from gc.rs sweep
            let out = ScriptRcString::new(out);
            self.strings[str] = Some(ScriptStringData {
                tag: Default::default(),
                string: out.clone(),
            });
            self.string_intern.insert(out, str);
            str
        } else {
            let out = ScriptRcString::new(out);
            let index = self.strings.len();
            self.strings.push(Some(ScriptStringData {
                tag: Default::default(),
                string: out.clone(),
            }));
            // New slot starts at generation 0
            let ret = ScriptString::new(index as _, crate::value::GENERATION_ZERO);
            self.string_intern.insert(out, ret);
            ret
        }
    }

    pub(crate) fn cast_to_string_len(&self, value: ScriptValue) -> usize {
        if let Some(len) = value.as_inline_string(|value| value.len()) {
            return len;
        }
        if let Some(value) = value.as_string() {
            return self.string(value).len();
        }
        // Non-string casts are all small, fixed-shape values. Formatting one
        // here cannot scale with script-controlled container contents.
        let mut out = String::new();
        self.cast_to_string(value, &mut out);
        out.len()
    }

    /// Copy a script value into an owned Rust string for a native binding,
    /// charging before the owned buffer is created. This is the safe bridge
    /// for sandbox-visible APIs that need to retain text outside ScriptHeap.
    pub fn cast_to_owned_string(
        &mut self,
        value: ScriptValue,
        operation: &'static str,
    ) -> Option<String> {
        let len = self.cast_to_string_len(value);
        if !self.charge_allocation(len, operation) {
            return None;
        }
        let mut out = String::new();
        out.reserve_exact(len);
        self.cast_to_string(value, &mut out);
        Some(out)
    }

    pub fn check_intern_string(&self, value: &str) -> Option<ScriptValue> {
        if let Some(v) = ScriptValue::from_inline_string(&value) {
            Some(v)
        } else if let Some(idx) = self.string_intern.get(value) {
            Some((*idx).into())
        } else {
            None
        }
    }

    pub fn string(&self, ptr: ScriptString) -> &str {
        if let Some(s) = &self.strings[ptr] {
            &s.string.0
        } else {
            ""
        }
    }

    pub fn string_to_bytes_array(&mut self, v: ScriptValue) -> ScriptArray {
        let arr = self.new_array();
        if self.is_allocation_poison_array(arr) {
            return arr;
        }
        let bytes = self.string_with(v, |_heap, value| value.len()).unwrap_or(0);
        if !self.charge_allocation(bytes, "converting a string to bytes") {
            return arr;
        }
        if v.as_inline_string(|str| {
            let array = &mut self.arrays[arr];
            if let ScriptArrayStorage::U8(v) = &mut array.storage {
                v.clear();
                v.extend(str.as_bytes())
            } else {
                array.storage = ScriptArrayStorage::U8(str.as_bytes().into());
            }
        })
        .is_some()
        {
        } else if let Some(str) = v.as_string() {
            let array = &mut self.arrays[arr];
            let str = if let Some(s) = &self.strings[str] {
                &s.string.0
            } else {
                ""
            };
            if let ScriptArrayStorage::U8(v) = &mut array.storage {
                v.clear();
                v.extend(str.as_bytes())
            } else {
                array.storage = ScriptArrayStorage::U8(str.as_bytes().into());
            }
        }
        return arr;
    }

    pub fn string_to_chars_array(&mut self, v: ScriptValue) -> ScriptArray {
        let arr = self.new_array();
        if self.is_allocation_poison_array(arr) {
            return arr;
        }
        let chars = self
            .string_with(v, |_heap, value| value.chars().count())
            .unwrap_or(0);
        let bytes = chars.checked_mul(4).unwrap_or(usize::MAX);
        if !self.charge_allocation(bytes, "converting a string to characters") {
            return arr;
        }
        if v.as_inline_string(|str| {
            let array = &mut self.arrays[arr];
            if let ScriptArrayStorage::U32(v) = &mut array.storage {
                v.clear();
                for c in str.chars() {
                    v.push(c as u32)
                }
            } else {
                array.storage = ScriptArrayStorage::U32(str.chars().map(|c| c as u32).collect());
            }
        })
        .is_some()
        {
        } else if let Some(str) = v.as_string() {
            let array = &mut self.arrays[arr];
            let str = if let Some(s) = &self.strings[str] {
                &s.string.0
            } else {
                ""
            };
            if let ScriptArrayStorage::U32(v) = &mut array.storage {
                v.clear();
                for c in str.chars() {
                    v.push(c as u32)
                }
            } else {
                array.storage = ScriptArrayStorage::U32(str.chars().map(|c| c as u32).collect());
            }
        }
        return arr;
    }

    pub fn cast_to_string(&self, v: ScriptValue, out: &mut String) {
        if v.as_inline_string(|s| write!(out, "{s}")).is_some() {
            return;
        }
        if let Some(v) = v.as_string() {
            let str = self.string(v);
            out.push_str(str);
            return;
        }
        if let Some(v) = v.as_f64() {
            write!(out, "{v}").ok();
            return;
        }
        if let Some(v) = v.as_u40() {
            write!(out, "{v}").ok();
            return;
        }
        if let Some(v) = v.as_bool() {
            write!(out, "{v}").ok();
            return;
        }
        if let Some(v) = v.as_id() {
            write!(out, "{v}").ok();
            return;
        }
        if v.is_nil() {
            return;
        }
        if let Some(v) = v.as_f32() {
            write!(out, "{v}").ok();
            return;
        }
        if let Some(v) = v.as_f16() {
            write!(out, "{v}").ok();
            return;
        }
        if let Some(v) = v.as_u32() {
            write!(out, "{v}").ok();
            return;
        }
        if let Some(v) = v.as_i32() {
            write!(out, "{v}").ok();
            return;
        }
        if let Some(_v) = v.as_object() {
            write!(out, "[ScriptObject]").ok();
            return;
        }
        if let Some(v) = v.as_color() {
            write!(out, "#{:08x}", v).ok();
            return;
        }
        if v.is_opcode() {
            write!(out, "[Opcode]").ok();
            return;
        }
        if v.is_err() {
            write!(out, "[Error:{}]", v).ok();
            return;
        }
        write!(out, "[Unknown]").ok();
        return;
    }
}
