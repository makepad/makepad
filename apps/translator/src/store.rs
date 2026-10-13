//! What the app remembers: the settings and the history of translations.

use makepad_widgets::makepad_micro_serde::*;

pub const HISTORY_LIMIT: usize = 200;

/// One finished translation.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct Entry {
    pub id: u64,
    /// `en` or `zh`.
    pub from: String,
    pub to: String,
    pub source: String,
    pub target: String,
    pub pinyin: String,
    pub favourite: bool,
}

#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct Saved {
    pub history: Vec<Entry>,
    pub next_id: u64,
    pub hub: String,
    pub model: String,
    pub traditional: bool,
    pub pinyin: bool,
    pub glossary: String,
    /// The credential a pairing invitation handed this device.
    pub credential: String,
    /// Kokoro voice ids per language (empty: the default).
    /// Optional so a file saved before voices existed still loads.
    pub voice_en: Option<String>,
    pub voice_zh: Option<String>,
}

impl Default for Saved {
    fn default() -> Self {
        Saved {
            history: Vec::new(),
            next_id: 1,
            hub: String::new(),
            model: crate::engine::HubSettings::default().model,
            traditional: false,
            pinyin: true,
            glossary: String::new(),
            credential: String::new(),
            voice_en: None,
            voice_zh: None,
        }
    }
}

impl Saved {
    pub fn decode(bytes: &[u8]) -> Option<Saved> {
        let text = std::str::from_utf8(bytes).ok()?;
        Saved::deserialize_json(text).ok()
    }

    pub fn encode(&self) -> Vec<u8> {
        self.serialize_json().into_bytes()
    }

    /// Remember a translation at the top. The same source/direction moves up
    /// instead of repeating; the oldest non-favourites fall off past the limit.
    pub fn remember(&mut self, from: &str, to: &str, source: &str, target: &str, pinyin: &str) -> u64 {
        let mut favourite = false;
        if let Some(i) = self.history.iter().position(|e| e.source == source && e.from == from && e.to == to) {
            favourite = self.history.remove(i).favourite;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.history.insert(
            0,
            Entry {
                id,
                from: from.into(),
                to: to.into(),
                source: source.into(),
                target: target.into(),
                pinyin: pinyin.into(),
                favourite,
            },
        );
        while self.history.len() > HISTORY_LIMIT {
            match self.history.iter().rposition(|e| !e.favourite) {
                Some(i) => {
                    self.history.remove(i);
                }
                None => break,
            }
        }
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_round_trips() {
        let mut s = Saved::default();
        s.remember("en", "zh", "hello", "你好", "nǐ hǎo");
        let back = Saved::decode(&s.encode()).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn repeats_move_up_and_keep_their_star() {
        let mut s = Saved::default();
        s.remember("en", "zh", "a", "A", "");
        s.history[0].favourite = true;
        s.remember("en", "zh", "b", "B", "");
        s.remember("en", "zh", "a", "A2", "");
        assert_eq!(s.history.len(), 2);
        assert_eq!(s.history[0].target, "A2");
        assert!(s.history[0].favourite);
    }

    #[test]
    fn the_oldest_plain_entries_fall_off_first() {
        let mut s = Saved::default();
        s.remember("en", "zh", "keep", "k", "");
        s.history[0].favourite = true;
        for i in 0..HISTORY_LIMIT + 5 {
            s.remember("en", "zh", &format!("t{i}"), "x", "");
        }
        assert_eq!(s.history.len(), HISTORY_LIMIT);
        assert!(s.history.iter().any(|e| e.source == "keep"));
    }
}
