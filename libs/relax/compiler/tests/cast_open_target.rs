//! A cast to a type with `_` in it (`Rc::new(v) as Rc<_>`) is decided by later uses, not by its
//! operand: rustc checks casts at the end of typeck (check_casts), where a `collect` into
//! `Vec<Rc<dyn Tr>>` has made the cast an unsizing coercion.
use std::rc::Rc;

trait Display {
    fn id(&self) -> u32;
}

struct Screen(u32);

impl Display for Screen {
    fn id(&self) -> u32 {
        self.0
    }
}

fn displays() -> Vec<Rc<dyn Display>> {
    vec![Screen(1), Screen(2)].into_iter().map(|screen| Rc::new(screen) as Rc<_>).collect()
}

#[test]
fn cast_target_from_the_collect() {
    assert_eq!(displays().iter().map(|d| d.id()).sum::<u32>(), 3);
    let boxed: Vec<Box<dyn Display>> = (1..3).map(|i| Box::new(Screen(i)) as Box<_>).collect();
    assert_eq!(boxed[1].id(), 2);
}

#[test]
fn cast_target_from_an_annotation() {
    let r: Rc<dyn Display> = Rc::new(Screen(7)) as Rc<_>;
    assert_eq!(r.id(), 7);
    let pair: (u8, Rc<dyn Display>) = (1, Rc::new(Screen(4)) as Rc<_>);
    assert_eq!(pair.1.id(), 4);
}
