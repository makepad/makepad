use makepad_relax_api::relax_api;
use crate::array::*;
use crate::heap::*;
use crate::string::*;
use crate::value::*;
use crate::vm_api::ScriptTextOut;
use std::fmt::{self, Write};

/// A sink used while the VM converts values into strings.
///
/// Ordinary [`String`] sinks preserve the upstream unbounded behavior. The
/// runtime can instead use [`ScriptStringBuffer`] to stop a script-created
/// string before it grows past the host-selected per-string ceiling or the
/// remaining allocation budget.
pub trait ScriptStringSink: Write {
    fn is_full(&self) -> bool;

    fn append_str(&mut self, value: &str) {
        let _ = self.write_str(value);
    }

    fn append_char(&mut self, value: char) {
        let _ = self.write_char(value);
    }
}

impl ScriptStringSink for String {
    fn is_full(&self) -> bool {
        false
    }
}

impl Write for crate::vm_api::ScriptTextOut<'_> {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.push(s);
        Ok(())
    }
}

impl ScriptStringSink for crate::vm_api::ScriptTextOut<'_> {
    fn is_full(&self) -> bool {
        false
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScriptStringLimitHit {
    /// The per-string ceiling (`ScriptHeap::set_max_string_bytes`).
    String,
    /// The remaining allocation budget, or the allocator itself.
    Heap,
}

/// A string builder that records a limit hit without allocating beyond its
/// configured logical byte length.
pub struct ScriptStringBuffer {
    value: String,
    max_string_bytes: Option<usize>,
    max_heap_bytes: Option<usize>,
    hit: Option<ScriptStringLimitHit>,
}

impl ScriptStringBuffer {
    fn new(value: String, max_string_bytes: Option<usize>, max_heap_bytes: Option<usize>) -> Self {
        Self {
            value,
            max_string_bytes,
            max_heap_bytes,
            hit: None,
        }
    }

    pub fn as_str(&self) -> &str {
        &self.value
    }

    pub fn len(&self) -> usize {
        self.value.len()
    }

    pub fn is_empty(&self) -> bool {
        self.value.is_empty()
    }

    fn into_parts(self) -> (String, Option<ScriptStringLimitHit>) {
        (self.value, self.hit)
    }

    fn reserve_append(&mut self, additional_bytes: usize) -> fmt::Result {
        if self.hit.is_some() {
            return Err(fmt::Error);
        }
        let Some(next_len) = self.value.len().checked_add(additional_bytes) else {
            self.hit = Some(ScriptStringLimitHit::Heap);
            return Err(fmt::Error);
        };
        if self
            .max_string_bytes
            .is_some_and(|maximum| next_len > maximum)
        {
            self.hit = Some(ScriptStringLimitHit::String);
            return Err(fmt::Error);
        }
        if self
            .max_heap_bytes
            .is_some_and(|maximum| next_len > maximum)
        {
            self.hit = Some(ScriptStringLimitHit::Heap);
            return Err(fmt::Error);
        }
        if self.value.try_reserve(additional_bytes).is_err() {
            self.hit = Some(ScriptStringLimitHit::Heap);
            return Err(fmt::Error);
        }
        Ok(())
    }
}

impl Write for ScriptStringBuffer {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        self.reserve_append(value.len())?;
        self.value.push_str(value);
        Ok(())
    }

    fn write_char(&mut self, value: char) -> fmt::Result {
        let mut encoded = [0; 4];
        self.write_str(value.encode_utf8(&mut encoded))
    }
}

impl ScriptStringSink for ScriptStringBuffer {
    fn is_full(&self) -> bool {
        self.hit.is_some()
    }
}

/// str_read's mode.
pub const STR_VIEW: u32 = 0;
pub const STR_INTERN: u32 = 1;
pub const STR_CAST: u32 = 2;
pub const STR_LEN: u32 = 3;

/// What str_read answers (by its mode).
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct StrRead {
    pub value: ScriptValue,
    pub ptr: *const u8,
    pub len: usize,
    pub found: bool,
}

#[relax_api]
impl ScriptHeap {
    // Strings

    /// Sets a maximum logical length for newly constructed script strings.
    ///
    /// `None` preserves the inherited VM behavior. A limit applies only to
    /// string construction after this call; existing heap strings remain
    /// valid so a host can safely lower a limit between completed runs.
    pub fn set_max_string_bytes(&mut self, maximum_bytes: Option<usize>) {
        self.max_string_bytes = maximum_bytes;
        self.string_limit_exceeded = false;
        self.pending_string_limit_error = None;
    }

    /// Returns the configured per-string ceiling, if any.
    pub fn max_string_bytes(&self) -> Option<usize> {
        self.max_string_bytes
    }

    /// Returns and clears a pending bounded-string construction failure.
    pub fn take_string_limit_exceeded(&mut self) -> bool {
        self.pending_string_limit_error = None;
        std::mem::take(&mut self.string_limit_exceeded)
    }

    #[inline]
    fn exceeds_string_limit(&self, len: usize) -> bool {
        self.max_string_bytes
            .is_some_and(|maximum| len > maximum)
    }

    /// Records a per-string ceiling refusal. The interpreter polls it through
    /// `take_allocation_error` and bails, uncatchably, before the next opcode.
    pub(crate) fn note_string_limit_exceeded(&mut self, len: usize, operation: &'static str) {
        self.string_limit_exceeded = true;
        if self.pending_string_limit_error.is_none() {
            self.allocation_error_pending = true;
            self.pending_string_limit_error = Some(format!(
                "script string allocation limit exceeded while {operation}: {len} bytes, maximum {}",
                self.max_string_bytes.unwrap_or(usize::MAX)
            ));
        }
    }

    /// Builds a script string through the per-string ceiling and the
    /// remaining allocation budget without materializing text beyond either.
    /// A refusal returns `NIL` and records the pending limit error.
    pub fn new_bounded_string_with<F: FnOnce(&mut Self, &mut ScriptStringBuffer)>(
        &mut self,
        cb: F,
    ) -> ScriptValue {
        const OPERATION: &str = "building a bounded string";
        let mut out = self.new_string_buffer();
        cb(self, &mut out);
        let (out, hit) = out.into_parts();
        match hit {
            Some(ScriptStringLimitHit::String) => {
                self.note_string_limit_exceeded(out.len(), OPERATION);
                NIL
            }
            Some(ScriptStringLimitHit::Heap) => {
                let _ = self.charge_allocation(usize::MAX, OPERATION);
                NIL
            }
            None => self.intern_or_store_string(out),
        }
    }

    /// Builds a temporary string through the same bounds as
    /// [`Self::new_bounded_string_with`]; the buffer is not retained, so it is
    /// not charged, but a limit hit still records the pending limit error.
    pub fn temp_bounded_string_with<R, F: FnOnce(&mut Self, &mut ScriptStringBuffer) -> R>(
        &mut self,
        cb: F,
    ) -> R {
        const OPERATION: &str = "building a temporary bounded string";
        let mut out = self.new_string_buffer();
        let r = cb(self, &mut out);
        let (out, hit) = out.into_parts();
        match hit {
            Some(ScriptStringLimitHit::String) => {
                self.note_string_limit_exceeded(out.len(), OPERATION);
            }
            Some(ScriptStringLimitHit::Heap) => {
                let _ = self.charge_allocation(usize::MAX, OPERATION);
            }
            None => self.recycle_string(out),
        }
        r
    }

    fn new_string_buffer(&mut self) -> ScriptStringBuffer {
        let out = self.strings_reuse.pop().unwrap_or_default();
        let remaining = self.allocation_remaining();
        let max_heap_bytes = if remaining == usize::MAX {
            None
        } else {
            Some(remaining.saturating_sub(Self::string_metadata_bytes()))
        };
        ScriptStringBuffer::new(out, self.max_string_bytes, max_heap_bytes)
    }

    fn recycle_string(&mut self, mut out: String) {
        out.clear();
        self.strings_reuse.push(out);
    }

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

    #[inline(always)]
    pub fn string_with<R, F: FnOnce(&Self, &str) -> R>(
        &self,
        value: ScriptValue,
        cb: F,
    ) -> Option<R> {
        if value.as_string().is_some() {
            return self.string_view(value).map(|s| cb(self, s));
        }
        value.as_inline_string(|s| cb(self, s))
    }

    /// The text of heap string `value` (None: no heap string; inline strings live in the value
    /// itself, ScriptValue::as_inline_string).
    #[inline]
    pub fn string_view(&self, value: ScriptValue) -> Option<&str> {
        let r = self.str_read(STR_VIEW, value, "", &mut ScriptTextOut::discard());
        // SAFETY: the view is the heap string's text, borrowed from `self`
        r.found.then(|| unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(r.ptr, r.len)) })
    }

    /// Writes `value` as text into `out` (what cast_to_string writes into a String).
    #[inline]
    pub fn cast_to_text(&self, value: ScriptValue, out: &mut ScriptTextOut) {
        self.str_read(STR_CAST, value, "", out);
    }

    /// Reads strings by `mode`: STR_VIEW the text of heap string `value` (`ptr`/`len`, `found`
    /// false when it is no heap string); STR_INTERN the value an existing string `text` has
    /// (`value`, `found` false when none); STR_CAST writes `value` as text into `out`; STR_LEN the
    /// length of `value` as text (`len`).
    #[relax_api]
    pub fn str_read(&self, mode: u32, value: ScriptValue, text: &str, out: &mut ScriptTextOut) -> StrRead {
        let mut r = StrRead { value: NIL, ptr: std::ptr::null(), len: 0, found: false };
        match mode {
            STR_VIEW => {
                if let Some(s) = value.as_string() {
                    if let Some(d) = self.strings[s].as_ref() {
                        let t = d.string.0.as_str();
                        r.ptr = t.as_ptr();
                        r.len = t.len();
                        r.found = true;
                    }
                }
            }
            STR_INTERN => {
                if !self.exceeds_string_limit(text.len()) {
                    if let Some(v) = ScriptValue::from_inline_string(text) {
                        r.value = v;
                        r.found = true;
                    } else if let Some(idx) = self.string_intern.get(text) {
                        r.value = (*idx).into();
                        r.found = true;
                    }
                }
            }
            STR_CAST => self.cast_to_string(value, out),
            _ => {
                r.len = if let Some(len) = value.as_inline_string(|value| value.len()) {
                    len
                } else if let Some(value) = value.as_string() {
                    self.string(value).len()
                } else {
                    // Non-string casts are all small, fixed-shape values. Formatting one
                    // here cannot scale with script-controlled container contents.
                    let mut out = String::new();
                    self.cast_to_string(value, &mut out);
                    out.len()
                };
            }
        }
        r
    }

    /// Charges the heap's allocation budget for an owned copy of `value`'s text (false: over
    /// budget, nothing to copy).
    pub fn charge_owned_string(&mut self, value: ScriptValue, operation: &'static str) -> bool {
        let len = self.cast_to_string_len(value);
        self.charge_allocation(len, operation)
    }

    #[relax_api]
    pub fn new_string_from_str(&mut self, value: &str) -> ScriptValue {
        if self.exceeds_string_limit(value.len()) {
            self.note_string_limit_exceeded(value.len(), "creating a string");
            return NIL;
        }
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
        if self.has_allocation_budget() && self.charge_native_strings_after {
            // Charged on its real length when it is stored.
            let mut out = String::new();
            cb(self, &mut out);
            return self.intern_or_store_string(out);
        }
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
        if self.exceeds_string_limit(len) {
            self.note_string_limit_exceeded(len, "concatenating strings");
            return NIL;
        }
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
        if self.exceeds_string_limit(out.len()) {
            self.note_string_limit_exceeded(out.len(), "storing a string");
            return NIL;
        }
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

    #[inline]
    fn string_metadata_bytes() -> usize {
        std::mem::size_of::<ScriptStringData>() + 2 * std::mem::size_of::<usize>()
    }

    fn charge_string_payload(&mut self, len: usize, operation: &'static str) -> bool {
        let bytes = len
            .checked_add(Self::string_metadata_bytes())
            .unwrap_or(usize::MAX);
        self.charge_allocation(bytes, operation)
    }

    /// Store a string whose payload has already been charged. Inline and
    /// intern hits are still handled because concat preflights before it has
    /// materialized the bytes. The per-string ceiling is checked on the exact
    /// length here because preflights only know an upper bound.
    fn intern_or_store_string_charged(&mut self, mut out: String) -> ScriptValue {
        if self.exceeds_string_limit(out.len()) {
            self.note_string_limit_exceeded(out.len(), "storing a string");
            return NIL;
        }
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
            let (index, generation) = self.strings.push(Some(ScriptStringData {
                tag: Default::default(),
                string: out.clone(),
            }));
            let ret = ScriptString::new(index, generation);
            self.string_intern.insert(out, ret);
            ret
        }
    }

    #[inline]
    pub(crate) fn cast_to_string_len(&self, value: ScriptValue) -> usize {
        self.str_read(STR_LEN, value, "", &mut ScriptTextOut::discard()).len
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

    #[inline]
    pub fn check_intern_string(&self, value: &str) -> Option<ScriptValue> {
        let r = self.str_read(STR_INTERN, NIL, value, &mut ScriptTextOut::discard());
        r.found.then_some(r.value)
    }

    #[inline]
    pub fn string(&self, ptr: ScriptString) -> &str {
        self.string_view(ptr.into()).unwrap_or("")
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

    pub fn cast_to_string<S: ScriptStringSink>(&self, v: ScriptValue, out: &mut S) {
        if v.as_inline_string(|s| out.append_str(s)).is_some() {
            return;
        }
        if let Some(v) = v.as_string() {
            let str = self.string(v);
            out.append_str(str);
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_buffer_stops_at_the_string_ceiling_without_over_allocating() {
        let mut heap = ScriptHeap::empty();
        heap.set_max_string_bytes(Some(5));
        let value = heap.new_bounded_string_with(|_, out| {
            out.append_str("abc");
            assert!(!out.is_full());
            out.append_char('d');
            out.append_str("efgh");
            assert!(out.is_full());
            assert_eq!(out.as_str(), "abcd");
            assert!(out.len() <= 5);
        });
        assert!(value.is_nil());
        assert!(heap
            .take_allocation_error()
            .is_some_and(|error| error.contains("script string allocation limit exceeded")));
        assert!(heap.take_string_limit_exceeded());
        assert!(heap.take_allocation_error().is_none());

        let value = heap.new_bounded_string_with(|_, out| out.append_str("abcde"));
        assert_eq!(heap.string_with(value, |_, text| text.to_owned()).as_deref(), Some("abcde"));
        assert!(!heap.take_string_limit_exceeded());
    }

    #[test]
    fn bounded_buffer_stops_at_the_remaining_heap_budget() {
        let mut heap = ScriptHeap::empty();
        heap.set_max_heap_bytes(Some(usize::MAX));
        let baseline = heap.accounted_heap_bytes();
        heap.set_max_heap_bytes(Some(baseline + 128));
        let value = heap.new_bounded_string_with(|_, out| {
            for _ in 0..64 {
                out.append_str("0123456789");
            }
            assert!(out.is_full());
            assert!(out.len() <= 128);
        });
        assert!(value.is_nil());
        assert!(!heap.take_string_limit_exceeded());
        assert!(heap
            .take_allocation_error()
            .is_some_and(|error| error.contains("script heap allocation limit exceeded")));
        assert!(heap.take_heap_limit_exceeded());
        assert!(heap.take_allocation_error().is_none());
    }

    #[test]
    fn store_paths_apply_the_exact_string_ceiling() {
        let mut heap = ScriptHeap::empty();
        heap.set_max_string_bytes(Some(12));
        assert!(heap.new_string_from_str("thirteen chars").is_nil());
        assert!(heap.take_allocation_error().is_some());
        assert!(heap.take_string_limit_exceeded());
        assert!(heap.intern_or_store_string("thirteen chars".to_owned()).is_nil());
        assert!(heap.take_string_limit_exceeded());
        assert!(heap.check_intern_string("thirteen chars").is_none());

        let a = heap.new_string_from_str("twelve!");
        let b = heap.new_string_from_str("twelve");
        assert!(heap.new_string_concat(a, b).is_nil());
        assert!(heap.take_string_limit_exceeded());

        let c = heap.new_string_from_str("abcde");
        let ok = heap.new_string_concat(a, c);
        assert_eq!(heap.string_with(ok, |_, text| text.to_owned()).as_deref(), Some("twelve!abcde"));
        assert!(!heap.take_string_limit_exceeded());
    }
}
