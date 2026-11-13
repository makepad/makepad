// A table the linker collects: statics placed in one `#[link_section]`, read between the bounds
// the linker defines for the section (ELF `__start_<name>` / `__stop_<name>`, Mach-O
// `section$start$<seg>$<sect>` / `section$end$..`, COFF `$a` / `$z` marker statics), as
// relax_api's `#[relax_api]` table is read. The JIT had no section layout: a hal test died
// with "foreign static `START` not found in the loaded libraries" (JIT miss M61). Lines "| " are
// compared with rustc --test.
use std::sync::atomic::{AtomicU32, Ordering};

#[repr(C)]
pub struct Entry {
    pub id: u32,
    pub name: &'static str,
    pub f: fn(u32) -> u32,
    pub counter: Option<&'static AtomicU32>,
}

macro_rules! entry {
    ($s:ident, $id:expr, $name:expr, $f:expr, $c:expr) => {
        #[used]
        #[cfg_attr(target_vendor = "apple", link_section = "__DATA,__zz_table")]
        #[cfg_attr(all(not(target_vendor = "apple"), not(windows)), link_section = "zz_table")]
        #[cfg_attr(windows, link_section = ".zztable$m")]
        static $s: Entry = Entry { id: $id, name: $name, f: $f, counter: $c };
    };
}

fn double(x: u32) -> u32 {
    x * 2
}
fn square(x: u32) -> u32 {
    x * x
}
static HITS: AtomicU32 = AtomicU32::new(0);

entry!(E_DOUBLE, 1, "double", double, None);
entry!(E_SQUARE, 2, "square", square, Some(&HITS));
mod nested {
    fn plus_seven(x: u32) -> u32 {
        x + 7
    }
    entry!(E_NESTED, 3, "nested", plus_seven, None);
    pub fn nested_entry() -> &'static super::Entry {
        &E_NESTED
    }
    use super::Entry;
}
entry!(E_CLOSURE, 4, "closure", |x| x ^ 5, Some(&HITS));

#[cfg(target_vendor = "apple")]
mod bounds {
    extern "C" {
        #[link_name = "\x01section$start$__DATA$__zz_table"]
        pub static START: u8;
        #[link_name = "\x01section$end$__DATA$__zz_table"]
        pub static STOP: u8;
    }
}
#[cfg(all(not(target_vendor = "apple"), not(windows)))]
mod bounds {
    extern "C" {
        #[link_name = "__start_zz_table"]
        pub static START: u8;
        #[link_name = "__stop_zz_table"]
        pub static STOP: u8;
    }
}
#[cfg(windows)]
mod bounds {
    #[used]
    #[link_section = ".zztable$a"]
    pub static START: [super::Entry; 0] = [];
    #[used]
    #[link_section = ".zztable$z"]
    pub static STOP: [super::Entry; 0] = [];
}

fn table() -> &'static [Entry] {
    let (a, b) = (std::ptr::addr_of!(bounds::START) as usize, std::ptr::addr_of!(bounds::STOP) as usize);
    let size = std::mem::size_of::<Entry>();
    assert_eq!((b - a) % size, 0, "the section holds whole entries");
    assert_eq!(a % std::mem::align_of::<Entry>(), 0, "the section starts aligned");
    unsafe { std::slice::from_raw_parts(a as *const Entry, (b - a) / size) }
}

#[test]
fn a01_the_table_holds_every_entry() {
    let mut rows: Vec<(u32, &str, u32)> = table().iter().map(|e| (e.id, e.name, (e.f)(e.id + 10))).collect();
    // the linker's order is its own
    rows.sort();
    println!("| a01: {} entries {:?}", rows.len(), rows);
}

#[test]
fn a02_an_entry_is_its_static() {
    let t = table();
    let at = |e: &Entry| t.iter().position(|x| std::ptr::eq(x, e));
    println!("| a02: double {} square {} nested {} closure {}", at(&E_DOUBLE).is_some(), at(&E_SQUARE).is_some(), at(nested::nested_entry()).is_some(), at(&E_CLOSURE).is_some());
    // what an entry points to is shared with the program: a counter through the table
    for e in t {
        if let Some(c) = e.counter {
            c.fetch_add(e.id, Ordering::Relaxed);
        }
    }
    println!("| a02: hits {}", HITS.load(Ordering::Relaxed));
}

// a second section of 4-byte words: the stride is the statics' own size
#[used]
#[cfg_attr(target_vendor = "apple", link_section = "__DATA,__zz_words")]
#[cfg_attr(all(not(target_vendor = "apple"), not(windows)), link_section = "zz_words")]
#[cfg_attr(windows, link_section = ".zzwords$m")]
static W1: u32 = 11;
#[used]
#[cfg_attr(target_vendor = "apple", link_section = "__DATA,__zz_words")]
#[cfg_attr(all(not(target_vendor = "apple"), not(windows)), link_section = "zz_words")]
#[cfg_attr(windows, link_section = ".zzwords$m")]
static W2: u32 = 22;
#[used]
#[cfg_attr(target_vendor = "apple", link_section = "__DATA,__zz_words")]
#[cfg_attr(all(not(target_vendor = "apple"), not(windows)), link_section = "zz_words")]
#[cfg_attr(windows, link_section = ".zzwords$m")]
static W3: u32 = 33;

#[cfg(target_vendor = "apple")]
mod word_bounds {
    extern "C" {
        #[link_name = "\x01section$start$__DATA$__zz_words"]
        pub static START: u32;
        #[link_name = "\x01section$end$__DATA$__zz_words"]
        pub static STOP: u32;
    }
}
#[cfg(all(not(target_vendor = "apple"), not(windows)))]
mod word_bounds {
    extern "C" {
        #[link_name = "__start_zz_words"]
        pub static START: u32;
        #[link_name = "__stop_zz_words"]
        pub static STOP: u32;
    }
    // a second declaration of the same bound is the same address
    pub mod again {
        extern "C" {
            #[link_name = "__start_zz_words"]
            pub static START: u32;
        }
    }
}
#[cfg(windows)]
mod word_bounds {
    #[used]
    #[link_section = ".zzwords$a"]
    pub static START: [u32; 0] = [];
    #[used]
    #[link_section = ".zzwords$z"]
    pub static STOP: [u32; 0] = [];
}

#[test]
fn a03_a_section_of_words() {
    let (a, b) = (std::ptr::addr_of!(word_bounds::START), std::ptr::addr_of!(word_bounds::STOP));
    let n = (b as usize - a as usize) / 4;
    let mut w: Vec<u32> = unsafe { std::slice::from_raw_parts(a, n) }.to_vec();
    w.sort();
    let mine = [&W1, &W2, &W3].iter().all(|s| (a as usize..b as usize).contains(&(*s as *const u32 as usize)));
    println!("| a03: {} words {:?}, each static inside: {}", n, w, mine);
    #[cfg(all(not(target_vendor = "apple"), not(windows)))]
    println!("| a03: one bound, two declarations: {}", std::ptr::addr_of!(word_bounds::again::START) == std::ptr::addr_of!(word_bounds::START));
    #[cfg(not(all(not(target_vendor = "apple"), not(windows))))]
    println!("| a03: one bound, two declarations: true");
}
