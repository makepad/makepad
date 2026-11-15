//! Link sections, laid out as a linker lays out an output section: the statics a program places
//! in one `#[link_section]` share one block, in order, each at its own alignment, and the bounds a
//! linker defines for the section are that block's ends -- ELF's `__start_<name>` /
//! `__stop_<name>` (for a name that is a C identifier), Mach-O's `section$start$<seg>$<sect>` /
//! `section$end$<seg>$<sect>` (a `\x01` link name: the exact symbol). A table the linker collects
//! (relax_api's `#[relax_api]` entries, `linkme`-style slices) reads the same entries as
//! under rustc. COFF defines no bounds: a section `name$part` joins `name`, its parts sorted by
//! their names, so a table's `$a` and `$z` marker statics enclose its `$m` entries.
//!
//! Constructor sections run before the program's entry, as the platform's start code runs them:
//! ELF's `.preinit_array`, `.init_array.<priority>` and `.init_array`, Mach-O's `__mod_init_func`,
//! COFF's `.CRT$XC*` (`run_constructors`). The tables the JIT does not run -- terminators, TLS
//! callbacks, C initializers, `.ctors` -- and constructors that take arguments are refused before
//! the entry runs, never skipped.
//!
//! A section is laid out whole when the program first names one of its statics or bounds: every
//! static of the program's current files that names it (rustc keeps a `#[used]` one however
//! unreferenced; the others are kept here too). The block is fixed from then on, so an edit that
//! adds, removes or moves one of its statics restarts the app (`edit_moves_a_section`).
//!
//! The statics of a crate the loader provides (bound: provided.rs) are not among them. They are
//! the loader's, in the loader's own section where its linker placed them, as that crate's other
//! statics are rustc's. A copy laid out from source would be a second one whose code is the
//! crate's own (a `#[relax_api]` entry's key memo, a handle's field offsets), which JIT code may
//! not run. The `#[relax_api]` table is the one such section a program reads: where the crate's
//! entries would lie, its block holds the loader's own (`relax_api::Entry` is `repr(C)`, the
//! program's checked to be laid out as the loader's), so `relax_api::entries()` lists what it
//! lists under rustc. A program that is a Relax host itself (a runner's test running its runner
//! under a test harness, which binds the CPU budget) binds those crates to the loader's code, as
//! the loader does, rather than compiling them again.

use crate::ast::{ItemId, ItemKind};
use crate::jit::Unit;
use crate::parser::ParsedFile;
use crate::program::{DefId, DefKind};
use crate::types::TyKind;

/// A laid-out output section: [start, end) and its statics in layout order.
pub(crate) struct Section {
    pub(crate) start: u64,
    pub(crate) end: u64,
    pub(crate) members: Vec<DefId>,
}

/// The `#[link_section]` item `item` names (its `cfg_attr`s applied), if any.
fn item_section(f: &ParsedFile, item: ItemId) -> Option<String> {
    for a in f.ast.list(f.ast.item(item).attrs) {
        if a.toks.hi >= a.toks.lo + 3 && f.tok_text(a.toks.lo) == "link_section" && f.tok_text(a.toks.lo + 1) == "=" {
            return Some(crate::lit::unescape_str(f.tok_text(a.toks.lo + 2)));
        }
    }
    None
}

/// The output section a `#[link_section]` lands in: Mach-O's `segment,section` (the attributes
/// after them dropped), COFF's name before `$`, ELF's whole name.
fn group_of(section: &str) -> &str {
    if cfg!(target_vendor = "apple") {
        match section.match_indices(',').nth(1) {
            Some((i, _)) => &section[..i],
            None => section,
        }
    } else if cfg!(windows) {
        section.split('$').next().unwrap_or(section)
    } else {
        section
    }
}

/// The section and end (`true`: the end) a linker-defined bound symbol names; `link` is the
/// `#[link_name]` as written (None: the item's own name).
fn bound_of(link: &str) -> Option<(String, bool)> {
    if cfg!(target_vendor = "apple") {
        // only the exact symbol: without `\x01` the C name gets Mach-O's `_` and names no bound
        let rest = link.strip_prefix("\u{1}section$")?;
        let (end, rest) = match rest.strip_prefix("start$") {
            Some(r) => (false, r),
            None => (true, rest.strip_prefix("end$")?),
        };
        let (seg, sect) = rest.split_once('$')?;
        Some((format!("{},{}", seg, sect), end))
    } else if cfg!(windows) {
        None
    } else {
        let link = link.strip_prefix('\u{1}').unwrap_or(link);
        let (end, name) = match link.strip_prefix("__start_") {
            Some(n) => (false, n),
            None => (true, link.strip_prefix("__stop_")?),
        };
        let ident = name.bytes().next().is_some_and(|b| b == b'_' || b.is_ascii_alphabetic()) && name.bytes().all(|b| b == b'_' || b.is_ascii_alphanumeric());
        ident.then(|| (name.to_string(), end))
    }
}

/// Foreign static `d`'s `#[link_name]` as written, else its name.
fn written_link_name(u: &Unit, d: DefId) -> String {
    let def = u.prog.def(d);
    let f = &u.prog.files[def.file as usize];
    for a in f.ast.list(f.ast.item(def.item).attrs) {
        if a.toks.hi >= a.toks.lo + 3 && f.tok_text(a.toks.lo) == "link_name" && f.tok_text(a.toks.lo + 1) == "=" {
            return crate::lit::unescape_str(f.tok_text(a.toks.lo + 2));
        }
    }
    u.prog.name(d).to_string()
}

impl Unit {
    /// The output section static `d` is placed in (`None` with it), or foreign static `d` bounds
    /// (`Some(end)`). A wasm32 program has none: its link sections are custom sections, not memory.
    pub(crate) fn section_place(&self, d: DefId) -> Option<(String, Option<bool>)> {
        if self.prog.ptr_bytes == 4 {
            return None;
        }
        let def = self.prog.def(d);
        match def.kind {
            DefKind::Static => item_section(&self.prog.files[def.file as usize], def.item).map(|s| (group_of(&s).to_string(), None)),
            DefKind::ForeignStatic => bound_of(&written_link_name(self, d)).map(|(s, end)| (group_of(&s).to_string(), Some(end))),
            _ => None,
        }
    }

    /// Whether foreign static `d` is a section bound the linker defines: it is the program's
    /// own static memory, read like a static (lowering, `static_addr`).
    pub(crate) fn is_section_bound(&self, d: DefId) -> bool {
        self.prog.def(d).kind == DefKind::ForeignStatic && matches!(self.section_place(d), Some((_, Some(_))))
    }
}

/// Lays out output section `group` (once): every static of the program's current files placed
/// in it, in one block. Their addresses are all recorded before any initializer runs, as for
/// any static (statics may name each other's addresses).
fn lay_out(u: &mut Unit, group: &str) -> (u64, u64) {
    if let Some(s) = u.sections.get(group) {
        return (s.start, s.end);
    }
    // the current versions only: a replaced file's defs stay for old frames
    let current: std::collections::HashSet<u32> = u.prog.mods.iter().map(|m| m.file).collect();
    // (part, 0, the program's static) or (part, 1, a provided crate's index in u.provided): a
    // provided crate's `#[relax_api]` entries, the loader's, where its link puts the crate's own
    let mut members: Vec<(String, u8, u32)> = Vec::new();
    let mut provided: Vec<(String, usize, DefId)> = Vec::new();
    for (i, def) in u.prog.defs.iter().enumerate() {
        if def.kind != DefKind::Static || !current.contains(&def.file) {
            continue;
        }
        let Some(s) = item_section(&u.prog.files[def.file as usize], def.item) else { continue };
        if group_of(&s) != group {
            continue;
        }
        match u.provided.iter().position(|(k, _)| *k == def.krate) {
            None => members.push((s, 0, i as u32)),
            Some(p) => provided.push((s, p, DefId(i as u32))),
        }
    }
    // per provided crate, its first static that is an entry decides (its layout checked once)
    let mut judged: Vec<usize> = Vec::new();
    for (s, p, d) in provided {
        if judged.contains(&p) {
            continue;
        }
        if let Some(same) = loader_table(u, d, p) {
            judged.push(p);
            if same {
                members.push((s, 1, p as u32));
            }
        }
    }
    // COFF orders a group's parts by name; within a part (and on ELF and Mach-O) the program's
    // definition order stands for the link order, the provided crates' entries after its own
    members.sort();
    let mut laid = Vec::new();
    let mut copied: Vec<(u64, usize)> = Vec::new();
    let (mut size, mut align) = (0u64, 1u64);
    for &(_, from, x) in &members {
        if from == 1 {
            let (n, a) = (u.provided[x as usize].1.table.len() as u64, std::mem::align_of::<relax_api::Entry>() as u64);
            size = size.div_ceil(a) * a;
            copied.push((size, x as usize));
            size += n * std::mem::size_of::<relax_api::Entry>() as u64;
            align = align.max(a);
            continue;
        }
        let d = DefId(x);
        let Some(t) = u.tcx.const_ty(&u.prog, d) else {
            // its type's error is its own; it takes no room
            crate::zone_alloc::kept_with(|| u.statics.insert(d, 0));
            continue;
        };
        let l = u.layout_of(t);
        let a = (l.align as u64).max(1);
        size = size.div_ceil(a) * a;
        laid.push((d, t, size, l.size as u64));
        size += l.size as u64;
        align = align.max(a);
    }
    // one byte past the end: the end bound is an address inside the block
    let base = u.rw_data(size + 1, align.max(8));
    for &(off, p) in &copied {
        for (i, e) in u.provided[p].1.table.iter().enumerate() {
            let at = base + off + (i * std::mem::size_of::<relax_api::Entry>()) as u64;
            // SAFETY: [at, at + size_of Entry) lies in the block just allocated for it, and `e`
            // is a whole entry of the loader's table
            unsafe { std::ptr::copy_nonoverlapping(*e as *const relax_api::Entry as *const u8, at as *mut u8, std::mem::size_of::<relax_api::Entry>()) };
        }
    }
    crate::zone_alloc::kept_with(|| {
        for &(d, _, off, len) in &laid {
            u.statics.insert(d, base + off);
            u.static_spans.insert(base + off, (d, len));
        }
        u.sections.insert(group.to_string(), Section { start: base, end: base + size, members: laid.iter().map(|m| m.0).collect() });
    });
    for &(d, t, off, len) in &laid {
        u.init_static(d, t, base + off, len);
    }
    (base, base + size)
}

/// Whether provided crate `u.provided[p]`'s static `d` is one of its `#[relax_api]` entries (a
/// `relax_api::Entry`; None: it is not), and the program's `relax_api` laid out as the loader's
/// (Some(true)): its block then takes the loader's entries of the crate (`ProvidedCrate::table`).
/// Another static of a provided crate stays the loader's alone (`place`). A program whose `Entry`
/// is laid out otherwise (another relax_api than the loader's) is told so (Some(false)): the
/// loader's entries would not read as its own.
fn loader_table(u: &mut Unit, d: DefId, p: usize) -> Option<bool> {
    use relax_api::Entry;
    let t = u.tcx.const_ty(&u.prog, d)?;
    let TyKind::Adt(adt, _, _) = u.tcx.tys.kind(t).clone() else { return None };
    if u.prog.def_path(adt) != std::any::type_name::<Entry>() {
        return None;
    }
    let l = u.layout_of(t);
    let rustc = [
        std::mem::offset_of!(Entry, kind),
        std::mem::offset_of!(Entry, module),
        std::mem::offset_of!(Entry, key),
        std::mem::offset_of!(Entry, sig),
        std::mem::offset_of!(Entry, shim),
        std::mem::offset_of!(Entry, slots),
        std::mem::offset_of!(Entry, size),
        std::mem::offset_of!(Entry, align),
        std::mem::offset_of!(Entry, offsets),
        std::mem::offset_of!(Entry, drop),
    ];
    if l.size as usize == std::mem::size_of::<Entry>() && l.align as usize == std::mem::align_of::<Entry>() && l.fields.iter().map(|&f| f as usize).eq(rustc) {
        return Some(true);
    }
    let (size, align, fields) = (l.size, l.align, l.fields.clone());
    let def = u.prog.def(d);
    let at = u.pos_str(def.file, u.prog.files[def.file as usize].ast.item(def.item).lo);
    u.note_error(format!("{}: `{}` is a #[relax_api] entry of provided crate `{}`, but this program's `relax_api::Entry` is laid out otherwise than the loader's (size {} align {} fields {:?}, the loader's {} {} {:?}): its relax_api is not the loader's -- rebuild makepad from this tree", at, u.prog.def_path(d), u.provided[p].1.name, size, align, fields, std::mem::size_of::<Entry>(), std::mem::align_of::<Entry>(), rustc));
    Some(false)
}

/// The address of static or bound `d` of output section `group` (`bound`: which end), the section
/// laid out first. 0 for a static the section was laid out without (an error, as the edit that
/// added it restarts the app).
pub(crate) fn place(u: &mut Unit, d: DefId, group: &str, bound: Option<bool>) -> u64 {
    let (start, end) = lay_out(u, group);
    let a = match bound {
        Some(false) => start,
        Some(true) => end,
        None => {
            if let Some(&a) = u.statics.get(&d) {
                return a;
            }
            let def = u.prog.def(d);
            let at = u.pos_str(def.file, u.prog.files[def.file as usize].ast.item(def.item).lo);
            if u.provided.iter().any(|(k, _)| *k == def.krate) {
                u.note_error(format!("{}: static `{}` is read from JIT-compiled code, but it is in link section `{}` of a provided crate (built by rustc into the loader): its one copy is the loader's, which JIT code reaches only through the crate's ops (#[relax_api])", at, u.prog.def_path(d), group));
            } else {
                u.note_error(format!("{}: static `{}` is in link section `{}`, which this run laid out without it -- restart the app to lay the section out again", at, u.prog.def_path(d), group));
            }
            0
        }
    };
    crate::zone_alloc::kept_with(|| u.statics.insert(d, a));
    a
}

/// Runs again the initializers of bound `d`'s section that waited for the post-boot loader: the
/// section's statics are often named by nothing but the table read between its bounds.
pub(crate) fn retry_deferred(u: &mut Unit, d: DefId) {
    let Some((group, Some(_))) = u.section_place(d) else { return };
    let waiting: Vec<DefId> = match u.sections.get(&group) {
        Some(s) => s.members.iter().copied().filter(|&m| crate::jit::static_deferred(m)).collect(),
        None => return,
    };
    for m in waiting {
        u.static_addr(m);
    }
}

/// An edit of file `old` (new text `new`) that adds, removes or moves a static of a link section
/// this run laid out: the section is one block holding the old set, so the new one needs a fresh
/// layout. The reason, for the restart rung.
pub(crate) fn edit_moves_a_section(u: &Unit, old: u32, new: &ParsedFile) -> Option<String> {
    if u.sections.is_empty() {
        return None;
    }
    let statics = |f: &ParsedFile| {
        let mut v: Vec<(Vec<u8>, String)> = Vec::new();
        for (i, it) in (&f.ast.items).into_iter().enumerate() {
            if !matches!(it.kind, ItemKind::Static(..)) {
                continue;
            }
            if let Some(s) = item_section(f, ItemId(i as u32)) {
                v.push((f.bytes(it.name.lo, it.name.hi).to_vec(), group_of(&s).to_string()));
            }
        }
        v.sort();
        v
    };
    let (was, now) = (statics(&u.prog.files[old as usize]), statics(new));
    if was == now {
        return None;
    }
    let moved = was.iter().filter(|x| !now.contains(x)).chain(now.iter().filter(|x| !was.contains(x)));
    for (name, group) in moved {
        if u.sections.contains_key(group) {
            return Some(format!("{}:1:1: static `{}` joins or leaves link section `{}`, which the running program laid out as one block -- restart the app", u.prog.file_paths[old as usize], String::from_utf8_lossy(name), group));
        }
    }
    None
}

/// What the platform's start code does with section `name`: runs its fn pointers before the
/// entry, ordered by (rank, key), or nothing the JIT does (why it refuses it).
enum Startup {
    Run(u32, String),
    Refuse(&'static str),
}

fn startup_of(name: &str) -> Option<Startup> {
    use Startup::*;
    if cfg!(target_vendor = "apple") {
        match name.split(',').nth(1).map(str::trim)? {
            "__mod_init_func" => Some(Run(0, String::new())),
            "__mod_term_func" => Some(Refuse("a terminator table (run at exit)")),
            _ => None,
        }
    } else if cfg!(windows) {
        let part = name.strip_prefix(".CRT$")?;
        match part.as_bytes().get(..2)? {
            // `_initterm` over [__xc_a, __xc_z): the parts in name order
            b"XC" => Some(Run(0, part.to_string())),
            b"XI" => Some(Refuse("a C initializer table (its int result can stop the start)")),
            b"XL" => Some(Refuse("a TLS callback table (run at thread start and end)")),
            b"XP" | b"XT" => Some(Refuse("a terminator table (run at exit)")),
            _ => None,
        }
    } else if name == ".preinit_array" {
        Some(Run(0, String::new()))
    } else if let Some(rest) = name.strip_prefix(".init_array") {
        // ld: SORT_BY_INIT_PRIORITY(.init_array.*) before the plain .init_array
        match rest.strip_prefix('.').map(|p| p.parse::<u32>()) {
            None if rest.is_empty() => Some(Run(2, String::new())),
            Some(Ok(n)) => Some(Run(1, format!("{:010}", n))),
            _ => None,
        }
    } else if name.starts_with(".fini_array") {
        Some(Refuse("a terminator table (run at exit)"))
    } else if name.starts_with(".ctors") || name.starts_with(".dtors") {
        Some(Refuse("a legacy constructor table"))
    } else {
        None
    }
}

/// Whether `t` is a constructor table entry the JIT can call: a fn pointer taking nothing, or an
/// array of them.
fn plain_ctor(u: &Unit, t: crate::types::TyId) -> bool {
    match u.tcx.tys.kind(t).clone() {
        TyKind::FnPtr(ps, ..) => u.tcx.tys.list(ps).is_empty(),
        TyKind::Array(e, _) => matches!(u.tcx.tys.kind(e), TyKind::FnPtr(ps, ..) if u.tcx.tys.list(*ps).is_empty()),
        _ => false,
    }
}

/// The def paths of the constructor tables the program crate `root` links (`constructors`).
pub(crate) unsafe fn constructor_paths(up: *mut Unit, root: u32) -> Result<Vec<String>, String> {
    let list = crate::owner::call(crate::owner::Class::Program, || constructors(&mut *up, root))?;
    Ok(crate::owner::call(crate::owner::Class::Program, || list.iter().map(|(d, _)| (&*up).prog.def_path(*d)).collect()))
}

/// The constructors of the program crate `root` links (its dependency closure, proc-macro crates
/// not), in the order the platform's start code calls them: (static, fn address). Err: the tables
/// it has that the JIT does not run, each named.
fn constructors(u: &mut Unit, root: u32) -> Result<Vec<(DefId, u64)>, String> {
    if u.prog.ptr_bytes == 4 {
        return Ok(Vec::new());
    }
    let mut linked = vec![false; u.prog.crates.len()];
    let mut next = vec![root];
    while let Some(k) = next.pop() {
        let Some(c) = u.prog.crates.get(k as usize) else { continue };
        if linked[k as usize] || c.proc_macro {
            continue;
        }
        linked[k as usize] = true;
        next.extend(c.deps.iter().map(|d| d.1));
    }
    let current: std::collections::HashSet<u32> = u.prog.mods.iter().map(|m| m.file).collect();
    let mut found: Vec<(u32, String, DefId)> = Vec::new();
    let mut refused = Vec::new();
    for (i, def) in u.prog.defs.iter().enumerate() {
        if def.kind != DefKind::Static || !current.contains(&def.file) || !linked.get(def.krate as usize).copied().unwrap_or(false) {
            continue;
        }
        let f = &u.prog.files[def.file as usize];
        let Some(name) = item_section(f, def.item) else { continue };
        let Some(how) = startup_of(&name) else { continue };
        let d = DefId(i as u32);
        let at = u.pos_str(def.file, f.ast.item(def.item).lo);
        match how {
            Startup::Refuse(why) => refused.push(format!("{}: static `{}` is in `{}`, {}: Relax does not run it -- run the program natively (cargo)", at, u.prog.def_path(d), name, why)),
            Startup::Run(rank, key) => match u.tcx.const_ty(&u.prog, d) {
                Some(t) if plain_ctor(u, t) => found.push((rank, key, d)),
                _ => refused.push(format!("{}: static `{}` is in `{}`, a constructor table, but is not a fn pointer taking nothing (or an array of them): Relax calls only those -- run the program natively (cargo)", at, u.prog.def_path(d), name)),
            },
        }
    }
    if !refused.is_empty() {
        return Err(refused.join("\n"));
    }
    // a part's order is its name's (COFF), then the program's definition order for the link order
    found.sort();
    let mut out = Vec::new();
    for (_, _, d) in found {
        let a = u.static_addr(d);
        if a == 0 {
            return Err(format!("constructor table `{}`: could not evaluate it -- see its own error", u.prog.def_path(d)));
        }
        let size = u.tcx.const_ty(&u.prog, d).map_or(0, |t| u.layout_of(t).size as u64);
        for w in 0..size / 8 {
            // a null entry is skipped, as the C runtime's `_initterm` skips one
            let f = unsafe { std::ptr::read_unaligned((a + 8 * w) as *const u64) };
            if f != 0 {
                out.push((d, f));
            }
        }
    }
    Ok(out)
}

/// Runs the constructors of the program crate `root` links, in domain `domain` (None: the unit's
/// compile domain), before its entry: the platform's start code runs them before `main` (and before
/// a test harness's). Err: a table the JIT refuses, or a constructor that panicked (natively a
/// panic out of an `extern "C"` fn aborts the start too).
pub(crate) unsafe fn run_constructors(up: *mut Unit, root: u32, domain: Option<u64>) -> Result<(), String> {
    let list = crate::owner::call(crate::owner::Class::Program, || constructors(&mut *up, root))?;
    let domain = domain.unwrap_or_else(|| (&*up).syms.dom0());
    for (d, f) in list {
        if let Err(p) = crate::jit::call_jit_in(up, domain, f, 0) {
            let name = crate::owner::call(crate::owner::Class::Program, || (&*up).prog.def_path(d));
            return Err(format!("a constructor in `{}` (before main) {}: {}", name, p.kind, p.message));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::program::{DefId, DefKind};

    /// A provided crate's static in a link section is the loader's: the section's block, laid out
    /// when the program reads the section, leaves it out, and JIT code that names it is told why.
    /// A runner's test (makepad-run's confined_macros under makepad test) binds the CPU budget and
    /// reads the `#[relax_api]` table: its block took the budget's entries from source, whose
    /// initializers JIT code may not run (`offset_of!(Wake, ..)`, a static key memo).
    #[test]
    fn a_provided_crates_section_statics_stay_the_loaders() {
        isolated_test!(a_provided_crates_section_statics_stay_the_loaders);
        crate::borrowck::lazy::disable_gandalf();
        if std::env::var_os("RELAX_STD").is_none() {
            std::env::set_var("RELAX_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
        }
        let dir = std::env::temp_dir().join(format!("relax-provided-section-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("lib.rs");
        std::fs::write(&path, "#[used]\n#[link_section = \"__DATA,__relax_t\"]\npub static ENTRY: u64 = 7;\n").unwrap();
        let (prog, core, krate) = crate::load_with_std(&std::sync::Arc::new(crate::Session::default()), path.to_str().unwrap(), "provided_section", false).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        let entry = (0..prog.defs.len()).map(|i| DefId(i as u32)).find(|&d| prog.def(d).krate == krate && prog.def(d).kind == DefKind::Static && prog.name(d) == "ENTRY").unwrap();
        let mut tcx = crate::tcx::Tcx::new();
        tcx.collect(&prog);
        assert!(tcx.errors.is_empty(), "{:?}", tcx.errors);
        let mut u = crate::jit::Unit::new(prog, tcx, core);
        // the crate provided as the loader provides the CPU budget (no op listed)
        unsafe extern "C" fn no_panic_text(_: *mut *const u8, _: *mut usize) {}
        let fixture = crate::provided::ProvidedCrate { name: "provided_section".into(), shims: Default::default(), types: Default::default(), layouts: Default::default(), per_side: Default::default(), last_panic: no_panic_text, source: 0, binding: 0, manifest: &[], table: Vec::new() };
        u.provided.push((krate, fixture));
        let (group, bound) = u.section_place(entry).expect("ENTRY is in a link section");
        assert_eq!(bound, None);
        let (start, end) = super::lay_out(&mut u, &group);
        assert_eq!(u.sections[&group].members, Vec::<DefId>::new(), "the section's block took the provided crate's static");
        assert_eq!(start, end, "the section's block has room for the provided crate's static");
        assert_eq!(super::place(&mut u, entry, &group, None), 0, "JIT code was given an address for the provided crate's static");
        assert!(u.errors.iter().any(|e| e.contains("static `") && e.contains("ENTRY` is read from JIT-compiled code, but it is in link section") && e.contains("of a provided crate")), "{:?}", u.errors);
        // (process exit owns the runtime's teardown)
        std::mem::forget(u);
    }
}
