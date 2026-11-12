// `impl Trait` in a function's generic parameter bounds is an anonymous parameter, as in its argument
// types (relax issue 74): `<I: Iterator<Item = impl AsRef<[u8]>>>` was E0562 ("not allowed in paths")
// wherever a caller's check lowered the bound.
pub struct Responses {
    bytes: usize,
}

impl Responses {
    pub fn add<I: Iterator<Item = impl AsRef<[u8]>>>(&mut self, it: I) -> usize {
        for r in it {
            self.bytes += r.as_ref().len();
        }
        self.bytes
    }
}

pub trait Sum64 {
    fn sum64<I: IntoIterator<Item = impl Into<u64>>>(&self, it: I) -> u64 {
        it.into_iter().map(Into::into).sum()
    }
}

impl Sum64 for Responses {}

fn longest<T: AsRef<str>, I: Iterator<Item = impl IntoIterator<Item = T>>>(rows: I, extra: impl Into<usize>) -> usize {
    rows.map(|r| r.into_iter().map(|s| s.as_ref().len()).sum::<usize>()).max().unwrap_or(0) + extra.into()
}

#[test]
fn impl_trait_in_a_parameter_bound_is_an_anonymous_parameter() {
    let mut r = Responses { bytes: 0 };
    assert_eq!(r.add(std::iter::once(vec![1u8, 2, 3])), 3);
    assert_eq!(r.add(["ab", "c"].into_iter()), 6);
    assert_eq!(r.sum64([1u8, 2]), 3);
    assert_eq!(r.sum64(vec![7u32]), 7);
    assert_eq!(longest(vec![vec!["ab", "cde"], vec!["x"]].into_iter(), 1u8), 6);
}
