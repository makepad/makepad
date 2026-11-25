//! In-place radix-2 audio transforms. Resolve the two places at the call, then
//! expand its mark once init and render have both contributed their table sizes.
use super::*;

pub(super) struct Site {
    span: Span,
    name: String,
    inverse: bool,
    init: bool,
    n: u32,
    re: Place,
    im: Place,
}

impl Site {
    fn function(&self) -> &'static str {
        if self.inverse { "ifft" } else { "fft" }
    }
}

impl Lowerer<'_> {
    pub(super) fn fft_call(&mut self, name: &str, args: &[Expr], span: Span) -> LResult<V> {
        let [re, im] = args else { return err(span, format!("{name} takes two f32 arrays")); };
        if self.b.scratch {
            return err(span, format!("call {name} from init(), block(), effect(), effect_block() or voice()"));
        }
        let mut array = |expr: &Expr| {
            fn label(expr: &Expr) -> String {
                match &expr.kind {
                    ExprKind::Ident(id) => id.clone(),
                    ExprKind::Field(base, field) => format!("{}.{field}", label(base)),
                    ExprKind::Index(base, _) => format!("{}[..]", label(base)),
                    _ => "argument".into(),
                }
            }
            let label = label(expr);
            if self.in_init && matches!(expr.kind, ExprKind::Ident(_)) && self.lookup(&label).is_none() && self.state_vars.iter().any(|v| v.name == label) {
                return err(span, format!("{name}: state array `{label}` is unavailable in init(); use a local array or a Shared table"));
            }
            if let V::Place(p) = self.expr(expr)? {
                if let T::Array(el, n) = &p.ty {
                    if **el == T::F {
                        if p.region == Region::Shared && !self.in_init {
                            return err(span, format!("{name}: Shared table `{label}` is writable only in init()"));
                        }
                        return Ok((label, *n, p));
                    }
                }
            }
            err(span, format!("{name}: `{label}` must be an f32 array"))
        };
        let (rname, n, re) = array(re)?;
        let (iname, m, im) = array(im)?;
        if n != m {
            return err(span, format!("{name}: arrays `{rname}` ({n}) and `{iname}` ({m}) must have the same length"));
        }
        if !n.is_power_of_two() {
            return err(span, format!("{name}: array `{rname}` has length {n}; expected a power of two"));
        }
        if n > 1 {
            // Later reads cannot reuse values loaded before this in-place operation.
            for (entries, _) in &mut self.b.cse {
                entries.retain(|key, _| !matches!(key, Key::Load(r, ..) if *r == re.region || *r == im.region));
            }
            // A source mark by itself is pure. Keep the pending operation in a
            // wrapper so branch-to-select lowering respects its writes.
            self.b.push(IS::Loop { cap: 1, body: vec![IS::Mark(self.fft_sites.len() as u32)] });
            self.fft_sites.push(Site { span, name: rname, inverse: name == "ifft", init: self.in_init, n, re, im });
        }
        Ok(V::Unit)
    }
}

/// Forward twiddles in two half-circle planes: cos and -sin, with (1, +0) at DC.
#[derive(Clone, Copy)]
struct Table {
    base: u32,
    n: u32,
}

impl Builder {
    /// The AIR loop itself enforces the exact trip count, including on native and wasm.
    fn fft_range(&mut self, count: u32, body: impl FnOnce(&mut Self, Val)) {
        let counter = self.var(Ty::I32);
        let zero = self.ci(0);
        let one = self.ci(1);
        self.set(counter, zero);
        self.open_loop();
        let i = self.get(counter);
        body(self, i);
        let next = self.ib(Bin::AddI, i, one);
        self.set(counter, next);
        let body = self.close();
        self.push(IS::Loop { cap: count, body });
    }

    fn fill_twiddles(&mut self, table: Table) {
        let Table { base, n } = table;
        let half = n / 2;
        let quarter = n / 4;
        let one = self.cf(1.0);
        let zero = self.cf(0.0);
        self.store(Region::Shared, base, 1, None, one);
        self.store(Region::Shared, base + half, 1, None, zero);
        if quarter > 0 {
            self.store(Region::Shared, base + quarter, 1, None, zero);
            let minus_one = self.cf(-1.0);
            self.store(Region::Shared, base + half + quarter, 1, None, minus_one);
        }
        if n < 8 { return; }
        self.fft_range(n / 8, |b, i| {
            let one = b.ci(1);
            let j = b.ib(Bin::AddI, i, one);
            let x = b.un(Ty::F64, Un::I2D, j);
            let scale = b.cd(1.0 / f64::from(n));
            let turn = b.def(Ty::F64, Op::Bin(Bin::MulD, x, scale));
            let tau = b.cd(std::f64::consts::TAU);
            let angle = b.def(Ty::F64, Op::Bin(Bin::MulD, tau, turn));
            let c = b.k_cos(angle);
            let s = b.k_sin(angle);
            let (c, s) = (b.d2f(c), b.d2f(s));
            let (nc, ns) = (b.fneg(c), b.fneg(s));
            let q = b.ci(quarter as i32);
            let h = b.ci(half as i32);
            let qmj = b.ib(Bin::SubI, q, j);
            let qpj = b.ib(Bin::AddI, q, j);
            let hmj = b.ib(Bin::SubI, h, j);
            for (k, re, im) in [(j, c, ns), (qmj, s, nc), (qpj, ns, nc), (hmj, nc, ns)] {
                b.store(Region::Shared, base, half, Some(k), re);
                b.store(Region::Shared, base + half, half, Some(k), im);
            }
        });
    }

    /// The caller already proved i is inside this array. A nested array can
    /// have a dynamic word offset inside its enclosing object.
    fn fft_offset(&mut self, p: &Place, i: Val) -> Val {
        match p.off {
            Some(off) => self.ib(Bin::AddI, off, i),
            None => i,
        }
    }

    fn fft_load(&mut self, p: &Place, i: Val) -> Val {
        let off = self.fft_offset(p, i);
        self.load(Ty::F32, p.region, p.root + p.stat, p.extent - p.stat, Some(off))
    }

    fn fft_store(&mut self, p: &Place, i: Val, value: Val) {
        let off = self.fft_offset(p, i);
        self.store(p.region, p.root + p.stat, p.extent - p.stat, Some(off), value);
    }

    fn fft_transform(&mut self, site: &Site, table: Table, n: u32) {
        if n == 1 { return; }
        let (re, im) = (&site.re, &site.im);
        let bits = n.ilog2();
        if n > 2 {
            self.fft_range(n, |b, i| {
                // Reverse the low log2(n) bits by swapping adjacent groups.
                let mut rev = i;
                let width = bits.next_power_of_two();
                for (shift, mask) in [(1, 0x55555555), (2, 0x33333333), (4, 0x0f0f0f0f), (8, 0x00ff00ff), (16, 0x0000ffff)] {
                    if shift >= width { break; }
                    let mask = b.ci(mask);
                    let amt = b.ci(shift as i32);
                    let lo = b.ib(Bin::AndI, rev, mask);
                    let lo = b.ib(Bin::ShlI, lo, amt);
                    let hi = b.ib(Bin::ShrUI, rev, amt);
                    let hi = b.ib(Bin::AndI, hi, mask);
                    rev = b.ib(Bin::OrI, hi, lo);
                }
                let pad = b.ci((width - bits) as i32);
                let j = b.ib(Bin::ShrUI, rev, pad);
                let swap = b.cmpi(Cmp::Lt, i, j);
                b.open();
                for plane in [re, im] {
                    let a = b.fft_load(plane, i);
                    let z = b.fft_load(plane, j);
                    b.fft_store(plane, i, z);
                    b.fft_store(plane, j, a);
                }
                let body = b.close();
                b.push(IS::If(swap, body, Vec::new()));
            });
        }

        let one = self.ci(1);
        let last = self.ci((table.n.ilog2() - 1) as i32);
        self.fft_range(bits, |b, stage| {
            let half = b.ib(Bin::ShlI, one, stage);
            let mask = b.ib(Bin::SubI, half, one);
            let shift = b.ib(Bin::SubI, last, stage);
            b.fft_range(n / 2, |b, i| {
                let j = b.ib(Bin::AndI, i, mask);
                let group = b.ib(Bin::SubI, i, j);
                let a = b.ib(Bin::AddI, i, group);
                let z = b.ib(Bin::AddI, a, half);
                let k = b.ib(Bin::ShlI, j, shift);
                let count = table.n / 2;
                let wr = b.load(Ty::F32, Region::Shared, table.base, count, Some(k));
                let wi = b.load(Ty::F32, Region::Shared, table.base + count, count, Some(k));
                let wi = if site.inverse { b.fneg(wi) } else { wi };
                let (ar, ai) = (b.fft_load(re, a), b.fft_load(im, a));
                let (br, bi) = (b.fft_load(re, z), b.fft_load(im, z));
                let (rr, ii) = (b.mul(wr, br), b.mul(wi, bi));
                let (ri, ir) = (b.mul(wr, bi), b.mul(wi, br));
                let (tr, ti) = (b.sub(rr, ii), b.add(ri, ir));
                let (zr, zi) = (b.sub(ar, tr), b.sub(ai, ti));
                let (ar, ai) = (b.add(ar, tr), b.add(ai, ti));
                b.fft_store(re, z, zr);
                b.fft_store(im, z, zi);
                b.fft_store(re, a, ar);
                b.fft_store(im, a, ai);
            });
        });
        if site.inverse {
            let scale = self.cf(1.0 / n as f32);
            self.fft_range(n, |b, i| {
                for plane in [re, im] {
                    let x = b.fft_load(plane, i);
                    let scaled = b.mul(x, scale);
                    b.fft_store(plane, i, scaled);
                }
            });
        }
    }
}

#[derive(Clone, Copy)]
struct Trial {
    span: Span,
    from: u32,
    to: u32,
}

relax_api::option_layouts! { Trial }

impl Trial {
    fn applies(self, site: &Site) -> bool {
        !site.init && self.span == site.span && self.from == site.n
    }
}

fn expand(mut program: Program, sites: &[Site], mut table: Table, init: bool, trial: Option<Trial>, mark_sites: bool) -> Program {
    if let Some(t) = trial {
        table.n = sites.iter().map(|s| if t.applies(s) { t.to } else { s.n }).max().unwrap();
    }
    let mut body = std::mem::take(&mut program.body);
    let mut b = Builder::new();
    b.consts.resize(program.vals.len(), None);
    b.prog = program;
    fn splice(block: &mut Block, b: &mut Builder, sites: &[Site], table: Table, trial: Option<Trial>, mark_sites: bool) {
        let mut out = Vec::with_capacity(block.len());
        for mut s in block.drain(..) {
            match &mut s {
                IS::Loop { cap: 1, body } if matches!(body.as_slice(), [IS::Mark(_)]) => {
                    let IS::Mark(id) = body[0] else { unreachable!() };
                    let site = &sites[id as usize];
                    let change = trial.filter(|t| t.applies(site));
                    let n = change.map_or(site.n, |t| t.to);
                    // A call has its own value numbering: nothing from another
                    // branch or a previous loop iteration dominates this point.
                    b.open_loop();
                    b.fft_transform(site, table, n);
                    if mark_sites { out.push(IS::Mark(id)); }
                    out.extend(b.close());
                    continue;
                }
                IS::If(_, yes, no) => {
                    splice(yes, b, sites, table, trial, mark_sites);
                    splice(no, b, sites, table, trial, mark_sites);
                }
                IS::Loop { body, .. } => splice(body, b, sites, table, trial, mark_sites),
                _ => {}
            }
            out.push(s);
        }
        *block = out;
    }
    if init { b.fill_twiddles(table); }
    splice(&mut body, &mut b, sites, table, trial, mark_sites);
    let mut prefix = b.blocks.pop().unwrap();
    prefix.append(&mut body);
    b.prog.body = prefix;
    dce(&mut b.prog);
    b.prog
}

/// Walk only branches contributing to Program::cost(), including ties. Within
/// the frame loop all nested caps count in full. Audio has no outlined calls.
fn worst_sites(block: &Block, in_loop: bool, ids: &mut Vec<usize>) {
    for stmt in block {
        match stmt {
            IS::Mark(id) => ids.push(*id as usize),
            IS::If(_, yes, no) => {
                let cost = |arm: &Block| {
                    let p = Program { body: arm.clone(), ..Default::default() };
                    if in_loop { p.total_cost() } else { p.cost() }
                };
                let (y, n) = (cost(yes), cost(no));
                if y >= n { worst_sites(yes, in_loop, ids); }
                if n >= y { worst_sites(no, in_loop, ids); }
            }
            IS::Loop { cap, body } if *cap > 0 => worst_sites(body, true, ids),
            _ => {}
        }
    }
}

pub(super) fn finish(sites: &[Site], shared: &mut Vec<u32>, init: &mut Option<Program>, render: &mut Program, init_span: Option<Span>) -> LResult<Option<ShaderWarning>> {
    let largest = sites.iter().max_by_key(|s| s.n).unwrap();
    let table = Table { base: shared.len() as u32, n: largest.n };
    let end = image_words(shared.len() as u64 + u64::from(table.n), largest.span)?;
    let marked = std::mem::take(render);
    *render = expand(marked.clone(), sites, table, false, None, false);
    let cost = render.cost();
    let warning = if cost > WARN_COST_PER_FRAME { frame_warning(&marked, sites, table, cost) } else { None };
    let mut p = init.take().unwrap_or_default();
    p = expand(p, sites, table, true, None, false);
    check_init_cost(&p, init_span.unwrap_or(largest.span))?;
    reserve_words(shared, table.n, largest.span)?;
    shared.resize(end as usize, 0);
    *init = Some(p);
    Ok(warning)
}

fn frame_warning(marked: &Program, sites: &[Site], table: Table, cost: u64) -> Option<ShaderWarning> {
    // Reprice the actual program (including enclosing loops, worst branches
    // and other transforms). Only a compile above the warning threshold takes these trials.
    let annotated = expand(marked.clone(), sites, table, false, None, true);
    let mut ids = Vec::new();
    worst_sites(&annotated.body, false, &mut ids);
    let mut tried = std::collections::HashSet::new();
    for &id in &ids {
        let site = &sites[id];
        if !tried.insert((site.span.start, site.span.end, site.n)) { continue; }
        let price = |to| {
            let trial = Trial { span: site.span, from: site.n, to };
            expand(marked.clone(), sites, table, false, Some(trial), false).cost()
        };
        if price(1) <= WARN_COST_PER_FRAME {
            let mut n = site.n / 2;
            while n > 1 && price(n) > WARN_COST_PER_FRAME { n /= 2; }
            return Some(warning(site, cost, format!("the largest power-of-two size within the threshold for this call is {n}")));
        }
    }
    if let Some(id) = ids.first() {
        return Some(warning(&sites[*id], cost, "no power-of-two size fits this call with the rest of the program".into()));
    }
    None
}

fn warning(site: &Site, cost: u64, fit: String) -> ShaderWarning {
    ShaderWarning { span: site.span, message: format!("{} on array `{}` of size {}: too much work per sample (worst case {cost} ops, warning threshold {WARN_COST_PER_FRAME}); {fit}. Run it in init() for precomputed spectra. For a transform needed once per hop, use effect_block() and choose a size within the peak work threshold.",
        site.function(), site.name, site.n) }
}
