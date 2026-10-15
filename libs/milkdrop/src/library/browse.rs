//! Moving through a library: which presets a filter keeps, the category
//! tree, stepping to the next or previous one, and a shuffle that does not
//! repeat itself too soon. All of it is plain arithmetic over the entries,
//! cheap enough for the UI thread (a filter over 10,000 entries is well
//! under a millisecond).

use super::{MilkVersion, PresetLibrary, UserPresets};

/// Which presets to show. The default keeps every preset.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Filter {
    /// Only this category (the first folder), when set.
    pub category: Option<String>,
    /// Only this subcategory under the category, when set.
    pub subcategory: Option<String>,
    /// Every word must appear in the title, authors, editors or folders.
    pub query: String,
    pub favourites_only: bool,
    /// At least this many stars (0 keeps unrated presets too).
    pub min_rating: u8,
    /// Only presets that carry a warp or composite shader.
    pub shaders_only: bool,
}

/// A category and its subcategories, with preset counts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CategoryNode {
    pub name: String,
    pub count: usize,
    pub subcategories: Vec<(String, usize)>,
}

impl PresetLibrary {
    /// The entries `filter` keeps, as indices in library order.
    pub fn filter(&self, filter: &Filter, user: &UserPresets) -> Vec<usize> {
        let query = makepad_search::fold_words(&filter.query);
        let words: Vec<&str> = query.split_whitespace().collect();
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                filter.category.as_ref().is_none_or(|c| *c == e.category)
                    && filter.subcategory.as_ref().is_none_or(|s| *s == e.subcategory)
                    && (!filter.shaders_only || e.warp_shader || e.comp_shader)
                    && (!filter.favourites_only || user.is_favourite(&e.rel))
                    && (filter.min_rating == 0 || user.rating(&e.rel) >= filter.min_rating)
                    && words.iter().all(|w| e.search.contains(w))
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// Categories in name order, each with its subcategories in name order.
    pub fn tree(&self) -> Vec<CategoryNode> {
        let mut nodes: Vec<CategoryNode> = Vec::new();
        for e in &self.entries {
            let at = match nodes.iter().position(|n| n.name == e.category) {
                Some(at) => at,
                None => {
                    nodes.push(CategoryNode { name: e.category.clone(), count: 0, subcategories: Vec::new() });
                    nodes.len() - 1
                }
            };
            let node = &mut nodes[at];
            node.count += 1;
            if !e.subcategory.is_empty() {
                match node.subcategories.iter_mut().find(|(s, _)| *s == e.subcategory) {
                    Some((_, n)) => *n += 1,
                    None => node.subcategories.push((e.subcategory.clone(), 1)),
                }
            }
        }
        nodes.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        for node in &mut nodes {
            node.subcategories.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
        }
        nodes
    }

    /// How many presets of each generation: (MD1, MD2, with shaders).
    pub fn counts(&self) -> (usize, usize, usize) {
        let md1 = self.entries.iter().filter(|e| e.version == MilkVersion::Md1).count();
        let md2 = self.entries.iter().filter(|e| e.version == MilkVersion::Md2).count();
        let shaders = self.entries.iter().filter(|e| e.warp_shader || e.comp_shader).count();
        (md1, md2, shaders)
    }
}

/// The entry `delta` places from `current` in `list` (the filtered, shown
/// order), wrapping at the ends. Without a current entry (or one the list
/// no longer holds) it starts at the first for forward steps and the last
/// for backward ones.
pub fn step(list: &[usize], current: Option<usize>, delta: isize) -> Option<usize> {
    if list.is_empty() {
        return None;
    }
    let len = list.len() as isize;
    let at = match current.and_then(|c| list.iter().position(|&i| i == c)) {
        Some(at) => (at as isize + delta).rem_euclid(len),
        None if delta < 0 => len - 1,
        None => 0,
    };
    Some(list[at as usize])
}

/// A random walk through a list that avoids the current preset and, where
/// the list is big enough, anything played recently. Deterministic for a
/// seed, so a test (or a VJ set) can replay it.
#[derive(Clone, Debug)]
pub struct Shuffle {
    state: u64,
}

impl Shuffle {
    pub fn new(seed: u64) -> Self {
        Self { state: seed ^ 0x9e37_79b9_7f4a_7c15 }
    }

    fn next_u64(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.state.max(1);
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// A random entry of `list` that is not `current` and, when at least
    /// one such entry exists, not recently played (`recent` answers for an
    /// entry index).
    pub fn pick(&mut self, list: &[usize], current: Option<usize>, recent: impl Fn(usize) -> bool) -> Option<usize> {
        let fresh: Vec<usize> = list.iter().copied().filter(|&i| Some(i) != current && !recent(i)).collect();
        let pool: Vec<usize> = if fresh.is_empty() { list.iter().copied().filter(|&i| Some(i) != current).collect() } else { fresh };
        if pool.is_empty() {
            return list.first().copied();
        }
        Some(pool[(self.next_u64() % pool.len() as u64) as usize])
    }
}

#[cfg(test)]
mod tests {
    use super::super::{index::Sniff, PresetEntry};
    use super::*;
    use std::path::Path;

    fn library() -> PresetLibrary {
        let root = Path::new("/lib");
        let entry = |rel: &str, shaders: bool| {
            let sniff = Sniff { version: if shaders { MilkVersion::Md2 } else { MilkVersion::Md1 }, warp_shader: shaders, comp_shader: false };
            PresetEntry::new(root.join(rel), rel.to_string(), sniff)
        };
        PresetLibrary {
            root: root.into(),
            entries: vec![
                entry("Fractal/Sierpinski/Flexi - explore space.milk", true),
                entry("Fractal/Sierpinski/Geiss - dust.milk", false),
                entry("Fractal/Julia/Flexi + Martin - julia.milk", true),
                entry("Waveform/Lines/Geiss - Bass Spikes.milk", false),
            ],
        }
    }

    #[test]
    fn filters_combine() {
        let lib = library();
        let mut user = UserPresets::default();
        let all = Filter::default();
        assert_eq!(lib.filter(&all, &user), vec![0, 1, 2, 3]);
        let fractal = Filter { category: Some("Fractal".into()), ..Filter::default() };
        assert_eq!(lib.filter(&fractal, &user), vec![0, 1, 2]);
        let julia = Filter { subcategory: Some("Julia".into()), ..fractal.clone() };
        assert_eq!(lib.filter(&julia, &user), vec![2]);
        assert_eq!(lib.filter(&Filter { query: "MARTIN".into(), ..Filter::default() }, &user), vec![2]);
        assert_eq!(lib.filter(&Filter { query: "geiss waveform".into(), ..Filter::default() }, &user), vec![3]);
        assert_eq!(lib.filter(&Filter { shaders_only: true, ..Filter::default() }, &user), vec![0, 2]);
        user.toggle_favourite(&lib.entries[3].rel);
        user.set_rating(&lib.entries[1].rel, 4);
        assert_eq!(lib.filter(&Filter { favourites_only: true, ..Filter::default() }, &user), vec![3]);
        assert_eq!(lib.filter(&Filter { min_rating: 3, ..Filter::default() }, &user), vec![1]);
        assert_eq!(lib.counts(), (2, 2, 2));
    }

    #[test]
    fn tree_counts_categories_and_subcategories() {
        let tree = library().tree();
        assert_eq!(tree[0], CategoryNode { name: "Fractal".into(), count: 3, subcategories: vec![("Julia".into(), 1), ("Sierpinski".into(), 2)] });
        assert_eq!(tree[1].name, "Waveform");
    }

    #[test]
    fn stepping_wraps_and_starts_at_the_ends() {
        let list = [4, 7, 9];
        assert_eq!(step(&list, Some(7), 1), Some(9));
        assert_eq!(step(&list, Some(9), 1), Some(4));
        assert_eq!(step(&list, Some(4), -1), Some(9));
        assert_eq!(step(&list, None, 1), Some(4));
        assert_eq!(step(&list, Some(99), -1), Some(9));
        assert_eq!(step(&[], Some(1), 1), None);
    }

    #[test]
    fn shuffle_avoids_current_and_recent() {
        let list: Vec<usize> = (0..10).collect();
        let mut shuffle = Shuffle::new(7);
        for _ in 0..200 {
            let pick = shuffle.pick(&list, Some(3), |i| i < 8).unwrap();
            assert!(pick == 8 || pick == 9);
        }
        // Everything recent: still never the current one.
        for _ in 0..50 {
            assert_ne!(shuffle.pick(&list, Some(3), |_| true), Some(3));
        }
        assert_eq!(shuffle.pick(&[5], Some(5), |_| false), Some(5));
        assert_eq!(Shuffle::new(1).pick(&list, None, |_| false), Shuffle::new(1).pick(&list, None, |_| false));
    }

    /// The pack's "! Transition" presets fade between presets; a random pick
    /// never lands on one while anything else is left.
    #[test]
    fn transitions_are_not_landed_on() {
        let root = Path::new("/lib");
        let sniff = Sniff { version: MilkVersion::Md2, warp_shader: true, comp_shader: false };
        let fade = PresetEntry::new(root.join("! Transition/Fade to black.milk"), "! Transition/Fade to black.milk".into(), sniff);
        let show = PresetEntry::new(root.join("Fractal/Flexi - space.milk"), "Fractal/Flexi - space.milk".into(), sniff);
        assert!(fade.is_transition());
        assert!(!show.is_transition());
        let lib = PresetLibrary { root: root.into(), entries: vec![fade, show] };
        let mut shuffle = Shuffle::new(7);
        for _ in 0..20 {
            assert_eq!(shuffle.pick(&[0, 1], None, |i| lib.entries[i].is_transition()), Some(1));
        }
    }
}
