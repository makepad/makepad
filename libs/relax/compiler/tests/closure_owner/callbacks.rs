pub fn foreign_file() -> impl Fn(u32) -> u32 + Clone {
    let offset = 5;
    move |value| value + offset
}

pub fn invoke() -> (u32, u32) {
    let offset = 5;
    super::wrap(move |value| value + offset)
}

pub fn mutable() -> impl FnMut(u32) -> u32 + Clone {
    let mut total = 5;
    move |value| { total += value; total }
}

pub fn consuming() -> impl FnOnce() -> u32 + Clone {
    let text = String::from("hello");
    move || { let len = text.len(); drop(text); len as u32 }
}

pub fn invoke_mut() -> (u32, u32) {
    let mut total = 5;
    super::wrap_mut(move |value| { total += value; total })
}

pub fn invoke_once() -> u32 {
    let text = String::from("hello");
    super::wrap_once(move || { let len = text.len(); drop(text); len as u32 })
}
