//! What a person keeps about presets: favourites, a 1–5 rating, and what
//! they played recently. Keyed by the preset's path relative to its library
//! root, so it survives the pack being downloaded again. The host decides
//! where the file lives and does the reading and writing off the UI thread
//! ([`UserPresets::to_text`] / [`UserPresets::from_text`]).

use std::collections::{HashMap, HashSet, VecDeque};

/// Recently played presets kept, newest first.
pub const RECENT_KEPT: usize = 200;

const HEADER: &str = "milkdrop-user 1";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct UserPresets {
    favourites: HashSet<String>,
    ratings: HashMap<String, u8>,
    recent: VecDeque<String>,
}

impl UserPresets {
    pub fn is_favourite(&self, rel: &str) -> bool {
        self.favourites.contains(rel)
    }

    /// Turn a favourite on or off; returns the new state.
    pub fn toggle_favourite(&mut self, rel: &str) -> bool {
        if self.favourites.remove(rel) {
            false
        } else {
            self.favourites.insert(rel.to_string());
            true
        }
    }

    /// 1–5, or 0 for unrated.
    pub fn rating(&self, rel: &str) -> u8 {
        self.ratings.get(rel).copied().unwrap_or(0)
    }

    /// Set a 1–5 rating; 0 (or anything above 5 clamps to 5) clears it.
    pub fn set_rating(&mut self, rel: &str, stars: u8) {
        if stars == 0 {
            self.ratings.remove(rel);
        } else {
            self.ratings.insert(rel.to_string(), stars.min(5));
        }
    }

    /// Note a preset as played now: it moves to the front of the recent
    /// list, which keeps the newest [`RECENT_KEPT`].
    pub fn played(&mut self, rel: &str) {
        if self.recent.front().is_some_and(|r| r == rel) {
            return;
        }
        self.recent.retain(|r| r != rel);
        self.recent.push_front(rel.to_string());
        self.recent.truncate(RECENT_KEPT);
    }

    /// Recently played, newest first.
    pub fn recent(&self) -> impl Iterator<Item = &str> {
        self.recent.iter().map(String::as_str)
    }

    /// How recently `rel` played: 0 is the newest, `None` if not recently.
    pub fn recency(&self, rel: &str) -> Option<usize> {
        self.recent.iter().position(|r| r == rel)
    }

    pub fn to_text(&self) -> String {
        let mut text = format!("{HEADER}\n");
        let mut favourites: Vec<&String> = self.favourites.iter().collect();
        favourites.sort();
        for rel in favourites.into_iter().filter(|r| writable(r)) {
            text.push_str(&format!("fav\t{rel}\n"));
        }
        let mut ratings: Vec<(&String, &u8)> = self.ratings.iter().collect();
        ratings.sort();
        for (rel, stars) in ratings.into_iter().filter(|(r, _)| writable(r)) {
            text.push_str(&format!("rate\t{rel}\t{stars}\n"));
        }
        for rel in self.recent.iter().filter(|r| writable(r)) {
            text.push_str(&format!("recent\t{rel}\n"));
        }
        text
    }

    /// Read what [`Self::to_text`] wrote; unknown or broken lines are
    /// skipped, and a missing header reads as empty.
    pub fn from_text(text: &str) -> Self {
        let mut out = Self::default();
        let mut lines = text.lines();
        if lines.next() != Some(HEADER) {
            return out;
        }
        for line in lines {
            let mut fields = line.split('\t');
            match (fields.next(), fields.next(), fields.next()) {
                (Some("fav"), Some(rel), None) if !rel.is_empty() => {
                    out.favourites.insert(rel.to_string());
                }
                (Some("rate"), Some(rel), Some(stars)) if !rel.is_empty() => {
                    if let Ok(stars) = stars.parse::<u8>() {
                        out.set_rating(rel, stars);
                    }
                }
                (Some("recent"), Some(rel), None) if !rel.is_empty() && out.recent.len() < RECENT_KEPT => {
                    if !out.recent.iter().any(|r| r == rel) {
                        out.recent.push_back(rel.to_string());
                    }
                }
                _ => {}
            }
        }
        out
    }
}

/// A name with a tab or line break cannot be written as one field.
fn writable(rel: &str) -> bool {
    !rel.contains(['\t', '\n', '\r'])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_favourites_ratings_and_recent_across_a_save() {
        let mut user = UserPresets::default();
        assert!(user.toggle_favourite("Fractal/a.milk"));
        user.set_rating("Fractal/a.milk", 4);
        user.set_rating("Waveform/b.milk", 9);
        user.played("Waveform/b.milk");
        user.played("Fractal/a.milk");
        user.played("Waveform/b.milk");
        assert_eq!(user.recent().collect::<Vec<_>>(), vec!["Waveform/b.milk", "Fractal/a.milk"]);
        assert_eq!(user.rating("Waveform/b.milk"), 5);
        let back = UserPresets::from_text(&(user.to_text() + "junk line\nrate\tx\tnope\n"));
        assert_eq!(back, user);
        assert!(!user.toggle_favourite("Fractal/a.milk"));
        user.set_rating("Fractal/a.milk", 0);
        assert_eq!(user.rating("Fractal/a.milk"), 0);
        assert_eq!(UserPresets::from_text("something else"), UserPresets::default());
    }

    #[test]
    fn recent_is_bounded() {
        let mut user = UserPresets::default();
        for i in 0..RECENT_KEPT + 10 {
            user.played(&format!("p{i}.milk"));
        }
        assert_eq!(user.recent().count(), RECENT_KEPT);
        assert_eq!(user.recency(&format!("p{}.milk", RECENT_KEPT + 9)), Some(0));
        assert_eq!(user.recency("p0.milk"), None);
    }
}
