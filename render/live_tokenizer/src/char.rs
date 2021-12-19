pub trait CharExt {
    fn is_identifier_start(self) -> bool;
    fn is_identifier_continue(self) -> bool;
    fn is_hex(self) -> bool;
}

impl CharExt for char {

    fn is_hex(self) -> bool {
        match self {
            'A'..='F' | 'a'..='f' | '0'..='9' => true,
            _ => false,
        }
    }

    fn is_identifier_start(self) -> bool {
        match self {
            'A'..='Z' | '_' | 'a'..='z' => true,
            _ => false,
        }
    }

    fn is_identifier_continue(self) -> bool {
        match self {
            '0'..='9' | 'A'..='Z' | '_' | 'a'..='z' => true,
            _ => false,
        }
    }
}
