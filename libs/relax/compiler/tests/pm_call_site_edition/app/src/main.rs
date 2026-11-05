// Edition 2024: a block tail's temporaries are dropped before its locals, an `if let`
// scrutinee's before the `else`, and `Box<[T]>::into_iter` iterates by value. Tokens the
// 2021 macros make keep 2021's rules, tokens carried from here keep 2024's.
use pmce_mac::{carried, fresh_block, fresh_braces, fresh_if_let, fresh_into_iter, fresh_rules};
use std::cell::RefCell;

thread_local! {
    static LOG: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
}

pub fn log(s: &'static str) {
    LOG.with(|l| l.borrow_mut().push(s));
}

fn take() -> String {
    LOG.with(|l| l.borrow_mut().drain(..).collect::<Vec<_>>().join(","))
}

pub struct Noisy(pub &'static str);

impl Drop for Noisy {
    fn drop(&mut self) {
        log(self.0);
    }
}

impl Noisy {
    pub fn get(&self) -> u8 {
        1
    }
    pub fn none(&self) -> Option<u8> {
        None
    }
}

pub fn boxed() -> Box<[u8]> {
    Box::new([1])
}

pub fn name_of<T>(_: T) -> &'static str {
    std::any::type_name::<T>()
}

fresh_rules!();

fn main() {
    let a = fresh_block!();
    let fresh = take();
    let b = carried!({ let _local = Noisy("local"); Noisy("temp").get() });
    let kept = take();
    let c = fresh_braces!(let _local = Noisy("local"); Noisy("temp").get());
    let braces = take();
    fresh_if_let!();
    let if_let = take();
    carried!(if let Some(_) = Noisy("scrutinee").none() {} else { log("else") });
    let kept_if_let = take();
    let d = made_tail!();
    let rules = take();
    println!("block {fresh} / {kept}; braces {braces}; if-let {if_let} / {kept_if_let}; into_iter {} / {}; rules {rules}; {}",
        fresh_into_iter!(), carried!(name_of(boxed().into_iter().next().unwrap())), a + b + c + d);
}
