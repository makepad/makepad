// Relax's runtime calling convention for host entries (see the macros). Each module that declares
// such entries includes this file (`include!`), so the macros are in its textual scope under any
// crate root that includes the module (the checker driver mounts the modules under its own root).

/// Host entries that compiled code calls (`Callee::Host`, the runtime glue's calls) and host -> JIT
/// calls use Relax's own convention: SysV on every x86-64 OS (Windows included; rustc's "sysv64",
/// the same as "C" on Linux) and C on arm64. `rt_fn!` declares such a fn, `rt_fn_ty!` its type.
///
/// A native panic never leaves an entry: the JIT frames above it have no native unwind tables, so
/// the unwinder's search ends at them (_URC_END_OF_STACK: "failed to initiate panic, error 5") and
/// the process aborts without a message. Each entry runs its body under catch_unwind and turns a
/// panic into a Relax panic of the JIT code that called it (jit.rs host_boundary: printed with
/// its payload, then unwound to that code's recovery point). `leaf fn` declares an entry that
/// cannot panic (pure arithmetic and memory moves on the hot paths): no guard.
#[cfg(target_arch = "x86_64")]
#[allow(unused_macros)]
macro_rules! rt_fn {
    ($($t:tt)*) => { rt_fn_abi! { "sysv64" $($t)* } };
}
#[cfg(not(target_arch = "x86_64"))]
#[allow(unused_macros)]
macro_rules! rt_fn {
    ($($t:tt)*) => { rt_fn_abi! { "C" $($t)* } };
}
#[allow(unused_macros)]
macro_rules! rt_fn_abi {
    ($abi:literal $(#[$m:meta])* $v:vis leaf fn $name:ident ($($a:ident : $t:ty),* $(,)?) $(-> $r:ty)? $body:block) => {
        $(#[$m])* $v extern $abi fn $name($($a: $t),*) $(-> $r)? $body
    };
    ($abi:literal $(#[$m:meta])* $v:vis fn $name:ident ($($a:ident : $t:ty),* $(,)?) $(-> $r:ty)? $body:block) => {
        $(#[$m])* $v extern $abi fn $name($($a: $t),*) $(-> $r)? {
            crate::jit::host_boundary(stringify!($name), move || $body)
        }
    };
}
#[cfg(target_arch = "x86_64")]
#[allow(unused_macros)]
macro_rules! rt_fn_ty {
    (fn($($t:ty),* $(,)?) $(-> $r:ty)?) => { extern "sysv64" fn($($t),*) $(-> $r)? };
}
#[cfg(not(target_arch = "x86_64"))]
#[allow(unused_macros)]
macro_rules! rt_fn_ty {
    (fn($($t:ty),* $(,)?) $(-> $r:ty)?) => { extern "C" fn($($t),*) $(-> $r)? };
}
