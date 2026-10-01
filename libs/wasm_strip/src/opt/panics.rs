//! Panics become traps (`OptimizeOptions::panic_trap`). A `call` that LLVM
//! follows with `unreachable` calls a function that never returns: a panic,
//! a failed unwrap or expect, a bounds or slice check, an allocation
//! failure. Each such site becomes a plain `unreachable`, and the
//! computation of its arguments goes with the call when it has no effect a
//! trap leaves visible (stack values, locals, loads, arithmetic). The panic
//! functions, the formatting behind them and their messages become dead
//! for the passes after. A panic then traps without printing its message.
//!
//! Each site keeps what it said for the symbol file (see `symbols`): the
//! callee, and the message and location its constant arguments point at.
//! For that the sites are marked (`i32.const <marker>` before the
//! `unreachable`) while the passes run, and `unmark` takes the markers out
//! last and says where each site ended up.

use super::demangle::demangle;
use super::ir::*;
use std::collections::HashMap;

/// What a panic site said before it became a trap.
#[derive(Clone, Debug, Default)]
pub struct Site {
    /// The function the site called (demangled).
    pub callee: String,
    /// A message its arguments point at, when one is a string constant.
    pub message: Option<String>,
    /// `file:line:col`, when an argument points at a panic location.
    pub location: Option<String>,
}

/// Site markers are `MARK + site` (5-byte constants nothing else produces).
const MARK: i32 = i32::MIN;

/// Has no effect left visible after the trap that follows: only the operand
/// stack and locals change, or the instruction traps itself.
fn removable(instr: &Instr) -> bool {
    matches!(
        instr,
        Instr::Nop
            | Instr::Drop
            | Instr::Select
            | Instr::SelectT(_)
            | Instr::LocalGet(_)
            | Instr::LocalSet(_)
            | Instr::LocalTee(_)
            | Instr::GlobalGet(_)
            | Instr::Load(..)
            | Instr::MemorySize(_)
            | Instr::I32Const(_)
            | Instr::I64Const(_)
            | Instr::F32Const(_)
            | Instr::F64Const(_)
            | Instr::Num(_)
            | Instr::RefNull(_)
            | Instr::RefIsNull
            | Instr::RefFunc(_)
            | Instr::TruncSat(_)
            | Instr::TableGet(_)
            | Instr::TableSize(_)
            | Instr::SimdMem(0x00..=0x0a | 0x5c | 0x5d, _)
            | Instr::V128Const(_)
            | Instr::I8x16Shuffle(_)
            | Instr::SimdLane(..)
            | Instr::Simd(_)
    )
}

/// Memory 0 as its active data segments with constant offsets lay it out.
struct Image<'a> {
    segments: Vec<(u32, &'a [u8])>,
}

impl<'a> Image<'a> {
    fn new(datas: &'a [Data]) -> Image<'a> {
        let mut segments = Vec::new();
        for data in datas {
            if let DataMode::Active { memory: 0, offset } = &data.mode {
                if let [Instr::I32Const(at), Instr::End] = offset.as_slice() {
                    segments.push((*at as u32, data.bytes.as_slice()));
                }
            }
        }
        Image { segments }
    }

    fn bytes(&self, at: u32, len: u32) -> Option<&'a [u8]> {
        self.segments.iter().find_map(|(start, bytes)| {
            let from = at.checked_sub(*start)? as usize;
            bytes.get(from..from.checked_add(len as usize)?)
        })
    }

    fn u32(&self, at: u32) -> Option<u32> {
        Some(u32::from_le_bytes(self.bytes(at, 4)?.try_into().ok()?))
    }

    /// Printable UTF-8 text at `at`.
    fn text(&self, at: u32, len: u32) -> Option<&'a str> {
        if len == 0 || len > 4096 {
            return None;
        }
        let text = std::str::from_utf8(self.bytes(at, len)?).ok()?;
        text.chars().all(|c| !c.is_control() || c == '\n' || c == '\t').then_some(text)
    }

    /// A `core::panic::Location` (file, line, column) at `at`.
    fn location(&self, at: u32) -> Option<String> {
        let file = self.text(self.u32(at)?, self.u32(at + 4)?)?;
        let line = self.u32(at + 8)?;
        let col = self.u32(at + 12)?;
        (file.ends_with(".rs") && line > 0 && line < 10_000_000 && col < 100_000)
            .then(|| format!("{file}:{line}:{col}"))
    }

    /// The first piece of a `fmt::Arguments` pieces array (`&[&str]`) at
    /// `at`.
    fn piece(&self, at: u32) -> Option<&'a str> {
        self.text(self.u32(at)?, self.u32(at + 4)?)
    }
}

/// What the constants of the instructions before a site point at.
fn describe(image: &Image, callee: String, before: &[Instr]) -> Site {
    let mut site = Site { callee, ..Site::default() };
    let consts: Vec<u32> = before
        .iter()
        .rev()
        .filter_map(|instr| match instr {
            Instr::I32Const(value) => Some(*value as u32),
            _ => None,
        })
        .collect();
    for (i, value) in consts.iter().copied().enumerate() {
        if let Some(location) = image.location(value) {
            site.location.get_or_insert(location);
            continue;
        }
        if site.message.is_none() {
            // A `&str` argument: the pointer pushed before its length; else
            // the first piece of a format string.
            let text = consts.get(i + 1).and_then(|ptr| image.text(*ptr, value));
            site.message = text.or_else(|| image.piece(value)).map(|text| text.to_string());
        }
    }
    site
}

/// The straight-line code before a site: its arguments and what built them
/// (a format string's pieces stored to the stack), back to the last
/// control instruction or call.
fn arguments(body: &[Instr], call: usize) -> &[Instr] {
    let start = body[..call]
        .iter()
        .rposition(|instr| {
            matches!(
                instr,
                Instr::Block(_)
                    | Instr::Loop(_)
                    | Instr::If(_)
                    | Instr::Else
                    | Instr::End
                    | Instr::Br(_)
                    | Instr::BrIf(_)
                    | Instr::BrTable(..)
                    | Instr::Return
                    | Instr::Unreachable
                    | Instr::Call(_)
                    | Instr::CallIndirect { .. }
            )
        })
        .map_or(0, |at| at + 1);
    &body[start.max(call.saturating_sub(WINDOW))..call]
}

/// How far back from a call the constants describing it are looked for.
const WINDOW: usize = 48;

/// Turns every panic site into a trap. With `mark`, each trap gets its site
/// marker; the sites are returned in marker order.
pub fn run(module: &mut Module, mark: bool) -> Vec<Site> {
    let imported = module.num_imported_funcs();
    let names: HashMap<u32, String> = module
        .names
        .as_ref()
        .map(|names| {
            names
                .funcs
                .iter()
                .map(|(i, s)| (*i, demangle(s).map(|d| d.name).unwrap_or_else(|| s.clone())))
                .collect()
        })
        .unwrap_or_default();
    let datas = if mark { module.datas.clone() } else { Vec::new() };
    let image = Image::new(&datas);
    let mut sites = Vec::new();
    for func in &mut module.funcs {
        let is_site = |body: &[Instr], i: usize| {
            matches!((&body[i], body.get(i + 1)), (Instr::Call(f), Some(Instr::Unreachable)) if *f >= imported)
        };
        if !(0..func.body.len()).any(|i| is_site(&func.body, i)) {
            continue;
        }
        let body = std::mem::take(&mut func.body);
        let mut out: Vec<Instr> = Vec::with_capacity(body.len());
        let mut i = 0;
        while i < body.len() {
            if !is_site(&body, i) {
                out.push(body[i].clone());
                i += 1;
                continue;
            }
            let Instr::Call(callee) = body[i] else { unreachable!() };
            let window = arguments(&body, i);
            let callee = names.get(&callee).cloned().unwrap_or_else(|| format!("func[{callee}]"));
            if mark {
                sites.push(describe(&image, callee, window));
            }
            while out.last().is_some_and(removable) {
                out.pop();
            }
            if mark {
                out.push(Instr::I32Const(MARK.wrapping_add(sites.len() as i32 - 1)));
            }
            out.push(Instr::Unreachable);
            i += 2;
        }
        func.body = out;
    }
    sites
}

/// Takes the site markers out and says where each site's trap is:
/// (function index, instruction index in its body, site).
pub fn unmark(module: &mut Module, sites: usize) -> Vec<(u32, usize, usize)> {
    let imported = module.num_imported_funcs();
    let site_of = |instr: &Instr| match instr {
        Instr::I32Const(value) => {
            let site = value.wrapping_sub(MARK) as u32 as usize;
            (site < sites).then_some(site)
        }
        _ => None,
    };
    let mut out = Vec::new();
    for (f, func) in module.funcs.iter_mut().enumerate() {
        let marked = func
            .body
            .windows(2)
            .any(|w| site_of(&w[0]).is_some() && w[1] == Instr::Unreachable);
        if !marked {
            continue;
        }
        let body = std::mem::take(&mut func.body);
        let mut kept = Vec::with_capacity(body.len());
        let mut i = 0;
        while i < body.len() {
            if let (Some(site), Some(Instr::Unreachable)) = (site_of(&body[i]), body.get(i + 1)) {
                out.push((imported + f as u32, kept.len(), site));
                kept.push(Instr::Unreachable);
                i += 2;
                continue;
            }
            kept.push(body[i].clone());
            i += 1;
        }
        func.body = kept;
    }
    out
}
