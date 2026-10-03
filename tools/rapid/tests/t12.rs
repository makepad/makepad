// Rapid: match ergonomics (default binding modes), projections on generic params
// (`I::Item` from a bound), foreign statics.

struct G {
    children: Vec<(u32, u64)>,
}

impl G {
    fn bump(&mut self) {
        for (_, c) in &mut self.children {
            *c += 1;
        }
    }
}

enum Ev {
    Key(u32),
    Move { x: i32, y: i32 },
    Quit,
}

fn code(e: &Ev) -> i32 {
    match e {
        Ev::Key(k) => *k as i32,
        Ev::Move { x, y } => *x + *y,
        Ev::Quit => -1,
    }
}

trait Source {
    type Item;
    fn get(&self) -> Self::Item;
}

struct Seven;

impl Source for Seven {
    type Item = u64;
    fn get(&self) -> u64 {
        7
    }
}

fn twice<S: Source>(s: &S) -> (S::Item, S::Item) {
    (s.get(), s.get())
}

fn first<S>(s: &S) -> S::Item
where
    S: Source,
{
    s.get()
}

#[test]
fn match_ergonomics() {
    let mut v = Vec::new();
    v.push((1u32, 10u64));
    let mut g = G { children: v };
    g.bump();
    assert!(g.children[0].1 == 11);
    let a = [(1u32, 2u32)];
    let mut s = 0;
    for (x, y) in &a {
        s += *x + *y;
    }
    assert!(s == 3);
    assert!(code(&Ev::Key(5)) == 5);
    assert!(code(&Ev::Move { x: 2, y: 3 }) == 5);
    assert!(code(&Ev::Quit) == -1);
    let pair = (4u8, 9u8);
    let (p, q) = &pair;
    assert!(*p + *q == 13);
}

#[test]
fn param_projections() {
    let (a, b) = twice(&Seven);
    assert!(a + b == 14);
    assert!(first(&Seven) == 7);
}
