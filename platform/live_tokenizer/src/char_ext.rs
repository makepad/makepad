/// Extension methods for `char`.
/// 
/// These methods assume that all identifiers are ASCII. This is not actually the case for Rust,
/// which identifiers follow the specification in Unicode Standard Annex #31. We intend to
/// implement this properly in the future, but doing so requires generating several large Unicode
/// character tables, which why we've held off from this for now. 
pub trait CharExt {
    /// Checks if `char` is the start of an identifier.
    fn is_identifier_start(self) -> bool;

    /// Checks if `char` is the continuation of an identifier.
    /// 
    /// Note that this method assumes all identifiers are ASCII.
    fn is_identifier_continue(self) -> bool;
}

impl CharExt for char {
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
