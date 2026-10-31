//! Differential fuzzing: random valid modules (built to give every pass
//! work: duplicate functions and types, dead functions, dead stores,
//! constant expressions, nested blocks, loops, tables) run under stitch
//! before and after optimising. Every exported function is called with the
//! same arguments; results, traps, memory and globals must match. Seeds are
//! fixed, so a failure names a reproducible module (`FUZZ_SEED=n` runs one).
//! Where stitch panics or disagrees, node (when installed) decides: stitch
//! mishandles some valid local reuse patterns the optimiser produces.
//!
//! The strip test does what a web export does with a coverage run: it plays
//! some exported functions, records which functions they entered, strips
//! the units that were never entered (stubbing their call sites), runs all
//! passes, and replays the same calls: they must behave the same. Every
//! result is validated by stitch and, with node, by `WebAssembly.validate`.

use makepad_stitch::{Engine, Linker, Module as StitchModule, Store, Val};
use makepad_wasm_strip::opt::{encode::encode, ir::*, units};
use makepad_wasm_strip::{wasm_optimize_checked, wasm_validate, OptimizeOptions};
use std::{path::PathBuf, process::Command};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }

    fn i32(&mut self) -> i32 {
        match self.below(4) {
            0 => self.below(4) as i32 - 1,
            1 => self.next() as i32,
            2 => [i32::MIN, i32::MAX, 0x7f, 0x80][self.below(4)],
            _ => self.below(300) as i32,
        }
    }
}

const TYS: [ValType; 4] = [ValType::I32, ValType::I64, ValType::F32, ValType::F64];

#[derive(Clone, Copy, PartialEq)]
enum Label {
    Empty,
    Value,
    Loop,
}

struct FuncGen<'a> {
    rng: &'a mut Rng,
    module: &'a Module,
    /// Functions this one may call: those before it.
    callable: u32,
    locals: Vec<ValType>,
    /// Loop counter locals, never touched by other code.
    counters: Vec<u32>,
    body: Vec<Instr>,
    depth: u32,
    /// Open labels, innermost last: their result type, or `Loop` (never a
    /// branch target outside the counter's own, so loops stay bounded).
    labels: Vec<Label>,
    /// A function that never returns (`(i32, i32) -> ()`, it traps), called
    /// the way a panic is: `call; unreachable` under a condition.
    panic: Option<u32>,
}

impl<'a> FuncGen<'a> {
    fn locals_of(&self, ty: ValType) -> Vec<u32> {
        (0..self.locals.len() as u32)
            .filter(|l| self.locals[*l as usize] == ty && !self.counters.contains(l))
            .collect()
    }

    fn konst(&mut self, ty: ValType) {
        let instr = match ty {
            ValType::I32 => Instr::I32Const(self.rng.i32()),
            ValType::I64 => Instr::I64Const(self.rng.i32() as i64 * (1 + self.rng.below(3) as i64 * 0x1_0000_0001)),
            ValType::F32 => Instr::F32Const((self.rng.below(2000) as f32 / 7.0 - 100.0).to_bits()),
            _ => Instr::F64Const((self.rng.below(2000) as f64 / 3.0 - 300.0).to_bits()),
        };
        self.body.push(instr);
    }

    fn leaf(&mut self, ty: ValType) {
        let locals = self.locals_of(ty);
        if !locals.is_empty() && self.rng.chance(50) {
            let l = locals[self.rng.below(locals.len())];
            self.body.push(Instr::LocalGet(l));
            return;
        }
        let globals: Vec<u32> = (0..self.module.globals.len() as u32)
            .filter(|g| self.module.globals[*g as usize].ty.ty == ty)
            .collect();
        if !globals.is_empty() && self.rng.chance(30) {
            self.body.push(Instr::GlobalGet(globals[self.rng.below(globals.len())]));
            return;
        }
        self.konst(ty);
    }

    fn addr(&mut self) {
        self.expr(ValType::I32);
        if self.rng.chance(95) {
            self.body.push(Instr::I32Const(0xff8));
            self.body.push(Instr::Num(0x71));
        }
    }

    fn mem_arg(&mut self, align: u32) -> MemArg {
        MemArg {
            align: self.rng.below(align as usize + 1) as u32,
            offset: self.rng.below(16) as u32,
            memory: 0,
        }
    }

    /// Pushes the arguments of and calls one of the earlier functions.
    fn call(&mut self, want: Option<ValType>) -> bool {
        let candidates: Vec<u32> = (0..self.callable)
            .filter(|f| {
                let ty = &self.module.types[self.module.funcs[*f as usize].ty as usize];
                ty.results.first().copied() == want && ty.results.len() <= 1
            })
            .collect();
        if candidates.is_empty() {
            return false;
        }
        let callee = candidates[self.rng.below(candidates.len())];
        let ty = self.module.funcs[callee as usize].ty;
        let params = self.module.types[ty as usize].params.clone();
        for param in params {
            self.expr(param);
        }
        if self.rng.chance(40) {
            // Indirectly, through a slot below this function's own index.
            self.expr(ValType::I32);
            self.body.push(Instr::I32Const(self.callable as i32));
            self.body.push(Instr::Num(0x70));
            if self.rng.chance(80) {
                self.body.push(Instr::Drop);
                self.body.push(Instr::I32Const(callee as i32));
            }
            self.body.push(Instr::CallIndirect { ty, table: 0 });
        } else {
            self.body.push(Instr::Call(callee));
        }
        true
    }

    fn expr(&mut self, ty: ValType) {
        if self.depth > 5 || self.rng.chance(25) {
            self.leaf(ty);
            return;
        }
        self.depth += 1;
        let is_int = matches!(ty, ValType::I32 | ValType::I64);
        match self.rng.below(12) {
            0 | 1 if is_int => {
                self.expr(ty);
                self.expr(ty);
                let base = if ty == ValType::I32 { 0x6a } else { 0x7c };
                self.body.push(Instr::Num(base + self.rng.below(15) as u8));
            }
            0 | 1 => {
                self.expr(ty);
                self.expr(ty);
                let base = if ty == ValType::F32 { 0x92 } else { 0xa0 };
                self.body.push(Instr::Num(base + self.rng.below(6) as u8));
            }
            2 if ty == ValType::I32 => {
                // A comparison or test of any type.
                let of = TYS[self.rng.below(4)];
                self.expr(of);
                match of {
                    ValType::I32 if self.rng.chance(30) => self.body.push(Instr::Num(0x45)),
                    ValType::I64 if self.rng.chance(30) => self.body.push(Instr::Num(0x50)),
                    _ => {
                        self.expr(of);
                        let (base, n) = match of {
                            ValType::I32 => (0x46, 10),
                            ValType::I64 => (0x51, 10),
                            ValType::F32 => (0x5b, 6),
                            _ => (0x61, 6),
                        };
                        self.body.push(Instr::Num(base + self.rng.below(n) as u8));
                    }
                }
            }
            2 if ty == ValType::I64 => {
                self.expr(ValType::I32);
                self.body.push(Instr::Num(0xac + self.rng.below(2) as u8));
            }
            3 => {
                self.addr();
                let (op, align) = match ty {
                    ValType::I32 => ([0x28, 0x2c, 0x2d, 0x2e, 0x2f][self.rng.below(5)], 2),
                    ValType::I64 => ([0x29, 0x30, 0x35][self.rng.below(3)], 3),
                    ValType::F32 => (0x2a, 2),
                    _ => (0x2b, 3),
                };
                let align = match op {
                    0x2c | 0x2d | 0x30 => 0,
                    0x2e | 0x2f => 1,
                    0x35 => 2,
                    _ => align,
                };
                let arg = self.mem_arg(align);
                self.body.push(Instr::Load(op, arg));
            }
            4 => {
                if !self.call(Some(ty)) {
                    self.leaf(ty);
                }
            }
            5 => {
                self.expr(ty);
                self.expr(ty);
                self.expr(ValType::I32);
                self.body.push(Instr::Select);
            }
            6 => {
                // A block whose value may leave early through br_if.
                self.body.push(Instr::Block(BlockType::Value(ty)));
                self.labels.push(Label::Value);
                self.stmts(2);
                if self.rng.chance(60) {
                    self.expr(ty);
                    self.expr(ValType::I32);
                    self.body.push(Instr::BrIf(0));
                    self.body.push(Instr::Drop);
                }
                self.expr(ty);
                self.labels.pop();
                self.body.push(Instr::End);
            }
            7 => {
                self.expr(ValType::I32);
                self.body.push(Instr::If(BlockType::Value(ty)));
                self.labels.push(Label::Value);
                self.stmts(2);
                self.expr(ty);
                self.body.push(Instr::Else);
                self.stmts(1);
                self.expr(ty);
                self.labels.pop();
                self.body.push(Instr::End);
            }
            8 => {
                let locals = self.locals_of(ty);
                if locals.is_empty() {
                    self.leaf(ty);
                } else {
                    self.expr(ty);
                    self.body.push(Instr::LocalTee(locals[self.rng.below(locals.len())]));
                }
            }
            9 if ty == ValType::I32 => {
                // Constant patterns the peephole pass folds.
                self.konst(ValType::I32);
                self.konst(ValType::I32);
                self.body.push(Instr::Num(0x6a + self.rng.below(15) as u8));
            }
            _ => self.leaf(ty),
        }
        self.depth -= 1;
    }

    fn stmt(&mut self) {
        if let Some(panic) = self.panic {
            if self.depth < 4 && self.rng.chance(15) {
                self.depth += 1;
                self.expr(ValType::I32);
                self.body.push(Instr::If(BlockType::Empty));
                self.labels.push(Label::Empty);
                self.expr(ValType::I32);
                self.expr(ValType::I32);
                self.body.push(Instr::Call(panic));
                self.body.push(Instr::Unreachable);
                self.labels.pop();
                self.body.push(Instr::End);
                self.depth -= 1;
                return;
            }
        }
        self.depth += 1;
        match self.rng.below(10) {
            0 | 1 => {
                let ty = TYS[self.rng.below(4)];
                let locals = self.locals_of(ty);
                if !locals.is_empty() {
                    self.expr(ty);
                    self.body.push(Instr::LocalSet(locals[self.rng.below(locals.len())]));
                }
            }
            2 => {
                let mutable: Vec<u32> = (0..self.module.globals.len() as u32)
                    .filter(|g| self.module.globals[*g as usize].ty.mutable)
                    .collect();
                if !mutable.is_empty() {
                    let g = mutable[self.rng.below(mutable.len())];
                    self.expr(self.module.globals[g as usize].ty.ty);
                    self.body.push(Instr::GlobalSet(g));
                }
            }
            3 => {
                self.addr();
                let ty = TYS[self.rng.below(4)];
                self.expr(ty);
                let (op, align) = match ty {
                    ValType::I32 => [(0x36, 2), (0x3a, 0), (0x3b, 1)][self.rng.below(3)],
                    ValType::I64 => [(0x37, 3), (0x3e, 2)][self.rng.below(2)],
                    ValType::F32 => (0x38, 2),
                    _ => (0x39, 3),
                };
                let arg = self.mem_arg(align);
                self.body.push(Instr::Store(op, arg));
            }
            4 => {
                let ty = TYS[self.rng.below(4)];
                self.expr(ty);
                self.body.push(Instr::Drop);
            }
            5 if self.depth < 4 => {
                self.expr(ValType::I32);
                self.body.push(Instr::If(BlockType::Empty));
                self.labels.push(Label::Empty);
                self.stmts(2);
                if self.rng.chance(50) {
                    self.body.push(Instr::Else);
                    self.stmts(2);
                }
                self.labels.pop();
                self.body.push(Instr::End);
            }
            6 if self.depth < 4 => {
                self.body.push(Instr::Block(BlockType::Empty));
                self.labels.push(Label::Empty);
                self.stmts(1);
                // br_if to this block or an outer empty one.
                let targets: Vec<u32> = (0..self.labels.len() as u32)
                    .filter(|d| self.labels[self.labels.len() - 1 - *d as usize] == Label::Empty)
                    .collect();
                self.expr(ValType::I32);
                self.body.push(Instr::BrIf(targets[self.rng.below(targets.len())]));
                self.stmts(1);
                if self.rng.chance(20) {
                    self.body.push(Instr::Br(0));
                    // Dead code after the branch.
                    self.stmts(1);
                }
                self.labels.pop();
                self.body.push(Instr::End);
            }
            7 if self.depth < 4 => {
                // A bounded loop on a private counter local.
                let counter = self.locals.len() as u32;
                self.locals.push(ValType::I32);
                self.counters.push(counter);
                self.body.push(Instr::I32Const(1 + self.rng.below(3) as i32));
                self.body.push(Instr::LocalSet(counter));
                self.body.push(Instr::Loop(BlockType::Empty));
                self.labels.push(Label::Loop);
                self.stmts(2);
                self.body.push(Instr::LocalGet(counter));
                self.body.push(Instr::I32Const(1));
                self.body.push(Instr::Num(0x6b));
                self.body.push(Instr::LocalTee(counter));
                self.body.push(Instr::BrIf(0));
                self.labels.pop();
                self.body.push(Instr::End);
            }
            8 => {
                if self.call(None) {
                } else if self.call(Some(ValType::I32)) {
                    self.body.push(Instr::Drop);
                }
            }
            _ => {
                // A store to a local nothing reads.
                let dead = self.locals.len() as u32;
                self.locals.push(ValType::I64);
                self.counters.push(dead);
                self.expr(ValType::I64);
                self.body.push(Instr::LocalSet(dead));
            }
        }
        self.depth -= 1;
    }

    fn stmts(&mut self, max: usize) {
        for _ in 0..self.rng.below(max + 1) {
            self.stmt();
        }
    }
}

fn gen_module(seed: u64) -> Module {
    gen_module_with(seed, false)
}

/// With `panics`, the functions call a never-returning function under
/// conditions, as panics are called.
fn gen_module_with(seed: u64, panics: bool) -> Module {
    let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
    let mut module = Module::default();
    for _ in 0..3 + rng.below(5) {
        let mut ty = FuncType::default();
        for _ in 0..rng.below(4) {
            ty.params.push(TYS[rng.below(4)]);
        }
        if rng.chance(80) {
            ty.results.push(TYS[rng.below(4)]);
        }
        module.types.push(ty);
    }
    // Structural duplicates of types.
    for _ in 0..rng.below(3) {
        let ty = module.types[rng.below(module.types.len())].clone();
        module.types.push(ty);
    }
    module.memories.push(MemType {
        limits: Limits { min: 1, max: Some(1) },
        shared: false,
    });
    for i in 0..rng.below(4) {
        let ty = TYS[rng.below(4)];
        let init = match ty {
            ValType::I32 => Instr::I32Const(i as i32 * 3),
            ValType::I64 => Instr::I64Const(i as i64 * 5),
            ValType::F32 => Instr::F32Const((i as f32).to_bits()),
            _ => Instr::F64Const((i as f64).to_bits()),
        };
        module.globals.push(Global {
            ty: GlobalType { ty, mutable: rng.chance(70) },
            init: vec![init, Instr::End],
        });
    }
    let num_funcs = 4 + rng.below(12);
    let panic = panics.then(|| {
        module.types.push(FuncType { params: vec![ValType::I32, ValType::I32], results: vec![] });
        num_funcs as u32
    });
    for f in 0..num_funcs {
        let ty = rng.below(module.types.len()) as u32;
        // Duplicate an earlier function of the same type now and then.
        let earlier: Vec<usize> = (0..f).filter(|g| module.funcs[*g].ty == ty).collect();
        if !earlier.is_empty() && rng.chance(25) {
            let copy = module.funcs[earlier[rng.below(earlier.len())]].clone();
            module.funcs.push(copy);
            continue;
        }
        let fty = module.types[ty as usize].clone();
        let mut locals = fty.params.clone();
        for _ in 0..rng.below(6) {
            locals.push(TYS[rng.below(4)]);
        }
        let mut gen = FuncGen {
            rng: &mut rng,
            module: &module,
            callable: f as u32,
            locals,
            counters: Vec::new(),
            body: Vec::new(),
            depth: 0,
            labels: Vec::new(),
            panic,
        };
        gen.stmts(5);
        if let Some(result) = fty.results.first() {
            gen.expr(*result);
        }
        gen.body.push(Instr::End);
        let body = gen.body;
        let locals = gen.locals[fty.params.len()..].to_vec();
        module.funcs.push(Func { ty, locals, body });
    }
    if panic.is_some() {
        let ty = module.types.len() as u32 - 1;
        module.funcs.push(Func { ty, locals: vec![], body: vec![Instr::Unreachable, Instr::End] });
    }
    module.tables.push(TableType {
        elem: ValType::FuncRef,
        limits: Limits {
            min: num_funcs as u32,
            max: Some(num_funcs as u32),
        },
    });
    module.elems.push(Elem {
        mode: ElemMode::Active {
            table: 0,
            offset: vec![Instr::I32Const(0), Instr::End],
        },
        items: ElemItems::Funcs((0..num_funcs as u32).collect()),
    });
    // Data the functions can read.
    module.datas.push(Data {
        mode: DataMode::Active {
            memory: 0,
            offset: vec![Instr::I32Const(16), Instr::End],
        },
        bytes: (0..64).map(|i| (i * 37 % 251) as u8).collect(),
    });
    for f in 0..num_funcs {
        if rng.chance(60) || f == num_funcs - 1 {
            module.exports.push(Export {
                name: format!("f{f}"),
                kind: ExternKind::Func,
                index: f as u32,
            });
        }
    }
    module.exports.push(Export {
        name: "mem".into(),
        kind: ExternKind::Memory,
        index: 0,
    });
    for g in 0..module.globals.len() {
        module.exports.push(Export {
            name: format!("g{g}"),
            kind: ExternKind::Global,
            index: g as u32,
        });
    }
    module
}

fn val_bits(val: &Val) -> String {
    match val {
        Val::I32(v) => format!("{v}"),
        Val::I64(v) => format!("{v}"),
        Val::F32(v) => float_text(*v as f64),
        Val::F64(v) => float_text(*v),
        other => format!("{other:?}"),
    }
}

/// Floats as both engines print them (NaN payloads are not compared).
fn float_text(v: f64) -> String {
    if v.is_nan() {
        "NaN".into()
    } else {
        format!("{v:e}")
    }
}

/// The argument of parameter `a` in call `k`: simple enough for the node
/// replay below to produce the same.
fn arg(seed: u64, k: u64, a: usize) -> i32 {
    (seed * 13 + k * 7 + a as u64) as i32
}

const CALLS: u64 = 3;

fn mem_hash(bytes: &[u8]) -> u32 {
    bytes.iter().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(*b as u32))
}

/// Runs every exported function (in name order) and returns a transcript
/// of results, traps, globals and memory; `None` when stitch itself panics.
fn run_stitch(bytes: &[u8], seed: u64, play: Option<&[String]>) -> Option<Vec<String>> {
    let bytes = bytes.to_vec();
    let play = play.map(|play| play.to_vec());
    std::panic::catch_unwind(move || {
        let engine = Engine::new();
        let mut store = Store::new(engine.clone());
        let module = StitchModule::new(&engine, &bytes).expect("stitch loads the module");
        let instance = match Linker::new().instantiate(&mut store, &module) {
            Ok(instance) => instance,
            Err(_) => return vec!["instantiation trapped".into()],
        };
        let mut names: Vec<String> = instance.exports().map(|(name, _)| name.to_string()).collect();
        names.sort();
        let mut out = Vec::new();
        for name in &names {
            if play.as_ref().is_some_and(|play| !play.contains(name)) {
                continue;
            }
            let Some(func) = instance.exported_func(name) else {
                continue;
            };
            let ty = func.type_(&store).clone();
            for k in 0..CALLS {
                let args: Vec<Val> = ty
                    .params()
                    .iter()
                    .enumerate()
                    .map(|(a, ty)| {
                        let v = arg(seed, k, a);
                        match ty {
                            makepad_stitch::ValType::I32 => Val::I32(v),
                            makepad_stitch::ValType::I64 => Val::I64(v as i64),
                            makepad_stitch::ValType::F32 => Val::F32(v as f32),
                            _ => Val::F64(v as f64),
                        }
                    })
                    .collect();
                let mut results: Vec<Val> = ty.results().iter().map(|ty| Val::default(*ty)).collect();
                match func.call(&mut store, &args, &mut results) {
                    Ok(()) => out.push(format!(
                        "{name}: {}",
                        results.iter().map(val_bits).collect::<Vec<_>>().join(",")
                    )),
                    Err(_) => out.push(format!("{name}: trap")),
                }
            }
        }
        for name in &names {
            if let Some(global) = instance.exported_global(name) {
                out.push(format!("{name} = {}", val_bits(&global.get(&store))));
            }
        }
        let mem = instance.exported_mem("mem").unwrap();
        out.push(format!("memory {}", mem_hash(mem.bytes(&store))));
        out
    })
    .ok()
}

/// The same transcript from node's engine, when node is installed.
const NODE_RUNNER: &str = r#"
const fs = require('fs');
const [, , path, seed, sigs, playArg] = process.argv;
const types = JSON.parse(sigs);
const play = playArg ? JSON.parse(playArg) : null;
const inst = new WebAssembly.Instance(new WebAssembly.Module(fs.readFileSync(path)), {});
const names = Object.keys(inst.exports).sort();
const text = (ty, v) => {
  if (ty === 'f32' || ty === 'f64') {
    if (Number.isNaN(v)) return 'NaN';
    let s = v.toExponential();
    return s.replace('e+', 'e');
  }
  return String(v);
};
const out = [];
for (const n of names) {
  const e = inst.exports[n];
  if (typeof e !== 'function') continue;
  if (play && !play.includes(n)) continue;
  const [params, results] = types[n];
  for (let k = 0; k < 3; k++) {
    const args = params.map((ty, a) => {
      const v = (Number(seed) * 13 + k * 7 + a) | 0;
      return ty === 'i64' ? BigInt(v) : v;
    });
    try {
      let r = e(...args);
      if (results.length === 0) out.push(n + ': ');
      else out.push(n + ': ' + text(results[0], r));
    } catch (err) {
      if (!(err instanceof WebAssembly.RuntimeError)) throw err;
      out.push(n + ': trap');
    }
  }
}
for (const n of names) {
  const g = inst.exports[n];
  if (g instanceof WebAssembly.Global) out.push(n + ' = ' + text(types[n], g.value));
}
const mem = new Uint8Array(inst.exports.mem.buffer);
let h = 0;
for (let i = 0; i < mem.length; i++) h = (Math.imul(h, 31) + mem[i]) >>> 0;
out.push('memory ' + h);
console.log(JSON.stringify(out));
"#;

fn ty_name(ty: ValType) -> &'static str {
    match ty {
        ValType::I32 => "i32",
        ValType::I64 => "i64",
        ValType::F32 => "f32",
        _ => "f64",
    }
}

fn run_node(
    module: &Module,
    bytes: &[u8],
    seed: u64,
    tag: &str,
    play: Option<&[String]>,
) -> Option<Vec<String>> {
    // One directory per test thread: the tests run in parallel.
    let thread = format!("{:?}", std::thread::current().id()).replace(|c: char| !c.is_ascii_alphanumeric(), "");
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("wasm_strip_fuzz")
        .join(format!("{}_{thread}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    let script = dir.join("runner.js");
    if !script.exists() {
        std::fs::write(&script, NODE_RUNNER).ok()?;
    }
    let wasm = dir.join(format!("{seed}_{tag}.wasm"));
    std::fs::write(&wasm, bytes).ok()?;
    // Export signatures, from the generated module (exports keep their
    // names and types through optimising).
    let funcs = module.func_type_indices();
    let mut sigs = Vec::new();
    for export in &module.exports {
        match export.kind {
            ExternKind::Func => {
                let ty = &module.types[funcs[export.index as usize] as usize];
                let list = |tys: &[ValType]| {
                    tys.iter().map(|t| format!("\"{}\"", ty_name(*t))).collect::<Vec<_>>().join(",")
                };
                sigs.push(format!("\"{}\":[[{}],[{}]]", export.name, list(&ty.params), list(&ty.results)));
            }
            ExternKind::Global => {
                let ty = module.globals[export.index as usize].ty.ty;
                sigs.push(format!("\"{}\":\"{}\"", export.name, ty_name(ty)));
            }
            _ => {}
        }
    }
    let mut command = Command::new("node");
    command
        .arg(&script)
        .arg(&wasm)
        .arg(seed.to_string())
        .arg(format!("{{{}}}", sigs.join(",")));
    if let Some(play) = play {
        let list: Vec<String> = play.iter().map(|name| format!("\"{name}\"")).collect();
        command.arg(format!("[{}]", list.join(",")));
    }
    let output = command.output().ok()?;
    let _ = std::fs::remove_file(&wasm);
    assert!(
        output.status.success(),
        "node failed on seed {seed} ({tag}): {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).ok()?;
    let list = text.trim().trim_start_matches('[').trim_end_matches(']');
    Some(
        list.split("\",\"")
            .map(|item| item.trim_matches('"').to_string())
            .collect(),
    )
}

fn configs() -> Vec<(&'static str, OptimizeOptions)> {
    let alone = |set: &dyn Fn(&mut OptimizeOptions)| {
        let mut opts = OptimizeOptions {
            strip: false,
            dce: false,
            peephole: false,
            locals: false,
            merge: false,
            compact: false,
            order: false,
            ..OptimizeOptions::default()
        };
        set(&mut opts);
        opts
    };
    vec![
        ("all passes", OptimizeOptions::default()),
        ("dce", alone(&|o| o.dce = true)),
        ("peephole", alone(&|o| o.peephole = true)),
        ("locals", alone(&|o| o.locals = true)),
        ("merge", alone(&|o| o.merge = true)),
        ("compact", alone(&|o| o.compact = true)),
        ("order", alone(&|o| o.order = true)),
        ("panics", OptimizeOptions { panic_trap: true, symbols: true, ..OptimizeOptions::default() }),
    ]
}

/// The checks of a test: (seed, config index).
fn work(test: &str) -> Vec<(u64, usize)> {
    let one = std::env::var("FUZZ_SEED").ok().map(|seed| seed.parse::<u64>().unwrap());
    let (range, cfgs): (std::ops::Range<u64>, std::ops::Range<usize>) = match test {
        "all" => (1..401, 0..1),
        "strip" => (2000..2200, STRIP..STRIP + 1),
        "panics" => (3000..3200, PANICS..PANICS + 1),
        _ => (1000..1150, 1..7),
    };
    let range = one.map_or(range, |seed| seed..seed + 1);
    range.flat_map(|seed| cfgs.clone().map(move |cfg| (seed, cfg))).collect()
}

/// The config index of the strip test (past the pass configs).
const STRIP: usize = 8;

/// The config index of the panic-trap test.
const PANICS: usize = 7;

/// One check: the generated module, its bytes, the optimised bytes, how
/// many functions went, and the calls to compare (all exports when `None`).
struct Check {
    module: Module,
    bytes: Vec<u8>,
    optimized: Vec<u8>,
    removed: usize,
    play: Option<Vec<String>>,
}

fn optimise(seed: u64, cfg: usize) -> Check {
    if cfg == STRIP {
        return strip(seed);
    }
    let module = gen_module_with(seed, cfg == PANICS);
    let bytes = encode(&module);
    if let Err(msg) = wasm_validate(&bytes) {
        panic!("seed {seed}: generator made an invalid module: {msg}");
    }
    let (name, opts) = &configs()[cfg];
    let (optimized, report) =
        wasm_optimize_checked(&bytes, opts).unwrap_or_else(|msg| panic!("seed {seed}: {msg}"));
    for pass in &report.passes {
        if let Some(reason) = &pass.reverted {
            panic!("seed {seed} ({name}): pass {} made an invalid module: {reason}", pass.name);
        }
    }
    let removed = report.functions_before - report.functions_after;
    Check { module, bytes, optimized, removed, play: None }
}

/// The generated module with every function named into one of a few units
/// (`app::uN`), as rustc names them.
fn named_module(seed: u64) -> Module {
    let mut module = gen_module(seed);
    let mut rng = Rng(seed.wrapping_mul(0x2545_f491_4f6c_dd1d) | 1);
    let units = 2 + rng.below(4);
    let funcs = (0..module.funcs.len() as u32)
        .map(|f| {
            let unit = format!("u{}", rng.below(units));
            let name = format!("f{f}");
            (f, format!("_RNvNtCs1_3app{}{unit}{}{name}", unit.len(), name.len()))
        })
        .collect();
    module.names = Some(Names { funcs, ..Names::default() });
    module
}

/// The exported functions a check plays: a seeded subset, in name order.
fn play_of(module: &Module, seed: u64) -> Vec<String> {
    let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0x5bd1_e995);
    let mut names: Vec<String> = module
        .exports
        .iter()
        .filter(|e| e.kind == ExternKind::Func)
        .map(|e| e.name.clone())
        .collect();
    names.sort();
    let play: Vec<String> = names.iter().filter(|_| rng.chance(40)).cloned().collect();
    if play.is_empty() {
        names.truncate(1);
        return names;
    }
    play
}

/// The functions the play entered: each function stores 1 to a global of
/// its own on entry, read back after the play. The play runs under stitch
/// in a child process (stitch aborts on some modules); `None` when it could
/// not run.
fn entered(seed: u64) -> Option<Vec<u32>> {
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "coverage_child", "--nocapture", "--test-threads=1"])
        .env("FUZZ_COVERAGE", seed.to_string())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().find_map(|line| line.strip_prefix("E "))?;
    Some(line.split(',').filter(|f| !f.is_empty()).map(|f| f.parse().unwrap()).collect())
}

#[test]
fn coverage_child() {
    let Ok(seed) = std::env::var("FUZZ_COVERAGE") else {
        return;
    };
    let seed: u64 = seed.parse().unwrap();
    let module = named_module(seed);
    let play = play_of(&module, seed);
    let mut probe = module.clone();
    let first = probe.global_types().len() as u32;
    for (f, func) in probe.funcs.iter_mut().enumerate() {
        let global = first + f as u32;
        func.body.splice(0..0, [Instr::I32Const(1), Instr::GlobalSet(global)]);
    }
    for f in 0..module.funcs.len() {
        probe.globals.push(Global {
            ty: GlobalType { ty: ValType::I32, mutable: true },
            init: vec![Instr::I32Const(0), Instr::End],
        });
        probe.exports.push(Export {
            name: format!("cov{f}"),
            kind: ExternKind::Global,
            index: first + f as u32,
        });
    }
    if let Some(transcript) = run_stitch(&encode(&probe), seed, Some(&play)) {
        let entered: Vec<String> = (0..module.funcs.len())
            .filter(|f| transcript.contains(&format!("cov{f} = 1")))
            .map(|f| f.to_string())
            .collect();
        println!("\nE {}", entered.join(","));
    }
}

/// The strip check: play, strip what the play never entered, optimise.
fn strip(seed: u64) -> Check {
    let module = named_module(seed);
    let bytes = encode(&module);
    if let Err(msg) = wasm_validate(&bytes) {
        panic!("seed {seed}: generator made an invalid module: {msg}");
    }
    let play = play_of(&module, seed);
    // Without a coverage run (stitch failed on the module), nothing goes.
    let entered = entered(seed).unwrap_or_else(|| (0..module.funcs.len() as u32).collect());
    let names = module.names.as_ref().unwrap();
    let coverage = units::Coverage {
        entered: names
            .funcs
            .iter()
            .filter(|(f, _)| entered.contains(f))
            .map(|(_, symbol)| (symbol.clone(), units::PHASE_USE))
            .collect(),
    };
    let plan = units::plan(&module, &coverage, &[]);
    let mut stripped = module.clone();
    units::apply(&mut stripped, &plan);
    let stubbed = encode(&stripped);
    if let Err(msg) = wasm_validate(&stubbed) {
        panic!("seed {seed}: stubbing made an invalid module: {msg}");
    }
    let (optimized, report) = wasm_optimize_checked(&stubbed, &OptimizeOptions::default())
        .unwrap_or_else(|msg| panic!("seed {seed}: {msg}"));
    for pass in &report.passes {
        if let Some(reason) = &pass.reverted {
            panic!("seed {seed} (strip): pass {} made an invalid module: {reason}", pass.name);
        }
    }
    Check {
        removed: plan.stripped.len(),
        module,
        bytes,
        optimized,
        play: Some(play),
    }
}

/// The stitch side runs in a child process (this test binary again): stitch
/// panics in places that abort the process, and the parent restarts past
/// them.
#[test]
fn stitch_child() {
    let Ok(spec) = std::env::var("FUZZ_CHILD") else {
        return;
    };
    let (test, from) = spec.split_once(':').unwrap();
    let from: usize = from.parse().unwrap();
    use std::io::Write;
    let mut stdout = std::io::stdout();
    for (i, (seed, cfg)) in work(test).into_iter().enumerate().skip(from) {
        writeln!(stdout, "\nS {i}").unwrap();
        stdout.flush().unwrap();
        let check = optimise(seed, cfg);
        let play = check.play.as_deref();
        let before = run_stitch(&check.bytes, seed, play);
        let after = run_stitch(&check.optimized, seed, play);
        let same = before.is_some() && before == after;
        writeln!(stdout, "R {i} {}", if same { "same" } else { "differ" }).unwrap();
        stdout.flush().unwrap();
    }
}

/// Indices of the checks stitch could not confirm (it disagreed, panicked or
/// aborted).
fn stitch_unconfirmed(test: &str, total: usize) -> Vec<usize> {
    let exe = std::env::current_exe().unwrap();
    let mut unconfirmed = Vec::new();
    let mut from = 0;
    while from < total {
        let output = Command::new(&exe)
            .args(["--exact", "stitch_child", "--nocapture", "--test-threads=1"])
            .env("FUZZ_CHILD", format!("{test}:{from}"))
            .output()
            .expect("run the stitch child");
        let text = String::from_utf8_lossy(&output.stdout);
        let mut started = None;
        let mut next = from;
        for line in text.lines() {
            let mut parts = line.split(' ');
            match (parts.next(), parts.next().and_then(|i| i.parse::<usize>().ok()), parts.next()) {
                (Some("S"), Some(i), _) => started = Some(i),
                (Some("R"), Some(i), Some(verdict)) => {
                    if verdict != "same" {
                        unconfirmed.push(i);
                    }
                    started = None;
                    next = i + 1;
                }
                _ => {}
            }
        }
        match started {
            // The child died in this check: count it and go on after it.
            Some(i) => {
                unconfirmed.push(i);
                from = i + 1;
            }
            None => {
                assert!(next >= total, "stitch child stopped early:\n{text}");
                from = next;
            }
        }
    }
    unconfirmed
}

/// Every output through node's `WebAssembly.validate`, in one node run.
fn node_validate(test: &str, outputs: &[(u64, Vec<u8>)]) {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("wasm_strip_fuzz")
        .join(format!("validate_{}_{test}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut paths = Vec::new();
    for (i, (seed, bytes)) in outputs.iter().enumerate() {
        let path = dir.join(format!("{i}_{seed}.wasm"));
        std::fs::write(&path, bytes).unwrap();
        paths.push(path);
    }
    let script = r#"
const fs = require('fs');
const bad = process.argv.slice(2).filter(p => !WebAssembly.validate(fs.readFileSync(p)));
console.log(JSON.stringify(bad));
"#;
    let output = Command::new("node").arg("-e").arg(script).args(&paths).output().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(output.status.success(), "node: {}", String::from_utf8_lossy(&output.stderr));
    let text = String::from_utf8_lossy(&output.stdout);
    assert_eq!(text.trim(), "[]", "{test}: node refused optimised modules: {text}");
}

fn node_available() -> bool {
    Command::new("node").arg("--version").output().map_or(false, |out| out.status.success())
}

fn fuzz(test: &str) -> usize {
    let work = work(test);
    let unconfirmed = stitch_unconfirmed(test, work.len());
    let node = node_available();
    let mut removed = 0;
    let mut outputs = Vec::new();
    for (i, (seed, cfg)) in work.iter().copied().enumerate() {
        let check = optimise(seed, cfg);
        removed += check.removed;
        if node {
            outputs.push((seed, check.optimized.clone()));
        }
        if !unconfirmed.contains(&i) {
            continue;
        }
        let name = configs().get(cfg).map_or("strip", |c| c.0);
        assert!(
            node,
            "seed {seed} ({name}): stitch could not confirm it and node is not available"
        );
        let play = check.play.as_deref();
        let before = run_node(&check.module, &check.bytes, seed, "in", play).unwrap();
        let after = run_node(&check.module, &check.optimized, seed, "out", play).unwrap();
        assert_eq!(before, after, "seed {seed} ({name}): behaviour changed (node)");
    }
    if node {
        node_validate(test, &outputs);
    }
    eprintln!(
        "fuzz {test}: {} checks, {} confirmed by stitch, {} by node",
        work.len(),
        work.len() - unconfirmed.len(),
        unconfirmed.len()
    );
    removed
}

#[test]
fn optimised_random_modules_behave_the_same() {
    let removed = fuzz("all");
    if std::env::var("FUZZ_SEED").is_err() {
        // The generator really gives the passes work.
        assert!(removed > 50, "only {removed} functions removed");
    }
}

/// Each pass on its own keeps behaviour too (passes must not rely on an
/// earlier one having run).
#[test]
fn each_pass_alone_behaves_the_same() {
    fuzz("each");
}

/// Units a play never entered are stripped (their call sites stubbed) and
/// the module optimised: the play behaves the same, and every result
/// validates.
#[test]
fn stripped_modules_replay_the_same() {
    let removed = fuzz("strip");
    if std::env::var("FUZZ_SEED").is_err() {
        assert!(removed > 100, "only {removed} functions stripped");
    }
}

/// Panic sites become traps: whatever traps still traps at the same call,
/// and everything else behaves the same.
#[test]
fn panic_traps_behave_the_same() {
    let removed = fuzz("panics");
    if std::env::var("FUZZ_SEED").is_err() {
        // Every module loses its panic function.
        assert!(removed >= 200, "only {removed} functions removed");
    }
}
