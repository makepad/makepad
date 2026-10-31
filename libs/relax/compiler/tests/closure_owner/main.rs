mod callbacks;

fn wrap<F: Fn(u32) -> u32 + Clone>(callback: F) -> (u32, u32) {
    let callback_clone = callback.clone();
    let block = move |value| callback_clone(value);
    (block(3), block(4))
}

fn same_file() -> impl Fn(u32) -> u32 + Clone {
    let offset = 7;
    move |value| value + offset
}

fn wrap_mut<F: FnMut(u32) -> u32 + Clone>(callback: F) -> (u32, u32) {
    let mut callback_clone = callback.clone();
    let mut block = move |value| callback_clone(value);
    (block(3), block(4))
}

fn wrap_once<F: FnOnce() -> u32 + Clone>(callback: F) -> u32 {
    let callback_clone = callback.clone();
    let block = move || callback_clone();
    block()
}

#[test]
fn cloned_foreign_callback() {
    assert_eq!(callbacks::invoke(), (8, 9));
    let offset = 7;
    assert_eq!(wrap(move |value| value + offset), (10, 11));
    assert_eq!(callbacks::invoke_mut(), (8, 12));
    assert_eq!(callbacks::invoke_once(), 5);
}
