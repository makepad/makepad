//! What the chat is told of the desk: the open score, where it plays, the
//! voice the person is on and the batch's results; and what its chips offer.

use super::Desk;

impl Desk {
    /// What the person sees, for the model.
    pub fn chat_context(&self) -> String {
        let mut out = String::new();
        let title = if self.document.title.is_empty() { "Untitled" } else { self.document.title.as_str() };
        out.push_str(&format!("The score \"{title}\", revision {}{}.\n", self.revision, if self.document.edited(&self.source) { ", with changes not saved yet" } else { "" }));
        let status = &self.status;
        if status.playing {
            let bar = self.doc.as_ref().map_or(4.0, |doc| doc.beats_per_bar());
            let bars = (status.beat / bar).floor();
            out.push_str(&format!("Playing at bar {}.{:.0} ({:.0} bpm){}.\n", bars as i64 + 1, status.beat - bars * bar + 1.0, status.bpm, status.section.as_ref().map_or(String::new(), |section| format!(", section {section}"))));
        } else {
            out.push_str("Stopped.\n");
        }
        if let Some(voice) = &self.panel.selected {
            out.push_str(&format!("The person is on the voice `{voice}` in the editor panel.\n"));
        }
        if let Some(batch) = &self.batch.batch {
            let ready = batch.songs.iter().filter(|song| song.state == makepad_music_iter::batch::State::Ready).count();
            out.push_str(&format!("The newest batch b{:03}: {ready} of {} songs ready{}.\n", batch.id, batch.songs.len(), self.batch.selected.map_or(String::new(), |song| format!(", song {} selected for rating", song + 1))));
        }
        out
    }

    /// The chips' state: the batch card's starting markers, whether a batch
    /// has ratings to learn from, whether one is being written or learned.
    pub fn batch_chips(&mut self) -> (Vec<String>, bool, bool) {
        self.batch.load();
        let rated = self.batch.batch.as_ref().is_some_and(|batch| batch.songs.iter().any(|song| song.rated()));
        (self.batch.card_markers(), rated, self.batch.busy())
    }
}
