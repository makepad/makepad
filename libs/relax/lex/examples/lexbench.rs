use std::io::Read;
fn walk(d: &std::path::Path, out: &mut Vec<Vec<u8>>) {
    let Ok(rd) = std::fs::read_dir(d) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() { walk(&p, out); } else if p.extension().map_or(false, |x| x == "rs") {
            let mut b = Vec::new();
            if std::fs::File::open(&p).and_then(|mut f| f.read_to_end(&mut b)).is_ok() { out.push(b); }
        }
    }
}
fn main() {
    let mut files = Vec::new();
    for a in std::env::args().skip(1) { walk(std::path::Path::new(&a), &mut files); }
    let bytes: usize = files.iter().map(|f| f.len()).sum();
    let mut toks = 0usize; let mut sum = 0u64;
    let mut out = Vec::new();
    for _ in 0..5 {
        for f in &files {
            out.clear();
            if relax_lex::lexer::lex(f, 2021, &mut out).is_ok() {
                toks += out.len();
                for t in &out { sum = sum.wrapping_mul(31).wrapping_add(t.kind as u64 ^ (t.lo as u64) << 8 ^ (t.hi as u64) << 32); }
            }
        }
    }
    println!("files {} bytes {} tokens {} checksum {:x}", files.len(), bytes, toks, sum);
}
