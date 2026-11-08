// M10: an enum whose derived `Hash` ran (a set holds a value's hash; the derive hashes the
// variant's discriminant, here its position).
//  v1: a variant added after the running ones keeps their positions: applies in place, and the
//      set still finds the held value.
//  v2: a variant inserted before a running one moves the position its hash is made from: the
//      restart rung, naming the derived `Hash`.
use std::cell::RefCell;
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Tab {
    Home,
    Settings,
    Help,
}

thread_local! {
    static SEEN: RefCell<HashSet<Tab>> = RefCell::new(HashSet::new());
}

#[test]
fn run() {
    SEEN.with(|s| {
        if s.borrow().is_empty() {
            s.borrow_mut().insert(Tab::Settings);
        }
        let s = s.borrow();
        println!("out has {} {} {} n {}", s.contains(&Tab::Settings), s.contains(&Tab::Home), s.contains(&Tab::Help), s.len());
    });
}
