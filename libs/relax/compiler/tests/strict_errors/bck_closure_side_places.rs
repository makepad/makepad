//~ none
// Places borrowck keeps beside the closure body's statements -- the callee of a call through a
// captured fn item value, a user type ascription of a captured place, the place a const
// pattern is compared with -- are rewritten to env projections like the statements' places
// (Amp: makepad_stage_vj_fx doc::engine_default_dials `let d = DialDecl::new; .. .map(|..| d(..))`
// panicked in nll place_regions with the closure's virtual upvar local 0x80000001).
struct Dial { label: String, index: u32 }
impl Dial {
    fn new(label: &str, index: u32) -> Dial { Dial { label: label.to_string(), index } }
}
const ZERO: u32 = 0;

fn dials(src: &[(&str, u32)]) -> Vec<Dial> {
    let d = Dial::new;
    src.iter().map(|(label, index)| d(label, *index)).collect()
}

fn ascribed(v: &Vec<u32>) -> u32 {
    let r = v;
    let n = 5u32;
    let f = || {
        let a: &Vec<u32> = r;
        let b: u32 = n;
        let _: &Vec<u32> = r;
        a.len() as u32 + b
    };
    f()
}

fn const_pat(x: u32) -> bool {
    let f = || match x {
        ZERO => true,
        _ => false,
    };
    f()
}

fn main() {
    let v = dials(&[("a", 1), ("b", 2)]);
    assert_eq!(v.len(), 2);
    assert_eq!(v[1].label, "b");
    assert_eq!(v[1].index, 2);
    assert_eq!(ascribed(&vec![1, 2]), 7);
    assert!(const_pat(0));
    assert!(!const_pat(3));
}
