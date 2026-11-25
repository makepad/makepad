// A compare of a select between constants, seen through a low mask narrower than a byte: the parity
// of a layer number a flag picks (`if top { 130 } else { 131 }`, then `n % 2 == 0`), as a board
// part on the bottom side mirrors its mask layer. The JIT's fold of a compare over a select tree read
// `n & 1` as a 1-bit narrowing, and its constant extension knew only 8, 16 and 32 bits, so both arms
// compared unmasked: the parity test became `false`, the even layer took the odd layer's mirror,
// and the part's far-side mask opening came out `None`. Each form runs often enough to reach the
// optimising tier, and every call is checked.
use std::hint::black_box;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Layer(u8);

impl Layer {
    const FRONT_MASK: Layer = Layer(130);
    const BACK_MASK: Layer = Layer(131);

    /// The same-kind layer on a side (even numbers are the front's).
    fn on(self, top: bool) -> Layer {
        match self.0 {
            n @ 128..=139 => Layer(if top { n & !1 } else { (n & !1) + 1 }),
            _ => self,
        }
    }

    /// Front and back swap.
    fn flipped(self) -> Layer {
        match self.0 {
            n @ 128..=139 => Layer(if n % 2 == 0 { n + 1 } else { n - 1 }),
            _ => self,
        }
    }
}

struct Part {
    bottom: bool,
    /// bit `l - 128` for each layer `l` the part is on
    layers: u64,
}

impl Part {
    fn has(&self, l: Layer) -> bool {
        l.0 >= 128 && (self.layers >> (l.0 - 128)) & 1 == 1
    }

    /// The mask layer of the part's opening on a side, `None` when it has no mask there.
    fn mask_on(&self, top: bool) -> Option<Layer> {
        let l = Layer::FRONT_MASK.on(top);
        let l = if self.bottom { l.flipped() } else { l };
        self.has(l).then_some(l)
    }
}

fn even(top: bool) -> bool {
    let n: u8 = if top { 130 } else { 131 };
    n % 2 == 0
}

fn low3(top: bool) -> bool {
    let n: u32 = if top { 130 } else { 131 };
    n & 7 == 2
}

fn low6_above(top: bool) -> bool {
    let n: u64 = if top { 0x1c2 } else { 0x101 };
    n & 0x3f > 1
}

#[test]
fn low_mask_compares_of_a_select() {
    // a through-hole part on the bottom: its mask on both sides
    let part = Part { bottom: true, layers: 0b1100 };
    let smd = Part { bottom: true, layers: 0b0100 };
    for round in 0..40u32 {
        for i in 0..5_000u32 {
            let top = black_box(i % 3 != 0);
            assert_eq!(even(top), top, "round {round}: the parity of the layer `top` picks");
            assert_eq!(low3(top), top, "round {round}: the low 3 bits of the layer `top` picks");
            assert_eq!(low6_above(top), top, "round {round}: the low 6 bits of the value `top` picks");
            let want = if top { Layer::BACK_MASK } else { Layer::FRONT_MASK };
            assert_eq!(part.mask_on(top), Some(want), "round {round}: the far-side mask opening of a bottom part");
            assert_eq!(smd.mask_on(top), (!top).then_some(Layer::FRONT_MASK), "round {round}: a bottom part's mask on its own side only");
        }
        // the optimising tier builds on its own thread: a moment for it once the calls crossed
        // its threshold
        if round == 2 {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}
