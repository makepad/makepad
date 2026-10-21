//! `global_asm!` (blocked by `--no-asm`): the program's module-level assembly, assembled and
//! placed on the first lookup of a foreign symbol (jit.rs foreign_addr), so a program without
//! it pays nothing. As rustc hands all `global_asm!` of a crate to LLVM as one module asm, the
//! blocks of a crate are assembled as one text (each starting in `.text`); its code goes to the
//! JIT's code memory, its data sections to static memory. The labels a block defines are the
//! symbols `extern` blocks and other blocks find by name. A name a block uses but does not
//! define resolves, as a linker would resolve it, to another block's label, a `#[no_mangle]` /
//! `#[export_name]` fn of the program, or a symbol of the host (the loader, its libraries).
//! Branches to symbols out of reach go through veneers placed after the block's text.

use crate::ast::*;
use crate::jit::Unit;
use crate::program::{DefId, DefKind};
use relax_backend::asm::{self as rasm, Arch, Arg, FixKind, Piece, SymRef, Value};
use std::collections::HashMap;

/// Every symbol the program's global asm defines, by name (placed on first use).
pub(crate) fn symbols(u: &Unit) -> &HashMap<String, u64> {
    u.global_asm.get_or_init(|| match place_all(u) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("makepad run: global_asm!: {}", e);
            HashMap::new()
        }
    })
}

/// One crate's blocks, assembled.
struct Crate {
    out: rasm::Output,
    /// sym operand k -> its address
    syms: Vec<u64>,
}

fn place_all(u: &Unit) -> Result<HashMap<String, u64>, String> {
    let prog = &u.prog;
    if prog.global_asms.is_empty() {
        return Ok(HashMap::new());
    }
    let arch = Arch::host();
    // per crate: the blocks' pieces in order, operands renumbered
    let mut crates: Vec<(u32, Vec<Piece>, Vec<Arg>, Vec<u64>, bool)> = Vec::new();
    for &(file, iid, module) in &prog.global_asms {
        let krate = prog.mods[module as usize].krate;
        let f = &prog.files[file as usize];
        let path = &prog.file_paths[file as usize];
        let ItemKind::Mac(m) = &f.ast.item(iid).kind else { continue };
        let m = f.ast.row(*m);
        if m.args == NO_MAC_ARGS {
            return Err(format!("{}: global_asm! arguments not understood", path));
        }
        let margs = f.ast.mac_arg(m.args);
        let Some(spec) = margs.asm else { return Err(format!("{}: global_asm! needs a template", path)) };
        let exprs = f.ast.list(margs.exprs);
        let mut templates = Vec::new();
        for &e in &exprs[..spec.templates as usize] {
            let ex = f.ast.expr(e);
            if !matches!(ex.kind, ExprKind::Lit(..)) {
                return Err(format!("{}: a global_asm! template must be a string literal (or a macro that makes one)", path));
            }
            templates.push(crate::lit::unescape_str(&String::from_utf8_lossy(f.bytes(ex.lo, ex.hi))));
        }
        let ops: Vec<AsmOp> = f.ast.list(spec.ops).to_vec();
        let ci = match crates.iter().position(|c| c.0 == krate) {
            Some(i) => i,
            None => {
                crates.push((krate, Vec::new(), Vec::new(), Vec::new(), spec.options & ASM_ATT_SYNTAX != 0));
                crates.len() - 1
            }
        };
        let base = crates[ci].2.len() as u32;
        let mut args = Vec::new();
        let mut names = Vec::new();
        for (k, op) in ops.iter().enumerate() {
            if let Some(n) = op.name {
                names.push((f.text(n).to_string(), k as u32));
            }
            let x = op.input.ok_or("global_asm!: operand without a value")?;
            match op.dir {
                AsmDir::Sym => {
                    let a = sym_addr(u, file, module, x)?;
                    let idx = crates[ci].3.len();
                    crates[ci].3.push(a);
                    args.push(Arg::Sym(SymRef::Name(format!("\u{1}sym{}", idx))));
                }
                AsmDir::Const => args.push(Arg::Const(const_int(&**f, x)?)),
                _ => return Err(format!("{}: global_asm! takes only `const` and `sym` operands", path)),
            }
        }
        let positional = ops.iter().take_while(|op| op.name.is_none()).count() as u32;
        let tpl: Vec<&str> = templates.iter().map(String::as_str).collect();
        let pieces = rasm::parse_template(&tpl, &names, positional, ops.len() as u32, spec.options & ASM_RAW != 0).map_err(|e| format!("{}: {}", path, e))?;
        let c = &mut crates[ci];
        c.1.push(Piece::Text(if c.1.is_empty() { ".text\n".to_string() } else { "\n.text\n".to_string() }));
        for p in pieces {
            c.1.push(match p {
                Piece::Op(k, m) => Piece::Op(k + base, m),
                t => t,
            });
        }
        c.2.extend(args);
    }
    let mut assembled = Vec::new();
    for (krate, pieces, args, syms, att) in crates {
        let out = rasm::assemble(arch, rasm::Format::host(), rasm::Kind::Global, &pieces, &args, att).map_err(|e| format!("crate `{}`: {}", prog.syms.str(prog.crates[krate as usize].name), e))?;
        assembled.push(Crate { out, syms });
    }
    // place: data sections first (their addresses go into the text), then the text
    let mut bases: Vec<Vec<u64>> = Vec::new();
    for c in &assembled {
        let mut b = Vec::new();
        for s in &c.out.sections {
            if s.exec {
                b.push(0);
                continue;
            }
            let a = u.rw_data_obj(s.bytes.len().max(1) as u64, s.align.max(8) as u64, crate::syms::DataKind::Bytes);
            unsafe { std::ptr::copy_nonoverlapping(s.bytes.as_ptr(), a as *mut u8, s.bytes.len()) };
            b.push(a);
        }
        bases.push(b);
    }
    // text: reserve its place (with room for one veneer per symbol branch) so labels have
    // addresses before any byte is written
    for (ci, c) in assembled.iter().enumerate() {
        for (si, s) in c.out.sections.iter().enumerate() {
            if s.exec {
                let n_br = c.out.fixups.iter().filter(|f| f.section == si as u32 && matches!(f.kind, FixKind::X64Branch32 | FixKind::A64Branch26)).count();
                let len = s.bytes.len() + 16 * n_br;
                bases[ci][si] = u.reserve_asm_code(len as u64, s.align.max(16) as u64);
            }
        }
    }
    let mut table: HashMap<String, u64> = HashMap::new();
    for (ci, c) in assembled.iter().enumerate() {
        for d in &c.out.defs {
            if d.global {
                if table.insert(c_name(&d.name).to_string(), bases[ci][d.section as usize] + d.offset as u64).is_some() {
                    return Err(format!("symbol `{}` is defined by two global_asm! blocks", d.name));
                }
            }
        }
    }
    let exports = exported_fns(u);
    for (ci, c) in assembled.iter().enumerate() {
        let local: HashMap<&str, u64> = c.out.defs.iter().map(|d| (d.name.as_str(), bases[ci][d.section as usize] + d.offset as u64)).collect();
        let resolve = |name: &str| -> Result<u64, String> {
            if let Some(k) = name.strip_prefix("\u{1}sym") {
                return Ok(c.syms[k.parse::<usize>().unwrap()]);
            }
            if let Some(&a) = local.get(name) {
                return Ok(a);
            }
            // from here on the C name (Mach-O symbols carry a leading underscore)
            let name = c_name(name);
            if let Some(&a) = table.get(name) {
                return Ok(a);
            }
            if let Some(&d) = exports.get(name) {
                return fn_addr(u, d);
            }
            if let Some(a) = crate::jit::loader_symbol(name.as_bytes()) {
                return Ok(a as u64);
            }
            let z = format!("{}\0", name);
            let p = unsafe { crate::os::proc::find(z.as_ptr()) };
            if p.is_null() {
                return Err(format!("global_asm!: undefined symbol `{}`", name));
            }
            Ok(p as u64)
        };
        let mut texts: Vec<(usize, Vec<u8>)> = c.out.sections.iter().enumerate().filter(|(_, s)| s.exec).map(|(i, s)| (i, s.bytes.clone())).collect();
        for f in &c.out.fixups {
            let target = match &f.target {
                Value::Sym(SymRef::Name(n), off) => resolve(n)?.wrapping_add(*off as u64),
                Value::Sym(SymRef::Sym(_), _) => return Err("global_asm!: unexpected Relax symbol".to_string()),
                Value::Label(rasm::LabelId::Named(n), off) => match local.get(n.as_str()) {
                    Some(&a) => a.wrapping_add(*off as u64),
                    None => resolve(n)?.wrapping_add(*off as u64),
                },
                Value::Label(..) => return Err("global_asm!: a numeric label used across sections".to_string()),
                Value::Const(k) => *k as u64,
            };
            let base = bases[ci][f.section as usize];
            let pc = base + f.at as u64;
            let sec = &c.out.sections[f.section as usize];
            if sec.exec {
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
                // data: written in place
                let mut b = vec![0u8; 8];
                let n = if f.kind == FixKind::Abs64 { 8 } else { 4 };
                unsafe { std::ptr::copy_nonoverlapping(pc as *const u8, b.as_mut_ptr(), n) };
                rasm::patch(&mut b, 0, f.kind, target as i64, pc as i64)?;
                unsafe { std::ptr::copy_nonoverlapping(b.as_ptr(), pc as *mut u8, n) };
            }
        }
        for (si, bytes) in texts {
            u.write_reserved_code(bases[ci][si], &bytes);
        }
    }
    Ok(table)
}

/// The C name of an assembly symbol: Mach-O prefixes C names with `_` (dlsym, extern blocks and
/// `#[no_mangle]` know them without it).
fn c_name(n: &str) -> &str {
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
        Arch::X64 => {
            b.extend_from_slice(&[0xff, 0x25, 0, 0, 0, 0]);
            b.extend_from_slice(&target.to_le_bytes());
            b.extend_from_slice(&[0xcc, 0xcc]);
        }
        Arch::Arm64 => {
            b.extend_from_slice(&0x5800_0050u32.to_le_bytes());
            b.extend_from_slice(&0xd61f_0200u32.to_le_bytes());
            b.extend_from_slice(&target.to_le_bytes());
        }
    }
}

/// The program's `#[no_mangle]` / `#[export_name]` fns by symbol.
fn exported_fns(u: &Unit) -> HashMap<String, DefId> {
    let prog = &u.prog;
    let mut out = HashMap::new();
    for (di, d) in prog.defs.iter().enumerate() {
        if d.kind != DefKind::Fn {
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

/// A fn item's callable address (its slot; compiled on its first call).
fn fn_addr(u: &Unit, d: DefId) -> Result<u64, String> {
    if u.prog.def(d).kind == DefKind::ForeignFn {
        return u.foreign_addr(d).ok_or_else(|| format!("`{}` has no address on this host", u.prog.name(d)));
    }
    let instance = crate::instance::Instance::resolve_for_fn_ptr(&u.tcx, &u.prog, d, &[]).map_err(|_| format!("`{}`: a generic fn cannot be a global_asm! symbol", u.prog.def_path(d)))?;
    Ok(u.entry(u.instance_fn_id(instance)))
}

/// A `sym` operand's address: a fn (its slot), a static (its memory), a foreign item.
fn sym_addr(u: &Unit, file: u32, module: u32, x: ExprId) -> Result<u64, String> {
    let prog = &u.prog;
    let f = &prog.files[file as usize];
    let ExprKind::Path(p) = &f.ast.expr(x).kind else { return Err("global_asm!: a `sym` operand must be a path".to_string()) };
    let segs = crate::program::path_segs(prog, file, p);
    let d = if segs.len() == 1 && !p.global() {
        prog.lookup_in_scope(module, segs[0].1, false)
    } else {
        prog.resolve_mod_path(module, p.global(), &segs[..segs.len() - 1], false, false).and_then(|c| prog.lookup_in_container(c, segs[segs.len() - 1].1, false))
    };
    let d = d.ok_or_else(|| format!("global_asm!: cannot find `{}`", String::from_utf8_lossy(f.bytes(f.ast.expr(x).lo, f.ast.expr(x).hi))))?;
    match prog.def(d).kind {
        DefKind::Fn | DefKind::ForeignFn => fn_addr(u, d),
        DefKind::ForeignStatic => u.foreign_addr(d).ok_or_else(|| format!("`{}` has no address on this host", prog.name(d))),
        DefKind::Static => Ok(u.static_addr_shared(d)),
        _ => Err("global_asm!: a `sym` operand must name a function or a static".to_string()),
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
    let v = v.map_err(|_| format!("global_asm!: a `const` operand must be an integer literal here, found `{}`", text))?;
    Ok(if neg { -v } else { v })
}
