/// Extensions to `char`.
pub trait CharExt {
    fn is_ascii_word(self) -> bool;
    
    fn is_word(self) -> bool;
}

impl CharExt for char {
    /// Returns `true` if the `char` is an ASCII word character:
    ///
    /// * U+0030 '0' ..= U+0039 '9', or
    /// * U+0041 'A' ..= U+005A 'Z', or
    /// * U+005F '_', or
    /// * U+0061 'a' ..= U+007A 'z', or
    fn is_ascii_word(self) -> bool {
        match self {
            '0'..='9' | 'A'..='Z' | '_' | 'a'..='z' => true,
            _ => false,
        }
    }

    /// Returns `true` if the `char` has the `Word` compatibility property.
    ///
    /// `Word` is described in Annex C (Compatibility Properties) of the
    /// [Unicode Technical Standard #18].
    ///
    /// [Unicode Technical Standard #18]: https://unicode.org/reports/tr18/
    fn is_word(self) -> bool {
        use super::unicode;

        if self.is_ascii() && self.is_ascii_word() {
            return true;
        }
        unicode::WORD
            .binary_search_by(|range| {
                use std::cmp::Ordering;

                if range.end < self {
                    return Ordering::Less;
                }
                if range.start > self {
                    return Ordering::Greater;
                }
                Ordering::Equal
            })
            .is_ok()
    }
}
