// A `let` initializer's tuple struct or variant constructor call extends the temporaries its
// `&` arguments borrow to the end of the binding's scope, through tuples and block tails too, as
// a tuple's elements are (Reference destructors.scope.lifetime-extension); an ordinary call's
// argument ends with its statement. The value lived until its statement ended and was then read
// after its drop (makepad PR #1531). "| " lines must equal rustc's (agree.sh).
struct Probe(u32);
impl Drop for Probe {
    fn drop(&mut self) {
        println!("| drop {}", self.0);
    }
}
struct Wrap<'a>(&'a Probe);
enum E<'a> {
    V(&'a Probe),
    #[allow(dead_code)]
    N,
}

#[test]
fn a01_constructor_arguments_live_to_the_scope_end() {
    let v = Some(&Probe(1));
    println!("| bound some");
    let w = Wrap(&Probe(2));
    println!("| bound tuple struct");
    let e = E::V(&Probe(3));
    println!("| bound variant");
    let nested = Some(Wrap(&Probe(4)));
    println!("| bound nested");
    let tup = (Some(&Probe(5)), 1);
    println!("| bound in a tuple");
    let blk = { Some(&Probe(6)) };
    println!("| bound in a block tail");
    let arg = std::convert::identity(Some(&Probe(7)).is_some());
    println!("| a call's argument ended with its statement: {}", arg);
    let e = match e {
        E::V(p) => p.0,
        E::N => 0,
    };
    println!("| read {} {} {} {} {} {}", v.unwrap().0, w.0 .0, e, nested.unwrap().0 .0, tup.0.unwrap().0, blk.unwrap().0);
}
