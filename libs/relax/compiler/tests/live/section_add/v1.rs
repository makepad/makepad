// Link sections across edits (sections.rs): a section the program laid out is one block holding
// the statics it had, read between its linker bounds.
//  v1: a static joins the section (a macro's, as `#[relax_api]` entries are): the restart rung,
//      naming the section; the running program keeps its table.
#[repr(C)]
pub struct Entry {
    pub id: u32,
}

macro_rules! entry {
    ($s:ident, $id:expr) => {
        #[used]
        #[cfg_attr(target_vendor = "apple", link_section = "__DATA,__zz_live")]
        #[cfg_attr(not(target_vendor = "apple"), link_section = "zz_live")]
        static $s: Entry = Entry { id: $id };
    };
}

entry!(A, 1);
entry!(B, 2);
entry!(C, 3);

#[cfg(target_vendor = "apple")]
extern "C" {
    #[link_name = "\x01section$start$__DATA$__zz_live"]
    static START: u8;
    #[link_name = "\x01section$end$__DATA$__zz_live"]
    static STOP: u8;
}
#[cfg(not(target_vendor = "apple"))]
extern "C" {
    #[link_name = "__start_zz_live"]
    static START: u8;
    #[link_name = "__stop_zz_live"]
    static STOP: u8;
}

fn table() -> &'static [Entry] {
    let (a, b) = (std::ptr::addr_of!(START) as usize, std::ptr::addr_of!(STOP) as usize);
    unsafe { std::slice::from_raw_parts(a as *const Entry, (b - a) / std::mem::size_of::<Entry>()) }
}

#[test]
fn run() {
    let mut ids: Vec<u32> = table().iter().map(|e| e.id).collect();
    ids.sort();
    println!("out entries {:?}", ids);
}
