use crate::s;
use std::hash::{BuildHasher, BuildHasherDefault, Hasher};
use std::hint::black_box;
#[derive(Default)]
pub struct LiveIdHasher(u64);

impl std::hash::Hasher for LiveIdHasher {
    fn write(&mut self, _: &[u8]) {
        unreachable!("Invalid use of IdHasher");
    }

    fn write_u8(&mut self, _n: u8) {
        unreachable!("Invalid use of IdHasher");
    }
    fn write_u16(&mut self, _n: u16) {
        unreachable!("Invalid use of IdHasher");
    }
    fn write_u32(&mut self, _n: u32) {
        unreachable!("Invalid use of IdHasher");
    }

    #[inline(always)]
    fn write_u64(&mut self, n: u64) {
        self.0 = n;
    }

    fn write_usize(&mut self, _n: usize) {
        unreachable!("Invalid use of IdHasher");
    }

    fn write_i8(&mut self, _n: i8) {
        unreachable!("Invalid use of IdHasher");
    }
    fn write_i16(&mut self, _n: i16) {
        unreachable!("Invalid use of IdHasher");
    }
    fn write_i32(&mut self, _n: i32) {
        unreachable!("Invalid use of IdHasher");
    }
    fn write_i64(&mut self, _n: i64) {
        unreachable!("Invalid use of IdHasher");
    }
    fn write_isize(&mut self, _n: isize) {
        unreachable!("Invalid use of IdHasher");
    }

    #[inline(always)]
    fn finish(&self) -> u64 {
        self.0
    }
}

#[derive(Copy, Clone, Default)]
pub struct LiveIdHasherBuilder {}

impl std::hash::BuildHasher for LiveIdHasherBuilder {
    type Hasher = LiveIdHasher;

    #[inline(always)]
    fn build_hasher(&self) -> LiveIdHasher {
        LiveIdHasher::default()
    }
}

// 64-bit FxHasher word step, from libs/fxhash/lib.rs (ROTATE=5, SEED64).
#[derive(Default)]
pub struct FxHasher(u64);
impl Hasher for FxHasher {
    fn write(&mut self, _: &[u8]) {
        unreachable!("u64 keys only");
    }
    fn write_u64(&mut self, x: u64) {
        self.0 = (self.0.rotate_left(5) ^ x).wrapping_mul(0x517cc1b727220a95);
    }
    fn finish(&self) -> u64 {
        self.0
    }
}
fn next(x: &mut u64) -> u64 {
    *x ^= *x << 13;
    *x ^= *x >> 7;
    *x ^= *x << 17;
    *x
}
fn random(n: usize) -> s::Vec<u64> {
    let mut v = s::Vec::new();
    let mut x = 42;
    for _ in 0..n {
        v.push(next(&mut x));
    }
    v
}
fn digest(v: &[u64]) -> u64 {
    let mut h = 0u64;
    for &x in v {
        h = h.rotate_left(7) ^ x;
    }
    black_box(h)
}
fn text_digest(v: &str) -> u64 {
    let mut h = 0u64;
    for b in v.as_bytes() {
        h = h.wrapping_mul(31).wrapping_add(*b as u64);
    }
    black_box(h)
}
fn map_work<H: BuildHasher>(mut m: s::HashMap<u64, u64, H>) -> u64 {
    let mut x = 42;
    for i in 0..1_000_000 {
        m.insert(next(&mut x) & !1, i);
    }
    x = 42;
    let mut sum = 0u64;
    for i in 0..1_000_000 {
        let k = (next(&mut x) & !1) | (i & 1);
        sum = sum.wrapping_add(m.get(black_box(&k)).copied().unwrap_or(0));
    }
    black_box(sum)
}
pub fn hashmap_u64_insert_get() -> u64 {
    map_work(s::HashMap::new())
}
pub fn hashmap_liveid() -> u64 {
    map_work(s::HashMap::with_hasher(LiveIdHasherBuilder {}))
}
pub fn hashmap_fx() -> u64 {
    map_work(s::HashMap::with_hasher(
        BuildHasherDefault::<FxHasher>::default(),
    ))
}
pub fn hashset_string() -> u64 {
    let mut keys = s::SetVec::new();
    for i in 0..200_000 {
        keys.push(s::set_key(i));
    }
    let mut set = s::HashSet::new();
    for k in &keys {
        set.insert(k.clone());
    }
    let mut n = 0;
    for k in &keys {
        n += set.contains(black_box(k)) as u64;
    }
    n
}
pub fn hashmap_remove_churn() -> u64 {
    let mut m = s::HashMap::new();
    let mut sum = 0;
    for i in 0..200_000u64 {
        m.insert(i, i);
    }
    for i in 0..1_000_000u64 {
        sum += m.remove(&i).unwrap();
        m.insert(i + 200_000, i + 200_000);
    }
    sum + m.len() as u64
}
pub fn vec_push() -> u64 {
    let mut v = s::Vec::new();
    for i in 0..10_000_000u32 {
        v.push(black_box(i));
    }
    let mut sum = 0;
    for x in v {
        sum += x as u64;
    }
    sum
}
pub fn vec_extend_from_slice() -> u64 {
    let a = [17u32; 1024];
    let mut v = s::Vec::new();
    for _ in 0..10_000 {
        v.extend_from_slice(black_box(&a));
    }
    let mut sum = 0;
    for x in v {
        sum += x as u64;
    }
    sum
}
pub fn sort_u64_random() -> u64 {
    let mut v = random(2_000_000);
    s::sort(&mut v);
    digest(&v)
}
pub fn sort_unstable_u64_random() -> u64 {
    let mut v = random(2_000_000);
    s::sort_unstable(&mut v);
    digest(&v)
}
#[derive(Clone, Copy)]
struct Record {
    value: f32,
    key: u32,
}
pub fn sort_by_key_struct() -> u64 {
    let mut v = s::Vec::new();
    let mut x = 42;
    for i in 0..2_000_000 {
        v.push(Record {
            value: i as f32,
            key: next(&mut x) as u32,
        });
    }
    s::sort_key(&mut v, |r| r.key);
    let mut h = 0u64;
    for r in v {
        h = h.rotate_left(7) ^ r.key as u64 ^ r.value.to_bits() as u64;
    }
    h
}
pub fn sort_sorted_and_reversed() -> u64 {
    let mut a = s::Vec::new();
    let mut b = s::Vec::new();
    for i in 0..2_000_000u64 {
        a.push(i);
        b.push(1_999_999 - i);
    }
    s::sort(&mut a);
    s::sort(&mut b);
    digest(&a).wrapping_add(digest(&b))
}
pub fn dedup_binary_search() -> u64 {
    let mut v = s::Vec::new();
    for i in 0..2_000_000u64 {
        v.push(i / 4);
    }
    v.dedup();
    let mut sum = 0;
    for i in 0..1_000_000 {
        sum += s::search(black_box(&v), black_box(i % 500_000)) as u64;
    }
    sum + v.len() as u64
}
pub fn string_push_str() -> u64 {
    let mut v = s::String::new();
    for _ in 0..2_000_000 {
        v.push_str(black_box("hello,world!\n"));
    }
    text_digest(&v)
}
pub fn format_ints() -> u64 {
    let mut h = 0u64;
    for i in 0..1_000_000 {
        h = h.wrapping_add(text_digest(&s::ints(black_box(i))));
    }
    h
}
pub fn format_floats() -> u64 {
    let mut h = 0u64;
    for i in 0..500_000 {
        let f = black_box(i as f64 / 37.0);
        h = h
            .wrapping_add(text_digest(&s::float_fixed(f)))
            .wrapping_add(text_digest(&s::float_short(f)));
    }
    h
}
pub fn write_to_string() -> u64 {
    let mut v = s::String::new();
    let mut h = 0u64;
    for i in 0..1_000_000 {
        v.clear();
        s::write_ints(&mut v, black_box(i));
        h = h.wrapping_add(text_digest(&v));
    }
    h
}
pub fn str_find() -> u64 {
    let mut v = s::String::new();
    for _ in 0..65_536 {
        v.push_str("abcdefghijklmnop");
    }
    v.push_str("needle!");
    let mut sum = 0;
    for _ in 0..1000 {
        sum += s::find(black_box(&v), "needle") as u64 + s::find_char(black_box(&v), '!') as u64;
    }
    sum
}
pub fn str_split_lines() -> u64 {
    let mut v = s::String::new();
    for _ in 0..100_000 {
        v.push_str("abc,def,ghi,jkl\n");
    }
    let mut sum = 0;
    for _ in 0..30 {
        sum += s::fields(black_box(&v)) as u64;
    }
    sum
}
pub fn parse_ints() -> u64 {
    let a = ["0", "123456789", "4294967295", "987654"];
    let b = [
        "-9223372036854775808",
        "123456789",
        "9223372036854775807",
        "-987654",
    ];
    let mut sum = 0u64;
    for i in 0..1_000_000 {
        sum = sum
            .wrapping_add(s::parse_u32(black_box(a[i % 4])) as u64)
            .wrapping_add(s::parse_i64(black_box(b[i % 4])) as u64);
    }
    sum
}
pub fn parse_floats() -> u64 {
    let a = [
        "3.141592653589793",
        "-1.234567890123e-100",
        "2.718281828459045",
        "1.7976931348623157e308",
    ];
    let mut h = 0u64;
    for i in 0..1_000_000 {
        h = h.wrapping_add(s::parse_f64(black_box(a[i % 4])).to_bits());
    }
    h
}
pub fn rc_refcell_churn() -> u64 {
    let r = s::Rc::new(s::RefCell::new(s::RcVec::new()));
    let mut sum = 0;
    for i in 0..5_000_000u32 {
        let c = black_box(r.clone());
        {
            let mut v = c.borrow_mut();
            v.push(i);
            sum += v.pop().unwrap() as u64;
        }
        drop(c);
    }
    sum
}
pub fn btreemap_insert_range() -> u64 {
    let mut m = s::BTreeMap::new();
    let mut x = 42;
    let mut keys = s::Vec::new();
    for i in 0..500_000 {
        let k = next(&mut x);
        keys.push(k);
        m.insert(k, i);
    }
    let mut sum = 0u64;
    for i in 0..1000 {
        let lo = keys[i];
        let hi = lo.saturating_add(u64::MAX / 1000);
        for (_, v) in m.range(lo..hi) {
            sum = sum.wrapping_add(*v);
        }
    }
    for k in keys {
        sum = sum.wrapping_add(m.remove(&k).unwrap());
    }
    sum
}
pub fn iter_chain() -> u64 {
    let mut a = s::Vec::new();
    for i in 0..2_000_000 {
        a.push((i % 1024) as f32);
    }
    let b: s::Vec<f32> = a
        .iter()
        .filter(|x| **x > 100.0)
        .map(|x| x * 0.5)
        .zip(a.iter())
        .map(|(x, y)| x + y)
        .collect();
    let sum: f32 = black_box(&b).iter().copied().sum();
    sum.to_bits() as u64
}
pub fn mpsc_ping() -> u64 {
    let (tx, rx) = s::channel::<u64>();
    let h = std::thread::spawn(move || {
        for i in 0..1_000_000 {
            tx.send(i).unwrap();
        }
    });
    let mut sum = 0;
    for _ in 0..1_000_000 {
        sum += rx.recv().unwrap();
    }
    h.join().unwrap();
    sum
}

// Fixed batches preserve the requested per-workload sizes and target 50–300 ms in A.
pub fn batch_size(name: &str) -> usize {
    match name {
        "hashmap_liveid" => 3,
        "hashmap_fx" => 3,
        "hashset_string" => 3,
        "vec_push" => 6,
        "vec_extend_from_slice" => 24,
        "sort_u64_random" => 2,
        "sort_unstable_u64_random" => 3,
        "sort_by_key_struct" => 2,
        "sort_sorted_and_reversed" => 7,
        "dedup_binary_search" => 5,
        "string_push_str" => 3,
        "write_to_string" => 3,
        "parse_ints" => 5,
        "parse_floats" => 6,
        "rc_refcell_churn" => 8,
        "iter_chain" => 16,
        "mpsc_ping" => 10,
        _ => 1,
    }
}
fn batch(f: fn() -> u64, count: usize) -> u64 {
    let mut checksum = 0u64;
    for _ in 0..count {
        checksum = checksum.wrapping_add(black_box(f()));
    }
    black_box(checksum)
}
pub fn measure(name: &str, f: fn() -> u64) {
    let count = batch_size(name);
    let expected = batch(f, count);
    let mut best = f64::MAX;
    for _ in 0..5 {
        let start = std::time::Instant::now();
        let result = batch(f, count);
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(result, expected);
        if ms < best {
            best = ms;
        }
    }
    println!("| {} {:.2} {}", name, best, expected);
}
pub fn run() {
    let selection = std::env::args().nth(1);
    if selection.is_none() || selection.as_deref() == Some("hashmap_u64_insert_get") {
        measure("hashmap_u64_insert_get", hashmap_u64_insert_get);
    }
    if selection.is_none() || selection.as_deref() == Some("hashmap_liveid") {
        measure("hashmap_liveid", hashmap_liveid);
    }
    if selection.is_none() || selection.as_deref() == Some("hashmap_fx") {
        measure("hashmap_fx", hashmap_fx);
    }
    if selection.is_none() || selection.as_deref() == Some("hashset_string") {
        measure("hashset_string", hashset_string);
    }
    if selection.is_none() || selection.as_deref() == Some("hashmap_remove_churn") {
        measure("hashmap_remove_churn", hashmap_remove_churn);
    }
    if selection.is_none() || selection.as_deref() == Some("vec_push") {
        measure("vec_push", vec_push);
    }
    if selection.is_none() || selection.as_deref() == Some("vec_extend_from_slice") {
        measure("vec_extend_from_slice", vec_extend_from_slice);
    }
    if selection.is_none() || selection.as_deref() == Some("sort_u64_random") {
        measure("sort_u64_random", sort_u64_random);
    }
    if selection.is_none() || selection.as_deref() == Some("sort_unstable_u64_random") {
        measure("sort_unstable_u64_random", sort_unstable_u64_random);
    }
    if selection.is_none() || selection.as_deref() == Some("sort_by_key_struct") {
        measure("sort_by_key_struct", sort_by_key_struct);
    }
    if selection.is_none() || selection.as_deref() == Some("sort_sorted_and_reversed") {
        measure("sort_sorted_and_reversed", sort_sorted_and_reversed);
    }
    if selection.is_none() || selection.as_deref() == Some("dedup_binary_search") {
        measure("dedup_binary_search", dedup_binary_search);
    }
    if selection.is_none() || selection.as_deref() == Some("string_push_str") {
        measure("string_push_str", string_push_str);
    }
    if selection.is_none() || selection.as_deref() == Some("format_ints") {
        measure("format_ints", format_ints);
    }
    if selection.is_none() || selection.as_deref() == Some("format_floats") {
        measure("format_floats", format_floats);
    }
    if selection.is_none() || selection.as_deref() == Some("write_to_string") {
        measure("write_to_string", write_to_string);
    }
    if selection.is_none() || selection.as_deref() == Some("str_find") {
        measure("str_find", str_find);
    }
    if selection.is_none() || selection.as_deref() == Some("str_split_lines") {
        measure("str_split_lines", str_split_lines);
    }
    if selection.is_none() || selection.as_deref() == Some("parse_ints") {
        measure("parse_ints", parse_ints);
    }
    if selection.is_none() || selection.as_deref() == Some("parse_floats") {
        measure("parse_floats", parse_floats);
    }
    if selection.is_none() || selection.as_deref() == Some("rc_refcell_churn") {
        measure("rc_refcell_churn", rc_refcell_churn);
    }
    if selection.is_none() || selection.as_deref() == Some("btreemap_insert_range") {
        measure("btreemap_insert_range", btreemap_insert_range);
    }
    if selection.is_none() || selection.as_deref() == Some("iter_chain") {
        measure("iter_chain", iter_chain);
    }
    if selection.is_none() || selection.as_deref() == Some("mpsc_ping") {
        measure("mpsc_ping", mpsc_ping);
    }
}
