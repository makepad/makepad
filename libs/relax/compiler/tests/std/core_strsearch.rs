fn observe(h: &str, n: &str) {
    println!("| case [{}] [{}] {:?} {:?} {}", h, n, h.find(n), h.rfind(n), h.contains(n));
    for (i, m) in h.match_indices(n) { println!("| m {} [{}]", i, m); }
    for (i, m) in h.rmatch_indices(n) { println!("| r {} [{}]", i, m); }
    for s in h.split(n) { println!("| s [{}]", s); }
    for s in h.rsplit(n) { println!("| b [{}]", s); }
    for s in h.split_terminator(n) { println!("| t [{}]", s); }
    for s in h.rsplit_terminator(n) { println!("| u [{}]", s); }
    for s in h.splitn(3, n) { println!("| n [{}]", s); }
    for s in h.rsplitn(3, n) { println!("| q [{}]", s); }
    println!("| replace [{}]", h.replace(n, "!"));
}
#[test]
fn a01_edges() {
    observe("", ""); observe("", "long"); observe("a", "long");
    observe("éê🦀aé", ""); observe("ababababa", "abab");
    observe("zzacbaacbazzacba", "acba"); observe("zzéêéê🦀éê", "éê");
    observe("zzabcabxyzabcabxyz", "abcabxyz");
}
#[test]
fn a02_long() {
    let n = "abab".repeat(20);
    let h = format!("z{}z{}", n, n);
    observe(&h, &n);
    let n = format!("{}c", n);
    observe(&h, &n);
}
fn random(seed: &mut u64) -> usize {
    *seed ^= *seed << 13; *seed ^= *seed >> 7; *seed ^= *seed << 17;
    *seed as usize
}
#[test]
fn a03_random() {
    let mut seed = 73u64;
    let alphabet = ['a', 'b', 'é', 'ê', 'ꁁ', '🦀'];
    for _ in 0..150 {
        let mut h = String::new();
        let mut n = String::new();
        let len = random(&mut seed) % 45;
        for _ in 0..len { h.push(alphabet[random(&mut seed) % 6]); }
        let len = random(&mut seed) % 6;
        for _ in 0..len { n.push(alphabet[random(&mut seed) % 6]); }
        observe(&h, &n);
        for c in alphabet {
            println!("| char {} {:?} {:?}", c, h.find(c), h.rfind(c));
        }
    }
}
