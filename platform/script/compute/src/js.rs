//! JavaScript backend: AIR as the text of a JavaScript function, for
//! comparing against the wasm backend in a browser (a host that can run
//! neither native code nor generated wasm would use it).
//!
//! `js::function(p)` is the source of an expression evaluating to
//! `function(F, I, ctx, state, shared, table, n, frame)`: `F` and `I` are
//! a Float32Array and an Int32Array over the host's memory, the others
//! byte addresses in it, as the wasm backend's entry takes them. Values
//! are JavaScript numbers kept at their AIR type: every f32 result goes
//! through `Math.fround` (correctly rounded: an f64 result of an f32 op
//! rounds once more without changing it), every i32 one through `| 0`,
//! so the results are the interpreter's, as for every backend. The
//! exceptions are NaN payloads (a JavaScript NaN is one value) and the
//! fused multiply-add, rounded exactly like the wasm backend's.

use crate::ir::{self, Bin, Block, Cmp, Fma, Op, Program, Region, Stmt, Ty, Un, Val};
use std::fmt::Write;

/// The function's source (None: audio I/O or host calls).
pub fn function(p: &Program) -> Option<String> {
    // Functions are inlined: the JS is one body.
    let flat = ir::flat(p);
    let p = &*flat;
    let mut g = Gen { p, out: String::new(), loops: 0, loop_stack: Vec::new(), bounds: ir::bounds(p), consts: vec![None; p.vals.len()] };
    crate::wasm::collect_consts(&p.body, &mut g.consts);
    fn ok(b: &Block) -> bool {
        b.iter().all(|s| match s {
            Stmt::Out { .. } | Stmt::CallHost { .. } | Stmt::Def(_, Op::In { .. }) => false,
            Stmt::If(_, t, e) => ok(t) && ok(e),
            Stmt::Loop { body, .. } => ok(body),
            _ => true,
        })
    }
    if !ok(&p.body) {
        return None;
    }
    let o = &mut g.out;
    o.push_str("(function(){\n");
    // Bit views for reinterpretations and the fused multiply-add.
    o.push_str("const PF=new Float32Array(1),PI=new Int32Array(PF.buffer),PD=new Float64Array(1),PDI=new Int32Array(PD.buffer);\n");
    o.push_str("const fr=Math.fround;\n");
    // a * b is exact in f64; s = p + c rounds, then fr rounds again. The
    // two roundings differ from one only where s lies exactly between two
    // f32 neighbours r and r2 and the sum was inexact: the exact sum then
    // lies on the error's side of the midpoint.
    o.push_str("function fma(a,b,c){const p=a*b,s=p+c,r=fr(s);if(s!==r){const r2=2*s-r;if(fr(r2)===r2){const bb=s-p,e=(p-(s-bb))+(c-bb);\n");
    o.push_str("if(e!==0){return ((r2>r)===(e>0))?r2:r;}}}return r;}\n");
    o.push_str("function f2i(x){return x!==x?0:x>=2147483647?2147483647:x<=-2147483648?-2147483648:x|0;}\n");
    o.push_str("function rnd(x){const t=Math.trunc(x);return Math.abs(x-t)>=0.5?t+(x<0?-1:1):t;}\n");
    o.push_str("return function(F,I,ctx,state,shared,table,n,frame){\n");
    o.push_str("ctx>>=2;state>>=2;shared>>=2;table>>=2;frame>>=2;\n");
    let mut used = Vec::new();
    crate::wasm::used_bufs(&p.body, &mut used);
    for k in used {
        let _ = writeln!(o, "const bp{k}=I[table+{}]>>2,bl{k}=I[table+{}]-1;", 2 * k as u32, 2 * k as u32 + 1);
    }
    let mut decl = String::new();
    for (k, t) in p.vals.iter().enumerate() {
        let _ = write!(decl, "{}v{}={}", if k == 0 { "let " } else { "," }, k, if *t == Ty::F64 || *t == Ty::F32 { "0.0" } else { "0" });
    }
    if !p.vals.is_empty() {
        decl.push_str(";\n");
    }
    for (k, _) in p.vars.iter().enumerate() {
        let _ = write!(decl, "{}w{}=0", if k == 0 { "let " } else { "," }, k);
    }
    if !p.vars.is_empty() {
        decl.push_str(";\n");
    }
    g.out.push_str(&decl);
    g.block(&p.body);
    g.out.push_str("};})()");
    Some(g.out)
}

struct Gen<'a> {
    p: &'a Program,
    out: String,
    loops: u32,
    loop_stack: Vec<u32>,
    bounds: Vec<Option<u32>>,
    consts: Vec<Option<i32>>,
}

fn v(x: Val) -> String {
    format!("v{}", x.0)
}

impl Gen<'_> {
    fn block(&mut self, b: &Block) {
        for s in b {
            self.stmt(s);
        }
    }

    /// The word index expression of an access.
    fn index(&self, region: Region, base: u32, extent: u32, off: Option<Val>) -> String {
        match region {
            Region::Buf(k) => {
                let i = match off {
                    None => format!("{}", base),
                    Some(o) => format!("{}+({}>>>0)", base, v(o)),
                };
                format!("bp{k}+Math.min({i},bl{k})")
            }
            r => {
                let p = match r {
                    Region::Ctx => "ctx",
                    Region::State => "state",
                    Region::Shared => "shared",
                    Region::Frame => "frame",
                    Region::Buf(_) => unreachable!(),
                };
                match off {
                    None => format!("{p}+{base}"),
                    Some(o) if self.bounds[o.0 as usize].is_some_and(|b| b < extent) => format!("{p}+{base}+{}", v(o)),
                    Some(o) => format!("{p}+{base}+Math.min({}>>>0,{})", v(o), extent - 1),
                }
            }
        }
    }

    fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Def(x, op) => {
                let e = self.op(*x, op);
                let _ = writeln!(self.out, "{}={};", v(*x), e);
            }
            Stmt::Set(var, x) => {
                let _ = writeln!(self.out, "w{}={};", var.0, v(*x));
            }
            Stmt::Store { region, base, extent, off, val } => {
                let at = self.index(*region, *base, *extent, *off);
                let arr = if self.p.vals[val.0 as usize] == Ty::F32 { "F" } else { "I" };
                let _ = writeln!(self.out, "{arr}[{at}]={};", v(*val));
            }
            Stmt::If(c, t, e) => {
                let _ = writeln!(self.out, "if({}!==0){{", v(*c));
                self.block(t);
                if !e.is_empty() {
                    self.out.push_str("}else{\n");
                    self.block(e);
                }
                self.out.push_str("}\n");
            }
            Stmt::Loop { cap, body } => {
                let id = self.loops;
                self.loops += 1;
                self.loop_stack.push(id);
                let _ = writeln!(self.out, "L{id}:for(let k{id}=0;k{id}<{cap};k{id}++){{");
                self.block(body);
                self.out.push_str("}\n");
                self.loop_stack.pop();
            }
            Stmt::Break(d) => {
                let id = self.loop_stack[self.loop_stack.len() - 1 - *d as usize];
                let _ = writeln!(self.out, "break L{id};");
            }
            Stmt::Continue(d) => {
                let id = self.loop_stack[self.loop_stack.len() - 1 - *d as usize];
                let _ = writeln!(self.out, "continue L{id};");
            }
            Stmt::Out { .. } | Stmt::CallHost { .. } => unreachable!("declined"),
            Stmt::Call { .. } => unreachable!("flattened"),
        }
    }

    fn op(&self, x: Val, op: &Op) -> String {
        match *op {
            Op::ConstF(c) => js_f(c as f64),
            Op::ConstD(c) => js_f(c),
            Op::ConstI(c) => format!("{}", c),
            Op::ConstB(c) => format!("{}", c as i32),
            Op::Get(var) => format!("w{}", var.0),
            Op::FrameCount => "n".into(),
            Op::BufLen(k) => format!("(bl{k}+1)"),
            Op::Load { region, base, extent, off } => {
                let at = self.index(region, base, extent, off);
                if self.p.vals[x.0 as usize] == Ty::F32 {
                    format!("F[{at}]")
                } else {
                    format!("I[{at}]")
                }
            }
            Op::Un(u, a) => {
                let a = v(a);
                match u {
                    Un::NegF | Un::NegD => format!("-{a}"),
                    Un::AbsF | Un::AbsD => format!("Math.abs({a})"),
                    Un::SqrtF => format!("fr(Math.sqrt({a}))"),
                    Un::SqrtD => format!("Math.sqrt({a})"),
                    Un::FloorF | Un::FloorD => format!("Math.floor({a})"),
                    Un::CeilF | Un::CeilD => format!("Math.ceil({a})"),
                    Un::TruncF | Un::TruncD => format!("Math.trunc({a})"),
                    Un::RoundF | Un::RoundD => format!("rnd({a})"),
                    Un::F2I | Un::D2I => format!("f2i({a})"),
                    Un::I2F => format!("fr({a})"),
                    Un::BitsFI => format!("(PF[0]={a},PI[0])"),
                    Un::BitsIF => format!("(PI[0]={a},PF[0])"),
                    Un::NegI => format!("(-{a})|0"),
                    Un::NotB => format!("({a}===0?1:0)"),
                    Un::F2D | Un::I2D => a,
                    Un::D2F => format!("fr({a})"),
                    Un::HiD => format!("(PD[0]={a},PDI[1])"),
                    Un::LoD => format!("(PD[0]={a},PDI[0])"),
                }
            }
            Op::Bin(b, a, c) => {
                let (a, c) = (v(a), v(c));
                match b {
                    Bin::AddF => format!("fr({a}+{c})"),
                    Bin::SubF => format!("fr({a}-{c})"),
                    Bin::MulF => format!("fr({a}*{c})"),
                    Bin::DivF => format!("fr({a}/{c})"),
                    Bin::MinF | Bin::MinD => format!("({a}<{c}?{a}:{c})"),
                    Bin::MaxF | Bin::MaxD => format!("({a}>{c}?{a}:{c})"),
                    Bin::AddI => format!("({a}+{c})|0"),
                    Bin::SubI => format!("({a}-{c})|0"),
                    Bin::MulI => format!("Math.imul({a},{c})"),
                    Bin::DivI => format!("({c}===0?0:({a}/{c})|0)"),
                    Bin::RemI => format!("({c}===0?{a}:({a}%{c})|0)"),
                    Bin::AndI | Bin::AndB => format!("{a}&{c}"),
                    Bin::OrI | Bin::OrB => format!("{a}|{c}"),
                    Bin::XorI => format!("{a}^{c}"),
                    Bin::ShlI => format!("{a}<<{c}"),
                    Bin::ShrI => format!("{a}>>{c}"),
                    Bin::ShrUI => format!("({a}>>>{c})|0"),
                    Bin::AddD => format!("{a}+{c}"),
                    Bin::SubD => format!("{a}-{c}"),
                    Bin::MulD => format!("{a}*{c}"),
                    Bin::DivD => format!("{a}/{c}"),
                    Bin::MakeD => format!("(PDI[1]={a},PDI[0]={c},PD[0])"),
                }
            }
            Op::CmpF(cc, a, b) | Op::CmpD(cc, a, b) | Op::CmpI(cc, a, b) => {
                let o = match cc {
                    Cmp::Lt => "<",
                    Cmp::Le => "<=",
                    Cmp::Gt => ">",
                    Cmp::Ge => ">=",
                    Cmp::Eq => "===",
                    Cmp::Ne => "!==",
                };
                format!("({}{}{}?1:0)", v(a), o, v(b))
            }
            Op::Sel(c, a, b) => format!("({}!==0?{}:{})", v(c), v(a), v(b)),
            Op::Fma(k, a, b, c) => {
                let (a, b, c) = (v(a), v(b), v(c));
                match k {
                    Fma::Add => format!("fma({a},{b},{c})"),
                    Fma::SubFrom => format!("fma(-{a},{b},{c})"),
                    Fma::Sub => format!("fma({a},{b},-{c})"),
                    Fma::MulAddI => format!("(Math.imul({a},{b})+{c})|0"),
                }
            }
            Op::Wrap(a, len) => {
                if len.is_power_of_two() {
                    format!("{}&{}", v(a), len - 1)
                } else {
                    let l = len as i32;
                    let _ = self.consts.len();
                    format!("(({}%{l})+{l})%{l}|0", v(a))
                }
            }
            Op::In { .. } => unreachable!("declined"),
        }
    }
}

/// A float literal JavaScript reads back as the same f64.
fn js_f(x: f64) -> String {
    if x.is_nan() {
        "NaN".into()
    } else if x.is_infinite() {
        if x > 0.0 { "Infinity".into() } else { "-Infinity".into() }
    } else if x == 0.0 && x.is_sign_negative() {
        "-0".into()
    } else {
        format!("{:?}", x)
    }
}
