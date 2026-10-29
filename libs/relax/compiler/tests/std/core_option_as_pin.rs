// futures-util's ForEach::poll: `self.as_mut().future().as_pin_mut()` on a pinned Option field
use std::pin::Pin;

#[test]
fn option_as_pin_mut_and_ref() {
    let mut o = Some(5u32);
    let mut p = Pin::new(&mut o);
    if let Some(mut v) = p.as_mut().as_pin_mut() { *v = 6; }
    assert_eq!(p.as_ref().as_pin_ref().map(|v| *v), Some(6));
    let mut n: Option<String> = None;
    assert!(Pin::new(&mut n).as_pin_mut().is_none());
}
