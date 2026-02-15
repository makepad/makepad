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
        self.new_string_with(|_, out| {
            out.push_str(value);
        })
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
        let mut out = if let Some(s) = self.strings_reuse.pop() {
            s
        } else {
            String::new()
        };
        cb(self, &mut out);
        self.intern_or_store_string(out)
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
        .into()
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
