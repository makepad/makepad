mod tables;

pub use {grapheme_cluster_break::GraphemeClusterBreak, word_break::WordBreak};

use tables::*;

/// Extends `char` with methods to access the properties in the Unicode Character Database (UCD).
pub trait Ucd {
    /// Returns the value of the `Extended_Pictographic` property for this `char`.
    fn extended_pictographic(self) -> bool;

    /// Returns the value of the `Grapheme_Cluster_Break` property for this `char`.
    fn grapheme_cluster_break(self) -> GraphemeClusterBreak;

    /// Returns the value of the `Word_Break` property for this `char`.
    fn word_break(self) -> WordBreak;
}

impl Ucd for char {
    fn extended_pictographic(self) -> bool {
        extended_pictographic::EXTENDED_PICTOGRAPHIC
            .search(self)
            .is_some()
    }

    fn grapheme_cluster_break(self) -> GraphemeClusterBreak {
        grapheme_cluster_break::GRAPHEME_CLUSTER_BREAK
            .search(self)
            .unwrap_or_default()
    }

    fn word_break(self) -> WordBreak {
        word_break::WORD_BREAK.search(self).unwrap_or_default()
    }
}

trait Search {
    type Output;

    fn search(&self, ch: char) -> Option<Self::Output>;
}

impl Search for [([u8; 3], [u8; 3])] {
    type Output = ();

    fn search(&self, ch: char) -> Option<Self::Output> {
        let code_point = ch as u32;
        self.binary_search_by(|(first_bytes, last_bytes)| {
            use std::cmp::Ordering::*;

            let first = u32::from_be_bytes([0, first_bytes[0], first_bytes[1], first_bytes[2]]);
            let last = u32::from_be_bytes([0, last_bytes[0], last_bytes[1], last_bytes[2]]);
            if last < code_point {
                Less
            } else if first > code_point {
                Greater
            } else {
                Equal
            }
        })
        .map(|_| ())
        .ok()
    }
}

impl<T: Copy> Search for [([u8; 3], [u8; 3], T)] {
    type Output = T;

    fn search(&self, ch: char) -> Option<Self::Output> {
        let code_point = ch as u32;
        self.binary_search_by(|(first_bytes, last_bytes, _)| {
            use std::cmp::Ordering::*;

            let first = u32::from_be_bytes([0, first_bytes[0], first_bytes[1], first_bytes[2]]);
            let last = u32::from_be_bytes([0, last_bytes[0], last_bytes[1], last_bytes[2]]);
            if last < code_point {
                Less
            } else if first > code_point {
                Greater
            } else {
                Equal
            }
        })
        .map(|index| {
            let (_, _, value) = self[index];
            value
        })
        .ok()
    }
}
