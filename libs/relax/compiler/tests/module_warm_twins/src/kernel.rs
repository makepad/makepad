use std::collections::{hash_map::DefaultHasher, HashMap};
use std::hash::BuildHasherDefault;

pub fn work(n: u32) -> u64 {
    let words = ["amber", "birch", "cedar", "dune", "elm", "fern", "glen"];
    let mut text = String::new();
    for i in 0..n {
        text.push_str(words[i as usize % words.len()]);
        text.push(' ');
    }
    let mut counts: HashMap<&str, u32, BuildHasherDefault<DefaultHasher>> = HashMap::with_hasher(BuildHasherDefault::default());
    for word in text.split_whitespace() {
        *counts.entry(word).or_insert(0) += 1;
    }
    let mut state = 0x1234_5678_9abc_def0u64;
    let mut values: Vec<u64> = (0..n).map(|_| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    }).collect();
    values.sort_unstable();
    let max_count = counts.values().copied().max().unwrap_or(0);
    let mut result = ((counts.len() as u64) << 32) | max_count as u64;
    for i in [0, values.len() / 2, values.len().saturating_sub(1)] {
        if let Some(&value) = values.get(i) {
            result = result.rotate_left(11) ^ value;
        }
    }
    result
}
