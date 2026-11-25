//! The host linked into this binary. The first reader of any host fact applies its registration.
//! A host's `apply` only writes its facts. Calling a getter that calls `init` from there would
//! re-enter the same OnceLock.

/// One host declaration in the link section. An anchor has no callback.
#[repr(C)]
pub struct Entry {
    pub name: &'static str,
    pub apply: Option<fn()>,
}

#[cfg(any(test, not(target_arch = "wasm32")))]
const fn anchor() -> Entry {
    Entry { name: "", apply: None }
}

#[cfg(all(not(windows), not(target_arch = "wasm32")))]
mod section {
    use super::Entry;
    extern "C" {
        #[cfg_attr(target_vendor = "apple", link_name = "\x01section$start$__DATA$__relax_host")]
        #[cfg_attr(not(target_vendor = "apple"), link_name = "__start_relax_host")]
        static START: u8;
        #[cfg_attr(target_vendor = "apple", link_name = "\x01section$end$__DATA$__relax_host")]
        #[cfg_attr(not(target_vendor = "apple"), link_name = "__stop_relax_host")]
        static STOP: u8;
    }
    // The bounds exist even in a binary with no linked host.
    #[used]
    #[cfg_attr(target_vendor = "apple", link_section = "__DATA,__relax_host")]
    #[cfg_attr(not(target_vendor = "apple"), link_section = "relax_host")]
    static ANCHOR: Entry = super::anchor();
    pub fn entries() -> &'static [Entry] {
        let _ = &ANCHOR;
        unsafe { super::between(core::ptr::addr_of!(START).cast(), core::ptr::addr_of!(STOP).cast()) }
    }
}

#[cfg(all(windows, not(target_arch = "wasm32")))]
mod section {
    use super::Entry;
    // The linker sorts the parts by suffix and merges them into one section.
    #[used]
    #[link_section = ".relaxhost$a"]
    static START: [Entry; 0] = [];
    #[used]
    #[link_section = ".relaxhost$m"]
    static ANCHOR: Entry = super::anchor();
    #[used]
    #[link_section = ".relaxhost$z"]
    static STOP: [Entry; 0] = [];
    pub fn entries() -> &'static [Entry] {
        let _ = &ANCHOR;
        unsafe { super::between(START.as_ptr(), STOP.as_ptr()) }
    }
}

#[cfg(not(target_arch = "wasm32"))]
// The linker bounds enclose the contiguous Entry statics of this binary for its lifetime.
unsafe fn between(start: *const Entry, stop: *const Entry) -> &'static [Entry] {
    let bytes = stop as usize - start as usize;
    let size = core::mem::size_of::<Entry>();
    assert!(bytes % size == 0 && start as usize % core::mem::align_of::<Entry>() == 0,
        "relax_host table has invalid bounds: {bytes} bytes at {start:p} (entry size {size})");
    core::slice::from_raw_parts(start, bytes / size)
}

#[cfg(target_arch = "wasm32")]
mod section {
    pub fn entries() -> &'static [super::Entry] { &[] }
}

/// Selects the binary's host, ignoring the anchors that keep the section present.
pub fn chosen(entries: &[Entry]) -> Result<Option<&Entry>, String> {
    let mut hosts = entries.iter().filter(|entry| entry.apply.is_some());
    let Some(first) = hosts.next() else { return Ok(None) };
    let Some(second) = hosts.next() else { return Ok(Some(first)) };
    let mut names = vec![first.name, second.name];
    names.extend(hosts.map(|entry| entry.name));
    Err(format!("multiple hosts are linked into this binary: {} (one binary, one host)", names.join(", ")))
}

/// Apply the linked host's facts once, before the caller reads them.
#[inline]
pub fn init() {
    static APPLIED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    APPLIED.get_or_init(|| {
        let host = chosen(section::entries()).unwrap_or_else(|message| panic!("{message}"));
        if let Some(apply) = host.and_then(|entry| entry.apply) { apply(); }
    });
}

#[cfg(test)]
#[test]
fn one_binary_has_at_most_one_host() {
    let host = |name| Entry { name, apply: Some(|| {}) };
    let entries = [anchor(), host("makepad"), anchor(), host("other"), host("third")];
    assert!(chosen(&[]).unwrap().is_none(), "an empty section has no host");
    assert!(chosen(&entries[..1]).unwrap().is_none(), "an anchor alone has no host");
    assert!(chosen(&entries[..3]).unwrap().is_some_and(|entry| std::ptr::eq(entry, &entries[1])), "anchors leave the one host selected");
    let error = chosen(&entries[..4]).err().expect("two hosts must be rejected");
    assert!(error.contains("makepad") && error.contains("other"), "the error must name both hosts: {error}");
    let error = chosen(&entries).err().expect("three hosts must be rejected");
    assert!(error.contains("makepad") && error.contains("other") && error.contains("third"), "the error must name every host: {error}");
}
