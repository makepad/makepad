//! Query cycles of lowering that are not engine queries (declaration defaults, the types of
//! const parameters, struct tails): the recursion is cut where it re-enters the same
//! computation, which rustc reports as E0391 (rustc_query_system cycle_error).
use std::cell::RefCell;

crate::gen_local! {
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
