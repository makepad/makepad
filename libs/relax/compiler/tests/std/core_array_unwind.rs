// `[S; N]::clone()` / array::from_fn whose element constructor panics: the elements made so far
// are dropped (core's array Guard); tuples too (tests/ui traits/clone-unwind-rc-cleanup.rs)
use std::rc::Rc;
struct S(Rc<()>);
impl Clone for S {
    fn clone(&self) -> Self {
        if Rc::strong_count(&self.0) == 7 { panic!("oops"); }
        S(self.0.clone())
    }
}
#[test]
fn clone_unwind_drops_made_elements() {
    std::panic::set_hook(Box::new(|_| {}));
    let counter = Rc::new(());
    let c = counter.clone();
    let r = std::panic::catch_unwind(move || { let _ = (S(c.clone()), S(c.clone()), S(c.clone()), S(c)).clone(); });
    assert!(r.is_err());
    assert_eq!(Rc::strong_count(&counter), 1);
    let c = counter.clone();
    let r = std::panic::catch_unwind(move || { let _ = [S(c.clone()), S(c.clone()), S(c.clone()), S(c)].clone(); });
    assert!(r.is_err());
    assert_eq!(Rc::strong_count(&counter), 1);
    let _ = std::panic::take_hook();
    let a: [String; 3] = std::array::from_fn(|i| i.to_string());
    assert_eq!(a.concat(), "012");
}
