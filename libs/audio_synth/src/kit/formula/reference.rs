//! Tests only: a direct tree-walking interpreter over the same syntax tree,
//! written independently of the compiler (real tables, closures, scopes,
//! sandboxes, prototype read-through) to hold the compiled programs to.
//! Math uses the same portable kernels the AIR builder lowers
//! (`makepad_csg_math::portable`), and the same generator, so results match
//! bit for bit.

use super::ast::*;
use super::compile::err;
use super::{FormulaInputs, FormulaIo, RNG_START};
use makepad_csg_math::portable as pm;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

#[derive(Clone)]
pub enum Value {
    Nil,
    Bool(bool),
    Num(f64),
    Str(Rc<[u8]>),
    Table(T),
    Func(Rc<Closure>),
    Builtin(&'static str),
}

pub type T = Rc<RefCell<Table>>;

#[derive(Default)]
pub struct Table {
    map: HashMap<Key, Value>,
    proto: Option<T>,
}

#[derive(Clone, PartialEq, Eq, Hash)]
enum Key {
    Num(u64),
    Bool(bool),
    Str(Rc<[u8]>),
    Table(usize),
}

fn key(v: &Value) -> Result<Option<Key>, i32> {
    Ok(Some(match v {
        Value::Nil => return Ok(None),
        Value::Num(x) => {
            if x.is_nan() {
                return Ok(None);
            }
            Key::Num((x + 0.0).to_bits())
        }
        Value::Bool(b) => Key::Bool(*b),
        Value::Str(s) => Key::Str(s.clone()),
        Value::Table(t) => Key::Table(Rc::as_ptr(t) as usize),
        _ => return Err(err::BAD_KEY),
    }))
}

pub struct Closure {
    body: Rc<FuncBody>,
    env: Scope,
    sandbox: Sb,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Sb {
    Top,
    Init,
    Process,
}

type Cell = Rc<RefCell<Value>>;

#[derive(Clone, Default)]
struct Scope(Option<Rc<(Rc<str>, Cell, Scope)>>);

impl Scope {
    fn find(&self, n: &str) -> Option<Cell> {
        let mut s = self.clone();
        while let Some(x) = s.0.clone() {
            if &*x.0 == n {
                return Some(x.1.clone());
            }
            s = x.2.clone();
        }
        None
    }
    fn with(&self, n: Rc<str>, v: Value) -> Scope {
        Scope(Some(Rc::new((n, Rc::new(RefCell::new(v)), self.clone()))))
    }
}

fn tget(t: &T, k: &Value) -> Value {
    let Ok(Some(kk)) = key(k) else { return Value::Nil };
    let tb = t.borrow();
    match tb.map.get(&kk) {
        Some(v) => v.clone(),
        None => match &tb.proto {
            Some(p) => p.borrow().map.get(&kk).cloned().unwrap_or(Value::Nil),
            None => Value::Nil,
        },
    }
}
fn tset(t: &T, k: &Value, v: Value) -> Result<(), i32> {
    let kk = key(k)?.ok_or(err::BAD_KEY)?;
    let mut tb = t.borrow_mut();
    if matches!(v, Value::Nil) {
        tb.map.remove(&kk);
    } else {
        tb.map.insert(kk, v);
    }
    Ok(())
}
fn new_table() -> T {
    Rc::new(RefCell::new(Table::default()))
}
fn s(x: &str) -> Value {
    Value::Str(x.as_bytes().into())
}
fn len_of(t: &T) -> f64 {
    let mut n = 0.0;
    loop {
        if matches!(tget(t, &Value::Num(n + 1.0)), Value::Nil) {
            return n;
        }
        n += 1.0;
    }
}
fn truthy(v: &Value) -> bool {
    !matches!(v, Value::Nil | Value::Bool(false))
}
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Nil, Value::Nil) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Num(x), Value::Num(y)) => x == y,
        (Value::Str(x), Value::Str(y)) => x == y,
        (Value::Table(x), Value::Table(y)) => Rc::ptr_eq(x, y),
        (Value::Func(x), Value::Func(y)) => Rc::ptr_eq(x, y),
        (Value::Builtin(x), Value::Builtin(y)) => x == y,
        _ => false,
    }
}

/// The generator (the compiled `rng_u32`/`rng_seed`).
pub struct Rng(pub [u32; 2]);
impl Rng {
    pub fn next_u32(&mut self) -> u32 {
        let [s0, s1] = self.0;
        let out = s0.wrapping_mul(0x9E37_79BB).rotate_left(5).wrapping_mul(5);
        let s1 = s1 ^ s0;
        self.0 = [s0.rotate_left(26) ^ s1 ^ (s1 << 9), s1.rotate_left(13)];
        out
    }
    pub fn unit(&mut self) -> f64 {
        let a = (self.next_u32() >> 5) as f64;
        let b = (self.next_u32() >> 6) as f64;
        (a * 67108864.0 + b) * (1.0 / 9007199254740992.0)
    }
    fn fmix(mut z: u32) -> u32 {
        z ^= z >> 16;
        z = z.wrapping_mul(0x85EB_CA6B);
        z ^= z >> 13;
        z = z.wrapping_mul(0xC2B2_AE35);
        z ^ (z >> 16)
    }
    pub fn seed(&mut self, x: f64) {
        let b = (x + 0.0).to_bits();
        let (lo, hi) = (b as u32, (b >> 32) as u32);
        let a = Self::fmix(lo ^ 0x6A09_E667);
        let c = Self::fmix(hi ^ 0xBB67_AE85 ^ a);
        let a = if a | c == 0 { 1 } else { a };
        self.0 = [a, c];
    }
}

enum Flow {
    Normal,
    Break,
    Return(Vec<Value>),
}

type R<X> = Result<X, i32>;

struct Interp<'a> {
    w: &'a mut World,
    sb: Sb,
}

/// What every instance of one formula shares.
pub struct World {
    globals: [HashMap<String, Value>; 3],
    rng: Rng,
    protos: [T; 3],
    surge: T,
    init: Option<Rc<Closure>>,
    process: Option<Rc<Closure>>,
}

const MATH: &[&str] = super::compile::MATH_FNS;

impl Interp<'_> {
    fn global(&self, n: &str) -> Value {
        if let Some(v) = self.w.globals[self.sb as usize].get(n) {
            return v.clone();
        }
        match n {
            "math" => Value::Builtin("@math"),
            "string" => Value::Builtin("@string"),
            "surge" => Value::Table(self.w.surge.clone()),
            "pi" => Value::Num(std::f64::consts::PI),
            "huge" => Value::Num(f64::INFINITY),
            _ => match MATH.iter().chain(["type", "tonumber", "print", "error", "assert"].iter()).find(|m| **m == n) {
                Some(m) => Value::Builtin(m),
                None => Value::Nil,
            },
        }
    }
    fn block(&mut self, b: &Block, mut sc: Scope) -> R<Flow> {
        for st in b {
            match self.stat(st, &mut sc)? {
                Flow::Normal => {}
                f => return Ok(f),
            }
        }
        Ok(Flow::Normal)
    }
    fn assign(&mut self, t: &Expr, v: Value, sc: &Scope, pre: Option<(Value, Value)>) -> R<()> {
        match &t.kind {
            ExprKind::Name(n) => {
                if let Some(c) = sc.find(n) {
                    *c.borrow_mut() = v;
                } else {
                    self.w.globals[self.sb as usize].insert(n.to_string(), v);
                }
                Ok(())
            }
            ExprKind::Index(..) => {
                let (o, k) = pre.unwrap();
                match o {
                    Value::Table(t) => tset(&t, &k, v),
                    _ => Err(err::INDEX),
                }
            }
            _ => unreachable!(),
        }
    }
    fn stat(&mut self, st: &Stat, sc: &mut Scope) -> R<Flow> {
        match &st.kind {
            StatKind::Local(names, vals) => {
                let vs = self.explist(vals, names.len(), sc)?;
                for (n, v) in names.iter().zip(vs) {
                    *sc = sc.with(n.clone(), v);
                }
            }
            StatKind::LocalFunc(n, f) => {
                *sc = sc.with(n.clone(), Value::Nil);
                let c = Value::Func(Rc::new(Closure { body: f.clone(), env: sc.clone(), sandbox: self.sb }));
                *sc.find(n).unwrap().borrow_mut() = c;
            }
            StatKind::Assign(ts, vals) => {
                let mut pres = Vec::new();
                for t in ts {
                    pres.push(match &t.kind {
                        ExprKind::Index(o, k) => {
                            let o = self.expr(o, sc)?;
                            let k = self.expr(k, sc)?;
                            Some((o, k))
                        }
                        _ => None,
                    });
                }
                let vs = self.explist(vals, ts.len(), sc)?;
                for ((t, v), pre) in ts.iter().zip(vs).zip(pres) {
                    self.assign(t, v, sc, pre)?;
                }
            }
            StatKind::Call(e) => {
                self.multi(e, sc)?;
            }
            StatKind::Do(b) => return self.block(b, sc.clone()),
            StatKind::While(c, b) => {
                while truthy(&self.expr(c, sc)?) {
                    match self.block(b, sc.clone())? {
                        Flow::Break => break,
                        Flow::Return(v) => return Ok(Flow::Return(v)),
                        Flow::Normal => {}
                    }
                }
            }
            StatKind::Repeat(b, c) => loop {
                let mut inner = sc.clone();
                let mut stop = None;
                for st in b {
                    match self.stat(st, &mut inner)? {
                        Flow::Normal => {}
                        f => {
                            stop = Some(f);
                            break;
                        }
                    }
                }
                match stop {
                    Some(Flow::Break) => break,
                    Some(f) => return Ok(f),
                    None => {}
                }
                if truthy(&self.expr(c, &inner)?) {
                    break;
                }
            },
            StatKind::If(arms, els) => {
                for (c, b) in arms {
                    if truthy(&self.expr(c, sc)?) {
                        return self.block(b, sc.clone());
                    }
                }
                if let Some(b) = els {
                    return self.block(b, sc.clone());
                }
            }
            StatKind::NumFor(n, a, b, c, body) => {
                let a = self.num(a, sc)?;
                let lim = self.num(b, sc)?;
                let step = match c {
                    Some(c) => self.num(c, sc)?,
                    None => 1.0,
                };
                let mut i = a;
                while if step > 0.0 { i <= lim } else { i >= lim } {
                    match self.block(body, sc.with(n.clone(), Value::Num(i)))? {
                        Flow::Break => break,
                        Flow::Return(v) => return Ok(Flow::Return(v)),
                        Flow::Normal => {}
                    }
                    i += step;
                }
            }
            StatKind::GenFor(..) => return Err(err::RAISED),
            StatKind::Return(es) => return Ok(Flow::Return(self.explist(es, usize::MAX, sc)?)),
            StatKind::Break => return Ok(Flow::Break),
        }
        Ok(Flow::Normal)
    }
    fn num(&mut self, e: &Expr, sc: &Scope) -> R<f64> {
        match self.expr(e, sc)? {
            Value::Num(x) => Ok(x),
            _ => Err(err::ARITH),
        }
    }
    fn explist(&mut self, es: &[Expr], n: usize, sc: &Scope) -> R<Vec<Value>> {
        let mut out = Vec::new();
        for (k, e) in es.iter().enumerate() {
            if k + 1 == es.len() {
                out.extend(self.multi(e, sc)?);
            } else {
                out.push(self.expr(e, sc)?);
            }
        }
        if n != usize::MAX {
            out.resize(n, Value::Nil);
        }
        Ok(out)
    }
    fn expr(&mut self, e: &Expr, sc: &Scope) -> R<Value> {
        Ok(self.multi(e, sc)?.into_iter().next().unwrap_or(Value::Nil))
    }
    fn multi(&mut self, e: &Expr, sc: &Scope) -> R<Vec<Value>> {
        Ok(vec![match &e.kind {
            ExprKind::Nil => Value::Nil,
            ExprKind::True => Value::Bool(true),
            ExprKind::False => Value::Bool(false),
            ExprKind::Num(x) => Value::Num(*x),
            ExprKind::Str(x) => s(x),
            ExprKind::Vararg => return Err(err::RAISED),
            ExprKind::Func(f) => Value::Func(Rc::new(Closure { body: f.clone(), env: sc.clone(), sandbox: self.sb })),
            ExprKind::Paren(x) => self.expr(x, sc)?,
            ExprKind::Name(n) => match sc.find(n) {
                Some(c) => c.borrow().clone(),
                None => self.global(n),
            },
            ExprKind::Table(fields) => {
                let t = new_table();
                let mut pos = 0.0;
                for (k, f) in fields.iter().enumerate() {
                    match f {
                        Field::Keyed(ke, ve) => {
                            let kv = self.expr(ke, sc)?;
                            let vv = self.expr(ve, sc)?;
                            tset(&t, &kv, vv)?;
                        }
                        Field::Pos(ve) => {
                            let vals = if k + 1 == fields.len() { self.multi(ve, sc)? } else { vec![self.expr(ve, sc)?] };
                            for v in vals {
                                pos += 1.0;
                                if !matches!(v, Value::Nil) {
                                    tset(&t, &Value::Num(pos), v)?;
                                }
                            }
                        }
                    }
                }
                Value::Table(t)
            }
            ExprKind::Un(op, x) => {
                let v = self.expr(x, sc)?;
                match op {
                    UnOp::Neg => match v {
                        Value::Num(x) => Value::Num(-x),
                        _ => return Err(err::ARITH),
                    },
                    UnOp::Not => Value::Bool(!truthy(&v)),
                    UnOp::Len => match v {
                        Value::Str(s) => Value::Num(s.len() as f64),
                        Value::Table(t) => Value::Num(len_of(&t)),
                        _ => return Err(err::LENGTH),
                    },
                }
            }
            ExprKind::Bin(op, a, b) => {
                if *op == BinOp::And || *op == BinOp::Or {
                    let av = self.expr(a, sc)?;
                    let t = truthy(&av);
                    return Ok(vec![if (*op == BinOp::And) == t { self.expr(b, sc)? } else { av }]);
                }
                let av = self.expr(a, sc)?;
                let bv = self.expr(b, sc)?;
                match op {
                    BinOp::Eq => Value::Bool(same(&av, &bv)),
                    BinOp::Ne => Value::Bool(!same(&av, &bv)),
                    _ => {
                        let (Value::Num(x), Value::Num(y)) = (&av, &bv) else {
                            return Err(if matches!(op, BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge) { err::COMPARE } else { err::ARITH });
                        };
                        let (x, y) = (*x, *y);
                        match op {
                            BinOp::Add => Value::Num(x + y),
                            BinOp::Sub => Value::Num(x - y),
                            BinOp::Mul => Value::Num(x * y),
                            BinOp::Div => Value::Num(x / y),
                            BinOp::Mod => Value::Num(x - (x / y).floor() * y),
                            BinOp::Pow => Value::Num(pm::powf(x, y)),
                            BinOp::Lt => Value::Bool(x < y),
                            BinOp::Le => Value::Bool(x <= y),
                            BinOp::Gt => Value::Bool(x > y),
                            BinOp::Ge => Value::Bool(x >= y),
                            _ => return Err(err::ARITH),
                        }
                    }
                }
            }
            ExprKind::Index(o, k) => {
                let ov = self.expr(o, sc)?;
                let kv = self.expr(k, sc)?;
                self.index(ov, kv)?
            }
            ExprKind::Call(f, args) => {
                let fv = self.expr(f, sc)?;
                let a = self.explist(args, usize::MAX, sc)?;
                return self.call(fv, a);
            }
            ExprKind::Method(o, name, args) => {
                let ov = self.expr(o, sc)?;
                let mut a = vec![ov.clone()];
                a.extend(self.explist(args, usize::MAX, sc)?);
                return self.method(name, a);
            }
        }])
    }
    fn index(&mut self, o: Value, k: Value) -> R<Value> {
        Ok(match o {
            Value::Table(t) => tget(&t, &k),
            Value::Builtin("@math") => match &k {
                Value::Str(n) => {
                    let n = std::str::from_utf8(n).unwrap();
                    match n {
                        "pi" => Value::Num(std::f64::consts::PI),
                        "huge" => Value::Num(f64::INFINITY),
                        _ => MATH.iter().find(|m| **m == n).map(|m| Value::Builtin(m)).unwrap_or(Value::Nil),
                    }
                }
                _ => Value::Nil,
            },
            Value::Builtin("@string") => match &k {
                Value::Str(n) => match &**n {
                    b"sub" => Value::Builtin("str.sub"),
                    b"find" => Value::Builtin("str.find"),
                    b"len" => Value::Builtin("str.len"),
                    b"byte" => Value::Builtin("str.byte"),
                    _ => Value::Nil,
                },
                _ => Value::Nil,
            },
            _ => return Err(err::INDEX),
        })
    }
    pub fn call(&mut self, f: Value, a: Vec<Value>) -> R<Vec<Value>> {
        match f {
            Value::Func(c) => {
                let mut sc = c.env.clone();
                let mut a = a.into_iter();
                for p in &c.body.params {
                    sc = sc.with(p.clone(), a.next().unwrap_or(Value::Nil));
                }
                let saved = std::mem::replace(&mut self.sb, c.sandbox);
                let r = self.block(&c.body.body, sc);
                self.sb = saved;
                Ok(match r? {
                    Flow::Return(v) => v,
                    _ => vec![],
                })
            }
            Value::Builtin(name) => self.builtin(name, a),
            _ => Err(err::RAISED),
        }
    }
    fn n(a: &[Value], k: usize) -> R<f64> {
        match a.get(k) {
            Some(Value::Num(x)) => Ok(*x),
            _ => Err(err::ARITH),
        }
    }
    fn n_or(a: &[Value], k: usize, d: f64) -> R<f64> {
        match a.get(k) {
            None | Some(Value::Nil) => Ok(d),
            Some(Value::Num(x)) => Ok(*x),
            _ => Err(err::ARITH),
        }
    }
    fn normal(&mut self, mean: f64, sd: f64) -> f64 {
        let u1 = 1.0 - self.w.rng.unit();
        let u2 = self.w.rng.unit();
        mean + sd * ((pm::ln(u1) * -2.0).sqrt() * pm::cos(u2 * (2.0 * std::f64::consts::PI)))
    }
    fn builtin(&mut self, name: &str, a: Vec<Value>) -> R<Vec<Value>> {
        let n1 = |f: fn(f64) -> f64| -> R<Vec<Value>> { Ok(vec![Value::Num(f(Self::n(&a, 0)?))]) };
        let one = |x: f64| -> R<Vec<Value>> { Ok(vec![Value::Num(x)]) };
        match name {
            "sin" => n1(pm::sin),
            "cos" => n1(pm::cos),
            "tan" => n1(pm::tan),
            "asin" => n1(pm::asin),
            "acos" => n1(pm::acos),
            "atan" => n1(pm::atan),
            "exp" => n1(pm::exp),
            "abs" => n1(f64::abs),
            "floor" => n1(f64::floor),
            "ceil" => n1(f64::ceil),
            "sqrt" => n1(f64::sqrt),
            "atan2" => one(pm::atan2(Self::n(&a, 0)?, Self::n(&a, 1)?)),
            "pow" => one(pm::powf(Self::n(&a, 0)?, Self::n(&a, 1)?)),
            "log" => {
                let l = pm::ln(Self::n(&a, 0)?);
                one(if a.len() > 1 { l / pm::ln(Self::n(&a, 1)?) } else { l })
            }
            "log10" => one(pm::ln(Self::n(&a, 0)?) / std::f64::consts::LN_10),
            "fmod" => {
                let (x, y) = (Self::n(&a, 0)?, Self::n(&a, 1)?);
                one(x - (x / y).trunc() * y)
            }
            "modf" => {
                let x = Self::n(&a, 0)?;
                let ip = x.trunc();
                Ok(vec![Value::Num(ip), Value::Num(if x.abs() == f64::INFINITY { 0.0 } else { x - ip })])
            }
            "min" | "max" => {
                let mut m = Self::n(&a, 0)?;
                for k in 1..a.len() {
                    let x = Self::n(&a, k)?;
                    m = if name == "min" { if x < m { x } else { m } } else if x > m { x } else { m };
                }
                one(m)
            }
            "sinh" => {
                let x = Self::n(&a, 0)?;
                one((pm::exp(x) - pm::exp(-x)) * 0.5)
            }
            "cosh" => {
                let x = Self::n(&a, 0)?;
                one((pm::exp(x) + pm::exp(-x)) * 0.5)
            }
            "tanh" => {
                let x = Self::n(&a, 0)?;
                let e = pm::exp(x * 2.0);
                one(if x.abs() > 20.0 { if x < 0.0 { -1.0 } else { 1.0 } } else { (e - 1.0) / (e + 1.0) })
            }
            "deg" => one(Self::n(&a, 0)? * (180.0 / std::f64::consts::PI)),
            "rad" => one(Self::n(&a, 0)? * (std::f64::consts::PI / 180.0)),
            "random" => {
                let u = self.w.rng.unit();
                match a.len() {
                    0 => one(u),
                    1 => {
                        let m = Self::n(&a, 0)?.floor();
                        if m < 1.0 {
                            return Err(err::RANDOM);
                        }
                        one((u * m).floor() + 1.0)
                    }
                    _ => {
                        let (m, n) = (Self::n(&a, 0)?.floor(), Self::n(&a, 1)?.floor());
                        if m > n {
                            return Err(err::RANDOM);
                        }
                        one(m + (u * (n - m + 1.0)).floor())
                    }
                }
            }
            "randomseed" => {
                let x = Self::n(&a, 0)?;
                self.w.rng.seed(x);
                Ok(vec![])
            }
            "clamp" | "limit_range" => {
                let (x, lo, hi) = (Self::n(&a, 0)?, Self::n(&a, 1)?, Self::n(&a, 2)?);
                let m = if x > lo { x } else { lo };
                one(if m < hi { m } else { hi })
            }
            "parity" => {
                let x = Self::n(&a, 0)?;
                one(if x - (x / 2.0).floor() * 2.0 == 0.0 { 0.0 } else { 1.0 })
            }
            "sgn" => {
                let x = Self::n(&a, 0)?;
                one(if x > 0.0 { 1.0 } else if x < 0.0 { -1.0 } else { 0.0 })
            }
            "sign" => one(if Self::n(&a, 0)? < 0.0 { -1.0 } else { 1.0 }),
            "rescale" => {
                let (v, il, ih, ol, oh) = (Self::n(&a, 0)?, Self::n(&a, 1)?, Self::n(&a, 2)?, Self::n(&a, 3)?, Self::n(&a, 4)?);
                one(ol + (v - il) * (oh - ol) / (ih - il))
            }
            "norm" => {
                let (x, y) = (Self::n(&a, 0)?, Self::n(&a, 1)?);
                one((x * x + y * y).sqrt())
            }
            "range" => one((Self::n(&a, 0)? - Self::n(&a, 1)?).abs()),
            "note_to_freq" => {
                let (n, r) = (Self::n(&a, 0)?, Self::n_or(&a, 1, 440.0)?);
                one(r * pm::powf(2.0, (n - 69.0) / 12.0))
            }
            "freq_to_note" => {
                let (f, r) = (Self::n(&a, 0)?, Self::n_or(&a, 1, 440.0)?);
                one(pm::ln(f / r) / std::f64::consts::LN_2 * 12.0 + 69.0)
            }
            "lin_to_dB" => one(pm::ln(Self::n(&a, 0)?) / std::f64::consts::LN_10 * 20.0),
            "dB_to_lin" => one(pm::powf(10.0, Self::n(&a, 0)? / 20.0)),
            "gcd" | "lcm" => {
                let (x, y) = (Self::n(&a, 0)?, Self::n(&a, 1)?);
                let (mut p, mut q) = (x.abs(), y.abs());
                for _ in 0..4096 {
                    if q == 0.0 || q.is_nan() {
                        break;
                    }
                    let m = p - (p / q).floor() * q;
                    p = q;
                    q = m;
                }
                one(if name == "gcd" { p } else { (x / p * y).abs() })
            }
            "random_normal" => {
                let (m, sd) = (Self::n_or(&a, 0, 0.0)?, Self::n_or(&a, 1, 1.0)?);
                one(self.normal(m, sd))
            }
            "random_exponential" => {
                let l = Self::n(&a, 0)?;
                let u = self.w.rng.unit();
                one(if l <= 0.0 { f64::INFINITY } else { -pm::ln(1.0 - u) / l })
            }
            "random_bernoulli" => {
                let p = Self::n_or(&a, 0, 0.5)?;
                Ok(vec![Value::Bool(self.w.rng.unit() < p)])
            }
            "random_poisson" => {
                let l = Self::n(&a, 0)?;
                if l < 30.0 {
                    let lim = pm::exp(-l);
                    let (mut k, mut p) = (0.0, 1.0);
                    for _ in 0..100_000 {
                        k += 1.0;
                        p *= self.w.rng.unit();
                        if p <= lim {
                            break;
                        }
                    }
                    one(k - 1.0)
                } else {
                    let x = self.normal(l, l.sqrt());
                    let f = (x + 0.5).floor();
                    one(if f > 0.0 { f } else { 0.0 })
                }
            }
            "type" => Ok(vec![s(match a.first().unwrap_or(&Value::Nil) {
                Value::Nil => "nil",
                Value::Bool(_) => "boolean",
                Value::Num(_) => "number",
                Value::Str(_) => "string",
                Value::Table(_) => "table",
                _ => "function",
            })]),
            "tonumber" => Ok(vec![match a.first() {
                Some(Value::Num(x)) => Value::Num(*x),
                Some(Value::Str(_)) => return Err(err::STRING),
                _ => Value::Nil,
            }]),
            "print" => Ok(vec![]),
            "error" => Err(err::RAISED),
            "assert" => match a.first() {
                Some(v) if truthy(v) => Ok(vec![v.clone()]),
                _ => Err(err::RAISED),
            },
            "str.len" => match a.first() {
                Some(Value::Str(x)) => one(x.len() as f64),
                _ => Err(err::STRING),
            },
            "str.sub" | "str.byte" | "str.find" => self.string(name, a),
            _ => Err(err::RAISED),
        }
    }
    fn pos(a: &[Value], k: usize, d: f64, len: f64) -> R<f64> {
        let x = Self::n_or(a, k, d)?.floor();
        Ok(if x < 0.0 { len + x + 1.0 } else { x })
    }
    fn string(&mut self, name: &str, a: Vec<Value>) -> R<Vec<Value>> {
        let Some(Value::Str(st)) = a.first().cloned() else { return Err(err::STRING) };
        let len = st.len() as f64;
        match name {
            "str.sub" => {
                let i = Self::pos(&a, 1, 1.0, len)?.max(1.0);
                let j = Self::pos(&a, 2, -1.0, len)?.min(len);
                if i > j {
                    return Ok(vec![s("")]);
                }
                Ok(vec![Value::Str(st[i as usize - 1..j as usize].into())])
            }
            "str.byte" => {
                let i = Self::pos(&a, 1, 1.0, len)?;
                Ok(vec![if i >= 1.0 && i <= len { Value::Num(st[i as usize - 1] as f64) } else { Value::Nil }])
            }
            _ => {
                let Some(Value::Str(p)) = a.get(1).cloned() else { return Err(err::STRING) };
                let plain = a.get(3).map(truthy).unwrap_or(false);
                if !plain {
                    for c in p.iter() {
                        if b"^$()%.[]".contains(c) || (p.len() != 1 && b"*+-?".contains(c)) {
                            return Err(err::STRING);
                        }
                    }
                }
                let init = Self::pos(&a, 2, 1.0, len)?.max(1.0);
                if init > len + 1.0 {
                    return Ok(vec![Value::Nil, Value::Nil]);
                }
                let start = init as usize - 1;
                for k in start..=st.len() {
                    if k + p.len() > st.len() {
                        break;
                    }
                    if st[k..k + p.len()] == p[..] {
                        return Ok(vec![Value::Num(k as f64 + 1.0), Value::Num((k + p.len()) as f64)]);
                    }
                }
                Ok(vec![Value::Nil, Value::Nil])
            }
        }
    }
    fn field(t: &T, n: &str) -> Value {
        tget(t, &s(n))
    }
    fn fnum(t: &T, n: &str) -> R<f64> {
        match Self::field(t, n) {
            Value::Num(x) => Ok(x),
            _ => Err(err::ARITH),
        }
    }
    fn set(t: &T, n: &str, v: Value) {
        let _ = tset(t, &s(n), v);
    }
    fn method(&mut self, name: &str, a: Vec<Value>) -> R<Vec<Value>> {
        match name {
            "sub" => return self.builtin("str.sub", a),
            "find" => return self.builtin("str.find", a),
            "len" => return self.builtin("str.len", a),
            "byte" => return self.builtin("str.byte", a),
            _ => {}
        }
        let proto_of = |w: &World, v: &Value| -> Option<usize> {
            if let Value::Table(t) = v {
                let p = t.borrow().proto.clone()?;
                return w.protos.iter().position(|q| Rc::ptr_eq(q, &p));
            }
            None
        };
        match name {
            "new" => {
                let Value::Table(p) = &a[0] else { return Err(err::METHOD) };
                let k = self.w.protos.iter().position(|q| Rc::ptr_eq(q, p)).ok_or(err::METHOD)?;
                let t = match a.get(1) {
                    Some(Value::Table(t)) => t.clone(),
                    None | Some(Value::Nil) => new_table(),
                    _ => return Err(err::INDEX),
                };
                t.borrow_mut().proto = Some(p.clone());
                if k == 0 {
                    Self::set(&t, "block_factor", Value::Num(0.0));
                    let sr = Self::fnum(&t, "samplerate")?;
                    let bs = Self::fnum(&t, "block_size")?;
                    if sr == 0.0 || bs == 0.0 {
                        Self::set(&t, "calculate", Value::Bool(false));
                    }
                    Self::set(&t, "block_factor", Value::Num((sr / 48000.0) / (bs / 32.0)));
                }
                Ok(vec![Value::Table(t)])
            }
            "run" => {
                if proto_of(self.w, &a[0]) != Some(0) {
                    return Err(err::METHOD);
                }
                let Value::Table(t) = &a[0] else { unreachable!() };
                if !truthy(&Self::field(t, "calculate")) {
                    return Ok(vec![Value::Num(0.0)]);
                }
                let input = Self::n(&a, 1)?;
                if truthy(&Self::field(t, "is_first")) {
                    Self::set(t, "prior", Value::Num(input));
                    Self::set(t, "is_first", Value::Bool(false));
                }
                let up = Self::n_or(&a, 2, 0.2)?.max(0.0);
                let up = if up > 0.0 { up } else { 0.0 };
                let bf = Self::fnum(t, "block_factor")?;
                let lim = |r: f64| 1.0 / (bf * 10000.0 * pm::powf(r, 3.0) + 1.0);
                let ul = lim(up);
                let dl = match a.get(3) {
                    None | Some(Value::Nil) => ul,
                    _ => {
                        let d = Self::n(&a, 3)?;
                        lim(if d > 0.0 { d } else { 0.0 })
                    }
                };
                let prior = Self::fnum(t, "prior")?;
                let mut delta = input - prior;
                delta = if delta > -dl { delta } else { -dl };
                delta = if delta < ul { delta } else { ul };
                let np = prior + delta;
                Self::set(t, "prior", Value::Num(np));
                Ok(vec![Value::Num(np)])
            }
            "tick" => {
                if proto_of(self.w, &a[0]) != Some(1) {
                    return Err(err::METHOD);
                }
                let Value::Table(t) = &a[0] else { unreachable!() };
                let (ip, ph) = (Self::n(&a, 1)?, Self::n(&a, 2)?);
                let num = Self::fnum(t, "numerator")?;
                let den = Self::fnum(t, "denominator")?;
                let beat = (ip + ph) * num / den;
                let ib = beat.floor();
                Self::set(t, "intphase", Value::Num(ib));
                Self::set(t, "ibeat", Value::Num(ib - (ib / num).floor() * num));
                Self::set(t, "phase", Value::Num(beat - ib));
                let prior = Self::field(t, "prioribeat");
                Self::set(t, "newbeat", Value::Bool(!same(&Value::Num(ib), &prior)));
                Self::set(t, "prioribeat", Value::Num(ib));
                Ok(vec![])
            }
            "at" => {
                if proto_of(self.w, &a[0]) != Some(2) {
                    return Err(err::METHOD);
                }
                let Value::Table(t) = &a[0] else { unreachable!() };
                let p = Self::n(&a, 1)?;
                let (at, h, d) = (Self::fnum(t, "a")?, Self::fnum(t, "h")?, Self::fnum(t, "d")?);
                let ah = at + h;
                let r = if p <= 0.0 {
                    0.0
                } else if p < at {
                    p / at
                } else if p < ah {
                    1.0
                } else if p < ah + d {
                    1.0 - (p - ah) / d
                } else {
                    0.0
                };
                Ok(vec![Value::Num(r)])
            }
            _ => Err(err::METHOD),
        }
    }
}

struct Inst {
    state: T,
    flags: FormulaIo,
    active: bool,
}

/// The reference runner: same surface as [`super::Runner`].
pub struct RefRunner {
    w: World,
    inst: Vec<Inst>,
    pub error: Option<i32>,
}

impl RefRunner {
    pub fn new(chunk: &Block, capacity: usize) -> RefRunner {
        let mk = |kv: &[(&str, Value)]| {
            let t = new_table();
            for (k, v) in kv {
                Interp::set(&t, k, v.clone());
            }
            t
        };
        let protos = [
            mk(&[
                ("prior", Value::Num(0.0)),
                ("block_factor", Value::Num(1.0)),
                ("block_size", Value::Num(0.0)),
                ("samplerate", Value::Num(0.0)),
                ("calculate", Value::Bool(true)),
                ("is_first", Value::Bool(true)),
            ]),
            mk(&[
                ("numerator", Value::Num(1.0)),
                ("denominator", Value::Num(1.0)),
                ("prioribeat", Value::Num(-1.0)),
                ("newbeat", Value::Bool(false)),
                ("intphase", Value::Num(0.0)),
                ("ibeat", Value::Num(0.0)),
                ("phase", Value::Num(0.0)),
            ]),
            mk(&[("a", Value::Num(0.1)), ("h", Value::Num(0.1)), ("d", Value::Num(0.7))]),
        ];
        let m = mk(&[("Slew", Value::Table(protos[0].clone())), ("ClockDivider", Value::Table(protos[1].clone())), ("AHDEnvelope", Value::Table(protos[2].clone()))]);
        let surge = mk(&[("mod", Value::Table(m))]);
        let mut w = World { globals: Default::default(), rng: Rng(RNG_START), protos, surge, init: None, process: None };
        let mut error = None;
        {
            let mut it = Interp { w: &mut w, sb: Sb::Top };
            if let Err(e) = it.block(chunk, Scope::default()) {
                error = Some(e);
            }
        }
        let take = |w: &mut World, n: &str| match w.globals[0].remove(n) {
            Some(Value::Func(c)) => Some(Rc::new(Closure { body: c.body.clone(), env: c.env.clone(), sandbox: if n == "init" { Sb::Init } else { Sb::Process } })),
            _ => None,
        };
        w.init = take(&mut w, "init");
        w.process = take(&mut w, "process");
        RefRunner { w, inst: (0..capacity).map(|_| Inst { state: new_table(), flags: FormulaIo::default(), active: false }).collect(), error }
    }

    pub fn acquire(&mut self) -> Option<usize> {
        let k = self.inst.iter().position(|i| !i.active)?;
        self.inst[k].active = true;
        Some(k)
    }

    fn write(t: &T, i: &FormulaInputs, at_init: bool) {
        let set = |n: &str, v: Value| Interp::set(t, n, v);
        let num = |n: &str, x: f64| set(n, Value::Num(x));
        if at_init {
            num("samplerate", i.samplerate);
            num("block_size", i.block_size);
            for n in ["clamp_output", "use_amplitude", "use_envelope", "use_rate"] {
                set(n, Value::Bool(true));
            }
        }
        for (n, e) in ["delay", "attack", "hold", "decay", "sustain", "release"].iter().zip(i.eg) {
            num(n, e as f64);
        }
        num("rate", i.rate as f64);
        num("startphase", i.startphase as f64);
        num("deform", i.deform as f64);
        num("amplitude", i.amplitude as f64);
        num("tempo", i.tempo as f64);
        num("songpos", i.songpos);
        set("is_playing", Value::Bool(i.is_playing));
        num("pb_range_up", i.pb_range_up as f64);
        num("pb_range_dn", i.pb_range_dn as f64);
        num("mpe_bendrange", i.mpe_bendrange as f64);
        num("voice_count", i.voice_count);
        num("poly_limit", i.poly_limit);
        num("scene_mode", i.scene_mode);
        num("play_mode", i.play_mode);
        num("split_point", i.split_point);
        num("lfo_id", i.lfo_id);
        set("mpe_enabled", Value::Bool(i.mpe_enabled));
        set("is_rendering_to_ui", Value::Bool(i.is_rendering_to_ui));
        set("retrigger_AEG", Value::Bool(false));
        set("retrigger_FEG", Value::Bool(false));
        set("is_voice", Value::Bool(i.voice.is_some()));
        if let Some(v) = &i.voice {
            num("channel", v.channel);
            num("key", v.key);
            num("velocity", v.velocity);
            num("voice_id", v.voice_id);
            num("tuned_key", v.tuned_key);
            set("released", Value::Bool(v.released));
            if !at_init {
                num("rel_velocity", v.rel_velocity);
                num("poly_at", v.poly_at as f64);
                num("mpe_bend", v.mpe_bend as f64);
                num("mpe_timbre", v.mpe_timbre as f64);
                num("mpe_pressure", v.mpe_pressure as f64);
            }
        }
        if !at_init {
            num("intphase", i.intphase);
            num("cycle", i.intphase);
            num("phase", i.phase as f64);
            num("pb", i.pb as f64);
            num("chan_at", i.chan_at as f64);
            num("cc_mw", i.cc_mw as f64);
            num("cc_breath", i.cc_breath as f64);
            num("cc_expr", i.cc_expr as f64);
            num("cc_sus", i.cc_sus as f64);
            num("lowest_key", i.lowest_key);
            num("highest_key", i.highest_key);
            num("latest_key", i.latest_key);
        }
        let m = new_table();
        for (j, x) in i.macros.iter().enumerate() {
            let _ = tset(&m, &Value::Num(j as f64 + 1.0), Value::Num(*x as f64));
        }
        set("macros", Value::Table(m));
    }

    fn flags(t: &T, old: FormulaIo) -> FormulaIo {
        let f = |n: &str| match Interp::field(t, n) {
            Value::Nil => false,
            Value::Bool(b) => b,
            Value::Num(x) => x != 0.0,
            _ => true,
        };
        FormulaIo { use_envelope: f("use_envelope"), use_amplitude: f("use_amplitude"), clamp_output: f("clamp_output"), retrigger_aeg: f("retrigger_AEG"), retrigger_feg: f("retrigger_FEG"), ..old }
    }

    pub fn start(&mut self, k: usize, i: &FormulaInputs) {
        if self.error.is_some() {
            return;
        }
        let st = new_table();
        let subs = new_table();
        Interp::set(&subs, "macros", Value::Table(new_table()));
        Interp::set(&st, "subscriptions", Value::Table(subs));
        Self::write(&st, i, true);
        self.inst[k].state = st.clone();
        self.inst[k].flags = FormulaIo::default();
        if let Some(c) = self.w.init.clone() {
            let mut it = Interp { w: &mut self.w, sb: Sb::Init };
            match it.call(Value::Func(c), vec![Value::Table(st)]) {
                Ok(r) => match r.into_iter().next() {
                    Some(Value::Table(t)) => self.inst[k].state = t,
                    _ => self.error = Some(err::INIT_RETURN),
                },
                Err(e) => self.error = Some(e),
            }
        }
        let t = self.inst[k].state.clone();
        self.inst[k].flags = Self::flags(&t, self.inst[k].flags);
    }

    pub fn process(&mut self, k: usize, i: &FormulaInputs) -> FormulaIo {
        let zero = FormulaIo::default();
        if self.error.is_some() {
            return zero;
        }
        let st = self.inst[k].state.clone();
        Self::write(&st, i, false);
        let c = self.w.process.clone().unwrap();
        let r = {
            let mut it = Interp { w: &mut self.w, sb: Sb::Process };
            it.call(Value::Func(c), vec![Value::Table(st.clone())])
        };
        let r = match r {
            Ok(r) => r.into_iter().next().unwrap_or(Value::Nil),
            Err(e) => {
                self.error = Some(e);
                return zero;
            }
        };
        let mut out = [0f64; 16];
        let mut count = 1;
        match r {
            Value::Num(x) => {
                out[0] = x;
                self.inst[k].flags.retrigger_aeg = false;
                self.inst[k].flags.retrigger_feg = false;
            }
            Value::Table(t) => {
                if !Rc::ptr_eq(&t, &st) {
                    // The reference takes the table as the new state; the
                    // compiler reports it (keep the two in step).
                    self.error = Some(err::RETURN_TABLE);
                    return zero;
                }
                self.inst[k].flags = Self::flags(&t, self.inst[k].flags);
                match Interp::field(&t, "output") {
                    Value::Num(x) => out[0] = x,
                    Value::Table(o) => {
                        count = 0;
                        for (j, slot) in out.iter_mut().enumerate() {
                            match tget(&o, &Value::Num(j as f64 + 1.0)) {
                                Value::Nil => break,
                                Value::Num(x) => *slot = x,
                                _ => *slot = 0.0,
                            }
                            count = j + 1;
                        }
                        count = count.max(1);
                    }
                    _ => {}
                }
            }
            _ => {
                self.error = Some(err::RETURN_TABLE);
                return zero;
            }
        }
        let flags = self.inst[k].flags;
        let mut res = FormulaIo { count, ..flags };
        for j in 0..count {
            res.outputs[j] = out[j] as f32;
        }
        res
    }
}

