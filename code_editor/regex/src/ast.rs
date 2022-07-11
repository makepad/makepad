pub enum Regex {
    Alt(Vec<Regex>),
    Cat(Vec<Regex>),
    Quest(Box<Regex>),
    Star(Box<Regex>),
    Plus(Box<Regex>),
    Char(char),
}
