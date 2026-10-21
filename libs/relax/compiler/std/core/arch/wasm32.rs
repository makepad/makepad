//! core::arch::wasm32 for the web target run on this machine (cfg.rs `web`): the linear
//! memory is the trace host's (`makepad-web-trace`, web_host.rs), which defines these symbols
//! with the semantics of the wasm instructions (memory index 0 only, sizes in 64 KiB pages).

extern "C" {
    fn relax_web_memory_size(mem: u32) -> usize;
    fn relax_web_memory_grow(mem: u32, delta: usize) -> usize;
}

/// `memory.size`: the current size of memory `MEM` in pages.
#[inline]
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(0))]
pub fn memory_size<const MEM: u32>() -> usize {
    unsafe { relax_web_memory_size(MEM) }
}

/// `memory.grow`: grows memory `MEM` by `delta` pages; the previous size in pages, or
/// `usize::MAX` when it cannot grow.
#[inline]
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(0))]
pub fn memory_grow<const MEM: u32>(delta: usize) -> usize {
    unsafe { relax_web_memory_grow(MEM, delta) }
}
