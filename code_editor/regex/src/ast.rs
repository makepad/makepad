use crate::CharClass;

#[derive(Clone, Debug)]
pub(crate) enum Ast {
    Char(char),
    CharClass(CharClass),
    Cap(Box<Ast>, usize),
    Assert(Pred),
    Rep(Box<Ast>, Quant),
    Cat(Vec<Ast>),
    Alt(Vec<Ast>),
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Pred {
    IsAtStartOfText,
    IsAtEndOfText,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Quant {
    Quest(bool),
    Star(bool),
    Plus(bool),
}
