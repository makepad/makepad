//! Search synonyms: what people call a widget when they do not know what
//! this library calls it.
//!
//! The catalogue's names are the library's names, and a person looking for
//! something arrives with their own vocabulary — "stepper", "autocomplete",
//! "switch", "date range". Matching only on the component name means the
//! search answers questions that already contain their own answer.
//!
//! The terms live in `synonyms.json` rather than in the story records for
//! two reasons. They are a vocabulary, and a vocabulary is easier to weigh
//! when it is all in one place than when it is spread across ninety files.
//! And the file is the thing to edit when a search misses: a term someone
//! typed and did not find belongs here, next to the terms that do work,
//! without touching any Rust.
//!
//! Two lists per component, because they answer different questions.
//! `aka` is what the widget is CALLED elsewhere; `does` is what it can DO.
//! Search treats them alike — the split is for whoever edits the file.
use crate::makepad_widgets::makepad_micro_serde::*;
use std::collections::HashMap;
use std::sync::OnceLock;

/// The vocabulary, compiled in. A catalogue that had to find a data file
/// beside itself would be one more thing to get wrong on someone else's
/// machine, and this one is small.
const SOURCE: &str = include_str!("synonyms.json");

#[derive(DeJson)]
pub struct Entry {
    pub component: String,
    /// Other names for the widget.
    pub aka: Vec<String>,
    /// What it can do.
    pub does: Vec<String>,
}

#[derive(DeJson)]
pub struct Book {
    pub components: Vec<Entry>,
}

/// Component name to every term written for it, lowercased once.
///
/// A parse failure is reported and then ignored: a malformed vocabulary
/// should cost the catalogue its synonyms, not its ability to start. The
/// test below is what actually keeps the file honest.
fn index() -> &'static HashMap<String, Vec<String>> {
    static INDEX: OnceLock<HashMap<String, Vec<String>>> = OnceLock::new();
    INDEX.get_or_init(|| {
        let mut map: HashMap<String, Vec<String>> = HashMap::new();
        match Book::deserialize_json(SOURCE) {
            Ok(book) => {
                for entry in book.components {
                    let terms: Vec<String> = entry
                        .aka
                        .iter()
                        .chain(entry.does.iter())
                        .map(|t| t.to_lowercase())
                        .collect();
                    map.entry(entry.component).or_default().extend(terms);
                }
            }
            Err(e) => {
                crate::makepad_widgets::log!("storybook: synonyms.json did not parse: {:?}", e);
            }
        }
        map
    })
}

/// Every term written for this component.
pub fn terms(component: &str) -> &'static [String] {
    index().get(component).map(|v| v.as_slice()).unwrap_or(&[])
}

/// Does any term for this component answer the query? The query is expected
/// already lowercased, as `registry::matches` has one to hand.
pub fn matches(component: &str, lowercase_query: &str) -> bool {
    terms(component).iter().any(|t| answers(t, lowercase_query))
}

/// A term answers a one-word query when one of its WORDS starts with it,
/// not merely when the term contains it somewhere.
///
/// Plain substring is what the rest of the search does, and on a vocabulary
/// this size it turns into noise: "pin" would answer with the spinner, "over"
/// with every popover and hover term, "art" with the charts. Word-prefix
/// keeps everything worth having — "step" still finds the stepper, "auto" the
/// autocomplete, "scrub" the scrubbable field — and drops the accidents.
///
/// A query with a space in it is somebody typing a phrase, and phrases are
/// matched whole.
fn answers(term: &str, query: &str) -> bool {
    if query.contains(' ') {
        return term.contains(query);
    }
    term.split(|c: char| !c.is_alphanumeric())
        .any(|word| word.starts_with(query))
}

/// How many components carry terms, for the log line at startup.
pub fn component_count() -> usize {
    index().len()
}

/// How many terms there are in total.
pub fn term_count() -> usize {
    index().values().map(|v| v.len()).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry;
    use std::collections::BTreeSet;

    fn book() -> Book {
        Book::deserialize_json(SOURCE).expect("synonyms.json parses")
    }

    #[test]
    fn the_file_parses() {
        assert!(!book().components.is_empty());
    }

    #[test]
    fn every_entry_names_a_component_that_exists() {
        let known: BTreeSet<&str> = registry::all()
            .map(|s| s.component)
            .chain(registry::all().flat_map(|s| s.also.iter().copied()))
            .collect();
        let unknown: Vec<String> = book()
            .components
            .iter()
            .map(|e| e.component.clone())
            .filter(|c| !known.contains(c.as_str()))
            .collect();
        assert!(unknown.is_empty(), "synonyms for components no story has: {unknown:?}");
    }

    #[test]
    fn every_component_has_terms() {
        // A component with no vocabulary is only findable by the name the
        // library chose for it, which is the problem this file exists to
        // fix. A new page must bring its words with it.
        let covered: BTreeSet<String> =
            book().components.iter().map(|e| e.component.clone()).collect();
        let missing: Vec<&str> = registry::all()
            .map(|s| s.component)
            .filter(|c| !covered.contains(*c))
            .collect();
        assert!(missing.is_empty(), "components with no synonyms: {missing:?}");
    }

    #[test]
    fn no_entry_is_listed_twice() {
        let mut seen = BTreeSet::new();
        for entry in book().components {
            assert!(seen.insert(entry.component.clone()), "{} listed twice", entry.component);
        }
    }

    #[test]
    fn no_term_is_blank_or_uppercase_or_padded() {
        for entry in book().components {
            for term in entry.aka.iter().chain(entry.does.iter()) {
                assert!(!term.trim().is_empty(), "{}: a blank term", entry.component);
                assert_eq!(term, &term.to_lowercase(), "{}: {term:?} is not lowercase", entry.component);
                assert_eq!(term, term.trim(), "{}: {term:?} has padding", entry.component);
            }
        }
    }

    #[test]
    fn a_term_is_not_repeated_within_its_own_entry() {
        for entry in book().components {
            let mut seen = BTreeSet::new();
            for term in entry.aka.iter().chain(entry.does.iter()) {
                assert!(seen.insert(term.clone()), "{}: {term:?} twice", entry.component);
            }
        }
    }

    #[test]
    fn a_term_is_not_a_substring_of_the_name_it_is_filed_under() {
        // The search already matches the component name, so such a term
        // buys nothing and reads as though it did.
        for entry in book().components {
            let name = entry.component.to_lowercase();
            for term in entry.aka.iter().chain(entry.does.iter()) {
                assert!(
                    !name.contains(term.as_str()),
                    "{}: {term:?} is already in the name",
                    entry.component
                );
            }
        }
    }

    #[test]
    fn no_term_is_so_broad_it_answers_everything() {
        // A term on a third of the catalogue is not a synonym, it is noise:
        // it turns a search into a list of everything and hides the page
        // that was actually wanted.
        let b = book();
        let total = b.components.len();
        let mut hits: HashMap<String, usize> = HashMap::new();
        for entry in &b.components {
            for term in entry.aka.iter().chain(entry.does.iter()) {
                *hits.entry(term.clone()).or_default() += 1;
            }
        }
        let broad: Vec<(String, usize)> = hits
            .into_iter()
            .filter(|(_, n)| *n * 3 > total)
            .collect();
        assert!(broad.is_empty(), "terms on more than a third of the catalogue: {broad:?}");
    }

    #[test]
    fn the_words_people_actually_typed_land_somewhere() {
        // Searched for during the catalogue's own review and found nothing.
        // Asserted through `registry::matches` rather than through this
        // module, because what matters is that the SEARCH answers — whether
        // it answers from a synonym, a tag or the name is its business.
        for (query, want) in [
            // ValueInput is a section of the NumberField page, and the combo
            // box a page of Select: the search lands on the page, filed under
            // its component.
            ("stepper", "NumberField"),
            ("spinbox", "NumberField"),
            ("autocomplete", "Select"),
            ("typeahead", "Select"),
            ("switch", "CheckBox"),
            ("segmented", "ButtonGroup"),
            // The pages added with the menus and the sliced picture.
            ("9 slice", "Image"),
            ("panel skin", "Image"),
            ("stretchable frame", "Image"),
            ("hollow frame", "Image"),
            ("corners keep their size", "Image"),
        ] {
            let found: Vec<&str> = registry::all()
                .filter(|s| registry::matches(s, query))
                .map(|s| s.component)
                .collect();
            assert!(
                found.contains(&want),
                "{query:?} reached {found:?}, not {want}"
            );
        }
    }

    #[test]
    fn a_word_answers_from_its_start_and_not_from_its_middle() {
        assert!(answers("stepper", "step"));
        assert!(answers("number stepper", "stepper"), "any word, not only the first");
        assert!(answers("spinner", "spin"));
        assert!(!answers("spinner", "pin"), "the middle of a word is not a match");
        assert!(!answers("chart", "art"));
        // A phrase is matched whole, since somebody who typed a space meant
        // the two words together.
        assert!(answers("date range", "date range"));
        assert!(!answers("date range", "range date"));
    }

    #[test]
    fn a_search_that_should_find_one_thing_does_not_find_the_catalogue() {
        // The other half of a useful search. A vocabulary that matches too
        // widely is the same failure as one that matches too narrowly.
        for query in ["stepper", "autocomplete", "typeahead", "segmented"] {
            let n = registry::all().filter(|s| registry::matches(s, query)).count();
            assert!(n > 0, "{query:?} finds nothing");
            assert!(n <= 6, "{query:?} finds {n} stories, which is not an answer");
        }
    }
}
