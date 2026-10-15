// Moving out of `*tmp` where tmp is a temporary Box (`*opt.take()?`, `*opt.take().unwrap()`, `*Box::new(..)`):
// the temporary is freed without dropping the moved contents again (rustc drop elaboration: box_free only).
// makepad-script's end_literal_trace (`let mut trace = *self.bx.literal_trace.take()?;`) double-freed its
// HashSets under Relax (makepad-stage: bus error in free() when a Motion document failed to load).
// "| " lines equal rustc's.
#[derive(Default)]
struct T {
    s: Vec<u64>,
    n: Vec<u64>,
}

struct B {
    t: Option<Box<T>>,
}

fn fresh(i: u64) -> B {
    B { t: Some(Box::new(T { s: vec![i; 3], n: vec![i; 2] })) }
}

fn via_try(b: &mut B) -> Option<T> {
    let t = *b.t.take()?;
    Some(t)
}

fn via_try_tail(b: &mut B) -> Option<T> {
    Some(*b.t.take()?)
}

fn via_unwrap(b: &mut B) -> T {
    *b.t.take().unwrap()
}

fn via_new(i: u64) -> T {
    *Box::new(T { s: vec![i; 4], n: vec![i] })
}

fn field_of_temp(b: &mut B) -> Option<Vec<u64>> {
    Some((*b.t.take()?).s)
}

fn let_pattern(b: &mut B) -> Option<Vec<u64>> {
    let T { s, .. } = *b.t.take()?;
    Some(s)
}

#[test]
fn a01_temp_box_moves() {
    let mut sum = 0usize;
    for i in 0..3000u64 {
        let mut b = fresh(i);
        let t = via_try(&mut b).unwrap();
        sum += t.s.len() + t.n.len();
        assert!(via_try(&mut b).is_none());
        let mut b = fresh(i);
        sum += via_try_tail(&mut b).map_or(0, |t| t.s.len());
        let mut b = fresh(i);
        sum += via_unwrap(&mut b).n.len();
        sum += via_new(i).s.len();
        let mut b = fresh(i);
        sum += field_of_temp(&mut b).unwrap().len();
        let mut b = fresh(i);
        sum += let_pattern(&mut b).unwrap().len();
    }
    println!("| sum {}", sum);
}

#[test]
fn a02_copy_contents() {
    // contents without drop glue: the temporary is dropped as before
    let n: u64 = *Box::new(41u64) + 1;
    let o: Option<Box<(u8, u16)>> = Some(Box::new((1, 2)));
    let (a, b) = *o.unwrap();
    println!("| copy {} {} {}", n, a, b);
}

static DROPS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

struct D(u32);

impl Drop for D {
    fn drop(&mut self) {
        DROPS.fetch_add(self.0 as usize, std::sync::atomic::Ordering::Relaxed);
    }
}

struct P {
    a: D,
    b: D,
}

fn drops() -> usize {
    DROPS.swap(0, std::sync::atomic::Ordering::Relaxed)
}

#[test]
fn a03_drop_counts() {
    drops();
    let mut o = Some(Box::new(P { a: D(1), b: D(10) }));
    let p = *o.take().unwrap();
    println!("| whole moved, before drop {}", drops());
    drop(p);
    println!("| whole dropped {}", drops());
    let o = Some(Box::new(P { a: D(1), b: D(10) }));
    let P { b, .. } = *o.unwrap();
    println!("| pattern b moved: rest dropped {}", drops());
    drop(b);
    println!("| pattern b dropped {}", drops());
}
