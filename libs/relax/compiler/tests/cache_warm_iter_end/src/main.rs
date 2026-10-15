// Slice iterators over arrays of different element sizes (`["l", "r"]`, a table of
// `(&str, Option<usize>)`, bytes, wide structs, zero-sized): a slice iterator's end pointer is
// `ptr + len * size_of::<T>()`; a warm run restoring the generic code from the disk cache must
// use each element type's own size. Every run (cold, warm) must count the same elements.
const JOINTS: [(&str, Option<usize>); 5] =
    [("hips", None), ("spine", Some(0)), ("chest", Some(1)), ("neck", Some(2)), ("head", Some(3))];

struct Wide {
    a: [u64; 5],
}

#[inline(never)]
fn sides(out: &mut Vec<String>) {
    for (i, side) in ["l", "r"].iter().enumerate() {
        out.push(format!("{i}{side}"));
    }
}

// the array is one promoted block; its address and its end address are two relocations of the
// cached code, to different offsets of that one block
#[inline(never)]
fn walk() -> usize {
    let a = &["l", "r"];
    let start = a as *const [&str; 2] as *const &str;
    let end = unsafe { start.add(2) };
    let (mut p, mut n) = (start, 0);
    while p != end && n < 10 {
        n += 1;
        p = unsafe { p.add(1) };
    }
    n
}

#[inline(never)]
fn once(v: &mut Vec<String>) {
    for (i, side) in ["l", "r"].iter().enumerate() {
        v.push(format!("{i}{side}"));
    }
}

// the shape of a tier-0 closure that walks a literal array with a captured table
fn closure_walk(hands: &[Option<u32>; 2]) -> usize {
    let chain = |side: &str| -> Option<usize> { Some(side.len()) };
    let mut targets: Vec<(usize, u32)> = Vec::new();
    let go = |t: &mut Vec<(usize, u32)>| {
        for (i, side) in ["l", "r"].iter().enumerate() {
            if let (Some(c), Some(h)) = (chain(side), hands[i]) {
                t.push((c, h));
            }
        }
    };
    go(&mut targets);
    targets.len()
}

fn main() {
    // hot: the tiers above the first install optimised code of these loops (the cache keeps it)
    let hot = |round: usize| {
        let mut t = 0usize;
        for _ in 0..round {
            for (i, side) in ["l", "r"].iter().enumerate() {
                t += i + side.len();
            }
            t += JOINTS.iter().enumerate().map(|(i, (_, p))| i + p.unwrap_or(0)).sum::<usize>();
            t += [(), ()].iter().count();
        }
        t
    };
    let mut hot_total = 0;
    for r in 0..2000 {
        hot_total += hot(50 + r % 3);
    }
    std::thread::sleep(std::time::Duration::from_millis(1500));
    for _ in 0..200 {
        hot_total += hot(50);
    }
    println!("hot {}", hot_total);
    let mut a = Vec::new();
    sides(&mut a);
    let mut b = Vec::new();
    once(&mut b);
    let joints: Vec<usize> = JOINTS.iter().enumerate().map(|(i, (_, p))| i + p.unwrap_or(0)).collect();
    let bytes = b"abc".iter().map(|x| *x as usize).sum::<usize>();
    let wide = [Wide { a: [1; 5] }, Wide { a: [2; 5] }];
    let w: u64 = wide.iter().map(|w| w.a[0]).sum();
    let zst = [(), (), ()].iter().count();
    let mut c = 0;
    let many = |n: &mut usize| {
        for (_i, _side) in ["l", "r"].iter().enumerate() {
            *n += 1;
        }
    };
    for _ in 0..3 {
        many(&mut c);
    }
    println!("cache_warm_iter_end result {} {} {} {} {} {} {} {} {}", a.len(), b.len(), joints.len(), bytes, w, zst, c, walk(), closure_walk(&[Some(1), None]));
}
