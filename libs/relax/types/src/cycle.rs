//! Query cycles of lowering that are not engine queries (declaration defaults, the types of
//! const parameters, struct tails): the recursion is cut where it re-enters the same
//! computation, which rustc reports as E0391 (rustc_query_system cycle_error).
use std::cell::RefCell;

crate::memo_local! {
    static ACTIVE: RefCell<Vec<(u8, u32, u32)>>;
}

/// What is being computed (a cycle needs the same kind and key to be entered twice).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// the default of a trait's generic parameter: (trait, parameter index)
    TraitDefault = 1,
    /// the declared type of a const parameter: (declaration, parameter index)
    ConstParamType = 2,
    /// whether a struct's tail is unsized: (type id, 0)
    UnsizedTail = 3,
    /// the layout of a type: (type id, 2)
    Layout = 4,
    /// a named constant folded without running code (const_eval::fold_without_execution): (def, 0)
    Fold = 5,
}

pub struct Guard { kind: u8, a: u32, b: u32 }

impl Drop for Guard {
    fn drop(&mut self) {
        let (k, a, b) = (self.kind, self.a, self.b);
        let _ = ACTIVE.try_with(|v| {
            let mut v = v.borrow_mut();
            if let Some(i) = v.iter().rposition(|x| *x == (k, a, b)) { v.remove(i); }
        });
    }
}

/// Enters the computation; Err when it is already in progress on this thread (a cycle).
pub fn enter(kind: Kind, a: u32, b: u32) -> Result<Guard, ()> {
    // (a thread being torn down compiles with its TLS gone: no guard then)
    match ACTIVE.try_with(|v| {
        let mut v = v.borrow_mut();
        let key = (kind as u8, a, b);
        if v.contains(&key) { return Err(()); }
        v.push(key);
        Ok(Guard { kind: key.0, a, b })
    }) {
        Ok(r) => r,
        Err(_) => Ok(Guard { kind: 0, a: 0, b: 0 }),
    }
}

/// The computations in progress on this thread, set aside while the owner thread serves a
/// request inside another request's wait (the compiler's per-request frame, jit::frame_enter):
/// what the request below has in progress is not this request's, so meeting it is no cycle (M55:
/// a Job served in a query wait during `OnceLock<..>`'s layout was refused that layout, E0391).
pub struct Active(Vec<(u8, u32, u32)>);

/// Takes this thread's computations in progress, leaving none.
pub fn take() -> Active {
    Active(ACTIVE.try_with(|v| std::mem::take(&mut *v.borrow_mut())).unwrap_or_default())
}

/// Puts back what `take` took (the computations entered since have ended).
pub fn restore(a: Active) {
    let _ = ACTIVE.try_with(|v| *v.borrow_mut() = a.0);
}
