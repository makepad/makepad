// `-> impl Iterator<..> + '_` (edition 2021): the bound names the elided input lifetime, which the
// hidden type captures (a `move` closure holding the argument). Without the bound rustc rejects it
// (E0700), and so does the check; with it both accept. An app's settings trees (M64) had this shape.
const TREES: &[(&str, &[&[(&str, &str)]])] = &[("a", &[&[("k", "v")], &[("k2", "v2")]]), ("b", &[&[("x", "y")]])];

fn defaults(folder: &str) -> impl Iterator<Item = (&'static str, &'static str)> + '_ {
    TREES.iter().filter(move |(f, _)| *f == folder).flat_map(|(_, tables)| tables.iter().flat_map(|t| t.iter().copied()))
}

fn bytes(x: &str) -> impl Iterator<Item = u8> + '_ {
    x.bytes()
}

struct Keys {
    k: String,
}

impl Keys {
    fn matching(&self) -> impl Iterator<Item = &'static str> + '_ {
        ["a", "b", "a"].into_iter().filter(move |s| *s == self.k)
    }
}

#[test]
fn anonymous_lifetime_bounds_capture_the_argument() {
    let folder = String::from("a");
    let got: Vec<_> = defaults(&folder).collect();
    assert_eq!(got, [("k", "v"), ("k2", "v2")]);
    assert_eq!(defaults("b").count(), 1);
    assert_eq!(bytes("hi").sum::<u8>(), b'h' + b'i');
    assert_eq!(Keys { k: "a".into() }.matching().count(), 2);
}
