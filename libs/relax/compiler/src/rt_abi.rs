// Relax's runtime calling convention for host entries (see the macros). Each module that declares
// such entries includes this file (`include!`), so the macros are in its textual scope under any
// crate root that includes the module (the checker driver mounts the modules under its own root).

/// Host entries that compiled code calls (`Callee::Host`, the runtime glue's calls) and host -> JIT
/// calls use Relax's own convention: SysV on every x86-64 OS (Windows included; rustc's "sysv64",
/// the same as "C" on Linux) and C on arm64. `rt_fn!` declares such a fn, `rt_fn_ty!` its type.
#[cfg(target_arch = "x86_64")]
#[allow(unused_macros)]
macro_rules! rt_fn {
    ($(#[$m:meta])* $v:vis fn $name:ident ($($a:ident : $t:ty),* $(,)?) $(-> $r:ty)? $body:block) => {
        $(#[$m])* $v extern "sysv64" fn $name($($a: $t),*) $(-> $r)? $body
    };
}
#[cfg(not(target_arch = "x86_64"))]
#[allow(unused_macros)]
macro_rules! rt_fn {
    ($(#[$m:meta])* $v:vis fn $name:ident ($($a:ident : $t:ty),* $(,)?) $(-> $r:ty)? $body:block) => {
        $(#[$m])* $v extern "C" fn $name($($a: $t),*) $(-> $r)? $body
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
