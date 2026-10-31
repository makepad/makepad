// Differential observations: driftsort, ipnsort, introselect and duplicate search.
fn next(seed: &mut u64) -> u64 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    *seed
}
#[test]
fn a01_patterns() {
    let mut seed = 0x123456789abcdefu64;
    for n in [
        0usize, 1, 2, 15, 16, 17, 20, 21, 31, 32, 33, 40, 64, 65, 511, 513, 4097,
    ] {
        for pattern in 0..6 {
            let mut a = Vec::new();
            for i in 0..n {
                let x = match pattern {
                    0 => i as u64,
                    1 => (n - i) as u64,
                    2 => (i % 13) as u64,
                    3 => 7,
                    4 => next(&mut seed) % 5,
                    _ => next(&mut seed),
                };
                a.push(x);
            }
            let mut b = a.clone();
            a.sort();
            b.sort_unstable();
            let mut hash = 0u64;
            for x in &a {
                hash = hash.wrapping_mul(31).wrapping_add(*x);
            }
            println!("| sort {} {} {} {}", n, pattern, a == b, hash);
        }
    }
}
#[test]
fn a02_stable_keys() {
    let mut seed = 0x71398475u64;
    for n in [0usize, 1, 19, 21, 33, 257, 1025] {
        let mut a = Vec::new();
        for i in 0..n {
            a.push((next(&mut seed) % 11, i));
        }
        let mut b = a.clone();
        a.sort_by_key(|x| x.0);
        b.sort_by_cached_key(|x| x.0);
        let mut stable = true;
        let mut hash = 0usize;
        for i in 0..n {
            hash = hash.wrapping_mul(31).wrapping_add(a[i].1);
            if i > 0 && a[i - 1].0 == a[i].0 && a[i - 1].1 > a[i].1 {
                stable = false;
            }
        }
        println!("| keys {} {} {} {}", n, stable, a == b, hash);
    }
}
#[test]
fn a03_select_and_search() {
    let mut seed = 0x99117u64;
    for n in [1usize, 2, 16, 17, 33, 127, 1025] {
        let mut input = Vec::new();
        for _ in 0..n {
            input.push((next(&mut seed) % 29) as i32);
        }
        let mut sorted = input.clone();
        sorted.sort();
        for index in [0usize, n / 3, n / 2, n - 1] {
            let mut v = input.clone();
            v.select_nth_unstable(index);
            let pivot = v[index];
            let mut valid = pivot == sorted[index];
            for i in 0..index {
                valid = valid && v[i] <= pivot;
            }
            for i in index + 1..n {
                valid = valid && v[i] >= pivot;
            }
            v.sort_unstable();
            println!("| select {} {} {} {}", n, index, valid, v == sorted);
        }
        for key in [-1i32, 0, 1, 14, 28, 29] {
            println!(
                "| search {} {} {:?} {:?} {:?}",
                n,
                key,
                sorted.binary_search(&key),
                sorted.binary_search_by(|x| x.cmp(&key)),
                sorted.binary_search_by_key(&key, |x| *x)
            );
        }
    }
}
#[test]
fn a04_strings_floats_and_large() {
    let mut words = vec![
        String::from("pear"),
        String::from("apple"),
        String::from("pear"),
        String::from("fig"),
    ];
    words.sort();
    println!("| words {:?}", words);
    words.sort_unstable();
    println!("| words unstable {:?}", words);
    let mut v = vec![3.0f64, -0.0, 0.0, -1.0, 2.0];
    v.sort_by(|a, b| a.total_cmp(b));
    println!("| floats {:?}", v);
    let mut seed = 0x314159265u64;
    let mut a = Vec::new();
    for _ in 0..100_000 {
        a.push(next(&mut seed));
    }
    let mut b = a.clone();
    a.sort();
    b.sort_unstable();
    println!("| large {} {} {}", a == b, a[0], a[99_999]);
}
#[derive(Clone)]
struct Large {
    key: u64,
    serial: usize,
    payload: [u64; 16],
}
#[test]
fn a05_large_elements() {
    let mut seed = 77881u64;
    let mut v = Vec::new();
    for i in 0..257 {
        v.push(Large {
            key: next(&mut seed) % 17,
            serial: i,
            payload: [i as u64; 16],
        });
    }
    let mut stable = v.clone();
    stable.sort_by_key(|x| x.key);
    let mut good = true;
    for i in 1..stable.len() {
        good = good
            && (stable[i - 1].key < stable[i].key
                || (stable[i - 1].key == stable[i].key && stable[i - 1].serial < stable[i].serial));
    }
    v.sort_unstable_by_key(|x| x.key);
    let mut same = true;
    let mut total = 0u64;
    for i in 0..v.len() {
        same = same && v[i].key == stable[i].key;
        total += v[i].payload[15];
    }
    println!("| large elements {} {} {}", good, same, total);
    v.reverse();
    v.select_nth_unstable_by(128, |a, b| a.key.cmp(&b.key));
    println!("| large select {}", v[128].key == stable[128].key);
}
#[test]
fn a06_small_lengths_and_narrow_types() {
    let mut seed = 1238711u64;
    let mut hash = 0u64;
    let mut same = true;
    for n in 0..=40 {
        let mut a = Vec::new();
        let mut b = Vec::new();
        for _ in 0..n {
            let x = next(&mut seed);
            a.push(x as i8);
            b.push(x as u32);
        }
        let mut aa = a.clone();
        let mut bb = b.clone();
        a.sort();
        aa.sort_unstable();
        b.sort();
        bb.sort_unstable();
        same = same && a == aa && b == bb;
        for x in &a {
            hash = hash.wrapping_mul(31).wrapping_add(*x as u64);
        }
        for x in &b {
            hash = hash.wrapping_mul(31).wrapping_add(*x as u64);
        }
    }
    println!("| narrow {} {}", same, hash);
}

#[test]
fn a07_owned_strings() {
    let mut seed = 0x166991u64;
    let mut v = Vec::new();
    for _ in 0..513 {
        v.push((next(&mut seed) % 79).to_string());
    }
    let mut expected = v.clone();
    expected.sort();
    let mut unstable = v.clone();
    unstable.sort_unstable();
    v.select_nth_unstable(256);
    let mut valid = v[256] == expected[256];
    for i in 0..256 {
        valid = valid && v[i] <= v[256];
    }
    for i in 257..513 {
        valid = valid && v[i] >= v[256];
    }
    println!(
        "| owned {} {} {} {}",
        unstable == expected,
        valid,
        expected[0],
        expected[512]
    );
}

#[derive(Clone, Copy)]
struct CopyItem {
    key: u32,
    value: f32,
}
#[test]
fn a08_copy_struct_keys() {
    let mut seed = 0x71398475u64;
    for n in [
        0usize, 1, 15, 16, 17, 19, 20, 21, 31, 32, 33, 34, 47, 48, 49, 63, 64, 65, 257,
    ] {
        let mut v = Vec::new();
        for i in 0..n {
            v.push(CopyItem {
                key: (next(&mut seed) % 7) as u32,
                value: i as f32,
            });
        }
        let mut unstable = v.clone();
        v.sort_by_key(|x| x.key);
        unstable.sort_unstable_by_key(|x| x.key);
        let mut stable = true;
        let mut same = true;
        let mut hash = 0u64;
        for i in 0..n {
            same = same && v[i].key == unstable[i].key;
            hash = hash.wrapping_mul(31).wrapping_add(v[i].value as u64);
            if i > 0 {
                stable = stable
                    && (v[i - 1].key < v[i].key
                        || (v[i - 1].key == v[i].key && v[i - 1].value < v[i].value));
            }
        }
        println!("| copy keys {} {} {} {}", n, stable, same, hash);
    }
}
