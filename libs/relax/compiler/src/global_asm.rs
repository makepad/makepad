//! `global_asm!` (blocked by `--no-asm`): the program's module-level assembly, assembled and
//! placed on the first lookup of a foreign symbol (jit.rs foreign_addr), so a program without
//! it pays nothing. As rustc hands all `global_asm!` of a crate to LLVM as one module asm, the
//! blocks of a crate are assembled as one text (each starting in `.text`); its code goes to the
//! JIT's code memory, its data sections to static memory. The labels a block defines are the
//! symbols `extern` blocks and other blocks find by name. A name a block uses but does not
//! define resolves, as a linker would resolve it, to another block's label, a `#[no_mangle]` /
//! `#[export_name]` fn of the program, or a symbol of the host (the loader, its libraries).
//! Branches to symbols out of reach go through veneers placed after the block's text.
//!
//! A crate whose blocks Relax cannot place (an instruction its assembler does not encode, such
//! as EVEX's zmm registers; a symbol nothing defines) is left out on its own, with one line that
//! names the crate, the block and why: the other crates' blocks are placed as before, and so is
//! a crate that uses none of its labels. A call to a symbol a left-out crate defines panics with
//! that reason (jit.rs missing_foreign_stub), as a call to a fn that does not compile does.

use crate::ast::*;
use crate::jit::Unit;
use crate::program::{DefId, DefKind};
use relax_backend::asm::{self as rasm, Arch, Arg, FixKind, Piece, SymRef, Value};
use std::collections::HashMap;

/// The program's global asm, placed.
#[derive(Default)]
pub(crate) struct Placed {
    /// every symbol the placed blocks define, by name
    pub(crate) symbols: HashMap<String, u64>,
    /// a symbol a left-out crate's blocks define (or declare global) -> why it is undefined
    pub(crate) missing: HashMap<String, String>,
}

/// Every symbol the program's global asm defines, by name (placed on first use).
pub(crate) fn symbols(u: &Unit) -> &HashMap<String, u64> {
    &u.global_asm.get_or_init(|| place_all(u)).symbols
}

/// Why `link` is undefined when a left-out crate's global asm defines it (once placed).
pub(crate) fn missing_reason<'a>(u: &'a Unit, link: &str) -> Option<&'a str> {
    u.global_asm.get().and_then(|p| p.missing.get(link)).map(String::as_str)
}

/// A `sym` operand: a fn or static of the program (its address), or a foreign item, which is
/// found by its link name when the blocks are placed -- as a linker would, so it can name a
/// label another block (or the same one) defines.
enum SymTarget {
    Addr(u64),
    Foreign(String),
}

/// One crate's blocks: gathered (their pieces in order, operands renumbered), then assembled.
struct Crate {
    krate: u32,
    pieces: Vec<Piece>,
    args: Vec<Arg>,
    /// sym operand k -> its address
    syms: Vec<SymTarget>,
    att: bool,
    /// per block: the crate text's line of its first template line, its file and position
    blocks: Vec<(u32, u32, u32)>,
    /// the newlines of the crate's text so far
    lines: u32,
    out: Option<rasm::Output>,
    /// why the crate is left out
    failed: Option<String>,
}

/// A placed crate's bytes, resolved but not yet written: its texts, its data words, and the
/// crates whose labels it uses (with one such label).
struct Resolved {
    texts: Vec<(usize, Vec<u8>)>,
    data: Vec<(u64, Vec<u8>)>,
    uses: Vec<(usize, String)>,
}

fn crate_name(prog: &crate::program::Program, krate: u32) -> &str {
    prog.syms.str(prog.crates[krate as usize].name)
}

/// `path:line:col` of a position.
fn at(prog: &crate::program::Program, file: u32, pos: u32) -> String {
    let (line, col) = prog.files[file as usize].line_col(pos);
    format!("{}:{}:{}", prog.file_paths[file as usize], line, col)
}

/// Adds one `global_asm!` block to its crate's text.
fn gather(u: &Unit, c: &mut Crate, file: u32, iid: ItemId, module: u32) -> Result<(), String> {
    let prog = &u.prog;
    let f = &prog.files[file as usize];
    let ItemKind::Mac(m) = &f.ast.item(iid).kind else { return Ok(()) };
    let m = f.ast.row(*m);
    if m.args == NO_MAC_ARGS {
        return Err("arguments not understood".to_string());
    }
    let margs = f.ast.mac_arg(m.args);
    let Some(spec) = margs.asm.get() else { return Err("a template is needed".to_string()) };
    let exprs = f.ast.list(margs.exprs);
    let mut templates = Vec::new();
    for &e in &exprs[..spec.templates as usize] {
        let ex = f.ast.expr(e);
        if !matches!(ex.kind, ExprKind::Lit(..)) {
            return Err("a template must be a string literal (or a macro that makes one)".to_string());
        }
        templates.push(crate::lit::unescape_str(&String::from_utf8_lossy(f.bytes(ex.lo, ex.hi))));
    }
    let ops: Vec<AsmOp> = f.ast.list(spec.ops).to_vec();
    if c.blocks.is_empty() {
        c.att = spec.options & ASM_ATT_SYNTAX != 0;
    }
    let base = c.args.len() as u32;
    let mut args = Vec::new();
    let mut names = Vec::new();
    for (k, op) in ops.iter().enumerate() {
        if let Some(n) = op.name.get() {
            names.push((f.text(n).to_string(), k as u32));
        }
        let x = op.input.get().ok_or("an operand without a value")?;
        match op.dir {
            AsmDir::Sym => {
                let a = sym_addr(u, file, module, x)?;
                args.push(Arg::Sym(SymRef::Name(format!("\u{1}sym{}", c.syms.len()))));
                c.syms.push(a);
            }
            AsmDir::Const => args.push(Arg::Const(const_int(&**f, x)?)),
            _ => return Err("only `const` and `sym` operands are taken".to_string()),
        }
    }
    let positional = ops.iter().take_while(|op| op.name.is_none()).count() as u32;
    let tpl: Vec<&str> = templates.iter().map(String::as_str).collect();
    let pieces = rasm::parse_template(&tpl, &names, positional, ops.len() as u32, spec.options & ASM_RAW != 0)?;
    // the assembler numbers the lines of the crate's text: one more for every newline before
    let head = if c.pieces.is_empty() { ".text\n" } else { "\n.text\n" };
    c.lines += head.matches('\n').count() as u32;
    c.pieces.push(Piece::Text(head.to_string()));
    c.blocks.push((c.lines + 1, file, f.ast.item(iid).lo));
    for p in pieces {
        c.pieces.push(match p {
            Piece::Op(k, m) => Piece::Op(k + base, m),
            Piece::Text(t) => {
                c.lines += t.matches('\n').count() as u32;
                Piece::Text(t)
            }
        });
    }
    c.args.extend(args);
    Ok(())
}

/// An assembler error of a crate's text, at the block (and its template line) it is in.
fn locate(prog: &crate::program::Program, c: &Crate, e: &rasm::AsmError) -> String {
    let block = c.blocks.iter().rev().find(|b| b.0 <= e.line).or(c.blocks.first());
    match block {
        Some(&(first, file, pos)) if e.line > 0 => format!("{}: {} (template line {})", at(prog, file, pos), e.msg, e.line - first + 1),
        Some(&(_, file, pos)) => format!("{}: {}", at(prog, file, pos), e.msg),
        None => e.msg.clone(),
    }
}

/// The C names a left-out crate defines: its global labels when it assembled, else the names
/// its text declares `.globl` / `.global`.
fn defined_names(c: &Crate) -> Vec<String> {
    if let Some(out) = &c.out {
        return out.defs.iter().filter(|d| d.global).map(|d| c_name(&d.name).to_string()).collect();
    }
    let mut names = Vec::new();
    for p in &c.pieces {
        let Piece::Text(t) = p else { continue };
        let mut words = t.split(|ch: char| ch.is_whitespace() || ch == ',' || ch == ';');
        while let Some(w) = words.next() {
            if w == ".globl" || w == ".global" {
                if let Some(n) = words.find(|n| !n.is_empty()) {
                    names.push(c_name(n).to_string());
                }
            }
        }
    }
    names
}

fn place_all(u: &Unit) -> Placed {
    let prog = &u.prog;
    let mut placed = Placed::default();
    if prog.global_asms.is_empty() {
        return placed;
    }
    let arch = Arch::host();
    let mut crates: Vec<Crate> = Vec::new();
    for &(file, iid, module) in &prog.global_asms {
        let krate = prog.mods[module as usize].krate;
        let ci = match crates.iter().position(|c| c.krate == krate) {
            Some(i) => i,
            None => {
                crates.push(Crate { krate, pieces: Vec::new(), args: Vec::new(), syms: Vec::new(), att: false, blocks: Vec::new(), lines: 0, out: None, failed: None });
                crates.len() - 1
            }
        };
        let c = &mut crates[ci];
        if c.failed.is_none() {
            if let Err(e) = gather(u, c, file, iid, module) {
                c.failed = Some(format!("{}: {}", at(prog, file, prog.files[file as usize].ast.item(iid).lo), e));
            }
        }
    }
    for c in crates.iter_mut().filter(|c| c.failed.is_none()) {
        match rasm::assemble(arch, rasm::Format::host(), rasm::Kind::Global, &c.pieces, &c.args, c.att) {
            Ok(out) => c.out = Some(out),
            Err(e) => c.failed = Some(locate(prog, c, &e)),
        }
    }
    // a global symbol two crates define: the later crate is left out
    let mut owner: HashMap<String, usize> = HashMap::new();
    for ci in 0..crates.len() {
        let Some(out) = crates[ci].out.as_ref().filter(|_| crates[ci].failed.is_none()) else { continue };
        let names: Vec<String> = out.defs.iter().filter(|d| d.global).map(|d| c_name(&d.name).to_string()).collect();
        if let Some(n) = names.iter().find(|n| owner.contains_key(n.as_str())) {
            let other = crate_name(prog, crates[owner[n]].krate).to_string();
            crates[ci].failed = Some(format!("symbol `{}` is defined by crate `{}`'s global_asm! too", n, other));
            continue;
        }
        owner.extend(names.into_iter().map(|n| (n, ci)));
    }
    // place: data sections first (their addresses go into the text), then the text
    let mut bases: Vec<Vec<u64>> = vec![Vec::new(); crates.len()];
    for (ci, c) in crates.iter().enumerate() {
        let Some(out) = c.out.as_ref().filter(|_| c.failed.is_none()) else { continue };
        for s in &out.sections {
            if s.exec {
                bases[ci].push(0);
                continue;
            }
            let a = u.rw_data_obj(s.bytes.len().max(1) as u64, s.align.max(8) as u64, crate::syms::DataKind::Bytes);
            unsafe { std::ptr::copy_nonoverlapping(s.bytes.as_ptr(), a as *mut u8, s.bytes.len()) };
            bases[ci].push(a);
        }
        // text: reserve its place (with room for one veneer per symbol branch) so labels have
        // addresses before any byte is written
        for (si, s) in out.sections.iter().enumerate() {
            if s.exec {
                let n_br = out.fixups.iter().filter(|f| f.section == si as u32 && matches!(f.kind, FixKind::X64Branch32 | FixKind::A64Branch26)).count();
                let len = s.bytes.len() + 16 * n_br;
                bases[ci][si] = u.reserve_asm_code(len as u64, s.align.max(16) as u64);
            }
        }
    }
    let mut table: HashMap<String, (u64, usize)> = HashMap::new();
    for (ci, c) in crates.iter().enumerate() {
        let Some(out) = c.out.as_ref().filter(|_| c.failed.is_none()) else { continue };
        for d in out.defs.iter().filter(|d| d.global) {
            table.insert(c_name(&d.name).to_string(), (bases[ci][d.section as usize] + d.offset as u64, ci));
        }
    }
    // what crates left out so far define: a crate that uses one of those names is left out too
    let mut left: HashMap<String, String> = HashMap::new();
    for c in crates.iter().filter(|c| c.failed.is_some()) {
        left.extend(defined_names(c).into_iter().map(|n| (n, crate_name(prog, c.krate).to_string())));
    }
    let exports = exported_fns(&u.prog);
    let mut resolved: Vec<Option<Resolved>> = (0..crates.len()).map(|_| None).collect();
    for ci in 0..crates.len() {
        let c = &crates[ci];
        let Some(out) = c.out.as_ref().filter(|_| c.failed.is_none()) else { continue };
        match resolve_crate(u, c, out, &bases[ci], &table, &left, &exports) {
            Ok(r) => resolved[ci] = Some(r),
            Err(e) => crates[ci].failed = Some(e),
        }
    }
    // a crate that uses a label of a left-out crate is left out too (its branch would land in
    // code never written)
    loop {
        let mut more = false;
        for ci in 0..crates.len() {
            let Some(r) = &resolved[ci] else { continue };
            if crates[ci].failed.is_some() {
                continue;
            }
            if let Some((other, name)) = r.uses.iter().find(|(o, _)| crates[*o].failed.is_some()) {
                crates[ci].failed = Some(format!("it uses `{}`, which crate `{}`'s left-out global_asm! defines", name, crate_name(prog, crates[*other].krate)));
                more = true;
            }
        }
        if !more {
            break;
        }
    }
    for (ci, c) in crates.iter().enumerate() {
        if let Some(why) = &c.failed {
            let name = crate_name(prog, c.krate);
            eprintln!("relax run: global_asm! of crate `{}` left out (its symbols stay undefined): {}", name, why);
            for n in defined_names(c) {
                let reason = format!("`{}` is not defined: crate `{}`'s global_asm! that defines it is left out ({})", n, name, why);
                placed.missing.insert(n, reason);
            }
            continue;
        }
        let Some(r) = resolved[ci].take() else { continue };
        for (pc, b) in r.data {
            unsafe { std::ptr::copy_nonoverlapping(b.as_ptr(), pc as *mut u8, b.len()) };
        }
        for (si, bytes) in r.texts {
            u.write_reserved_code(bases[ci][si], &bytes);
        }
    }
    for (name, (a, ci)) in table {
        if crates[ci].failed.is_none() {
            placed.symbols.insert(name, a);
        }
    }
    placed
}

/// A placed crate's fixups, resolved against the program's global asm symbols (`table`; `left`:
/// the names left-out crates define), its exported fns and the host; nothing is written yet.
fn resolve_crate(u: &Unit, c: &Crate, out: &rasm::Output, bases: &[u64], table: &HashMap<String, (u64, usize)>, left: &HashMap<String, String>, exports: &HashMap<String, DefId>) -> Result<Resolved, String> {
    let arch = Arch::host();
    let local: HashMap<&str, u64> = out.defs.iter().map(|d| (d.name.as_str(), bases[d.section as usize] + d.offset as u64)).collect();
    let mut uses: Vec<(usize, String)> = Vec::new();
    let mut resolve = |name: &str| -> Result<u64, String> {
        let mut name = name;
        if let Some(k) = name.strip_prefix("\u{1}sym") {
            match &c.syms[k.parse::<usize>().unwrap()] {
                SymTarget::Addr(a) => return Ok(*a),
                SymTarget::Foreign(link) => name = link,
            }
        } else if let Some(&a) = local.get(name) {
            return Ok(a);
        }
        // from here on the C name (Mach-O symbols carry a leading underscore)
        let name = c_name(name);
        if let Some(&(a, ci)) = table.get(name) {
            uses.push((ci, name.to_string()));
            return Ok(a);
        }
        if let Some(k) = left.get(name) {
            return Err(format!("it uses `{}`, which crate `{}`'s left-out global_asm! defines", name, k));
        }
        if let Some(&d) = exports.get(name) {
            return fn_addr(u, d);
        }
        if let Some(a) = crate::jit::loader_symbol(name.as_bytes()) {
            return Ok(a as u64);
        }
        if let Some(a) = crate::run::native_definition(name) {
            return Ok(a);
        }
        let z = format!("{}\0", name);
        let p = unsafe { crate::os::proc::find(z.as_ptr()) };
        if p.is_null() {
            return Err(format!("undefined symbol `{}`", name));
        }
        Ok(p as u64)
    };
    let mut texts: Vec<(usize, Vec<u8>)> = out.sections.iter().enumerate().filter(|(_, s)| s.exec).map(|(i, s)| (i, s.bytes.clone())).collect();
    let mut data = Vec::new();
    for f in &out.fixups {
        let target = match &f.target {
            Value::Sym(SymRef::Name(n), off) => resolve(n)?.wrapping_add(*off as u64),
            Value::Sym(SymRef::Sym(_), _) => return Err("an unexpected Relax symbol".to_string()),
            Value::Label(rasm::LabelId::Named(n), off) => match local.get(n.as_str()) {
                Some(&a) => a.wrapping_add(*off as u64),
                None => resolve(n)?.wrapping_add(*off as u64),
            },
            Value::Label(..) => return Err("a numeric label used across sections".to_string()),
            Value::Const(k) => *k as u64,
        };
        let base = bases[f.section as usize];
        let pc = base + f.at as u64;
        if out.sections[f.section as usize].exec {
            let ti = texts.iter().position(|t| t.0 == f.section as usize).unwrap();
            let bytes = &mut texts[ti].1;
            let r = rasm::patch(bytes, f.at as usize, f.kind, target as i64, pc as i64);
            if r.is_err() && matches!(f.kind, FixKind::X64Branch32 | FixKind::A64Branch26) {
                // out of reach: a veneer after the text
                let v = bytes.len();
                veneer(arch, bytes, target);
                rasm::patch(bytes, f.at as usize, f.kind, (base + v as u64) as i64, pc as i64)?;
            } else {
                r?;
            }
        } else {
            // data: the word as placed, patched (written when the crate is placed)
            let n = if f.kind == FixKind::Abs64 { 8 } else { 4 };
            let mut b = vec![0u8; 8];
            unsafe { std::ptr::copy_nonoverlapping(pc as *const u8, b.as_mut_ptr(), n) };
            rasm::patch(&mut b, 0, f.kind, target as i64, pc as i64)?;
            b.truncate(n);
            data.push((pc, b));
        }
    }
    Ok(Resolved { texts, data, uses })
}

/// The C name of an assembly symbol: Mach-O prefixes C names with `_` (dlsym, extern blocks and
/// `#[no_mangle]` know them without it).
pub(crate) fn c_name(n: &str) -> &str {
    if cfg!(target_vendor = "apple") {
        n.strip_prefix('_').unwrap_or(n)
    } else {
        n
    }
}

/// A jump to `target` from anywhere (x64: jmp [rip + 0]; .quad target -- arm64: ldr x16, 8;
/// br x16; .quad target -- IP0, as AAPCS64 lets veneers use it).
fn veneer(arch: Arch, b: &mut Vec<u8>, target: u64) {
    match arch {
        #[cfg(not(target_arch = "aarch64"))]
        Arch::X64 => {
            b.extend_from_slice(&[0xff, 0x25, 0, 0, 0, 0]);
            b.extend_from_slice(&target.to_le_bytes());
            b.extend_from_slice(&[0xcc, 0xcc]);
        }
        #[cfg(target_arch = "aarch64")]
        Arch::Arm64 => {
            b.extend_from_slice(&0x5800_0050u32.to_le_bytes());
            b.extend_from_slice(&0xd61f_0200u32.to_le_bytes());
            b.extend_from_slice(&target.to_le_bytes());
        }
    }
}

/// The program's `#[no_mangle]` / `#[export_name]` fns by symbol.
pub(crate) fn exported_fns(prog: &crate::program::Program) -> HashMap<String, DefId> {
    exported_fns_of(prog, |_| true)
}

/// `exported_fns` of the crates `in_crate` keeps (a domain's own).
pub(crate) fn exported_fns_of(prog: &crate::program::Program, in_crate: impl Fn(u32) -> bool) -> HashMap<String, DefId> {
    let mut out = HashMap::new();
    for (di, d) in prog.defs.iter().enumerate() {
        if d.kind != DefKind::Fn || !in_crate(d.krate) {
            continue;
        }
        let f = &prog.files[d.file as usize];
        for a in f.ast.list(f.ast.item(d.item).attrs) {
            let toks: Vec<&str> = (a.toks.lo..a.toks.hi).map(|t| f.tok_text(t)).collect();
            if toks.contains(&"no_mangle") {
                out.insert(prog.name(DefId(di as u32)).to_string(), DefId(di as u32));
            } else if let Some(k) = toks.iter().position(|t| *t == "export_name") {
                if let (Some(&"="), Some(lit)) = (toks.get(k + 1), toks.get(k + 2)) {
                    out.insert(lit.trim_matches('"').to_string(), DefId(di as u32));
                }
            }
        }
    }
    out
}

/// Symbol name of a foreign item: `#[link_name = ".."]` or its own name.
pub(crate) fn foreign_link_name(prog: &crate::program::Program, d: DefId) -> String {
    let def = prog.def(d);
    let f = &prog.files[def.file as usize];
    for a in f.ast.list(f.ast.item(def.item).attrs) {
        if a.toks.hi >= a.toks.lo + 3 && f.tok_text(a.toks.lo) == "link_name" && f.tok_text(a.toks.lo + 1) == "=" {
            let name = crate::lit::unescape_str(f.tok_text(a.toks.lo + 2));
            // LLVM's leading SOH requests an exact object-file symbol. Our lookups use
            // C names, so undo Mach-O's ABI prefix only for these exact names.
            if let Some(exact) = name.strip_prefix('\u{1}') {
                return c_name(exact).to_string();
            }
            return name;
        }
    }
    prog.name(d).to_string()
}

/// The symbols the extern blocks of the crates `in_crate` keeps import (fns and statics): what a
/// linker resolves against the program's static libraries.
pub(crate) fn foreign_names_of(prog: &crate::program::Program, in_crate: impl Fn(u32) -> bool) -> std::collections::HashSet<String> {
    (0..prog.defs.len() as u32).map(DefId)
        .filter(|&d| matches!(prog.def(d).kind, DefKind::ForeignFn | DefKind::ForeignStatic) && in_crate(prog.def(d).krate))
        .map(|d| foreign_link_name(prog, d))
        .collect()
}

/// A fn item's callable address (its slot; compiled on its first call).
pub(crate) fn fn_addr(u: &Unit, d: DefId) -> Result<u64, String> {
    let instance = crate::instance::Instance::resolve_for_fn_ptr(&u.tcx, &u.prog, d, &[]).map_err(|_| format!("`{}`: a generic fn cannot be a global_asm! symbol", u.prog.def_path(d)))?;
    Ok(u.entry(u.instance_fn_id(instance)))
}

/// A `sym` operand's address: a fn (its slot), a static (its memory), a foreign item.
fn sym_addr(u: &Unit, file: u32, module: u32, x: ExprId) -> Result<SymTarget, String> {
    let prog = &u.prog;
    let f = &prog.files[file as usize];
    let ExprKind::Path(p) = &f.ast.expr(x).kind else { return Err("a `sym` operand must be a path".to_string()) };
    let segs = crate::program::path_segs(prog, file, p);
    let d = if segs.len() == 1 && !p.global() {
        prog.lookup_in_scope(module, segs[0].1, false)
    } else {
        prog.resolve_mod_path(module, p.global(), &segs[..segs.len() - 1], false, false).and_then(|c| prog.lookup_in_container(c, segs[segs.len() - 1].1, false))
    };
    let d = d.ok_or_else(|| format!("cannot find `{}`", String::from_utf8_lossy(f.bytes(f.ast.expr(x).lo, f.ast.expr(x).hi))))?;
    match prog.def(d).kind {
        DefKind::ForeignFn | DefKind::ForeignStatic => Ok(SymTarget::Foreign(u.link_name(d))),
        DefKind::Fn => fn_addr(u, d).map(SymTarget::Addr),
        DefKind::Static => Ok(SymTarget::Addr(u.static_addr_shared(d))),
        _ => Err("a `sym` operand must name a function or a static".to_string()),
    }
}

/// A `const` operand: an integer literal (possibly negated).
fn const_int(f: &crate::parser::ParsedFile, x: ExprId) -> Result<i128, String> {
    let text = String::from_utf8_lossy(f.bytes(f.ast.expr(x).lo, f.ast.expr(x).hi)).replace('_', "");
    let (neg, t) = match text.strip_prefix('-') {
        Some(r) => (true, r.trim().to_string()),
        None => (false, text.trim().to_string()),
    };
    let mut t = t.as_str();
    for suf in ["usize", "isize", "u128", "i128", "u64", "i64", "u32", "i32", "u16", "i16", "u8", "i8"] {
        if let Some(r) = t.strip_suffix(suf) {
            t = r;
            break;
        }
    }
    let v = if let Some(h) = t.strip_prefix("0x") { i128::from_str_radix(h, 16) } else if let Some(b) = t.strip_prefix("0b") { i128::from_str_radix(b, 2) } else if let Some(o) = t.strip_prefix("0o") { i128::from_str_radix(o, 8) } else { t.parse::<i128>() };
    let v = v.map_err(|_| format!("a `const` operand must be an integer literal here, found `{}`", text))?;
    Ok(if neg { -v } else { v })
}
