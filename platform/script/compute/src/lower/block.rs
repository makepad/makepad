//! Stereo block processing inside the ordinary audio frame loop.
use super::*;

pub(super) const BASE_RATE: &str =
    "effect_block() uses a base-rate hop; remove rate() and oversampling or put them in a separate effect";

pub(super) fn check_declarations(items: &[Item], node: RateOverride, span: Span) -> LResult<()> {
    if node.factor != 0 { return err(span, BASE_RATE); }
    for item in items {
        match item {
            Item::Decl { name, span, .. } if name == "rate" => return err(*span, BASE_RATE),
            Item::Let { value, .. } if matches!(&value.kind, ExprKind::Call(f, _) if f == "input" || f == "output") => {
                return err(value.span, "input() and output() ports on effect_block() are not supported yet: side signals need their own rings; use the stereo frame arrays or a separate effect");
            }
            _ => {}
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
pub(super) struct Layout {
    size: u32,
    hop: u32,
    pos: Var,
    input: [u32; 2],
    output: [u32; 2],
    frame: [u32; 2],
}

relax_api::option_layouts! { Layout }

impl Lowerer<'_> {
    pub(super) fn add_block_layout(&mut self, span: Span) -> LResult<Layout> {
        let dimension = |name: &str| {
            let n = match self.globals.get(name) {
                Some(Bind::Const(V::Lit(n))) => *n,
                Some(Bind::Scalar(T::I, w)) => (*w as i32) as f64,
                Some(Bind::Scalar(T::F, w)) => f32::from_bits(*w) as f64,
                _ => 0.0,
            };
            let size = n as u32;
            if f64::from(size) != n || !size.is_power_of_two() {
                return err(span, format!("effect_block() needs a positive power of two for {name}; declare it with a top-level let"));
            }
            Ok(size)
        };
        let size = dimension("BLOCK")?;
        let hop = dimension("HOP")?;
        if hop > size {
            return err(span, "effect_block() needs HOP <= BLOCK; reduce HOP or increase BLOCK");
        }
        self.latency = self.latency.checked_add(size).ok_or_else(|| ShaderError::new(span.start, span.end,
            "declared latency plus BLOCK exceeds 4294967295 frames; reduce latency() or BLOCK".into()))?;

        // Names distinguish both dimensions even when only the hop changes.
        let prefix = format!("$effect_block.{size}.{hop}.");
        self.add_state(&format!("{prefix}pos"), CInit::W(0, T::I), span)?;
        let pos = self.promoted.last().unwrap().1;
        let mut stereo = |role: &str| -> LResult<[u32; 2]> {
            let mut roots = [0; 2];
            for (root, channel) in roots.iter_mut().zip(["l", "r"]) {
                *root = self.state_init.len() as u32;
                self.add_state(&format!("{prefix}{role}_{channel}"), CInit::Repeat(Box::new(CInit::W(0, T::F)), size), span)?;
            }
            Ok(roots)
        };
        Ok(Layout { size, hop, pos, input: stereo("in")?, output: stereo("out")?, frame: stereo("frame")? })
    }

    pub(super) fn block_sample(&mut self, layout: Layout, entry: &FnDecl, iv: Val) -> LResult<()> {
        let Layout { pos, size, hop, input, output, frame } = layout;
        let p = self.b.get(pos);
        let silence = self.b.cf(0.0);
        for ch in 0..2 {
            let sample = self.b.raw(Ty::F32, Op::In { ch: ch as u16, idx: iv }, None);
            let due = self.b.load(Ty::F32, Region::State, output[ch], size, Some(p));
            self.b.store(Region::State, output[ch], size, Some(p), silence);
            self.b.store(Region::State, input[ch], size, Some(p), sample);
            self.b.push(IS::Out { ch: ch as u16, idx: iv, val: due });
        }
        let one = self.b.ci(1);
        let mask = self.b.ci((size - 1) as i32);
        let next = self.b.ib(Bin::AddI, p, one);
        let q = self.b.ib(Bin::AndI, next, mask);
        self.b.set(pos, q);
        let hop_mask = self.b.ci((hop - 1) as i32);
        let phase = self.b.ib(Bin::AndI, q, hop_mask);
        let zero = self.b.ci(0);
        let between_hops = self.b.cmpi(Cmp::Ne, phase, zero);
        // A one-pass block with an early exit lets the work counter charge
        // fixed-trip loops only on hops that actually reach them.
        self.b.open();
        self.b.push(IS::If(between_hops, vec![IS::Break(0)], vec![]));
        // q is both the oldest input and the next output due.
        self.b.block_transfer(size, q, |b, i, ring| {
            for ch in 0..2 {
                let x = b.load(Ty::F32, Region::State, input[ch], size, Some(ring));
                b.store(Region::State, frame[ch], size, Some(i), x);
            }
        });
        let args = frame.iter().map(|&root| V::Place(Place {
            region: Region::State, root, extent: size, off: None, stat: 0,
            ty: T::Array(Box::new(T::F), size),
        })).collect();
        self.call(entry, args, entry.span)?;
        self.b.block_transfer(size, q, |b, i, ring| {
            for ch in 0..2 {
                let x = b.load(Ty::F32, Region::State, frame[ch], size, Some(i));
                let sum = if hop == size { x } else {
                    let tail = b.load(Ty::F32, Region::State, output[ch], size, Some(ring));
                    b.add(tail, x)
                };
                b.store(Region::State, output[ch], size, Some(ring), sum);
            }
        });
        let work = self.b.close();
        self.b.push(IS::Loop { cap: 1, body: work });
        Ok(())
    }
}

impl Builder {
    fn block_transfer(&mut self, size: u32, start: Val, copy: impl FnOnce(&mut Self, Val, Val)) {
        let index = self.var(Ty::I32);
        let zero = self.ci(0);
        self.set(index, zero);
        self.open_loop();
        let i = self.get(index);
        let mask = self.ci((size - 1) as i32);
        let slot = self.ib(Bin::AddI, start, i);
        let ring = self.ib(Bin::AndI, slot, mask);
        copy(self, i, ring);
        let one = self.ci(1);
        let next = self.ib(Bin::AddI, i, one);
        self.set(index, next);
        let body = self.close();
        self.push(IS::Loop { cap: size, body });
    }
}
