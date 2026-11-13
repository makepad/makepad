use std::cell::Cell;
use std::rc::Rc;

struct Probe(Rc<Cell<usize>>, String);
impl Drop for Probe {
    fn drop(&mut self) { self.0.set(self.0.get() + 1); }
}
struct Borrowed<'a>(&'a Probe);

#[test]
fn a01_variant_constructor_keeps_the_borrowed_temporary_alive() {
    let drops = Rc::new(Cell::new(0));
    {
        let value = Some(&Probe(drops.clone(), "variant".into()));
        assert_eq!(drops.get(), 0);
        assert_eq!(value.unwrap().1, "variant");
    }
    assert_eq!(drops.get(), 1);
    println!("| variant temporary drops at the end of its binding's scope");
}

#[test]
fn a02_tuple_struct_keeps_the_borrowed_temporary_alive() {
    let drops = Rc::new(Cell::new(0));
    {
        let value = Borrowed(&Probe(drops.clone(), "tuple".into()));
        assert_eq!(drops.get(), 0);
        assert_eq!(value.0.1, "tuple");
    }
    assert_eq!(drops.get(), 1);
    println!("| tuple struct temporary drops at the end of its binding's scope");
}

#[test]
fn a03_ordinary_calls_do_not_extend_their_arguments() {
    fn consume(value: Borrowed<'_>) -> usize { value.0.0.get() }
    let drops = Rc::new(Cell::new(0));
    let during_call = consume(Borrowed(&Probe(drops.clone(), "call".into())));
    assert_eq!(during_call, 0);
    assert_eq!(drops.get(), 1);
    println!("| ordinary call's argument drops at the end of the statement");
}
