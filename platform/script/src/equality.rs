//! Iterative structural equality. Container pairs are visited once, including
//! cyclic graphs and shared DAGs. Language opcodes supply fuel/deadline checks
//! (see `ScriptVm::handle_structural_equality`); the raw host entry point
//! `ScriptHeap::deep_eq` is iterative and cycle-safe but consumes no VM fuel.
//!
//! Every heap string is interned (`string_heap.rs` routes every store through
//! the intern table, and short strings are inline values), so two string-like
//! values are equal exactly when their bits are equal. String pairs therefore
//! cost one work unit instead of a metered byte compare.

use crate::makepad_live_id::LiveId;
use crate::{array::ScriptArrayStorage, heap::ScriptHeap, value::ScriptValue};
use std::collections::HashSet;

/// Independent ceiling on native comparison work and its temporary storage.
/// Every queued edge, processed pair and typed-array element costs one unit,
/// so the worklist and the visited set can never hold more entries than this.
pub const MAX_EQUALITY_WORK: usize = 65_536;

impl ScriptHeap {
    /// Trusted host comparison without VM fuel. Still iterative and cycle-safe.
    pub fn deep_eq(&self, a: ScriptValue, b: ScriptValue) -> bool {
        self.deep_eq_bounded(a, b, usize::MAX, || true)
            .unwrap_or(false)
    }

    /// `None` is resource exhaustion, never a comparison result. Every queued
    /// edge and processed pair consumes work before allocation or traversal;
    /// `charge_host` runs once per unit and returns `false` to abort.
    pub fn deep_eq_bounded(
        &self,
        a: ScriptValue,
        b: ScriptValue,
        maximum_work: usize,
        mut charge_host: impl FnMut() -> bool,
    ) -> Option<bool> {
        let mut remaining = maximum_work;
        let mut charge = || {
            remaining = remaining.checked_sub(1)?;
            charge_host().then_some(())
        };
        // The root pair is held outside the worklist and the visited set is
        // made on first use, so a scalar comparison allocates nothing: `==` on
        // numbers and strings is hot (string-tag dispatch, `a.kind == "..."`)
        // and only a container pair needs either structure.
        let mut root = Some((a, b));
        let mut pending: Vec<(ScriptValue, ScriptValue)> = Vec::new();
        let mut visited: Option<HashSet<(ScriptValue, ScriptValue)>> = None;
        while let Some((a, b)) = root.take().or_else(|| pending.pop()) {
            charge()?;
            if a.is_nan() || b.is_nan() {
                return Some(false);
            }
            if a == b {
                continue;
            }
            // Interned strings: bit-inequality is content inequality.
            if a.is_string_like() || b.is_string_like() {
                return Some(false);
            }
            if let Some(a) = a.as_number() {
                if b.as_number() != Some(a) {
                    return Some(false);
                }
                continue;
            }
            if let (Some(pa), Some(pb)) = (a.as_object(), b.as_object()) {
                if !visited.get_or_insert_with(HashSet::new).insert((a, b)) {
                    continue;
                }
                let oa = &self.objects[pa];
                let ob = &self.objects[pb];
                if oa.vec.len() != ob.vec.len() || oa.map_len() != ob.map_len() {
                    return Some(false);
                }
                charge()?;
                pending.push((oa.proto, ob.proto));
                for (a, b) in oa.vec.iter().zip(&ob.vec) {
                    charge()?;
                    pending.push((a.key, b.key));
                    charge()?;
                    pending.push((a.value, b.value));
                }
                if let Some(result) = oa.map_iter_ret(|key, value| {
                    if charge().is_none() {
                        return Some(None);
                    }
                    let other = ob.map_get(&key).or_else(|| {
                        // JSON string keys and source identifier keys name the
                        // same fields without changing either object.
                        if let Some(id) = key.as_id().filter(|_| ob.tag.is_string_keys()) {
                            id.as_string(|s| {
                                s.and_then(|s| self.check_intern_string(s))
                                    .and_then(|key| ob.map_get(&key))
                            })
                        } else if key.is_string_like() && !ob.tag.is_string_keys() {
                            self.string_with(key, |_, s| ob.map_get(&LiveId::from_str(s).into()))
                                .flatten()
                        } else {
                            None
                        }
                    });
                    let Some(other) = other else {
                        return Some(Some(false));
                    };
                    pending.push((value, other));
                    None
                }) {
                    return result;
                }
                continue;
            }
            if let (Some(pa), Some(pb)) = (a.as_array(), b.as_array()) {
                if !visited.get_or_insert_with(HashSet::new).insert((a, b)) {
                    continue;
                }
                let a = &self.arrays[pa].storage;
                let b = &self.arrays[pb].storage;
                macro_rules! scalar_array {
                    ($a:expr, $b:expr) => {{
                        if $a.len() != $b.len() {
                            return Some(false);
                        }
                        for (a, b) in $a.iter().zip($b) {
                            charge()?;
                            if a != b {
                                return Some(false);
                            }
                        }
                    }};
                }
                match (a, b) {
                    (ScriptArrayStorage::ScriptValue(a), ScriptArrayStorage::ScriptValue(b)) => {
                        if a.len() != b.len() {
                            return Some(false);
                        }
                        for (&a, &b) in a.iter().zip(b) {
                            charge()?;
                            pending.push((a, b));
                        }
                    }
                    (ScriptArrayStorage::F32(a), ScriptArrayStorage::F32(b)) => scalar_array!(a, b),
                    (ScriptArrayStorage::U32(a), ScriptArrayStorage::U32(b)) => scalar_array!(a, b),
                    (ScriptArrayStorage::U16(a), ScriptArrayStorage::U16(b)) => scalar_array!(a, b),
                    (ScriptArrayStorage::U8(a), ScriptArrayStorage::U8(b)) => scalar_array!(a, b),
                    _ => return Some(false),
                }
                continue;
            }
            return Some(false);
        }
        Some(true)
    }
}
