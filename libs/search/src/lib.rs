//! Transport- and UI-independent search primitives extracted from Scope.
//! Callers own worker scheduling, document storage and immutable result pages.
use std::collections::{BTreeMap, BTreeSet, HashMap};

pub fn fold(text: &str) -> String {
    text.chars().flat_map(char::to_lowercase).collect()
}
pub fn trigrams(text: &str) -> Vec<[char; 3]> {
    let chars: Vec<char> = text.chars().collect();
    let mut out: Vec<_> = chars.windows(3).map(|w| [w[0], w[1], w[2]]).collect();
    out.sort_unstable();
    out.dedup();
    out
}
/// Scope's verified term-level trigram matcher. `ordered` sorts folded terms.
/// Returns None on cancellation; scores are exact=1, prefix=2, substring=3.
/// Trigrams only select candidates: every candidate is verified against text.
pub fn matching_terms<'a>(
    atom: &str,
    case_sensitive: bool,
    substring: bool,
    ordered: &[u32],
    len: usize,
    term: impl Fn(u32) -> (&'a str, &'a str),
    posting: impl Fn(&[char; 3]) -> Option<&'a [u32]>,
    cancel: impl Fn() -> bool,
) -> Option<Vec<(u32, u8)>> {
    let needle = fold(atom);
    let grams = trigrams(&needle);
    let start = ordered.partition_point(|id| term(*id).1 < needle.as_str());
    let mut candidates = Vec::new();
    for &id in &ordered[start..] {
        if !term(id).1.starts_with(&needle) {
            break;
        }
        candidates.push(id);
    }
    if substring && grams.is_empty() {
        candidates.extend(0..len as u32);
    }
    if substring && !grams.is_empty() {
        let mut lists: Vec<_> = grams.iter().filter_map(posting).collect();
        if lists.len() == grams.len() {
            lists.sort_unstable_by_key(|p| p.len());
            for (i, &id) in lists[0].iter().enumerate() {
                if i & 255 == 0 && cancel() {
                    return None;
                }
                if lists[1..]
                    .iter()
                    .all(|list| list.binary_search(&id).is_ok())
                {
                    candidates.push(id);
                }
            }
        }
    }
    candidates.sort_unstable();
    candidates.dedup();
    let mut out = Vec::new();
    for (i, id) in candidates.into_iter().enumerate() {
        if i & 255 == 0 && cancel() {
            return None;
        }
        let (spelling, folded) = term(id);
        let (word, pattern) = if case_sensitive {
            (spelling, atom)
        } else {
            (folded, needle.as_str())
        };
        if word.starts_with(pattern) || substring && word.contains(pattern) {
            out.push((
                id,
                if word == pattern {
                    1
                } else if word.starts_with(pattern) {
                    2
                } else {
                    3
                },
            ));
        }
    }
    Some(out)
}
/// Prepared independently on any worker; sorted terms serialize as UTF-8 lines.
pub fn document_terms(fields: &[&str]) -> Vec<String> {
    let mut words = BTreeSet::new();
    for field in fields {
        for word in field.split(|c: char| !c.is_alphanumeric() && c != '_') {
            if !word.is_empty() {
                words.insert(fold(word));
            }
        }
    }
    words.into_iter().collect()
}
#[derive(Default)]
pub struct TextIndex {
    ids: BTreeMap<String, u32>,
    words: Vec<String>,
    grams: HashMap<[char; 3], Vec<u32>>,
    ordered: Vec<u32>,
    postings: Vec<Vec<u64>>,
    documents: HashMap<u64, Vec<u32>>,
}
impl TextIndex {
    pub fn len(&self) -> usize {
        self.documents.len()
    }
    pub fn is_empty(&self) -> bool {
        self.documents.is_empty()
    }
    pub fn insert(&mut self, document: u64, words: &[String]) {
        self.insert_terms(document, words.iter().map(String::as_str));
    }
    /// Insert borrowed terms, for example lines from a serialized cache. Only
    /// previously unseen vocabulary needs a string allocation.
    pub fn insert_terms<'a>(&mut self, document: u64, words: impl IntoIterator<Item = &'a str>) {
        self.remove(document);
        let words = words.into_iter();
        let mut terms = Vec::with_capacity(words.size_hint().0);
        for word in words {
            let id = if let Some(id) = self.ids.get(word) {
                *id
            } else {
                let id = self.words.len() as u32;
                self.ids.insert(word.to_owned(), id);
                self.words.push(word.to_owned());
                self.postings.push(Vec::new());
                for gram in trigrams(word) {
                    self.grams.entry(gram).or_default().push(id);
                }
                id
            };
            let list = &mut self.postings[id as usize];
            if let Err(at) = list.binary_search(&document) {
                list.insert(at, document);
            }
            terms.push(id);
        }
        terms.sort_unstable();
        terms.dedup();
        self.documents.insert(document, terms);
    }
    pub fn remove(&mut self, document: u64) {
        if let Some(terms) = self.documents.remove(&document) {
            for id in terms {
                let list = &mut self.postings[id as usize];
                if let Ok(at) = list.binary_search(&document) {
                    list.remove(at);
                }
            }
        }
    }
    /// Unicode substring matching within terms, including partial typing.
    /// Intersect returned sets for AND; callers verify phrases/field predicates.
    pub fn candidates(&mut self, atom: &str, cancel: impl Fn() -> bool) -> Option<BTreeSet<u64>> {
        if self.ordered.len() != self.words.len() {
            self.ordered = self.ids.values().copied().collect();
        }
        let terms = matching_terms(
            atom,
            false,
            true,
            &self.ordered,
            self.words.len(),
            |id| {
                let word = self.words[id as usize].as_str();
                (word, word)
            },
            |g| self.grams.get(g).map(Vec::as_slice),
            &cancel,
        )?;
        let mut documents = BTreeSet::new();
        for (id, _) in terms {
            if cancel() {
                return None;
            }
            documents.extend(self.postings[id as usize].iter().copied());
        }
        Some(documents)
    }
}
