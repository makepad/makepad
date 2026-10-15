//! Word segmentation: the most probable split of a run of Chinese text over a
//! word-frequency dictionary (jieba's dictionary, MIT, packed by
//! `tools/build_zh_lexicon.py`). What jieba does without its HMM for unknown
//! words: a character no word covers stands alone.

use super::super::data;

const MAX_WORD_CHARS: usize = 12;

fn entry(word: &str) -> Option<(u64, &'static str)> {
    let value = data::words().get(word)?;
    let (freq, pos) = value.split_once(' ')?;
    Some((freq.parse().ok()?, pos))
}

/// The dictionary's part-of-speech tag for `word`, if it has the word.
pub fn tag(word: &str) -> Option<&'static str> {
    entry(word).map(|(_, pos)| pos)
}

pub fn contains(word: &str) -> bool {
    entry(word).is_some()
}

fn total_frequency() -> f64 {
    static TOTAL: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    *TOTAL.get_or_init(|| data::words().get("\0total").and_then(|v| v.parse::<f64>().ok()).unwrap_or(6.0e7))
}

fn is_han(c: char) -> bool {
    ('\u{4e00}'..='\u{9fff}').contains(&c)
}

/// Split `text` into words. Runs of Chinese characters are cut by the best
/// path through the dictionary; any other character is a word of its own.
pub fn cut(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if !is_han(chars[i]) {
            out.push(chars[i].to_string());
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && is_han(chars[i]) {
            i += 1;
        }
        cut_han(&chars[start..i], &mut out);
    }
    out
}

fn cut_han(run: &[char], out: &mut Vec<String>) {
    let n = run.len();
    let total = total_frequency().ln();
    // route[i] = (best log probability of run[i..], end of the first word).
    let mut route = vec![(0.0f64, 0usize); n + 1];
    route[n] = (0.0, n);
    for i in (0..n).rev() {
        let mut best = (f64::NEG_INFINITY, i + 1);
        let longest = (i + MAX_WORD_CHARS).min(n);
        for j in i + 1..=longest {
            let word: String = run[i..j].iter().collect();
            let freq = match entry(&word) {
                Some((freq, _)) => freq as f64,
                // A single unknown character counts once; a longer unknown
                // string is no edge.
                None if j == i + 1 => 1.0,
                None => continue,
            };
            let score = freq.ln() - total + route[j].0;
            if score > best.0 {
                best = (score, j);
            }
        }
        route[i] = best;
    }
    let mut at = 0;
    while at < n {
        let end = route[at].1;
        out.push(run[at..end].iter().collect());
        at = end;
    }
}

/// jieba's `cut_for_search` of one word: the dictionary words inside it
/// (2- and 3-character pieces), then the pieces `cut` makes, each followed by
/// its sub-pieces. Order matters to the caller only through length.
pub fn cut_for_search(word: &str) -> Vec<String> {
    let mut out = Vec::new();
    for piece in cut(word) {
        let chars: Vec<char> = piece.chars().collect();
        if chars.len() > 2 {
            for i in 0..chars.len() - 1 {
                let gram: String = chars[i..i + 2].iter().collect();
                if contains(&gram) {
                    out.push(gram);
                }
            }
        }
        if chars.len() > 3 {
            for i in 0..chars.len() - 2 {
                let gram: String = chars[i..i + 3].iter().collect();
                if contains(&gram) {
                    out.push(gram);
                }
            }
        }
        out.push(piece);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_sentences_cut_into_words() {
        assert_eq!(cut("我爱北京天安门"), vec!["我", "爱", "北京", "天安门"]);
        assert_eq!(cut("你好，世界"), vec!["你好", "，", "世界"]);
        assert_eq!(cut("今天天气很好"), vec!["今天", "天气", "很", "好"]);
    }

    #[test]
    fn the_dictionary_knows_tags() {
        assert_eq!(tag("北京"), Some("ns"));
        assert!(contains("天气"));
        assert!(!contains("天气很"));
    }
}
