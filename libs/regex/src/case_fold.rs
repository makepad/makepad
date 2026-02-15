use {
    super::{range::Range, unicode::CASE_FOLDS},
    std::cmp::Ordering,
};

pub fn case_fold_range<F>(range: Range<char>, mut f: F)
where
    F: FnMut(Range<char>),
{
    if !contains_case_fold(range) {
        // No entry in the unicode case fold table — add the original range
        // plus simple upper/lowercase mappings for each character
        f(range);
        let start = range.start as u32;
        let end = (range.end as u32) + 1;
        for c in (start..end).filter_map(char::from_u32) {
            for mapped in c.to_uppercase() {
                if mapped != c {
                    f(Range::new(mapped, mapped));
                }
            }
            for mapped in c.to_lowercase() {
                if mapped != c {
                    f(Range::new(mapped, mapped));
                }
            }
        }
        return;
    }
    let start = range.start as u32;
    let end = (range.end as u32) + 1;
    let mut next_c = None;
    for c in (start..end).filter_map(char::from_u32) {
        if next_c.map_or(false, |next_c| c < next_c) {
            f(Range::new(c, c));
            continue;
        }
        match find_case_fold(c) {
            Ok(folding) => {
                for &c in folding {
                    f(Range::new(c, c))
                }
            }
            Err(c) => {
                next_c = c;
                continue;
            }
        }
    }
}

fn contains_case_fold(range: Range<char>) -> bool {
    CASE_FOLDS
        .binary_search_by(|&(c, _)| {
            if c < range.start {
                return Ordering::Less;
            }
            if c > range.end {
                return Ordering::Greater;
            }
            return Ordering::Equal;
        })
        .is_ok()
}

fn find_case_fold(c: char) -> Result<&'static [char], Option<char>> {
    CASE_FOLDS
        .binary_search_by_key(&c, |&(c, _)| c)
        .map(|i| CASE_FOLDS[i].1)
        .map_err(|i| {
            if i < CASE_FOLDS.len() {
                Some(CASE_FOLDS[i].0)
            } else {
                None
            }
        })
}
