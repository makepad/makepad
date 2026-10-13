// std-core differential test (core_diff.sh): Vec and slices. Prints primitives only.

fn make(n: u32) -> Vec<u32> {
    let mut v: Vec<u32> = Vec::new();
    let mut i = 0;
    while i < n {
        v.push(i * 7 % 11);
        i += 1;
    }
    v
}

fn show(tag: &str, s: &[u32]) {
    print!("| {} {}:", tag, s.len());
    for x in s.iter() {
        print!(" {}", x);
    }
    println!();
}

#[test]
fn a01_push_pop_insert_remove() {
    let mut v = make(10);
    show("made", &v);
    println!("| pop {}", v.pop().unwrap());
    v.insert(2, 99);
    v.remove(0);
    let r = v.swap_remove(1);
    println!("| swap_removed {}", r);
    show("after", &v);
    v.truncate(4);
    show("trunc", &v);
    v.clear();
    println!("| empty {} {}", v.is_empty(), v.len());
}

#[test]
fn a02_sort_family() {
    let mut v = make(20);
    v.sort();
    show("sort", &v);
    let mut w = make(20);
    w.sort_unstable();
    show("unstable", &w);
    let mut d = make(20);
    d.sort_by(|a, b| u32::cmp(b, a));
    show("desc", &d);
    let mut k = make(20);
    k.sort_by_key(|x| *x % 3);
    show("bykey", &k);
    v.dedup();
    show("dedup", &v);
    match v.binary_search(&7) {
        Ok(i) => println!("| found {}", i),
        Err(i) => println!("| missing {}", i),
    }
    match v.binary_search(&50) {
        Ok(i) => println!("| found {}", i),
        Err(i) => println!("| missing {}", i),
    }
}

#[test]
fn a03_slice_ops() {
    let mut v = make(12);
    v.reverse();
    show("rev", &v);
    v.rotate_left(3);
    show("rotl", &v);
    v.swap(0, 11);
    show("swap", &v);
    let (a, b) = v.split_at(5);
    show("left", a);
    show("right", b);
    println!("| contains {} {}", v.contains(&10), v.contains(&42));
    println!("| first {} last {}", v.first().unwrap(), v.last().unwrap());
    println!("| starts {}", v.starts_with(&v[..3]));
    let mut c = 0;
    for ch in v.chunks(5) {
        c += ch.len() * 100 + ch[0] as usize;
    }
    println!("| chunks {}", c);
    let mut w = 0;
    for win in v.windows(3) {
        w += (win[0] + win[1] * 2 + win[2] * 3) as usize;
    }
    println!("| windows {}", w);
}

#[test]
fn a04_retain_drain_extend() {
    let mut v = make(15);
    v.retain(|x| x % 2 == 0);
    show("retain", &v);
    let mut sum = 0;
    for x in v.drain(1..3) {
        sum += x;
    }
    println!("| drained {}", sum);
    show("rest", &v);
    let other = make(4);
    v.extend_from_slice(&other);
    show("ext", &v);
    v.extend(other.iter().map(|x| x + 100));
    show("ext2", &v);
    let tail = v.split_off(5);
    show("head", &v);
    show("tail", &tail);
    v.resize(8, 7);
    show("resize", &v);
}

#[test]
fn a05_iter_into_iter() {
    let v = make(9);
    let mut s = 0;
    for (i, x) in v.iter().enumerate() {
        s += i as u32 * x;
    }
    println!("| weighted {}", s);
    let mut back = 0;
    for x in v.iter().rev() {
        back = back * 3 + x;
    }
    println!("| rev-fold {}", back);
    let mut total = 0;
    for x in v.into_iter() {
        total += x;
    }
    println!("| owned {}", total);
}

#[test]
fn a06_clone_eq_capacity() {
    let v = make(30);
    let c = v.clone();
    println!("| eq {} {}", v == c, v.len());
    let mut g: Vec<u64> = Vec::new();
    let mut caps = 0;
    let mut i = 0;
    while i < 100 {
        g.push(i);
        caps += g.capacity();
        i += 1;
    }
    println!("| caps {}", caps);
    let w: Vec<u8> = Vec::with_capacity(13);
    println!("| with_cap {}", w.capacity());
}
