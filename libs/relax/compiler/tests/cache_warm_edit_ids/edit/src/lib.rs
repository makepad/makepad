// The crate the series edits (the script VM of the VM / HAL series): line insertions shift the
// offsets of everything after them, and a closure added or removed before the existing ones
// renumbers their expressions. A warm start must never link code to a startup image slot whose
// key no longer decodes.
#[inline(never)]
pub fn eval(v: u64) -> u64 {
    // EDIT-ANCHOR
    let a = |x: u64| x + 1;
    let b = move |x: u64| x * 2 + v;
    let m: u64 = (0..v).map(|i| i * 3).filter(|i| i % 2 == 0).sum();
    a(v) + b(v) + m + Some(v).map(|x| x + 4).unwrap_or(0)
}

pub fn callbacks() -> Vec<Box<dyn FnMut(u64) -> u64>> {
    let mut n = 0u64;
    vec![Box::new(move |x| {
        n += 1;
        x + n
    }), Box::new(|x| x ^ 5)]
}

pub struct Item(pub u64);

impl Item {
    #[inline(never)]
    pub fn later(&self) -> u64 {
        let c = |x: u64| x.wrapping_mul(31);
        let d: Vec<u64> = [self.0, 2, 3].iter().map(|&x| c(x)).collect();
        d.iter().sum()
    }
}
