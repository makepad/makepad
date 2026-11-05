// Does not compile (E0308): `makepad test bt-app --features broken` fails this target, naming
// this file, and still runs the package's other targets (run.sh).
#[test]
fn does_not_compile() {
    let n: u32 = "not a number";
    assert_eq!(n, 0);
}
