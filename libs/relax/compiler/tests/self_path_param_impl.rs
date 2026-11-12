// relax issue 49: in a blanket impl `impl<P, T: Default + Build<P>> Build<Wrapped<P>> for T`,
// `Self::from` names the one trait method three impls provide (the bound's, an identity blanket
// impl's and the impl's own), and the prelude's `From::from` also applies to `Self`. rustc takes
// the param's where clause `T: Build<P>` first and checks the call; relax reported E0034
// (multiple applicable items).
pub trait Build<Part>: Sized {
    fn from(part: Part) -> Result<Self, String>;
    fn from_option(part: Option<Part>, missing: &str) -> Result<Self, String> {
        part.map(Self::from).transpose()?.ok_or_else(|| missing.to_string())
    }
}

impl<X> Build<X> for X {
    fn from(part: X) -> Result<Self, String> {
        Ok(part)
    }
}

pub struct Wrapped<P>(pub P);

impl<P, T: Default + Build<P>> Build<Wrapped<P>> for T {
    fn from(part: Wrapped<P>) -> Result<Self, String> {
        Self::from(part.0)
    }
    fn from_option(part: Option<Wrapped<P>>, _missing: &str) -> Result<Self, String> {
        part.map(|w| Self::from(w.0)).transpose().map(Option::unwrap_or_default)
    }
}

#[derive(Default, Debug, PartialEq)]
pub struct Width(u32);

impl Build<u16> for Width {
    fn from(n: u16) -> Result<Self, String> {
        Ok(Width(n as u32 + 1))
    }
}

#[test]
fn one_method_behind_three_impls() {
    assert_eq!(Width::from_option(Some(Wrapped(4u16)), "none"), Ok(Width(5)), "through the bound's impl");
    assert_eq!(<Width as Build<Wrapped<u16>>>::from_option(None, "none"), Ok(Width(0)), "the default when absent");
    assert_eq!(<Width as Build<Width>>::from_option(None, "none"), Err("none".to_string()), "the identity impl's default method");
    assert_eq!(<Width as Build<Wrapped<u16>>>::from(Wrapped(9)), Ok(Width(10)), "the impl's own method");
}
