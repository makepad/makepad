//! Pure conversion from loop transcriptions to compact splat-cell rolls.

use makepad_score_view::build::{DrumHit, DrumVoice, PitchedNote};

#[derive(Clone, Debug, PartialEq)]
pub struct CellBlocks {
    pub bars: u8,
    pub blocks: Vec<Block>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Block {
    pub start_beats: f32,
    pub len_beats: f32,
    /// Bottom-up lane index.
    pub lane: u8,
    pub lanes: u8,
    pub velocity: f32,
}

pub fn drum_blocks(hits: &[DrumHit], bars: u8) -> CellBlocks {
    let blocks = hits
        .iter()
        .map(|hit| Block {
            start_beats: hit.time_beats as f32,
            len_beats: 0.125,
            lane: match hit.voice {
                DrumVoice::Kick => 0,
                DrumVoice::Snare | DrumVoice::SideStick => 1,
                DrumVoice::HiHatClosed | DrumVoice::HiHatOpen | DrumVoice::HiHatPedal => 2,
                DrumVoice::TomHigh
                | DrumVoice::TomMid
                | DrumVoice::TomLow
                | DrumVoice::TomFloor => 3,
                DrumVoice::Ride | DrumVoice::RideBell | DrumVoice::Crash => 4,
            },
            lanes: 5,
            velocity: hit.velocity.clamp(0.0, 1.0),
        })
        .collect();
    CellBlocks { bars, blocks }
}

pub fn pitched_blocks(notes: &[PitchedNote], bars: u8) -> CellBlocks {
    let Some((min_midi, max_midi)) = notes
        .iter()
        .map(|note| note.midi)
        .min()
        .zip(notes.iter().map(|note| note.midi).max())
    else {
        return CellBlocks { bars, blocks: Vec::new() };
    };
    let lanes = max_midi.saturating_sub(min_midi).saturating_add(1).clamp(1, 24);
    let blocks = notes
        .iter()
        .map(|note| Block {
            start_beats: note.onset_beats as f32,
            len_beats: note.duration_beats.max(0.0) as f32,
            lane: note.midi.saturating_sub(min_midi).min(lanes - 1),
            lanes,
            velocity: note.velocity.clamp(0.0, 1.0),
        })
        .collect();
    CellBlocks { bars, blocks }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drum_lane_assignment_groups_voices_bottom_to_top() {
        let hits = [
            DrumHit { time_beats: 0.0, voice: DrumVoice::Kick, velocity: 0.5 },
            DrumHit { time_beats: 0.5, voice: DrumVoice::Snare, velocity: 0.5 },
            DrumHit { time_beats: 1.0, voice: DrumVoice::HiHatOpen, velocity: 0.5 },
            DrumHit { time_beats: 1.5, voice: DrumVoice::TomFloor, velocity: 0.5 },
            DrumHit { time_beats: 2.0, voice: DrumVoice::Crash, velocity: 0.5 },
        ];
        let result = drum_blocks(&hits, 4);
        assert_eq!(result.bars, 4);
        assert_eq!(result.blocks.iter().map(|block| block.lane).collect::<Vec<_>>(), [0, 1, 2, 3, 4]);
        assert!(result.blocks.iter().all(|block| block.lanes == 5 && block.len_beats == 0.125));
    }

    #[test]
    fn pitched_lanes_follow_local_range_and_clamp_to_twenty_four() {
        let notes = [
            PitchedNote { onset_beats: 0.0, duration_beats: 0.5, midi: 20, velocity: 0.2 },
            PitchedNote { onset_beats: 0.5, duration_beats: 1.0, midi: 32, velocity: 0.6 },
            PitchedNote { onset_beats: 1.5, duration_beats: 1.5, midi: 80, velocity: 1.2 },
        ];
        let result = pitched_blocks(&notes, 2);
        assert_eq!(result.blocks.iter().map(|block| block.lanes).collect::<Vec<_>>(), [24; 3]);
        assert_eq!(result.blocks.iter().map(|block| block.lane).collect::<Vec<_>>(), [0, 12, 23]);
        assert_eq!(result.blocks[2].velocity, 1.0);
    }

    #[test]
    fn single_pitch_and_empty_input_have_stable_lane_counts() {
        let note = PitchedNote { onset_beats: 0.0, duration_beats: 1.0, midi: 64, velocity: 0.7 };
        let single = pitched_blocks(&[note], 1);
        assert_eq!((single.blocks[0].lane, single.blocks[0].lanes), (0, 1));

        let empty = pitched_blocks(&[], 8);
        assert_eq!(empty, CellBlocks { bars: 8, blocks: Vec::new() });
        assert_eq!(drum_blocks(&[], 2), CellBlocks { bars: 2, blocks: Vec::new() });
    }
}
