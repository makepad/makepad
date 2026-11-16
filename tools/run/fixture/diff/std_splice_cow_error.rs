// diff_fast corpus program: three std items a crate graph needed: `std::vec::Splice` named as a type (and a
// splice dropped unconsumed, which still replaces the range), `array::IntoIter::as_mut_slice` after the
// iterator has advanced from both ends, and `Box<dyn Error (+ Send + Sync)>: From<Cow<str>>` reached through a
// blanket `From` bounded on it, for borrowed and owned strings.
// rustc --test -O and `makepad test` (all tiers) must print identical D lines.
use std::borrow::Cow;
use std::error::Error;
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
fn rounds(f: fn()) {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..4 { f(); if r < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() { rounds(work); }

type Spliced<'a> = std::vec::Splice<'a, std::vec::IntoIter<String>>;

#[inline(never)]
fn take_splice(v: &mut Vec<String>) -> Vec<String> {
    let s: Spliced<'_> = v.splice(1..3, vec!["x".to_string(), "y".into(), "z".into()]);
    s.collect()
}

#[derive(Debug)]
struct Wrapped(Box<dyn Error + Send + Sync + 'static>);
impl<E> From<E> for Wrapped where Box<dyn Error + Send + Sync + 'static>: From<E> {
    fn from(e: E) -> Self { Wrapped(Box::from(e)) }
}
#[inline(never)]
fn fail(msg: impl Into<Cow<'static, str>>) -> Result<u8, Wrapped> {
    let r: Result<u8, Cow<'static, str>> = Err(msg.into());
    Ok(r?)
}

fn work() {
    let mut v: Vec<String> = ["a", "b", "c", "d"].iter().map(|s| s.to_string()).collect();
    let removed = take_splice(&mut v);
    d!("splice removed {:?} now {:?}", removed, v);
    drop(v.splice(..1, ["p".to_string(), "q".into()]));
    d!("splice dropped unconsumed: {:?}", v);

    let mut it = [String::from("1"), "2".into(), "3".into(), "4".into(), "5".into()].into_iter();
    let first = it.next();
    let last = it.next_back();
    it.as_mut_slice()[0].push('!');
    it.as_mut_slice().reverse();
    d!("as_mut_slice: first {:?} last {:?} rest {:?} len {}", first, last, it.as_slice(), it.len());
    d!("as_mut_slice then iterate: {:?}", it.collect::<Vec<_>>());

    let borrowed = fail("borrowed message").unwrap_err();
    let owned = fail(format!("owned {}", 7)).unwrap_err();
    d!("cow error: {} / {:?} / {} / {:?}", borrowed.0, borrowed.0, owned.0, owned.0);
    let plain: Box<dyn Error> = Box::from(Cow::Borrowed("plain"));
    let plain_owned: Box<dyn Error> = Cow::<str>::Owned("plain owned".into()).into();
    d!("dyn Error from cow: {} {:?} {}", plain, plain_owned, plain_owned.source().is_none());
}
