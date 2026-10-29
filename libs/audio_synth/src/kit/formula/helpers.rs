//! The builtin library and the helper objects, lowered to AIR. Behaviour is
//! from MODULATION.md section 8.3 and FORMULA-ADDENDUM.md (sections 0-3,
//! 7), written in our own code.
//!
//! Helper objects are tables whose prototype (`surge.mod.Slew` and the
//! others, real tables made by the load program) supplies the fields they
//! do not set themselves; a script may change a prototype's defaults.

use super::compile::*;
use makepad_script_compute::ir::{Bin, Cmp, Stmt, Ty, Un, Val};
use makepad_script_compute::lower::MathFn;

impl Cx<'_> {
    fn arg_num(&mut self, a: &[V], k: usize) -> Val {
        let v = match a.get(k) {
            Some(v) => *v,
            None => self.nil(),
        };
        self.to_num(v)
    }
    /// A numeric argument, or `default` when absent or nil.
    fn arg_num_or(&mut self, a: &[V], k: usize, default: f64) -> Val {
        let Some(v) = a.get(k).copied() else {
            return self.cd(default);
        };
        let nil = self.is_nil(v);
        if self.const_bool(nil) == Some(true) {
            return self.cd(default);
        }
        let notnil = self.not(nil);
        let notnum = self.cmpic(Cmp::Ne, v.tag, T_NUM);
        let bad = self.and(notnil, notnum);
        self.fail_if(bad, err::ARITH);
        let d = self.cd(default);
        self.b.sel(nil, d, v.num)
    }
    fn math1(&mut self, f: MathFn, x: Val) -> Val {
        self.b.math(f, &[x])
    }
    fn nums(&mut self, x: Val) -> Vec<CV> {
        vec![CV::Rt(self.num(x))]
    }
    fn ln(&mut self, x: Val) -> Val {
        self.math1(MathFn::Ln, x)
    }
    fn exp(&mut self, x: Val) -> Val {
        self.math1(MathFn::Exp, x)
    }
    fn floor(&mut self, x: Val) -> Val {
        self.du(Un::FloorD, x)
    }

    pub(super) fn builtin(&mut self, name: &str, a: Vec<V>, _want: usize) -> CR<Vec<CV>> {
        let un = |f: MathFn| Some(f);
        let m1 = match name {
            "sin" => un(MathFn::Sin),
            "cos" => un(MathFn::Cos),
            "tan" => un(MathFn::Tan),
            "asin" => un(MathFn::Asin),
            "acos" => un(MathFn::Acos),
            "atan" => un(MathFn::Atan),
            "exp" => un(MathFn::Exp),
            _ => None,
        };
        if let Some(f) = m1 {
            let x = self.arg_num(&a, 0);
            let r = self.math1(f, x);
            return Ok(self.nums(r));
        }
        let du = match name {
            "abs" => Some(Un::AbsD),
            "floor" => Some(Un::FloorD),
            "ceil" => Some(Un::CeilD),
            "sqrt" => Some(Un::SqrtD),
            _ => None,
        };
        if let Some(op) = du {
            let x = self.arg_num(&a, 0);
            let r = self.du(op, x);
            return Ok(self.nums(r));
        }
        Ok(match name {
            "atan2" => {
                let y = self.arg_num(&a, 0);
                let x = self.arg_num(&a, 1);
                let r = self.b.math(MathFn::Atan2, &[y, x]);
                self.nums(r)
            }
            "pow" => {
                let x = self.arg_num(&a, 0);
                let y = self.arg_num(&a, 1);
                let r = self.b.math(MathFn::Pow, &[x, y]);
                self.nums(r)
            }
            "log" => {
                let x = self.arg_num(&a, 0);
                let l = self.ln(x);
                let r = if a.len() > 1 {
                    let base = self.arg_num(&a, 1);
                    let lb = self.ln(base);
                    self.db(Bin::DivD, l, lb)
                } else {
                    l
                };
                self.nums(r)
            }
            "log10" => {
                let x = self.arg_num(&a, 0);
                let r = self.log10(x);
                self.nums(r)
            }
            "fmod" => {
                let x = self.arg_num(&a, 0);
                let y = self.arg_num(&a, 1);
                let q = self.db(Bin::DivD, x, y);
                let t = self.du(Un::TruncD, q);
                let m = self.db(Bin::MulD, t, y);
                let r = self.db(Bin::SubD, x, m);
                self.nums(r)
            }
            "modf" => {
                let x = self.arg_num(&a, 0);
                let ip = self.du(Un::TruncD, x);
                let fr = self.db(Bin::SubD, x, ip);
                let ax = self.du(Un::AbsD, x);
                let inf = self.cmpdc(Cmp::Eq, ax, f64::INFINITY);
                let z = self.cd(0.0);
                let fr = self.b.sel(inf, z, fr);
                vec![CV::Rt(self.num(ip)), CV::Rt(self.num(fr))]
            }
            "min" | "max" => {
                if a.is_empty() {
                    return cerr(self.line, format!("math.{} needs an argument", name));
                }
                let mut m = self.arg_num(&a, 0);
                for k in 1..a.len() {
                    let x = self.arg_num(&a, k);
                    m = self.db(if name == "min" { Bin::MinD } else { Bin::MaxD }, x, m);
                }
                self.nums(m)
            }
            "sinh" | "cosh" => {
                let x = self.arg_num(&a, 0);
                let e = self.exp(x);
                let nx = self.du(Un::NegD, x);
                let en = self.exp(nx);
                let s = self.db(if name == "sinh" { Bin::SubD } else { Bin::AddD }, e, en);
                let r = self.dbc(Bin::MulD, s, 0.5);
                self.nums(r)
            }
            "tanh" => {
                let x = self.arg_num(&a, 0);
                let r = self.tanh(x);
                self.nums(r)
            }
            "deg" => {
                let x = self.arg_num(&a, 0);
                let r = self.dbc(Bin::MulD, x, 180.0 / std::f64::consts::PI);
                self.nums(r)
            }
            "rad" => {
                let x = self.arg_num(&a, 0);
                let r = self.dbc(Bin::MulD, x, std::f64::consts::PI / 180.0);
                self.nums(r)
            }
            "random" => {
                let r = self.random(&a);
                self.nums(r)
            }
            "randomseed" => {
                let x = self.arg_num(&a, 0);
                self.rng_seed(x);
                vec![]
            }
            "clamp" | "limit_range" => {
                let x = self.arg_num(&a, 0);
                let lo = self.arg_num(&a, 1);
                let hi = self.arg_num(&a, 2);
                let m = self.db(Bin::MaxD, x, lo);
                let r = self.db(Bin::MinD, m, hi);
                self.nums(r)
            }
            "parity" => {
                let x = self.arg_num(&a, 0);
                let two = self.cd(2.0);
                let m = self.fmod_floor(x, two);
                let even = self.cmpdc(Cmp::Eq, m, 0.0);
                let z = self.cd(0.0);
                let o = self.cd(1.0);
                let r = self.b.sel(even, z, o);
                self.nums(r)
            }
            "sgn" => {
                let x = self.arg_num(&a, 0);
                let pos = self.cmpdc(Cmp::Gt, x, 0.0);
                let neg = self.cmpdc(Cmp::Lt, x, 0.0);
                let one = self.cd(1.0);
                let m1 = self.cd(-1.0);
                let z = self.cd(0.0);
                let r = self.b.sel(neg, m1, z);
                let r = self.b.sel(pos, one, r);
                self.nums(r)
            }
            "sign" => {
                let x = self.arg_num(&a, 0);
                let neg = self.cmpdc(Cmp::Lt, x, 0.0);
                let one = self.cd(1.0);
                let m1 = self.cd(-1.0);
                let r = self.b.sel(neg, m1, one);
                self.nums(r)
            }
            "rescale" => {
                let v = self.arg_num(&a, 0);
                let il = self.arg_num(&a, 1);
                let ih = self.arg_num(&a, 2);
                let ol = self.arg_num(&a, 3);
                let oh = self.arg_num(&a, 4);
                let d = self.db(Bin::SubD, v, il);
                let os = self.db(Bin::SubD, oh, ol);
                let is = self.db(Bin::SubD, ih, il);
                let m = self.db(Bin::MulD, d, os);
                let q = self.db(Bin::DivD, m, is);
                let r = self.db(Bin::AddD, ol, q);
                self.nums(r)
            }
            "norm" => {
                let x = self.arg_num(&a, 0);
                let y = self.arg_num(&a, 1);
                let xx = self.db(Bin::MulD, x, x);
                let yy = self.db(Bin::MulD, y, y);
                let s = self.db(Bin::AddD, xx, yy);
                let r = self.du(Un::SqrtD, s);
                self.nums(r)
            }
            "range" => {
                let x = self.arg_num(&a, 0);
                let y = self.arg_num(&a, 1);
                let d = self.db(Bin::SubD, x, y);
                let r = self.du(Un::AbsD, d);
                self.nums(r)
            }
            "note_to_freq" => {
                let n = self.arg_num(&a, 0);
                let rf = self.arg_num_or(&a, 1, 440.0);
                let d = self.dbc(Bin::SubD, n, 69.0);
                let e = self.dbc(Bin::DivD, d, 12.0);
                let two = self.cd(2.0);
                let p = self.b.math(MathFn::Pow, &[two, e]);
                let r = self.db(Bin::MulD, rf, p);
                self.nums(r)
            }
            "freq_to_note" => {
                let f = self.arg_num(&a, 0);
                let rf = self.arg_num_or(&a, 1, 440.0);
                let q = self.db(Bin::DivD, f, rf);
                let l = self.ln(q);
                let l2 = self.dbc(Bin::DivD, l, std::f64::consts::LN_2);
                let m = self.dbc(Bin::MulD, l2, 12.0);
                let r = self.dbc(Bin::AddD, m, 69.0);
                self.nums(r)
            }
            "lin_to_dB" => {
                let x = self.arg_num(&a, 0);
                let l = self.log10(x);
                let r = self.dbc(Bin::MulD, l, 20.0);
                self.nums(r)
            }
            "dB_to_lin" => {
                let x = self.arg_num(&a, 0);
                let e = self.dbc(Bin::DivD, x, 20.0);
                let ten = self.cd(10.0);
                let r = self.b.math(MathFn::Pow, &[ten, e]);
                self.nums(r)
            }
            "gcd" | "lcm" => {
                let x = self.arg_num(&a, 0);
                let y = self.arg_num(&a, 1);
                let g = self.gcd(x, y);
                let r = if name == "gcd" {
                    g
                } else {
                    let q = self.db(Bin::DivD, x, g);
                    let m = self.db(Bin::MulD, q, y);
                    self.du(Un::AbsD, m)
                };
                self.nums(r)
            }
            "random_normal" => {
                let mean = self.arg_num_or(&a, 0, 0.0);
                let sd = self.arg_num_or(&a, 1, 1.0);
                let r = self.normal(mean, sd);
                self.nums(r)
            }
            "random_exponential" => {
                let l = self.arg_num(&a, 0);
                let u = self.rng_unit();
                let one = self.cd(1.0);
                let om = self.db(Bin::SubD, one, u);
                let ln = self.ln(om);
                let nl = self.du(Un::NegD, ln);
                let q = self.db(Bin::DivD, nl, l);
                let le0 = self.cmpdc(Cmp::Le, l, 0.0);
                let inf = self.cd(f64::INFINITY);
                let r = self.b.sel(le0, inf, q);
                self.nums(r)
            }
            "random_bernoulli" => {
                let p = self.arg_num_or(&a, 0, 0.5);
                let u = self.rng_unit();
                let c = self.cmpd(Cmp::Lt, u, p);
                vec![CV::Rt(self.boolean(c))]
            }
            "random_poisson" => {
                let l = self.arg_num(&a, 0);
                let r = self.poisson(l)?;
                self.nums(r)
            }
            "type" => {
                let v = a.first().copied().unwrap_or_else(|| self.nil());
                let names = ["nil", "boolean", "number", "string", "table", "function"];
                let mut r = self.strc("nil").num;
                for (k, n) in names.iter().enumerate().skip(1) {
                    let is = self.cmpic(Cmp::Eq, v.tag, k as i32);
                    let p = self.strc(n).num;
                    r = self.b.sel(is, p, r);
                }
                let t = self.ci(T_STR);
                vec![CV::Rt(V { tag: t, num: r })]
            }
            "tonumber" => {
                let v = a.first().copied().unwrap_or_else(|| self.nil());
                let is_str = self.cmpic(Cmp::Eq, v.tag, T_STR);
                self.fail_if(is_str, err::STRING);
                let is_num = self.cmpic(Cmp::Eq, v.tag, T_NUM);
                let nt = self.ci(T_NIL);
                let tag = self.b.sel(is_num, v.tag, nt);
                vec![CV::Rt(V { tag, num: v.num })]
            }
            "print" => vec![],
            "error" => {
                let t = self.cb(true);
                self.fail_if(t, err::RAISED);
                vec![]
            }
            "assert" => {
                let v = a.first().copied().unwrap_or_else(|| self.nil());
                let t = self.truthy(v);
                let f = self.not(t);
                self.fail_if(f, err::RAISED);
                vec![CV::Rt(v)]
            }
            "str.len" => {
                let s = self.str_arg(&a)?;
                vec![CV::Rt(self.length(s))]
            }
            "str.sub" => vec![CV::Rt(self.str_sub(&a)?)],
            "str.byte" => vec![CV::Rt(self.str_byte(&a)?)],
            "str.find" => self.str_find(&a)?,
            _ => return cerr(self.line, format!("'{}' is not supported", name)),
        })
    }

    fn log10(&mut self, x: Val) -> Val {
        let l = self.ln(x);
        self.dbc(Bin::DivD, l, std::f64::consts::LN_10)
    }
    fn tanh(&mut self, x: Val) -> Val {
        let x2 = self.dbc(Bin::MulD, x, 2.0);
        let e = self.exp(x2);
        let n = self.dbc(Bin::SubD, e, 1.0);
        let d = self.dbc(Bin::AddD, e, 1.0);
        let q = self.db(Bin::DivD, n, d);
        let ax = self.du(Un::AbsD, x);
        let big = self.cmpdc(Cmp::Gt, ax, 20.0);
        let neg = self.cmpdc(Cmp::Lt, x, 0.0);
        let one = self.cd(1.0);
        let m1 = self.cd(-1.0);
        let s = self.b.sel(neg, m1, one);
        self.b.sel(big, s, q)
    }
    /// `math.random()`, `random(m)`, `random(m, n)`.
    fn random(&mut self, a: &[V]) -> Val {
        let u = self.rng_unit();
        match a.len() {
            0 => u,
            1 => {
                let m = self.arg_num(a, 0);
                let m = self.floor(m);
                let bad = self.cmpdc(Cmp::Lt, m, 1.0);
                self.fail_if(bad, err::RANDOM);
                let x = self.db(Bin::MulD, u, m);
                let f = self.floor(x);
                self.dbc(Bin::AddD, f, 1.0)
            }
            _ => {
                let m = self.arg_num(a, 0);
                let n = self.arg_num(a, 1);
                let m = self.floor(m);
                let n = self.floor(n);
                let bad = self.cmpd(Cmp::Gt, m, n);
                self.fail_if(bad, err::RANDOM);
                let w = self.db(Bin::SubD, n, m);
                let w = self.dbc(Bin::AddD, w, 1.0);
                let x = self.db(Bin::MulD, u, w);
                let f = self.floor(x);
                self.db(Bin::AddD, m, f)
            }
        }
    }
    /// Box-Muller with u1 = 1 - random(), u2 = random().
    fn normal(&mut self, mean: Val, sd: Val) -> Val {
        let u = self.rng_unit();
        let one = self.cd(1.0);
        let u1 = self.db(Bin::SubD, one, u);
        let u2 = self.rng_unit();
        let l = self.ln(u1);
        let m2 = self.dbc(Bin::MulD, l, -2.0);
        let r = self.du(Un::SqrtD, m2);
        let ang = self.dbc(Bin::MulD, u2, 2.0 * std::f64::consts::PI);
        let c = self.math1(MathFn::Cos, ang);
        let z = self.db(Bin::MulD, r, c);
        let s = self.db(Bin::MulD, sd, z);
        self.db(Bin::AddD, mean, s)
    }
    fn poisson(&mut self, l: Val) -> CR<Val> {
        let r = self.b.var(Ty::F64);
        let small = self.cmpdc(Cmp::Lt, l, 30.0);
        self.if_else(
            small,
            |s| {
                let nl = s.du(Un::NegD, l);
                let lim = s.exp(nl);
                let k = s.b.var(Ty::F64);
                let p = s.b.var(Ty::F64);
                let z = s.cd(0.0);
                let o = s.cd(1.0);
                s.b.set(k, z);
                s.b.set(p, o);
                s.internal_loop(100_000, |s| {
                    let kv = s.b.get(k);
                    let k1 = s.dbc(Bin::AddD, kv, 1.0);
                    s.b.set(k, k1);
                    let u = s.rng_unit();
                    let pv = s.b.get(p);
                    let p1 = s.db(Bin::MulD, pv, u);
                    s.b.set(p, p1);
                    let done = s.cmpd(Cmp::Le, p1, lim);
                    s.break_if(done);
                });
                let kv = s.b.get(k);
                let km = s.dbc(Bin::SubD, kv, 1.0);
                s.b.set(r, km);
                Ok(())
            },
            |s| {
                let sd = s.du(Un::SqrtD, l);
                let n = s.normal(l, sd);
                let h = s.dbc(Bin::AddD, n, 0.5);
                let f = s.floor(h);
                let f = s.dbc(Bin::MaxD, f, 0.0);
                s.b.set(r, f);
                Ok(())
            },
        )?;
        Ok(self.b.get(r))
    }
    /// Euclid with floored remainders.
    fn gcd(&mut self, x: Val, y: Val) -> Val {
        let a = self.b.var(Ty::F64);
        let b = self.b.var(Ty::F64);
        let ax = self.du(Un::AbsD, x);
        let ay = self.du(Un::AbsD, y);
        self.b.set(a, ax);
        self.b.set(b, ay);
        self.internal_loop(4096, |s| {
            let bv = s.b.get(b);
            let z = s.cmpdc(Cmp::Eq, bv, 0.0);
            let nan = s.cmpd(Cmp::Ne, bv, bv);
            let stop = s.or(z, nan);
            s.break_if(stop);
            let av = s.b.get(a);
            let m = s.fmod_floor(av, bv);
            s.b.set(a, bv);
            s.b.set(b, m);
        });
        self.b.get(a)
    }

    // -- strings --------------------------------------------------------------
    fn str_arg(&mut self, a: &[V]) -> CR<V> {
        let s = match a.first() {
            Some(s) => *s,
            None => return cerr(self.line, "string function without a string"),
        };
        let bad = self.cmpic(Cmp::Ne, s.tag, T_STR);
        self.fail_if(bad, err::STRING);
        Ok(s)
    }
    /// A position argument: negative counts from the end (`len + i + 1`).
    fn str_pos(&mut self, a: &[V], k: usize, default: f64, len: Val) -> Val {
        let x = self.arg_num_or(a, k, default);
        let x = self.floor(x);
        let ld = self.i2d(len);
        let neg = self.cmpdc(Cmp::Lt, x, 0.0);
        let fromend = self.db(Bin::AddD, ld, x);
        let fromend = self.dbc(Bin::AddD, fromend, 1.0);
        self.b.sel(neg, fromend, x)
    }
    fn str_sub(&mut self, a: &[V]) -> CR<V> {
        let s = self.str_arg(a)?;
        let (off, len) = self.str_parts(s.num);
        let i = self.str_pos(a, 1, 1.0, len);
        let j = self.str_pos(a, 2, -1.0, len);
        let i = self.dbc(Bin::MaxD, i, 1.0);
        let ld = self.i2d(len);
        let j = self.db(Bin::MinD, j, ld);
        let n = self.db(Bin::SubD, j, i);
        let n = self.dbc(Bin::AddD, n, 1.0);
        let n = self.dbc(Bin::MaxD, n, 0.0);
        let ii = self.d2i(i);
        let o = self.ib(Bin::AddI, off, ii);
        let o = self.addi(o, -1);
        let ni = self.d2i(n);
        let empty = self.cmpic(Cmp::Eq, ni, 0);
        let o = self.b.sel(empty, off, o);
        let p = self.slice_payload(o, ni);
        let t = self.ci(T_STR);
        Ok(V { tag: t, num: p })
    }
    fn str_byte(&mut self, a: &[V]) -> CR<V> {
        let s = self.str_arg(a)?;
        let (off, len) = self.str_parts(s.num);
        let i = self.str_pos(a, 1, 1.0, len);
        let ld = self.i2d(len);
        let ge = self.cmpdc(Cmp::Ge, i, 1.0);
        let le = self.cmpd(Cmp::Le, i, ld);
        let ok = self.and(ge, le);
        let ii = self.d2i(i);
        let at = self.ib(Bin::AddI, off, ii);
        let at = self.addi(at, -1);
        let byte = self.pool_byte(at);
        let bd = self.i2d(byte);
        let nt = self.ci(T_NIL);
        let numt = self.ci(T_NUM);
        let tag = self.b.sel(ok, numt, nt);
        let z = self.cd(0.0);
        let num = self.b.sel(ok, bd, z);
        Ok(V { tag, num })
    }
    /// `s:find(pat [, init [, plain]])` for plain patterns: a pattern with
    /// a magic character raises an error (a lone `* + - ?` is literal).
    fn str_find(&mut self, a: &[V]) -> CR<Vec<CV>> {
        let s = self.str_arg(a)?;
        let p = match a.get(1) {
            Some(p) => *p,
            None => return cerr(self.line, "string.find without a pattern"),
        };
        let bad = self.cmpic(Cmp::Ne, p.tag, T_STR);
        self.fail_if(bad, err::STRING);
        let (so, sl) = self.str_parts(s.num);
        let (po, pl) = self.str_parts(p.num);
        let plain = match a.get(3) {
            Some(v) => self.truthy(*v),
            None => self.cb(false),
        };
        // Magic characters (none when plain).
        let magic = self.b.var(Ty::Bool);
        let f = self.cb(false);
        self.b.set(magic, f);
        let np = self.not(plain);
        self.when(np, |s| {
            let single = s.cmpic(Cmp::Eq, pl, 1);
            let j = s.b.var(Ty::I32);
            let z = s.ci(0);
            s.b.set(j, z);
            s.internal_loop(STR_LOOP, |s| {
                let jv = s.b.get(j);
                let done = s.cmpi(Cmp::Ge, jv, pl);
                s.break_if(done);
                let at = s.ib(Bin::AddI, po, jv);
                let c = s.pool_byte(at);
                let mut any = s.cb(false);
                for ch in b"^$()%.[]" {
                    let e = s.cmpic(Cmp::Eq, c, *ch as i32);
                    any = s.or(any, e);
                }
                let mut quant = s.cb(false);
                for ch in b"*+-?" {
                    let e = s.cmpic(Cmp::Eq, c, *ch as i32);
                    quant = s.or(quant, e);
                }
                let ns = s.not(single);
                let q = s.and(quant, ns);
                let m = s.or(any, q);
                s.when(m, |s| {
                    let t = s.cb(true);
                    s.b.set(magic, t);
                });
                let jn = s.addi(jv, 1);
                s.b.set(j, jn);
            });
        });
        let mg = self.b.get(magic);
        self.fail_if(mg, err::STRING);
        let init = self.str_pos(a, 2, 1.0, sl);
        let init = self.dbc(Bin::MaxD, init, 1.0);
        let r0 = self.var_pair();
        let r1 = self.var_pair();
        let nil = self.nil();
        self.set_pair(r0, nil);
        self.set_pair(r1, nil);
        let sld = self.i2d(sl);
        let past = self.dbc(Bin::AddD, sld, 1.0);
        let in_range = self.cmpd(Cmp::Le, init, past);
        self.when(in_range, |s| {
            let start = s.d2i(init);
            let start = s.addi(start, -1);
            let k = s.b.var(Ty::I32);
            s.b.set(k, start);
            s.internal_loop(STR_LOOP, |s| {
                let kv = s.b.get(k);
                let end = s.ib(Bin::AddI, kv, pl);
                let over = s.cmpi(Cmp::Gt, end, sl);
                s.break_if(over);
                let at = s.ib(Bin::AddI, so, kv);
                let e = s.bytes_eq(at, po, pl);
                s.b.open();
                let k1 = s.addi(kv, 1);
                let d1 = s.i2d(k1);
                let v1 = s.num(d1);
                s.set_pair(r0, v1);
                let de = s.i2d(end);
                let v2 = s.num(de);
                s.set_pair(r1, v2);
                s.b.push(Stmt::Break(0));
                let arm = s.b.close();
                s.b.push(Stmt::If(e, arm, Vec::new()));
                let kn = s.addi(kv, 1);
                s.b.set(k, kn);
            });
        });
        let v0 = self.get_pair(r0);
        let v1 = self.get_pair(r1);
        Ok(vec![CV::Rt(v0), CV::Rt(v1)])
    }

    // -- helper objects ---------------------------------------------------------
    /// The load program's prototypes, with their defaults (addendum 1-3).
    pub(super) fn prototypes(&mut self) -> CR<()> {
        for _ in 0..PINNED {
            self.alloc(0);
        }
        let n = |s: &mut Self, x: f64| s.numc(x);
        let slew = self.table_const(PROTO_SLEW);
        for (k, v) in [("prior", 0.0), ("block_factor", 1.0), ("block_size", 0.0), ("samplerate", 0.0)] {
            let v = n(self, v);
            self.set_field(slew, k, v)?;
        }
        for k in ["calculate", "is_first"] {
            let t = self.boolc(true);
            self.set_field(slew, k, t)?;
        }
        let clock = self.table_const(PROTO_CLOCK);
        for (k, v) in [("numerator", 1.0), ("denominator", 1.0), ("prioribeat", -1.0), ("intphase", 0.0), ("ibeat", 0.0), ("phase", 0.0)] {
            let v = n(self, v);
            self.set_field(clock, k, v)?;
        }
        let f = self.boolc(false);
        self.set_field(clock, "newbeat", f)?;
        let ahd = self.table_const(PROTO_AHD);
        for (k, v) in [("a", 0.1), ("h", 0.1), ("d", 0.7)] {
            let v = n(self, v);
            self.set_field(ahd, k, v)?;
        }
        Ok(())
    }

    fn field_num(&mut self, t: V, name: &str) -> CR<Val> {
        let v = self.field(t, name)?;
        Ok(self.to_num(v))
    }
    fn set_num(&mut self, t: V, name: &str, x: Val) -> CR<()> {
        let v = self.num(x);
        self.set_field(t, name, v)
    }

    /// `Proto:new(t)`: `t` (or a new table) read through to the prototype.
    pub(super) fn helper_new(&mut self, a: Vec<V>) -> CR<V> {
        let proto = match self.const_f64(a[0].num) {
            Some(p) if self.const_i32(a[0].tag) == Some(T_TABLE) && (p as u32) < PINNED && p.fract() == 0.0 => p as u32,
            _ => return cerr(self.line, "':new' is only supported on surge.mod.Slew, surge.mod.ClockDivider and surge.mod.AHDEnvelope"),
        };
        let t = match a.get(1) {
            Some(t) if self.const_i32(t.tag) != Some(T_NIL) => {
                let t = *t;
                let not_tab = self.cmpic(Cmp::Ne, t.tag, T_TABLE);
                self.fail_if(not_tab, err::INDEX);
                let st = self.cmpdc(Cmp::Eq, t.num, STATE_ID);
                self.fail_if(st, err::STATE_DYNAMIC);
                t
            }
            _ => self.alloc(0),
        };
        self.set_proto(t, proto);
        if proto == PROTO_SLEW {
            let z = self.cd(0.0);
            self.set_num(t, "block_factor", z)?;
            let sr = self.field_num(t, "samplerate")?;
            let bs = self.field_num(t, "block_size")?;
            let z1 = self.cmpdc(Cmp::Eq, sr, 0.0);
            let z2 = self.cmpdc(Cmp::Eq, bs, 0.0);
            let off = self.or(z1, z2);
            self.if_else(
                off,
                |s| {
                    let f = s.boolc(false);
                    s.set_field(t, "calculate", f)
                },
                |_| Ok(()),
            )?;
            let a1 = self.dbc(Bin::DivD, sr, 48000.0);
            let b1 = self.dbc(Bin::DivD, bs, 32.0);
            let bf = self.db(Bin::DivD, a1, b1);
            self.set_num(t, "block_factor", bf)?;
        }
        Ok(t)
    }
    fn check_proto(&mut self, t: V, proto: u32) {
        let not_tab = self.cmpic(Cmp::Ne, t.tag, T_TABLE);
        self.fail_if(not_tab, err::METHOD);
        let tab = self.not(not_tab);
        let st = self.cmpdc(Cmp::Eq, t.num, STATE_ID);
        let bad_st = self.and(tab, st);
        self.fail_if(bad_st, err::METHOD);
        let nst = self.not(st);
        let heap = self.and(tab, nst);
        let r = self.b.var(Ty::Bool);
        let f = self.cb(false);
        self.b.set(r, f);
        self.when(heap, |s| {
            let p = s.proto_of(t);
            let e = s.cmpic(Cmp::Eq, p, proto as i32 + 1);
            s.b.set(r, e);
        });
        let ok = self.b.get(r);
        let tab_ok = self.and(heap, ok);
        let wrong = self.not(tab_ok);
        let wrong = self.and(tab, wrong);
        self.fail_if(wrong, err::METHOD);
    }
    fn limit(&mut self, bf: Val, rate: Val) -> Val {
        let three = self.cd(3.0);
        let p = self.b.math(MathFn::Pow, &[rate, three]);
        let m = self.dbc(Bin::MulD, bf, 10000.0);
        let m = self.db(Bin::MulD, m, p);
        let d = self.dbc(Bin::AddD, m, 1.0);
        let one = self.cd(1.0);
        self.db(Bin::DivD, one, d)
    }
    /// `slew:run(input, up_rate [, down_rate])` (addendum 1).
    pub(super) fn slew_run(&mut self, a: Vec<V>) -> CR<V> {
        let t = a[0];
        self.check_proto(t, PROTO_SLEW);
        let r = self.b.var(Ty::F64);
        let z = self.cd(0.0);
        self.b.set(r, z);
        let calc = self.field(t, "calculate")?;
        let on = self.truthy(calc);
        let a2 = a.clone();
        self.if_else(
            on,
            |s| {
                let input = s.arg_num(&a2, 1);
                let first = s.field(t, "is_first")?;
                let first = s.truthy(first);
                s.if_else(
                    first,
                    |s| {
                        s.set_num(t, "prior", input)?;
                        let f = s.boolc(false);
                        s.set_field(t, "is_first", f)
                    },
                    |_| Ok(()),
                )?;
                let up = s.arg_num_or(&a2, 2, 0.2);
                let up = s.dbc(Bin::MaxD, up, 0.0);
                let bf = s.field_num(t, "block_factor")?;
                let up_limit = s.limit(bf, up);
                let down_limit = match a2.get(3).copied() {
                    None => up_limit,
                    Some(dv) => {
                        let missing = s.is_nil(dv);
                        let d = s.arg_num_or(&a2, 3, 0.0);
                        let d = s.dbc(Bin::MaxD, d, 0.0);
                        let dl = s.limit(bf, d);
                        s.b.sel(missing, up_limit, dl)
                    }
                };
                let prior = s.field_num(t, "prior")?;
                let delta = s.db(Bin::SubD, input, prior);
                let nd = s.du(Un::NegD, down_limit);
                let delta = s.db(Bin::MaxD, delta, nd);
                let delta = s.db(Bin::MinD, delta, up_limit);
                let np = s.db(Bin::AddD, prior, delta);
                s.set_num(t, "prior", np)?;
                s.b.set(r, np);
                Ok(())
            },
            |_| Ok(()),
        )?;
        let rv = self.b.get(r);
        Ok(self.num(rv))
    }
    /// `clock:tick(intphase, phase)` (addendum 2).
    pub(super) fn clock_tick(&mut self, a: Vec<V>) -> CR<()> {
        let t = a[0];
        self.check_proto(t, PROTO_CLOCK);
        let ip = self.arg_num(&a, 1);
        let ph = self.arg_num(&a, 2);
        let num = self.field_num(t, "numerator")?;
        let den = self.field_num(t, "denominator")?;
        let s = self.db(Bin::AddD, ip, ph);
        let m = self.db(Bin::MulD, s, num);
        let beat = self.db(Bin::DivD, m, den);
        let ib = self.floor(beat);
        self.set_num(t, "intphase", ib)?;
        let ibeat = self.fmod_floor(ib, num);
        self.set_num(t, "ibeat", ibeat)?;
        let fr = self.db(Bin::SubD, beat, ib);
        self.set_num(t, "phase", fr)?;
        let prior = self.field(t, "prioribeat")?;
        let ibv = self.num(ib);
        let same = self.eq(ibv, prior);
        let nb = self.not(same);
        let nbv = self.boolean(nb);
        self.set_field(t, "newbeat", nbv)?;
        self.set_num(t, "prioribeat", ib)
    }
    /// `env:at(phase)` (addendum 3).
    pub(super) fn ahd_at(&mut self, a: Vec<V>) -> CR<V> {
        let t = a[0];
        self.check_proto(t, PROTO_AHD);
        let p = self.arg_num(&a, 1);
        let at = self.field_num(t, "a")?;
        let h = self.field_num(t, "h")?;
        let d = self.field_num(t, "d")?;
        let ah = self.db(Bin::AddD, at, h);
        let ahd = self.db(Bin::AddD, ah, d);
        let rise = self.db(Bin::DivD, p, at);
        let pa = self.db(Bin::SubD, p, ah);
        let q = self.db(Bin::DivD, pa, d);
        let one = self.cd(1.0);
        let fall = self.db(Bin::SubD, one, q);
        let zero = self.cd(0.0);
        // Innermost last: p <= 0, p < a, p < a+h, p < a+h+d, else 0.
        let c4 = self.cmpd(Cmp::Lt, p, ahd);
        let r = self.b.sel(c4, fall, zero);
        let c3 = self.cmpd(Cmp::Lt, p, ah);
        let r = self.b.sel(c3, one, r);
        let c2 = self.cmpd(Cmp::Lt, p, at);
        let r = self.b.sel(c2, rise, r);
        let c1 = self.cmpdc(Cmp::Le, p, 0.0);
        let r = self.b.sel(c1, zero, r);
        Ok(self.num(r))
    }
}

const STR_LOOP: u32 = 1 << 16;

