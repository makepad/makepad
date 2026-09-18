//! Folding accented Latin text down to the ASCII somebody will actually type.
//!
//! Two spellings of one word have to reach each other. A record filed as
//! "Café del Mar" is found by an operator typing "cafe", and a record filed
//! as "Cafe" is found by one typing "café" — otherwise the accent a name
//! happens to be written with decides whether search can see it, which is
//! not a decision anybody made on purpose.
//!
//! DELIBERATELY THIS TABLE AND NOT CANONICAL DECOMPOSITION. The rules here
//! answer questions decomposition does not: 'ß' is "ss" and 'æ' is "ae",
//! which no amount of stripping combining marks will tell you. The repo
//! already had exactly this table, tested, inside the importer's alias
//! slugger; it lives here now so the alias a record is filed under and the
//! terms it is indexed by fold by construction rather than by coincidence.
//!
//! Anything with no ASCII answer becomes `-`, which every consumer here
//! already treats as a separator.

/// Fold one character onto `out`.
///
/// Expects a character that has already been lowercased — the callers all
/// go through `char::to_lowercase` first, and an upper-case letter has no
/// arm here and would fold to `-`.
pub fn push_ascii_fold(out: &mut String, c: char) {
    let folded = match c {
        'a'..='z' | '0'..='9' => {
            out.push(c);
            return;
        }
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => "a",
        'æ' => "ae",
        'ç' => "c",
        'è' | 'é' | 'ê' | 'ë' => "e",
        'ì' | 'í' | 'î' | 'ï' => "i",
        'ð' => "d",
        'ñ' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' => "o",
        'ù' | 'ú' | 'û' | 'ü' => "u",
        'ý' | 'ÿ' => "y",
        'þ' => "th",
        'ß' => "ss",
        _ => "-",
    };
    out.push_str(folded);
}

/// Fold a whole string: lower-cased, then every character through
/// [`push_ascii_fold`].
///
/// Idempotent, which is the property the search index leans on: the terms a
/// folded query produces are the terms folding it again would produce, so
/// "café" and "cafe" cannot ask two different questions.
pub fn fold_to_ascii(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars().flat_map(char::to_lowercase) {
        push_ascii_fold(&mut out, c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_accented_word_folds_to_the_letters_somebody_would_type() {
        assert_eq!(fold_to_ascii("Café"), "cafe");
        assert_eq!(fold_to_ascii("Ünïcödé"), "unicode");
        assert_eq!(fold_to_ascii("Björk"), "bjork");
    }

    #[test]
    fn the_letters_no_decomposition_would_answer_are_answered_here() {
        assert_eq!(fold_to_ascii("Straße"), "strasse", "not 'strae', and not 'straße'");
        assert_eq!(fold_to_ascii("Æther"), "aether");
        assert_eq!(fold_to_ascii("Þing"), "thing");
        assert_eq!(fold_to_ascii("Øre"), "ore");
    }

    #[test]
    fn ascii_passes_through_untouched_but_for_its_case() {
        assert_eq!(fold_to_ascii("Deep House 2"), "deep-house-2");
        assert_eq!(fold_to_ascii("abc123"), "abc123");
    }

    #[test]
    fn folding_a_folded_string_changes_nothing_more() {
        // The index and the query both lean on this: a query folded once
        // and a query folded twice must ask the same question.
        for text in ["Café del Mar", "Straße", "plain ascii", "Ünïcödé", "æøå"] {
            let once = fold_to_ascii(text);
            assert_eq!(fold_to_ascii(&once), once, "{text:?} is not idempotent");
        }
    }

    #[test]
    fn anything_with_no_ascii_answer_becomes_a_separator() {
        assert_eq!(fold_to_ascii("日本"), "--");
        assert_eq!(fold_to_ascii("a日b"), "a-b");
    }
}
