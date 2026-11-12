//! A generic parameter named like a trait in scope hides the trait in its item: `Piece::WIDTH` and
//! `Piece::build()` are the parameter's, and the trait is still `self::Piece` in the bound (relax
//! issue 32; rustc's generics come before the module's items).
pub trait Piece: Sized {
    const WIDTH: usize;
    fn build() -> Self;
}

impl Piece for u8 {
    const WIDTH: usize = 1;
    fn build() -> Self {
        7
    }
}

impl Piece for [u8; 4] {
    const WIDTH: usize = 4;
    fn build() -> Self {
        [1; 4]
    }
}

fn take<Piece>(room: usize) -> Option<Piece>
where
    Piece: self::Piece,
{
    if Piece::WIDTH <= room { Some(Piece::build()) } else { None }
}

// with a lifetime on the trait, as a lexer's source reads its pieces
pub trait Window<'a>: Sized {
    const SPAN: usize;
    fn at(bytes: &'a [u8]) -> Self;
}

impl<'a> Window<'a> for &'a [u8] {
    const SPAN: usize = 2;
    fn at(bytes: &'a [u8]) -> Self {
        &bytes[..2]
    }
}

fn first<'a, Window>(bytes: &'a [u8]) -> Option<Window>
where
    Window: self::Window<'a>,
{
    if Window::SPAN <= bytes.len() { Some(Window::at(bytes)) } else { None }
}

#[test]
fn a_generic_parameter_hides_a_trait_of_its_name() {
    assert_eq!((take::<u8>(2), take::<[u8; 4]>(2)), (Some(7), None), "the parameter's WIDTH and build");
    assert_eq!(first::<&[u8]>(b"abc"), Some(&b"ab"[..]), "a lifetime-generic trait hidden by the parameter");
    assert_eq!(first::<&[u8]>(b"a"), None, "the parameter's SPAN");
}
