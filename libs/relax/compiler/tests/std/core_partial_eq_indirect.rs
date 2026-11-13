// Built-in equality where an enum's inactive payload holds stale bytes (const patterns), and
// equality through generic and opaque values of aggregates: each compares the active fields
// (makepad PR #1531). "| " lines must equal rustc's (agree.sh).
#[derive(PartialEq, Eq, Debug, Clone, Copy)]
enum K {
    A(u32),
    B,
}
const KB: K = K::B;
const NONE32: Option<u32> = None;
const PAIR: (Option<u64>, K) = (None, K::B);

#[inline(never)]
fn reset_k(k: &mut K) {
    *k = K::B;
}

#[inline(never)]
fn reset_o(o: &mut Option<u32>) {
    *o = None;
}

fn eq_generic<T: PartialEq>(a: T, b: T) -> bool {
    a == b
}

fn opaque<T: PartialEq>(v: T) -> impl PartialEq {
    v
}

#[test]
fn a01_stale_payload_patterns() {
    let mut k = K::A(0xdead_beef);
    reset_k(&mut k);
    let mut o = Some(0xdead_beefu32);
    reset_o(&mut o);
    println!("| enum const {} {}", matches!(k, KB), matches!(K::A(1), KB));
    println!("| option const {} {}", matches!(o, NONE32), matches!(Some(3u32), NONE32));
    let mut p = (Some(9u64), K::A(7));
    p.0 = None;
    reset_k(&mut p.1);
    println!("| tuple const {}", matches!(p, PAIR));
    println!("| derived eq {} {}", k == K::B, o == None);
}

#[test]
fn a02_generic_and_opaque_aggregates() {
    let a = [0.0f64; 128];
    let b = [1.0f64; 128];
    println!("| generic big {} {}", eq_generic(a, b), eq_generic(a, a));
    println!("| opaque big {} {}", opaque(a) == opaque(b), opaque([f64::NAN; 64]) == opaque([f64::NAN; 64]));
    let mut k = K::A(5);
    reset_k(&mut k);
    println!("| opaque enum {} {}", opaque(k) == opaque(K::B), opaque((1u8, K::A(2))) == opaque((1u8, K::A(3))));
    let s = (String::from("x"), 1u8);
    let (x, y) = ((String::from("x"), 1u8), (String::from("y"), 1u8));
    println!("| opaque tuple ref {} {}", opaque(&s) == opaque(&x), opaque(&s) == opaque(&y));
}
