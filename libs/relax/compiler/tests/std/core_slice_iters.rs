// Run with core_diff.sh. Keep observations primitive and test order alphabetical.
fn make() -> Vec<i32> {
    let mut v = Vec::new();
    let mut i = 0;
    while i < 13 {
        v.push(i * 7 % 11);
        i += 1;
    }
    v
}
fn show(tag: &str, s: &[i32]) {
    print!("| {} {}:", tag, s.len());
    let mut i = 0;
    while i < s.len() {
        print!(" {}", s[i]);
        i += 1;
    }
    println!();
}
fn change(s: &mut [i32]) {
    let mut i = 0;
    while i < s.len() {
        s[i] += 100;
        i += 1;
    }
}

#[test]
fn a01_windows() {
    let v = make();
    {
        let mut it = (&v[..]).windows(3);
        let mut step = 0;
        loop {
            let (lo, hi) = it.size_hint();
            println!("| windows hint {} {}", lo, hi.unwrap_or(999));
            let item = if step % 2 == 0 {
                it.next()
            } else {
                it.next_back()
            };
            match item {
                Some(x) => {
                    show("piece", x);
                }
                None => break,
            }
            step += 1;
        }
    }
    println!("| windows count {}", (&v[..]).windows(3).count());
    match (&v[..]).windows(3).nth(1) {
        Some(x) => show("nth(1)", x),
        None => println!("| nth(1) none"),
    }
    match (&v[..]).windows(3).nth_back(1) {
        Some(x) => show("nth_back(1)", x),
        None => println!("| nth_back(1) none"),
    }
    match (&v[..]).windows(3).last() {
        Some(x) => show("last()", x),
        None => println!("| last() none"),
    }
}

#[test]
fn a02_chunks() {
    let v = make();
    {
        let mut it = (&v[..]).chunks(3);
        let mut step = 0;
        loop {
            let (lo, hi) = it.size_hint();
            println!("| chunks hint {} {}", lo, hi.unwrap_or(999));
            let item = if step % 2 == 0 {
                it.next()
            } else {
                it.next_back()
            };
            match item {
                Some(x) => {
                    show("piece", x);
                }
                None => break,
            }
            step += 1;
        }
    }
    println!("| chunks count {}", (&v[..]).chunks(3).count());
    match (&v[..]).chunks(3).nth(1) {
        Some(x) => show("nth(1)", x),
        None => println!("| nth(1) none"),
    }
    match (&v[..]).chunks(3).nth_back(1) {
        Some(x) => show("nth_back(1)", x),
        None => println!("| nth_back(1) none"),
    }
    match (&v[..]).chunks(3).last() {
        Some(x) => show("last()", x),
        None => println!("| last() none"),
    }
}

#[test]
fn a03_chunks_mut() {
    let mut v = make();
    {
        let mut it = (&mut v[..]).chunks_mut(3);
        let mut step = 0;
        loop {
            let (lo, hi) = it.size_hint();
            println!("| chunks_mut hint {} {}", lo, hi.unwrap_or(999));
            let item = if step % 2 == 0 {
                it.next()
            } else {
                it.next_back()
            };
            match item {
                Some(x) => {
                    show("piece", x);
                    change(x);
                }
                None => break,
            }
            step += 1;
        }
    }
    show("mutated", &v);
    println!("| chunks_mut count {}", (&mut v[..]).chunks_mut(3).count());
    match (&mut v[..]).chunks_mut(3).nth(1) {
        Some(x) => show("nth(1)", x),
        None => println!("| nth(1) none"),
    }
    match (&mut v[..]).chunks_mut(3).nth_back(1) {
        Some(x) => show("nth_back(1)", x),
        None => println!("| nth_back(1) none"),
    }
    match (&mut v[..]).chunks_mut(3).last() {
        Some(x) => show("last()", x),
        None => println!("| last() none"),
    }
}

#[test]
fn a04_chunks_exact() {
    let v = make();
    {
        let mut it = (&v[..]).chunks_exact(3);
        let mut step = 0;
        loop {
            let (lo, hi) = it.size_hint();
            println!("| chunks_exact hint {} {}", lo, hi.unwrap_or(999));
            let item = if step % 2 == 0 {
                it.next()
            } else {
                it.next_back()
            };
            match item {
                Some(x) => {
                    show("piece", x);
                }
                None => break,
            }
            step += 1;
        }
        show("remainder", it.remainder());
    }
    println!("| chunks_exact count {}", (&v[..]).chunks_exact(3).count());
    match (&v[..]).chunks_exact(3).nth(1) {
        Some(x) => show("nth(1)", x),
        None => println!("| nth(1) none"),
    }
    match (&v[..]).chunks_exact(3).nth_back(1) {
        Some(x) => show("nth_back(1)", x),
        None => println!("| nth_back(1) none"),
    }
    match (&v[..]).chunks_exact(3).last() {
        Some(x) => show("last()", x),
        None => println!("| last() none"),
    }
}

#[test]
fn a05_chunks_exact_mut() {
    let mut v = make();
    {
        let mut it = (&mut v[..]).chunks_exact_mut(3);
        let mut step = 0;
        loop {
            let (lo, hi) = it.size_hint();
            println!("| chunks_exact_mut hint {} {}", lo, hi.unwrap_or(999));
            let item = if step % 2 == 0 {
                it.next()
            } else {
                it.next_back()
            };
            match item {
                Some(x) => {
                    show("piece", x);
                    change(x);
                }
                None => break,
            }
            step += 1;
        }
        show("remainder", it.into_remainder());
    }
    show("mutated", &v);
    println!(
        "| chunks_exact_mut count {}",
        (&mut v[..]).chunks_exact_mut(3).count()
    );
    match (&mut v[..]).chunks_exact_mut(3).nth(1) {
        Some(x) => show("nth(1)", x),
        None => println!("| nth(1) none"),
    }
    match (&mut v[..]).chunks_exact_mut(3).nth_back(1) {
        Some(x) => show("nth_back(1)", x),
        None => println!("| nth_back(1) none"),
    }
    match (&mut v[..]).chunks_exact_mut(3).last() {
        Some(x) => show("last()", x),
        None => println!("| last() none"),
    }
}

#[test]
fn a06_rchunks() {
    let v = make();
    {
        let mut it = (&v[..]).rchunks(3);
        let mut step = 0;
        loop {
            let (lo, hi) = it.size_hint();
            println!("| rchunks hint {} {}", lo, hi.unwrap_or(999));
            let item = if step % 2 == 0 {
                it.next()
            } else {
                it.next_back()
            };
            match item {
                Some(x) => {
                    show("piece", x);
                }
                None => break,
            }
            step += 1;
        }
    }
    println!("| rchunks count {}", (&v[..]).rchunks(3).count());
    match (&v[..]).rchunks(3).nth(1) {
        Some(x) => show("nth(1)", x),
        None => println!("| nth(1) none"),
    }
    match (&v[..]).rchunks(3).nth_back(1) {
        Some(x) => show("nth_back(1)", x),
        None => println!("| nth_back(1) none"),
    }
    match (&v[..]).rchunks(3).last() {
        Some(x) => show("last()", x),
        None => println!("| last() none"),
    }
}

#[test]
fn a07_rchunks_mut() {
    let mut v = make();
    {
        let mut it = (&mut v[..]).rchunks_mut(3);
        let mut step = 0;
        loop {
            let (lo, hi) = it.size_hint();
            println!("| rchunks_mut hint {} {}", lo, hi.unwrap_or(999));
            let item = if step % 2 == 0 {
                it.next()
            } else {
                it.next_back()
            };
            match item {
                Some(x) => {
                    show("piece", x);
                    change(x);
                }
                None => break,
            }
            step += 1;
        }
    }
    show("mutated", &v);
    println!(
        "| rchunks_mut count {}",
        (&mut v[..]).rchunks_mut(3).count()
    );
    match (&mut v[..]).rchunks_mut(3).nth(1) {
        Some(x) => show("nth(1)", x),
        None => println!("| nth(1) none"),
    }
    match (&mut v[..]).rchunks_mut(3).nth_back(1) {
        Some(x) => show("nth_back(1)", x),
        None => println!("| nth_back(1) none"),
    }
    match (&mut v[..]).rchunks_mut(3).last() {
        Some(x) => show("last()", x),
        None => println!("| last() none"),
    }
}

#[test]
fn a08_rchunks_exact() {
    let v = make();
    {
        let mut it = (&v[..]).rchunks_exact(3);
        let mut step = 0;
        loop {
            let (lo, hi) = it.size_hint();
            println!("| rchunks_exact hint {} {}", lo, hi.unwrap_or(999));
            let item = if step % 2 == 0 {
                it.next()
            } else {
                it.next_back()
            };
            match item {
                Some(x) => {
                    show("piece", x);
                }
                None => break,
            }
            step += 1;
        }
        show("remainder", it.remainder());
    }
    println!(
        "| rchunks_exact count {}",
        (&v[..]).rchunks_exact(3).count()
    );
    match (&v[..]).rchunks_exact(3).nth(1) {
        Some(x) => show("nth(1)", x),
        None => println!("| nth(1) none"),
    }
    match (&v[..]).rchunks_exact(3).nth_back(1) {
        Some(x) => show("nth_back(1)", x),
        None => println!("| nth_back(1) none"),
    }
    match (&v[..]).rchunks_exact(3).last() {
        Some(x) => show("last()", x),
        None => println!("| last() none"),
    }
}

#[test]
fn a09_rchunks_exact_mut() {
    let mut v = make();
    {
        let mut it = (&mut v[..]).rchunks_exact_mut(3);
        let mut step = 0;
        loop {
            let (lo, hi) = it.size_hint();
            println!("| rchunks_exact_mut hint {} {}", lo, hi.unwrap_or(999));
            let item = if step % 2 == 0 {
                it.next()
            } else {
                it.next_back()
            };
            match item {
                Some(x) => {
                    show("piece", x);
                    change(x);
                }
                None => break,
            }
            step += 1;
        }
        show("remainder", it.into_remainder());
    }
    show("mutated", &v);
    println!(
        "| rchunks_exact_mut count {}",
        (&mut v[..]).rchunks_exact_mut(3).count()
    );
    match (&mut v[..]).rchunks_exact_mut(3).nth(1) {
        Some(x) => show("nth(1)", x),
        None => println!("| nth(1) none"),
    }
    match (&mut v[..]).rchunks_exact_mut(3).nth_back(1) {
        Some(x) => show("nth_back(1)", x),
        None => println!("| nth_back(1) none"),
    }
    match (&mut v[..]).rchunks_exact_mut(3).last() {
        Some(x) => show("last()", x),
        None => println!("| last() none"),
    }
}

#[test]
fn a10_chunk_by() {
    let v = make();
    {
        let mut it = (&v[..]).chunk_by(|a: &i32, b: &i32| *a <= *b);
        let mut step = 0;
        loop {
            let (lo, hi) = it.size_hint();
            println!("| chunk_by hint {} {}", lo, hi.unwrap_or(999));
            let item = if step % 2 == 0 {
                it.next()
            } else {
                it.next_back()
            };
            match item {
                Some(x) => {
                    show("piece", x);
                }
                None => break,
            }
            step += 1;
        }
    }
    println!(
        "| chunk_by count {}",
        (&v[..]).chunk_by(|a: &i32, b: &i32| *a <= *b).count()
    );
    match (&v[..]).chunk_by(|a: &i32, b: &i32| *a <= *b).nth(1) {
        Some(x) => show("nth(1)", x),
        None => println!("| nth(1) none"),
    }
    match (&v[..]).chunk_by(|a: &i32, b: &i32| *a <= *b).nth_back(1) {
        Some(x) => show("nth_back(1)", x),
        None => println!("| nth_back(1) none"),
    }
    match (&v[..]).chunk_by(|a: &i32, b: &i32| *a <= *b).last() {
        Some(x) => show("last()", x),
        None => println!("| last() none"),
    }
}

#[test]
fn a11_chunk_by_mut() {
    let mut v = make();
    {
        let mut it = (&mut v[..]).chunk_by_mut(|a: &i32, b: &i32| *a <= *b);
        let mut step = 0;
        loop {
            let (lo, hi) = it.size_hint();
            println!("| chunk_by_mut hint {} {}", lo, hi.unwrap_or(999));
            let item = if step % 2 == 0 {
                it.next()
            } else {
                it.next_back()
            };
            match item {
                Some(x) => {
                    show("piece", x);
                    change(x);
                }
                None => break,
            }
            step += 1;
        }
    }
    show("mutated", &v);
    println!(
        "| chunk_by_mut count {}",
        (&mut v[..])
            .chunk_by_mut(|a: &i32, b: &i32| *a <= *b)
            .count()
    );
    match (&mut v[..])
        .chunk_by_mut(|a: &i32, b: &i32| *a <= *b)
        .nth(1)
    {
        Some(x) => show("nth(1)", x),
        None => println!("| nth(1) none"),
    }
    match (&mut v[..])
        .chunk_by_mut(|a: &i32, b: &i32| *a <= *b)
        .nth_back(1)
    {
        Some(x) => show("nth_back(1)", x),
        None => println!("| nth_back(1) none"),
    }
    match (&mut v[..])
        .chunk_by_mut(|a: &i32, b: &i32| *a <= *b)
        .last()
    {
        Some(x) => show("last()", x),
        None => println!("| last() none"),
    }
}

#[test]
fn a12_split() {
    let v = make();
    {
        let mut it = (&v[..]).split(|x: &i32| *x % 3 == 0);
        let mut step = 0;
        loop {
            let (lo, hi) = it.size_hint();
            println!("| split hint {} {}", lo, hi.unwrap_or(999));
            let item = if step % 2 == 0 {
                it.next()
            } else {
                it.next_back()
            };
            match item {
                Some(x) => {
                    show("piece", x);
                }
                None => break,
            }
            step += 1;
        }
    }
    println!(
        "| split count {}",
        (&v[..]).split(|x: &i32| *x % 3 == 0).count()
    );
    match (&v[..]).split(|x: &i32| *x % 3 == 0).nth(1) {
        Some(x) => show("nth(1)", x),
        None => println!("| nth(1) none"),
    }
    match (&v[..]).split(|x: &i32| *x % 3 == 0).nth_back(1) {
        Some(x) => show("nth_back(1)", x),
        None => println!("| nth_back(1) none"),
    }
    match (&v[..]).split(|x: &i32| *x % 3 == 0).last() {
        Some(x) => show("last()", x),
        None => println!("| last() none"),
    }
}

#[test]
fn a13_split_mut() {
    let mut v = make();
    {
        let mut it = (&mut v[..]).split_mut(|x: &i32| *x % 3 == 0);
        let mut step = 0;
        loop {
            let (lo, hi) = it.size_hint();
            println!("| split_mut hint {} {}", lo, hi.unwrap_or(999));
            let item = if step % 2 == 0 {
                it.next()
            } else {
                it.next_back()
            };
            match item {
                Some(x) => {
                    show("piece", x);
                    change(x);
                }
                None => break,
            }
            step += 1;
        }
    }
    show("mutated", &v);
    println!(
        "| split_mut count {}",
        (&mut v[..]).split_mut(|x: &i32| *x % 3 == 0).count()
    );
    match (&mut v[..]).split_mut(|x: &i32| *x % 3 == 0).nth(1) {
        Some(x) => show("nth(1)", x),
        None => println!("| nth(1) none"),
    }
    match (&mut v[..]).split_mut(|x: &i32| *x % 3 == 0).nth_back(1) {
        Some(x) => show("nth_back(1)", x),
        None => println!("| nth_back(1) none"),
    }
    match (&mut v[..]).split_mut(|x: &i32| *x % 3 == 0).last() {
        Some(x) => show("last()", x),
        None => println!("| last() none"),
    }
}

#[test]
fn a14_split_inclusive() {
    let v = make();
    {
        let mut it = (&v[..]).split_inclusive(|x: &i32| *x % 3 == 0);
        let mut step = 0;
        loop {
            let (lo, hi) = it.size_hint();
            println!("| split_inclusive hint {} {}", lo, hi.unwrap_or(999));
            let item = if step % 2 == 0 {
                it.next()
            } else {
                it.next_back()
            };
            match item {
                Some(x) => {
                    show("piece", x);
                }
                None => break,
            }
            step += 1;
        }
    }
    println!(
        "| split_inclusive count {}",
        (&v[..]).split_inclusive(|x: &i32| *x % 3 == 0).count()
    );
    match (&v[..]).split_inclusive(|x: &i32| *x % 3 == 0).nth(1) {
        Some(x) => show("nth(1)", x),
        None => println!("| nth(1) none"),
    }
    match (&v[..]).split_inclusive(|x: &i32| *x % 3 == 0).nth_back(1) {
        Some(x) => show("nth_back(1)", x),
        None => println!("| nth_back(1) none"),
    }
    match (&v[..]).split_inclusive(|x: &i32| *x % 3 == 0).last() {
        Some(x) => show("last()", x),
        None => println!("| last() none"),
    }
}

#[test]
fn a15_split_inclusive_mut() {
    let mut v = make();
    {
        let mut it = (&mut v[..]).split_inclusive_mut(|x: &i32| *x % 3 == 0);
        let mut step = 0;
        loop {
            let (lo, hi) = it.size_hint();
            println!("| split_inclusive_mut hint {} {}", lo, hi.unwrap_or(999));
            let item = if step % 2 == 0 {
                it.next()
            } else {
                it.next_back()
            };
            match item {
                Some(x) => {
                    show("piece", x);
                    change(x);
                }
                None => break,
            }
            step += 1;
        }
    }
    show("mutated", &v);
    println!(
        "| split_inclusive_mut count {}",
        (&mut v[..])
            .split_inclusive_mut(|x: &i32| *x % 3 == 0)
            .count()
    );
    match (&mut v[..])
        .split_inclusive_mut(|x: &i32| *x % 3 == 0)
        .nth(1)
    {
        Some(x) => show("nth(1)", x),
        None => println!("| nth(1) none"),
    }
    match (&mut v[..])
        .split_inclusive_mut(|x: &i32| *x % 3 == 0)
        .nth_back(1)
    {
        Some(x) => show("nth_back(1)", x),
        None => println!("| nth_back(1) none"),
    }
    match (&mut v[..])
        .split_inclusive_mut(|x: &i32| *x % 3 == 0)
        .last()
    {
        Some(x) => show("last()", x),
        None => println!("| last() none"),
    }
}

#[test]
fn a16_splitn() {
    let v = make();
    {
        let mut it = (&v[..]).splitn(3, |x: &i32| *x % 3 == 0);
        let mut step = 0;
        loop {
            let (lo, hi) = it.size_hint();
            println!("| splitn hint {} {}", lo, hi.unwrap_or(999));
            let item = it.next();
            match item {
                Some(x) => {
                    show("piece", x);
                }
                None => break,
            }
            step += 1;
        }
    }
    println!(
        "| splitn count {}",
        (&v[..]).splitn(3, |x: &i32| *x % 3 == 0).count()
    );
    match (&v[..]).splitn(3, |x: &i32| *x % 3 == 0).nth(1) {
        Some(x) => show("nth(1)", x),
        None => println!("| nth(1) none"),
    }
    match (&v[..]).splitn(3, |x: &i32| *x % 3 == 0).last() {
        Some(x) => show("last()", x),
        None => println!("| last() none"),
    }
}

#[test]
fn a17_splitn_mut() {
    let mut v = make();
    {
        let mut it = (&mut v[..]).splitn_mut(3, |x: &i32| *x % 3 == 0);
        let mut step = 0;
        loop {
            let (lo, hi) = it.size_hint();
            println!("| splitn_mut hint {} {}", lo, hi.unwrap_or(999));
            let item = it.next();
            match item {
                Some(x) => {
                    show("piece", x);
                    change(x);
                }
                None => break,
            }
            step += 1;
        }
    }
    show("mutated", &v);
    println!(
        "| splitn_mut count {}",
        (&mut v[..]).splitn_mut(3, |x: &i32| *x % 3 == 0).count()
    );
    match (&mut v[..]).splitn_mut(3, |x: &i32| *x % 3 == 0).nth(1) {
        Some(x) => show("nth(1)", x),
        None => println!("| nth(1) none"),
    }
    match (&mut v[..]).splitn_mut(3, |x: &i32| *x % 3 == 0).last() {
        Some(x) => show("last()", x),
        None => println!("| last() none"),
    }
}

#[test]
fn a18_rsplit() {
    let v = make();
    {
        let mut it = (&v[..]).rsplit(|x: &i32| *x % 3 == 0);
        let mut step = 0;
        loop {
            let (lo, hi) = it.size_hint();
            println!("| rsplit hint {} {}", lo, hi.unwrap_or(999));
            let item = if step % 2 == 0 {
                it.next()
            } else {
                it.next_back()
            };
            match item {
                Some(x) => {
                    show("piece", x);
                }
                None => break,
            }
            step += 1;
        }
    }
    println!(
        "| rsplit count {}",
        (&v[..]).rsplit(|x: &i32| *x % 3 == 0).count()
    );
    match (&v[..]).rsplit(|x: &i32| *x % 3 == 0).nth(1) {
        Some(x) => show("nth(1)", x),
        None => println!("| nth(1) none"),
    }
    match (&v[..]).rsplit(|x: &i32| *x % 3 == 0).nth_back(1) {
        Some(x) => show("nth_back(1)", x),
        None => println!("| nth_back(1) none"),
    }
    match (&v[..]).rsplit(|x: &i32| *x % 3 == 0).last() {
        Some(x) => show("last()", x),
        None => println!("| last() none"),
    }
}

#[test]
fn a19_rsplit_mut() {
    let mut v = make();
    {
        let mut it = (&mut v[..]).rsplit_mut(|x: &i32| *x % 3 == 0);
        let mut step = 0;
        loop {
            let (lo, hi) = it.size_hint();
            println!("| rsplit_mut hint {} {}", lo, hi.unwrap_or(999));
            let item = if step % 2 == 0 {
                it.next()
            } else {
                it.next_back()
            };
            match item {
                Some(x) => {
                    show("piece", x);
                    change(x);
                }
                None => break,
            }
            step += 1;
        }
    }
    show("mutated", &v);
    println!(
        "| rsplit_mut count {}",
        (&mut v[..]).rsplit_mut(|x: &i32| *x % 3 == 0).count()
    );
    match (&mut v[..]).rsplit_mut(|x: &i32| *x % 3 == 0).nth(1) {
        Some(x) => show("nth(1)", x),
        None => println!("| nth(1) none"),
    }
    match (&mut v[..]).rsplit_mut(|x: &i32| *x % 3 == 0).nth_back(1) {
        Some(x) => show("nth_back(1)", x),
        None => println!("| nth_back(1) none"),
    }
    match (&mut v[..]).rsplit_mut(|x: &i32| *x % 3 == 0).last() {
        Some(x) => show("last()", x),
        None => println!("| last() none"),
    }
}

#[test]
fn a20_rsplitn() {
    let v = make();
    {
        let mut it = (&v[..]).rsplitn(3, |x: &i32| *x % 3 == 0);
        let mut step = 0;
        loop {
            let (lo, hi) = it.size_hint();
            println!("| rsplitn hint {} {}", lo, hi.unwrap_or(999));
            let item = it.next();
            match item {
                Some(x) => {
                    show("piece", x);
                }
                None => break,
            }
            step += 1;
        }
    }
    println!(
        "| rsplitn count {}",
        (&v[..]).rsplitn(3, |x: &i32| *x % 3 == 0).count()
    );
    match (&v[..]).rsplitn(3, |x: &i32| *x % 3 == 0).nth(1) {
        Some(x) => show("nth(1)", x),
        None => println!("| nth(1) none"),
    }
    match (&v[..]).rsplitn(3, |x: &i32| *x % 3 == 0).last() {
        Some(x) => show("last()", x),
        None => println!("| last() none"),
    }
}

#[test]
fn a21_rsplitn_mut() {
    let mut v = make();
    {
        let mut it = (&mut v[..]).rsplitn_mut(3, |x: &i32| *x % 3 == 0);
        let mut step = 0;
        loop {
            let (lo, hi) = it.size_hint();
            println!("| rsplitn_mut hint {} {}", lo, hi.unwrap_or(999));
            let item = it.next();
            match item {
                Some(x) => {
                    show("piece", x);
                    change(x);
                }
                None => break,
            }
            step += 1;
        }
    }
    show("mutated", &v);
    println!(
        "| rsplitn_mut count {}",
        (&mut v[..]).rsplitn_mut(3, |x: &i32| *x % 3 == 0).count()
    );
    match (&mut v[..]).rsplitn_mut(3, |x: &i32| *x % 3 == 0).nth(1) {
        Some(x) => show("nth(1)", x),
        None => println!("| nth(1) none"),
    }
    match (&mut v[..]).rsplitn_mut(3, |x: &i32| *x % 3 == 0).last() {
        Some(x) => show("last()", x),
        None => println!("| last() none"),
    }
}

#[test]
fn a22_as_chunks() {
    let v = make();
    let (chunks, rem) = (&v[..]).as_chunks::<3>();
    show("remainder", rem);
    let mut i = 0;
    while i < chunks.len() {
        show("array", &chunks[i]);
        i += 1;
    }
}

#[test]
fn a23_as_chunks_mut() {
    let mut v = make();
    let (chunks, rem) = (&mut v[..]).as_chunks_mut::<3>();
    show("remainder", rem);
    change(rem);
    let mut i = 0;
    while i < chunks.len() {
        show("array", &chunks[i]);
        change(&mut chunks[i]);
        i += 1;
    }
    show("mutated", &v);
}

#[test]
fn a24_as_rchunks() {
    let v = make();
    let (rem, chunks) = (&v[..]).as_rchunks::<3>();
    show("remainder", rem);
    let mut i = 0;
    while i < chunks.len() {
        show("array", &chunks[i]);
        i += 1;
    }
}

#[test]
fn a25_as_rchunks_mut() {
    let mut v = make();
    let (rem, chunks) = (&mut v[..]).as_rchunks_mut::<3>();
    show("remainder", rem);
    change(rem);
    let mut i = 0;
    while i < chunks.len() {
        show("array", &chunks[i]);
        change(&mut chunks[i]);
        i += 1;
    }
    show("mutated", &v);
}

#[test]
fn a26_split_first_chunk_mut() {
    let mut v = make();
    match (&mut v[..]).split_first_chunk_mut::<3>() {
        Some((a, b)) => {
            show("a", a);
            show("b", b);
            change(a);
            change(b);
        }
        None => println!("| none"),
    }
    match (&mut v[..]).split_first_chunk_mut::<0>() {
        Some((a, b)) => println!("| zero {} {}", a.len(), b.len()),
        None => println!("| zero none"),
    }
    println!(
        "| too large {}",
        (&mut v[..]).split_first_chunk_mut::<20>().is_none()
    );
    show("mutated", &v);
}

#[test]
fn a27_split_last_chunk_mut() {
    let mut v = make();
    match (&mut v[..]).split_last_chunk_mut::<3>() {
        Some((a, b)) => {
            show("a", a);
            show("b", b);
            change(a);
            change(b);
        }
        None => println!("| none"),
    }
    match (&mut v[..]).split_last_chunk_mut::<0>() {
        Some((a, b)) => println!("| zero {} {}", a.len(), b.len()),
        None => println!("| zero none"),
    }
    println!(
        "| too large {}",
        (&mut v[..]).split_last_chunk_mut::<20>().is_none()
    );
    show("mutated", &v);
}
