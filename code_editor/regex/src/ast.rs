pub enum Ast {
    Alt(Vec<Ast>),
    Cat(Vec<Ast>),
    Quest(Box<Ast>),
    Star(Box<Ast>),
    Plus(Box<Ast>),
    Char(char),
}
