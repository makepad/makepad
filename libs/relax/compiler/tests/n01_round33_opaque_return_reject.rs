// rejects: lifetime may not live long enough
// A call's opaque result returned as the caller's `impl Sized + 'static`: rustc relates the call's
// output to the caller's opaque type under the call's own category, Return (nightly borrowck
// type_check/relate_tys.rs register_goals), and blames that return: "lifetime may not live long
// enough". E0521 is for an assignment or a call argument (diagnostics/region_errors.rs), which
// `x` passed to mk is, but the return comes after it on the path from 'a to 'static.
#![allow(dead_code)]
fn mk<'a>(x: &'a u8) -> impl Sized + 'a { x }
fn escapes<'a>(x: &'a u8) -> impl Sized + 'static { mk(x) }
