//! One effect chain's standing settings: every knob on the rack, as the
//! operator left it.
//!
//! The settings know nothing about where the chain sits. On a deck they
//! are part of the channel strip -- a swap carries them, a load leaves
//! them -- but the same struct stands behind the video, the pads, the
//! synths and the mix, which is what lets those targets have a rack at
//! all. Every setter clamps the way the engine clamps, so the value
//! stored is the value heard, and returns the parameters the engine has
//! to be told about; the caller decides which chain they go to.

use crate::mixer::EffectParam;
use crate::music_dsp::LevelMode;
use crate::settings::Settings;

#[derive(Clone, Debug)]
pub struct ChainState {
    /// Which echo rung is on; 0 is off, otherwise an index into
    /// [`crate::music_dsp::ECHO_RUNGS`] plus one. Part of the channel
    /// strip, like resonance beside it.
    pub echo_rung: usize,
    /// Whether the echo's repeats land on the other channel.
    pub echo_pingpong: bool,
    /// How much of an echo repeat feeds the next one, in
    /// [`crate::music_dsp::ECHO_FEEDBACK_MAX`]'s range. Edited from the
    /// rack on the mix page rather than the deck header -- there is no room there
    /// for a fourth knob -- so it is part of the channel strip like the
    /// rung beside it: a swap carries it and a load leaves it.
    pub echo_feedback: f32,
    /// Whether the flanger is on. Same channel-strip treatment as the
    /// echo: a swap carries it, a load leaves it.
    pub flanger_on: bool,
    /// The flanger LFO's sweep speed, in Hz.
    pub flanger_rate: f32,
    /// How far the flanger's sweep reaches from its centre delay, 0..1.
    pub flanger_depth: f32,
    /// How much of the flanger's delayed tap feeds back into its line.
    pub flanger_feedback: f32,
    /// Which rung of the sync ladder the flanger sweep runs on:
    /// `LFO_SYNC_FREE` to follow `flanger_rate`'s Hz, or eighths of a
    /// cycle per beat to follow the grid.
    pub flanger_sync_units: u32,
    /// Where in the cycle the flanger sweep starts when it engages, 0..1.
    pub flanger_beat_offset: f32,
    /// Whether the bitcrusher is on. Same channel-strip treatment as
    /// the flanger beside it.
    pub bitcrusher_on: bool,
    /// How often the bitcrusher's hold captures a fresh sample, in Hz.
    pub bitcrusher_rate: f32,
    /// The bitcrusher's quantizer bit depth.
    pub bitcrusher_bits: f32,
    /// Whether the tremolo is on. Same channel-strip treatment as the
    /// bitcrusher beside it.
    pub tremolo_on: bool,
    /// The tremolo LFO's speed, in Hz.
    pub tremolo_rate: f32,
    /// The tremolo LFO's swing, 0..1.
    pub tremolo_depth: f32,
    /// Which rung of the sync ladder the tremolo runs on:
    /// `LFO_SYNC_FREE` to follow `tremolo_rate`'s Hz, or eighths of a
    /// cycle per beat to follow the grid.
    pub tremolo_sync_units: u32,
    /// Where in the cycle the tremolo starts when it engages, 0..1.
    pub tremolo_beat_offset: f32,
    /// Whether the distortion is on. Same channel-strip treatment as
    /// the tremolo beside it.
    pub distortion_on: bool,
    /// The distortion's pre-gain into the soft clip.
    pub distortion_drive: f32,
    /// Whether the phaser is on. Same channel-strip treatment as the
    /// distortion beside it.
    pub phaser_on: bool,
    /// The phaser LFO's sweep speed, in Hz.
    pub phaser_rate: f32,
    /// How much of the phaser's own output feeds back into its first
    /// stage.
    pub phaser_feedback: f32,
    /// Which rung of the sync ladder the phaser sweep runs on:
    /// `LFO_SYNC_FREE` to follow `phaser_rate`'s Hz, or eighths of a
    /// cycle per beat to follow the grid.
    pub phaser_sync_units: u32,
    /// Where in the cycle the phaser sweep starts when it engages, 0..1.
    pub phaser_beat_offset: f32,
    /// Whether the autopan is on. Same channel-strip treatment as the
    /// phaser beside it.
    pub autopan_on: bool,
    /// The autopan LFO's sweep speed, in Hz.
    pub autopan_rate: f32,
    /// Which rung of the sync ladder the autopan swing runs on:
    /// `LFO_SYNC_FREE` to follow `autopan_rate`'s Hz, or eighths of a
    /// cycle per beat to follow the grid.
    pub autopan_sync_units: u32,
    /// Where in the cycle the autopan swing starts when it engages, 0..1.
    pub autopan_beat_offset: f32,
    /// The echo's wet/dry mix, 0 = inaudible, 1 = all of it. The
    /// effect's own on/off stays the engage; this is how much of it is
    /// heard once it is on.
    pub echo_mix: f32,
    /// What the echo's slot does about the level it returns: follow the
    /// deck, leave it alone, match the input, or hold under a ceiling.
    pub echo_level_mode: crate::music_dsp::LevelMode,
    /// The amplitude the echo holds under on Ceiling.
    pub echo_ceiling: f32,
    /// The flanger's wet/dry mix, 0 = inaudible, 1 = all of it. The
    /// effect's own on/off stays the engage; this is how much of it is
    /// heard once it is on.
    pub flanger_mix: f32,
    /// What the flanger's slot does about the level it returns: follow the
    /// deck, leave it alone, match the input, or hold under a ceiling.
    pub flanger_level_mode: crate::music_dsp::LevelMode,
    /// The amplitude the flanger holds under on Ceiling.
    pub flanger_ceiling: f32,
    /// The bitcrusher's wet/dry mix, 0 = inaudible, 1 = all of it. The
    /// effect's own on/off stays the engage; this is how much of it is
    /// heard once it is on.
    pub bitcrusher_mix: f32,
    /// What the bitcrusher's slot does about the level it returns: follow the
    /// deck, leave it alone, match the input, or hold under a ceiling.
    pub bitcrusher_level_mode: crate::music_dsp::LevelMode,
    /// The amplitude the bitcrusher holds under on Ceiling.
    pub bitcrusher_ceiling: f32,
    /// The tremolo's wet/dry mix, 0 = inaudible, 1 = all of it. The
    /// effect's own on/off stays the engage; this is how much of it is
    /// heard once it is on.
    pub tremolo_mix: f32,
    /// What the tremolo's slot does about the level it returns: follow the
    /// deck, leave it alone, match the input, or hold under a ceiling.
    pub tremolo_level_mode: crate::music_dsp::LevelMode,
    /// The amplitude the tremolo holds under on Ceiling.
    pub tremolo_ceiling: f32,
    /// The distortion's wet/dry mix, 0 = inaudible, 1 = all of it. The
    /// effect's own on/off stays the engage; this is how much of it is
    /// heard once it is on.
    pub distortion_mix: f32,
    /// What the distortion's slot does about the level it returns: follow the
    /// deck, leave it alone, match the input, or hold under a ceiling.
    pub distortion_level_mode: crate::music_dsp::LevelMode,
    /// The amplitude the distortion holds under on Ceiling.
    pub distortion_ceiling: f32,
    /// The phaser's wet/dry mix, 0 = inaudible, 1 = all of it. The
    /// effect's own on/off stays the engage; this is how much of it is
    /// heard once it is on.
    pub phaser_mix: f32,
    /// What the phaser's slot does about the level it returns: follow the
    /// deck, leave it alone, match the input, or hold under a ceiling.
    pub phaser_level_mode: crate::music_dsp::LevelMode,
    /// The amplitude the phaser holds under on Ceiling.
    pub phaser_ceiling: f32,
    /// The autopan's wet/dry mix, 0 = inaudible, 1 = all of it. The
    /// effect's own on/off stays the engage; this is how much of it is
    /// heard once it is on.
    pub autopan_mix: f32,
    /// What the autopan's slot does about the level it returns: follow the
    /// deck, leave it alone, match the input, or hold under a ceiling.
    pub autopan_level_mode: crate::music_dsp::LevelMode,
    /// The amplitude the autopan holds under on Ceiling.
    pub autopan_ceiling: f32,
    /// The stereo width's wet/dry mix, 0 = inaudible, 1 = all of it. The
    /// effect's own on/off stays the engage; this is how much of it is
    /// heard once it is on.
    pub stereo_width_mix: f32,
    /// What the stereo width's slot does about the level it returns: follow the
    /// deck, leave it alone, match the input, or hold under a ceiling.
    pub stereo_width_level_mode: crate::music_dsp::LevelMode,
    /// The amplitude the stereo width holds under on Ceiling.
    pub stereo_width_ceiling: f32,
    /// The plate reverb's wet/dry mix, 0 = inaudible, 1 = all of it. The
    /// effect's own on/off stays the engage; this is how much of it is
    /// heard once it is on.
    pub plate_reverb_mix: f32,
    /// What the plate reverb's slot does about the level it returns: follow the
    /// deck, leave it alone, match the input, or hold under a ceiling.
    pub plate_reverb_level_mode: crate::music_dsp::LevelMode,
    /// The amplitude the plate reverb holds under on Ceiling.
    pub plate_reverb_ceiling: f32,
    /// The ladder filter's wet/dry mix, 0 = inaudible, 1 = all of it. The
    /// effect's own on/off stays the engage; this is how much of it is
    /// heard once it is on.
    pub moog_ladder_mix: f32,
    /// What the ladder filter's slot does about the level it returns: follow the
    /// deck, leave it alone, match the input, or hold under a ceiling.
    pub moog_ladder_level_mode: crate::music_dsp::LevelMode,
    /// The amplitude the ladder filter holds under on Ceiling.
    pub moog_ladder_ceiling: f32,
    /// The policy every effect that has not been pinned follows.
    pub level_default: crate::music_dsp::LevelMode,
    /// Whether the compressor is on. Same channel-strip treatment as
    /// the ladder beside it.
    pub compressor_on: bool,
    /// Where it starts working, in decibels below full scale.
    pub compressor_threshold_db: f32,
    /// How hard it works above that.
    pub compressor_ratio: f32,
    /// Where this deck's three EQ bands are split, in Hz.
    pub eq_low_hz: f32,
    pub eq_high_hz: f32,
    /// Whether the stereo width is on. Same channel-strip treatment as
    /// the autopan beside it.
    pub stereo_width_on: bool,
    /// How far the side signal is scaled: 0 collapses to mono, 1 is the
    /// original image, above 1 widens further.
    pub stereo_width_amount: f32,
    /// Whether the plate reverb is on. Same channel-strip treatment as
    /// the stereo width beside it.
    pub plate_reverb_on: bool,
    /// How long the reverb tank's tail rings.
    pub plate_reverb_size: f32,
    /// Whether the Moog ladder is on. Same channel-strip treatment as
    /// the plate reverb beside it.
    pub moog_ladder_on: bool,
    /// Where the ladder starts rolling off, in Hz.
    pub moog_ladder_cutoff: f32,
    /// How much of the last stage feeds back into the first.
    pub moog_ladder_resonance: f32,
}

impl Default for ChainState {
    fn default() -> Self {
        Self {
            echo_rung: 0,
            echo_pingpong: false,
            echo_feedback: crate::music_dsp::ECHO_FEEDBACK,
            flanger_on: false,
            flanger_rate: crate::music_dsp::FLANGER_RATE_DEFAULT,
            flanger_depth: crate::music_dsp::FLANGER_DEPTH_DEFAULT,
            flanger_feedback: crate::music_dsp::FLANGER_FEEDBACK_DEFAULT,
            flanger_sync_units: crate::music_dsp::LFO_SYNC_FREE,
            flanger_beat_offset: 0.0,
            bitcrusher_on: false,
            bitcrusher_rate: crate::music_dsp::BITCRUSHER_RATE_DEFAULT,
            bitcrusher_bits: crate::music_dsp::BITCRUSHER_BITS_DEFAULT,
            tremolo_on: false,
            tremolo_rate: crate::music_dsp::TREMOLO_RATE_DEFAULT,
            tremolo_depth: crate::music_dsp::TREMOLO_DEPTH_DEFAULT,
            tremolo_sync_units: crate::music_dsp::LFO_SYNC_FREE,
            tremolo_beat_offset: 0.0,
            distortion_on: false,
            distortion_drive: crate::music_dsp::DISTORTION_DRIVE_DEFAULT,
            phaser_on: false,
            phaser_rate: crate::music_dsp::PHASER_RATE_DEFAULT,
            phaser_feedback: crate::music_dsp::PHASER_FEEDBACK_DEFAULT,
            phaser_sync_units: crate::music_dsp::LFO_SYNC_FREE,
            phaser_beat_offset: 0.0,
            autopan_on: false,
            autopan_rate: crate::music_dsp::AUTOPAN_RATE_DEFAULT,
            autopan_sync_units: crate::music_dsp::LFO_SYNC_FREE,
            autopan_beat_offset: 0.0,
            echo_mix: 1.0,
            echo_level_mode: crate::music_dsp::LevelMode::Follow,
            echo_ceiling: 1.0,
            flanger_mix: 1.0,
            flanger_level_mode: crate::music_dsp::LevelMode::Follow,
            flanger_ceiling: 1.0,
            bitcrusher_mix: 1.0,
            bitcrusher_level_mode: crate::music_dsp::LevelMode::Follow,
            bitcrusher_ceiling: 1.0,
            tremolo_mix: 1.0,
            tremolo_level_mode: crate::music_dsp::LevelMode::Follow,
            tremolo_ceiling: 1.0,
            distortion_mix: 1.0,
            distortion_level_mode: crate::music_dsp::LevelMode::Follow,
            distortion_ceiling: 1.0,
            phaser_mix: 1.0,
            phaser_level_mode: crate::music_dsp::LevelMode::Follow,
            phaser_ceiling: 1.0,
            autopan_mix: 1.0,
            autopan_level_mode: crate::music_dsp::LevelMode::Follow,
            autopan_ceiling: 1.0,
            stereo_width_mix: 1.0,
            stereo_width_level_mode: crate::music_dsp::LevelMode::Follow,
            stereo_width_ceiling: 1.0,
            plate_reverb_mix: 1.0,
            plate_reverb_level_mode: crate::music_dsp::LevelMode::Follow,
            plate_reverb_ceiling: 1.0,
            moog_ladder_mix: 1.0,
            moog_ladder_level_mode: crate::music_dsp::LevelMode::Follow,
            moog_ladder_ceiling: 1.0,
            level_default: crate::music_dsp::LevelMode::Off,
            compressor_on: false,
            compressor_threshold_db: crate::music_dsp::COMPRESSOR_THRESHOLD_DEFAULT_DB,
            compressor_ratio: crate::music_dsp::COMPRESSOR_RATIO_DEFAULT,
            eq_low_hz: crate::music_dsp::EQ_LOW_HZ,
            eq_high_hz: crate::music_dsp::EQ_HIGH_HZ,
            stereo_width_on: false,
            stereo_width_amount: crate::music_dsp::STEREO_WIDTH_DEFAULT,
            plate_reverb_on: false,
            plate_reverb_size: crate::music_dsp::PLATE_REVERB_SIZE_DEFAULT,
            moog_ladder_on: false,
            moog_ladder_cutoff: crate::music_dsp::MOOG_LADDER_CUTOFF_DEFAULT,
            moog_ladder_resonance: crate::music_dsp::MOOG_LADDER_RESONANCE_DEFAULT,
        }
    }
}

impl ChainState {
    /// The whole standing intent, every slot, in the order the engine has
    /// always received it when a deck loads. A fresh chain -- a record
    /// just installed, a target just given a rack -- is brought up to
    /// date with exactly this list and nothing else.
    pub fn params(&self) -> Vec<EffectParam> {
        vec![
            EffectParam::Echo(self.echo_fraction()),
            EffectParam::EchoPingpong(self.echo_pingpong),
            EffectParam::EchoFeedback(self.echo_feedback),
            EffectParam::Flanger(self.flanger_on),
            EffectParam::FlangerRate(self.flanger_rate),
            EffectParam::FlangerDepth(self.flanger_depth),
            EffectParam::FlangerFeedback(self.flanger_feedback),
            EffectParam::FlangerSyncUnits(self.flanger_sync_units),
            EffectParam::FlangerBeatOffset(self.flanger_beat_offset),
            EffectParam::Bitcrusher(self.bitcrusher_on),
            EffectParam::BitcrusherRate(self.bitcrusher_rate),
            EffectParam::BitcrusherBits(self.bitcrusher_bits),
            EffectParam::Tremolo(self.tremolo_on),
            EffectParam::TremoloRate(self.tremolo_rate),
            EffectParam::TremoloDepth(self.tremolo_depth),
            EffectParam::TremoloSyncUnits(self.tremolo_sync_units),
            EffectParam::TremoloBeatOffset(self.tremolo_beat_offset),
            EffectParam::Distortion(self.distortion_on),
            EffectParam::DistortionDrive(self.distortion_drive),
            EffectParam::Phaser(self.phaser_on),
            EffectParam::PhaserRate(self.phaser_rate),
            EffectParam::PhaserFeedback(self.phaser_feedback),
            EffectParam::PhaserSyncUnits(self.phaser_sync_units),
            EffectParam::PhaserBeatOffset(self.phaser_beat_offset),
            EffectParam::Autopan(self.autopan_on),
            EffectParam::AutopanRate(self.autopan_rate),
            EffectParam::AutopanSyncUnits(self.autopan_sync_units),
            EffectParam::AutopanBeatOffset(self.autopan_beat_offset),
            EffectParam::EchoMix(self.echo_mix),
            EffectParam::EchoLevelMode(self.echo_level_mode),
            EffectParam::EchoCeiling(self.echo_ceiling),
            EffectParam::FlangerMix(self.flanger_mix),
            EffectParam::FlangerLevelMode(self.flanger_level_mode),
            EffectParam::FlangerCeiling(self.flanger_ceiling),
            EffectParam::BitcrusherMix(self.bitcrusher_mix),
            EffectParam::BitcrusherLevelMode(self.bitcrusher_level_mode),
            EffectParam::BitcrusherCeiling(self.bitcrusher_ceiling),
            EffectParam::TremoloMix(self.tremolo_mix),
            EffectParam::TremoloLevelMode(self.tremolo_level_mode),
            EffectParam::TremoloCeiling(self.tremolo_ceiling),
            EffectParam::DistortionMix(self.distortion_mix),
            EffectParam::DistortionLevelMode(self.distortion_level_mode),
            EffectParam::DistortionCeiling(self.distortion_ceiling),
            EffectParam::PhaserMix(self.phaser_mix),
            EffectParam::PhaserLevelMode(self.phaser_level_mode),
            EffectParam::PhaserCeiling(self.phaser_ceiling),
            EffectParam::AutopanMix(self.autopan_mix),
            EffectParam::AutopanLevelMode(self.autopan_level_mode),
            EffectParam::AutopanCeiling(self.autopan_ceiling),
            EffectParam::StereoWidthMix(self.stereo_width_mix),
            EffectParam::StereoWidthLevelMode(self.stereo_width_level_mode),
            EffectParam::StereoWidthCeiling(self.stereo_width_ceiling),
            EffectParam::PlateReverbMix(self.plate_reverb_mix),
            EffectParam::PlateReverbLevelMode(self.plate_reverb_level_mode),
            EffectParam::PlateReverbCeiling(self.plate_reverb_ceiling),
            EffectParam::MoogLadderMix(self.moog_ladder_mix),
            EffectParam::MoogLadderLevelMode(self.moog_ladder_level_mode),
            EffectParam::MoogLadderCeiling(self.moog_ladder_ceiling),
            EffectParam::LevelDefault(self.level_default),
            EffectParam::Compressor(self.compressor_on),
            EffectParam::CompressorThreshold(self.compressor_threshold_db),
            EffectParam::CompressorRatio(self.compressor_ratio),
            EffectParam::Crossovers(self.eq_low_hz, self.eq_high_hz),
            EffectParam::StereoWidth(self.stereo_width_on),
            EffectParam::StereoWidthAmount(self.stereo_width_amount),
            EffectParam::PlateReverb(self.plate_reverb_on),
            EffectParam::PlateReverbSize(self.plate_reverb_size),
            EffectParam::MoogLadder(self.moog_ladder_on),
            EffectParam::MoogLadderCutoff(self.moog_ladder_cutoff),
            EffectParam::MoogLadderResonance(self.moog_ladder_resonance),
        ]
    }

    /// The fraction the mixer's echo is sent for this chain's rung, or
    /// none for off.
    pub fn echo_fraction(&self) -> Option<(u32, u32)> {
        (self.echo_rung > 0)
            .then(|| crate::music_dsp::ECHO_RUNGS.get(self.echo_rung - 1))
            .flatten()
            .copied()
    }

    // ---- compressor + EQ split ------------------------------------------

    /// The compressor's on/off switch.
    pub fn toggle_compressor(&mut self) -> Vec<EffectParam> {
        self.compressor_on = !self.compressor_on;
        vec![EffectParam::Compressor(self.compressor_on)]
    }

    /// Set it to an explicit side, for a MIX-linked broadcast.
    pub fn set_compressor(&mut self, on: bool) -> Vec<EffectParam> {
        self.compressor_on = on;
        vec![EffectParam::Compressor(on)]
    }

    /// Where it starts working, in decibels below full scale.
    pub fn set_compressor_threshold(&mut self, db: f32) -> Vec<EffectParam> {
        self.compressor_threshold_db = db.clamp(
            crate::music_dsp::COMPRESSOR_THRESHOLD_MIN_DB,
            crate::music_dsp::COMPRESSOR_THRESHOLD_MAX_DB,
        );
        vec![EffectParam::CompressorThreshold(self.compressor_threshold_db)]
    }

    /// How hard it works above that.
    pub fn set_compressor_ratio(&mut self, ratio: f32) -> Vec<EffectParam> {
        self.compressor_ratio = ratio.clamp(
            crate::music_dsp::COMPRESSOR_RATIO_MIN,
            crate::music_dsp::COMPRESSOR_RATIO_MAX,
        );
        vec![EffectParam::CompressorRatio(self.compressor_ratio)]
    }

    /// Where this chain's bands are split. Clamped the same way the
    /// engine clamps, so the stored value and the audible one agree, and
    /// the gap between the corners is the engine's to keep.
    pub fn set_crossovers(&mut self, low_hz: f32, high_hz: f32) -> Vec<EffectParam> {
        let Some((low, high)) = crate::music_dsp::eq_crossovers_for(low_hz, high_hz) else {
            return Vec::new();
        };
        self.eq_low_hz = low;
        self.eq_high_hz = high;
        vec![EffectParam::Crossovers(self.eq_low_hz, self.eq_high_hz)]
    }

    // ---- echo -------------------------------------------------------------

    /// Step the echo to its next rung, round and round: off, whole beat,
    /// half, quarter, off.
    pub fn cycle_echo(&mut self) -> Vec<EffectParam> {
        let rungs = crate::music_dsp::ECHO_RUNGS.len() + 1; // + off
        self.echo_rung = (self.echo_rung + 1) % rungs;
        vec![EffectParam::Echo(self.echo_fraction())]
    }

    /// Put the echo on an explicit rung rather than stepping to the next
    /// one -- what a dropdown sends, and what a MIX broadcast needs so
    /// both decks land on the same rung rather than each stepping from
    /// wherever it happened to be.
    pub fn set_echo_rung(&mut self, rung: usize) -> Vec<EffectParam> {
        let rungs = crate::music_dsp::ECHO_RUNGS.len() + 1;
        self.echo_rung = rung.min(rungs - 1);
        vec![EffectParam::Echo(self.echo_fraction())]
    }

    /// Set the ping-pong to an explicit side, for the same reason.
    pub fn set_echo_pingpong(&mut self, on: bool) -> Vec<EffectParam> {
        self.echo_pingpong = on;
        vec![EffectParam::EchoPingpong(on)]
    }

    /// Whether the echo's repeats land on the other channel.
    pub fn toggle_echo_pingpong(&mut self) -> Vec<EffectParam> {
        self.echo_pingpong = !self.echo_pingpong;
        vec![EffectParam::EchoPingpong(self.echo_pingpong)]
    }

    /// How much of a repeat feeds the next one. The same clamp the
    /// mixer's own setter applies, so the stored state and the audible
    /// one never disagree.
    pub fn set_echo_feedback(&mut self, feedback: f32) -> Vec<EffectParam> {
        self.echo_feedback = feedback.clamp(0.0, crate::music_dsp::ECHO_FEEDBACK_MAX);
        vec![EffectParam::EchoFeedback(self.echo_feedback)]
    }

    // ---- flanger ----------------------------------------------------------

    /// The flanger's on/off switch.
    pub fn toggle_flanger(&mut self) -> Vec<EffectParam> {
        self.flanger_on = !self.flanger_on;
        vec![EffectParam::Flanger(self.flanger_on)]
    }

    /// Set the flanger's on/off switch to an explicit value, rather than
    /// flipping whatever it already was -- what a MIX-linked broadcast
    /// needs, since two decks starting on different sides of the switch
    /// must land on the SAME side, not each flip its own.
    pub fn set_flanger(&mut self, on: bool) -> Vec<EffectParam> {
        self.flanger_on = on;
        vec![EffectParam::Flanger(on)]
    }

    /// The flanger LFO's sweep speed, in Hz.
    pub fn set_flanger_rate(&mut self, hz: f32) -> Vec<EffectParam> {
        self.flanger_rate =
            hz.clamp(crate::music_dsp::FLANGER_RATE_MIN, crate::music_dsp::FLANGER_RATE_MAX);
        vec![EffectParam::FlangerRate(self.flanger_rate)]
    }

    /// How far the flanger's sweep reaches from its centre delay.
    pub fn set_flanger_depth(&mut self, depth: f32) -> Vec<EffectParam> {
        self.flanger_depth = depth.clamp(0.0, 1.0);
        vec![EffectParam::FlangerDepth(self.flanger_depth)]
    }

    /// How much of the flanger's delayed tap feeds back into its line.
    pub fn set_flanger_feedback(&mut self, feedback: f32) -> Vec<EffectParam> {
        self.flanger_feedback = feedback.clamp(0.0, crate::music_dsp::FLANGER_FEEDBACK_MAX);
        vec![EffectParam::FlangerFeedback(self.flanger_feedback)]
    }

    /// Which rung of the sync ladder the flanger sweep runs on. The dropdown
    /// emits an explicit rung rather than a flip, so unlike the on/off
    /// gestures beside it this needs no toggling twin: a MIX broadcast
    /// simply sends the same rung to both decks.
    pub fn set_flanger_sync_units(&mut self, units: u32) -> Vec<EffectParam> {
        self.flanger_sync_units = units.min(crate::music_dsp::LFO_SYNC_MAX_UNITS);
        vec![EffectParam::FlangerSyncUnits(self.flanger_sync_units)]
    }

    /// Where in the cycle the flanger sweep starts when it engages, 0..1.
    pub fn set_flanger_beat_offset(&mut self, offset: f32) -> Vec<EffectParam> {
        self.flanger_beat_offset = offset.clamp(0.0, 1.0);
        vec![EffectParam::FlangerBeatOffset(self.flanger_beat_offset)]
    }

    // ---- bitcrusher -------------------------------------------------------

    /// The bitcrusher's on/off switch.
    pub fn toggle_bitcrusher(&mut self) -> Vec<EffectParam> {
        self.bitcrusher_on = !self.bitcrusher_on;
        vec![EffectParam::Bitcrusher(self.bitcrusher_on)]
    }

    /// Set the bitcrusher's on/off switch to an explicit value, rather
    /// than flipping whatever it already was -- what a MIX-linked
    /// broadcast needs, the same reason [`Self::set_flanger`] exists.
    pub fn set_bitcrusher(&mut self, on: bool) -> Vec<EffectParam> {
        self.bitcrusher_on = on;
        vec![EffectParam::Bitcrusher(on)]
    }

    /// How often the bitcrusher's hold captures a fresh sample, in Hz.
    pub fn set_bitcrusher_rate(&mut self, hz: f32) -> Vec<EffectParam> {
        self.bitcrusher_rate = hz.clamp(
            crate::music_dsp::BITCRUSHER_RATE_MIN,
            crate::music_dsp::BITCRUSHER_RATE_MAX,
        );
        vec![EffectParam::BitcrusherRate(self.bitcrusher_rate)]
    }

    /// The bitcrusher's quantizer bit depth.
    pub fn set_bitcrusher_bits(&mut self, bits: f32) -> Vec<EffectParam> {
        self.bitcrusher_bits = bits.clamp(
            crate::music_dsp::BITCRUSHER_BITS_MIN,
            crate::music_dsp::BITCRUSHER_BITS_MAX,
        );
        vec![EffectParam::BitcrusherBits(self.bitcrusher_bits)]
    }

    // ---- tremolo ----------------------------------------------------------

    /// The tremolo's on/off switch.
    pub fn toggle_tremolo(&mut self) -> Vec<EffectParam> {
        self.tremolo_on = !self.tremolo_on;
        vec![EffectParam::Tremolo(self.tremolo_on)]
    }

    /// Set the tremolo's on/off switch to an explicit value, rather
    /// than flipping whatever it already was -- what a MIX-linked
    /// broadcast needs, the same reason [`Self::set_flanger`] exists.
    pub fn set_tremolo(&mut self, on: bool) -> Vec<EffectParam> {
        self.tremolo_on = on;
        vec![EffectParam::Tremolo(on)]
    }

    /// The tremolo LFO's speed, in Hz.
    pub fn set_tremolo_rate(&mut self, hz: f32) -> Vec<EffectParam> {
        self.tremolo_rate =
            hz.clamp(crate::music_dsp::TREMOLO_RATE_MIN, crate::music_dsp::TREMOLO_RATE_MAX);
        vec![EffectParam::TremoloRate(self.tremolo_rate)]
    }

    /// The tremolo LFO's swing.
    pub fn set_tremolo_depth(&mut self, depth: f32) -> Vec<EffectParam> {
        self.tremolo_depth = depth.clamp(0.0, 1.0);
        vec![EffectParam::TremoloDepth(self.tremolo_depth)]
    }

    /// Which rung of the sync ladder the tremolo runs on. The dropdown
    /// emits an explicit rung rather than a flip, so unlike the on/off
    /// gestures beside it this needs no toggling twin: a MIX broadcast
    /// simply sends the same rung to both decks.
    pub fn set_tremolo_sync_units(&mut self, units: u32) -> Vec<EffectParam> {
        self.tremolo_sync_units = units.min(crate::music_dsp::LFO_SYNC_MAX_UNITS);
        vec![EffectParam::TremoloSyncUnits(self.tremolo_sync_units)]
    }

    /// Where in the cycle the tremolo starts when it engages, 0..1.
    pub fn set_tremolo_beat_offset(&mut self, offset: f32) -> Vec<EffectParam> {
        self.tremolo_beat_offset = offset.clamp(0.0, 1.0);
        vec![EffectParam::TremoloBeatOffset(self.tremolo_beat_offset)]
    }

    // ---- distortion -------------------------------------------------------

    /// The distortion's on/off switch.
    pub fn toggle_distortion(&mut self) -> Vec<EffectParam> {
        self.distortion_on = !self.distortion_on;
        vec![EffectParam::Distortion(self.distortion_on)]
    }

    /// Set the distortion's on/off switch to an explicit value, rather
    /// than flipping whatever it already was -- what a MIX-linked
    /// broadcast needs, the same reason [`Self::set_flanger`] exists.
    pub fn set_distortion(&mut self, on: bool) -> Vec<EffectParam> {
        self.distortion_on = on;
        vec![EffectParam::Distortion(on)]
    }

    /// The distortion's pre-gain into the soft clip.
    pub fn set_distortion_drive(&mut self, drive: f32) -> Vec<EffectParam> {
        self.distortion_drive = drive.clamp(
            crate::music_dsp::DISTORTION_DRIVE_MIN,
            crate::music_dsp::DISTORTION_DRIVE_MAX,
        );
        vec![EffectParam::DistortionDrive(self.distortion_drive)]
    }

    // ---- phaser -----------------------------------------------------------

    /// The phaser's on/off switch.
    pub fn toggle_phaser(&mut self) -> Vec<EffectParam> {
        self.phaser_on = !self.phaser_on;
        vec![EffectParam::Phaser(self.phaser_on)]
    }

    /// Set the phaser's on/off switch to an explicit value, rather than
    /// flipping whatever it already was -- what a MIX-linked broadcast
    /// needs, the same reason [`Self::set_flanger`] exists.
    pub fn set_phaser(&mut self, on: bool) -> Vec<EffectParam> {
        self.phaser_on = on;
        vec![EffectParam::Phaser(on)]
    }

    /// The phaser LFO's sweep speed, in Hz.
    pub fn set_phaser_rate(&mut self, hz: f32) -> Vec<EffectParam> {
        self.phaser_rate =
            hz.clamp(crate::music_dsp::PHASER_RATE_MIN, crate::music_dsp::PHASER_RATE_MAX);
        vec![EffectParam::PhaserRate(self.phaser_rate)]
    }

    /// How much of the phaser's own output feeds back into its first
    /// stage.
    pub fn set_phaser_feedback(&mut self, feedback: f32) -> Vec<EffectParam> {
        self.phaser_feedback =
            feedback.clamp(0.0, crate::music_dsp::PHASER_FEEDBACK_MAX);
        vec![EffectParam::PhaserFeedback(self.phaser_feedback)]
    }

    /// Which rung of the sync ladder the phaser sweep runs on. The dropdown
    /// emits an explicit rung rather than a flip, so unlike the on/off
    /// gestures beside it this needs no toggling twin: a MIX broadcast
    /// simply sends the same rung to both decks.
    pub fn set_phaser_sync_units(&mut self, units: u32) -> Vec<EffectParam> {
        self.phaser_sync_units = units.min(crate::music_dsp::LFO_SYNC_MAX_UNITS);
        vec![EffectParam::PhaserSyncUnits(self.phaser_sync_units)]
    }

    /// Where in the cycle the phaser sweep starts when it engages, 0..1.
    pub fn set_phaser_beat_offset(&mut self, offset: f32) -> Vec<EffectParam> {
        self.phaser_beat_offset = offset.clamp(0.0, 1.0);
        vec![EffectParam::PhaserBeatOffset(self.phaser_beat_offset)]
    }

    // ---- autopan ----------------------------------------------------------

    /// The autopan's on/off switch.
    pub fn toggle_autopan(&mut self) -> Vec<EffectParam> {
        self.autopan_on = !self.autopan_on;
        vec![EffectParam::Autopan(self.autopan_on)]
    }

    /// Set the autopan's on/off switch to an explicit value, rather
    /// than flipping whatever it already was -- what a MIX-linked
    /// broadcast needs, the same reason [`Self::set_flanger`] exists.
    pub fn set_autopan(&mut self, on: bool) -> Vec<EffectParam> {
        self.autopan_on = on;
        vec![EffectParam::Autopan(on)]
    }

    /// The autopan LFO's sweep speed, in Hz.
    pub fn set_autopan_rate(&mut self, hz: f32) -> Vec<EffectParam> {
        self.autopan_rate =
            hz.clamp(crate::music_dsp::AUTOPAN_RATE_MIN, crate::music_dsp::AUTOPAN_RATE_MAX);
        vec![EffectParam::AutopanRate(self.autopan_rate)]
    }

    /// Which rung of the sync ladder the autopan swing runs on. The dropdown
    /// emits an explicit rung rather than a flip, so unlike the on/off
    /// gestures beside it this needs no toggling twin: a MIX broadcast
    /// simply sends the same rung to both decks.
    pub fn set_autopan_sync_units(&mut self, units: u32) -> Vec<EffectParam> {
        self.autopan_sync_units = units.min(crate::music_dsp::LFO_SYNC_MAX_UNITS);
        vec![EffectParam::AutopanSyncUnits(self.autopan_sync_units)]
    }

    /// Where in the cycle the autopan swing starts when it engages, 0..1.
    pub fn set_autopan_beat_offset(&mut self, offset: f32) -> Vec<EffectParam> {
        self.autopan_beat_offset = offset.clamp(0.0, 1.0);
        vec![EffectParam::AutopanBeatOffset(self.autopan_beat_offset)]
    }

    // ---- the level trios --------------------------------------------------

    /// How much of the echo is heard once it is engaged.
    pub fn set_echo_mix(&mut self, mix: f32) -> Vec<EffectParam> {
        self.echo_mix = mix.clamp(0.0, 1.0);
        vec![EffectParam::EchoMix(self.echo_mix)]
    }

    /// What the echo's slot does about the level it returns.
    pub fn set_echo_level_mode(&mut self, mode: crate::music_dsp::LevelMode) -> Vec<EffectParam> {
        self.echo_level_mode = mode;
        vec![EffectParam::EchoLevelMode(mode)]
    }

    /// The amplitude the echo holds under on Ceiling.
    pub fn set_echo_ceiling(&mut self, ceiling: f32) -> Vec<EffectParam> {
        self.echo_ceiling = ceiling.clamp(0.01, 1.0);
        vec![EffectParam::EchoCeiling(self.echo_ceiling)]
    }

    /// How much of the flanger is heard once it is engaged.
    pub fn set_flanger_mix(&mut self, mix: f32) -> Vec<EffectParam> {
        self.flanger_mix = mix.clamp(0.0, 1.0);
        vec![EffectParam::FlangerMix(self.flanger_mix)]
    }

    /// What the flanger's slot does about the level it returns.
    pub fn set_flanger_level_mode(
        &mut self,
        mode: crate::music_dsp::LevelMode,
    ) -> Vec<EffectParam> {
        self.flanger_level_mode = mode;
        vec![EffectParam::FlangerLevelMode(mode)]
    }

    /// The amplitude the flanger holds under on Ceiling.
    pub fn set_flanger_ceiling(&mut self, ceiling: f32) -> Vec<EffectParam> {
        self.flanger_ceiling = ceiling.clamp(0.01, 1.0);
        vec![EffectParam::FlangerCeiling(self.flanger_ceiling)]
    }

    /// How much of the bitcrusher is heard once it is engaged.
    pub fn set_bitcrusher_mix(&mut self, mix: f32) -> Vec<EffectParam> {
        self.bitcrusher_mix = mix.clamp(0.0, 1.0);
        vec![EffectParam::BitcrusherMix(self.bitcrusher_mix)]
    }

    /// What the bitcrusher's slot does about the level it returns.
    pub fn set_bitcrusher_level_mode(
        &mut self,
        mode: crate::music_dsp::LevelMode,
    ) -> Vec<EffectParam> {
        self.bitcrusher_level_mode = mode;
        vec![EffectParam::BitcrusherLevelMode(mode)]
    }

    /// The amplitude the bitcrusher holds under on Ceiling.
    pub fn set_bitcrusher_ceiling(&mut self, ceiling: f32) -> Vec<EffectParam> {
        self.bitcrusher_ceiling = ceiling.clamp(0.01, 1.0);
        vec![EffectParam::BitcrusherCeiling(self.bitcrusher_ceiling)]
    }

    /// How much of the tremolo is heard once it is engaged.
    pub fn set_tremolo_mix(&mut self, mix: f32) -> Vec<EffectParam> {
        self.tremolo_mix = mix.clamp(0.0, 1.0);
        vec![EffectParam::TremoloMix(self.tremolo_mix)]
    }

    /// What the tremolo's slot does about the level it returns.
    pub fn set_tremolo_level_mode(
        &mut self,
        mode: crate::music_dsp::LevelMode,
    ) -> Vec<EffectParam> {
        self.tremolo_level_mode = mode;
        vec![EffectParam::TremoloLevelMode(mode)]
    }

    /// The amplitude the tremolo holds under on Ceiling.
    pub fn set_tremolo_ceiling(&mut self, ceiling: f32) -> Vec<EffectParam> {
        self.tremolo_ceiling = ceiling.clamp(0.01, 1.0);
        vec![EffectParam::TremoloCeiling(self.tremolo_ceiling)]
    }

    /// How much of the distortion is heard once it is engaged.
    pub fn set_distortion_mix(&mut self, mix: f32) -> Vec<EffectParam> {
        self.distortion_mix = mix.clamp(0.0, 1.0);
        vec![EffectParam::DistortionMix(self.distortion_mix)]
    }

    /// What the distortion's slot does about the level it returns.
    pub fn set_distortion_level_mode(
        &mut self,
        mode: crate::music_dsp::LevelMode,
    ) -> Vec<EffectParam> {
        self.distortion_level_mode = mode;
        vec![EffectParam::DistortionLevelMode(mode)]
    }

    /// The amplitude the distortion holds under on Ceiling.
    pub fn set_distortion_ceiling(&mut self, ceiling: f32) -> Vec<EffectParam> {
        self.distortion_ceiling = ceiling.clamp(0.01, 1.0);
        vec![EffectParam::DistortionCeiling(self.distortion_ceiling)]
    }

    /// How much of the phaser is heard once it is engaged.
    pub fn set_phaser_mix(&mut self, mix: f32) -> Vec<EffectParam> {
        self.phaser_mix = mix.clamp(0.0, 1.0);
        vec![EffectParam::PhaserMix(self.phaser_mix)]
    }

    /// What the phaser's slot does about the level it returns.
    pub fn set_phaser_level_mode(
        &mut self,
        mode: crate::music_dsp::LevelMode,
    ) -> Vec<EffectParam> {
        self.phaser_level_mode = mode;
        vec![EffectParam::PhaserLevelMode(mode)]
    }

    /// The amplitude the phaser holds under on Ceiling.
    pub fn set_phaser_ceiling(&mut self, ceiling: f32) -> Vec<EffectParam> {
        self.phaser_ceiling = ceiling.clamp(0.01, 1.0);
        vec![EffectParam::PhaserCeiling(self.phaser_ceiling)]
    }

    /// How much of the autopan is heard once it is engaged.
    pub fn set_autopan_mix(&mut self, mix: f32) -> Vec<EffectParam> {
        self.autopan_mix = mix.clamp(0.0, 1.0);
        vec![EffectParam::AutopanMix(self.autopan_mix)]
    }

    /// What the autopan's slot does about the level it returns.
    pub fn set_autopan_level_mode(
        &mut self,
        mode: crate::music_dsp::LevelMode,
    ) -> Vec<EffectParam> {
        self.autopan_level_mode = mode;
        vec![EffectParam::AutopanLevelMode(mode)]
    }

    /// The amplitude the autopan holds under on Ceiling.
    pub fn set_autopan_ceiling(&mut self, ceiling: f32) -> Vec<EffectParam> {
        self.autopan_ceiling = ceiling.clamp(0.01, 1.0);
        vec![EffectParam::AutopanCeiling(self.autopan_ceiling)]
    }

    /// How much of the stereo width is heard once it is engaged.
    pub fn set_stereo_width_mix(&mut self, mix: f32) -> Vec<EffectParam> {
        self.stereo_width_mix = mix.clamp(0.0, 1.0);
        vec![EffectParam::StereoWidthMix(self.stereo_width_mix)]
    }

    /// What the stereo width's slot does about the level it returns.
    pub fn set_stereo_width_level_mode(
        &mut self,
        mode: crate::music_dsp::LevelMode,
    ) -> Vec<EffectParam> {
        self.stereo_width_level_mode = mode;
        vec![EffectParam::StereoWidthLevelMode(mode)]
    }

    /// The amplitude the stereo width holds under on Ceiling.
    pub fn set_stereo_width_ceiling(&mut self, ceiling: f32) -> Vec<EffectParam> {
        self.stereo_width_ceiling = ceiling.clamp(0.01, 1.0);
        vec![EffectParam::StereoWidthCeiling(self.stereo_width_ceiling)]
    }

    /// How much of the plate reverb is heard once it is engaged.
    pub fn set_plate_reverb_mix(&mut self, mix: f32) -> Vec<EffectParam> {
        self.plate_reverb_mix = mix.clamp(0.0, 1.0);
        vec![EffectParam::PlateReverbMix(self.plate_reverb_mix)]
    }

    /// What the plate reverb's slot does about the level it returns.
    pub fn set_plate_reverb_level_mode(
        &mut self,
        mode: crate::music_dsp::LevelMode,
    ) -> Vec<EffectParam> {
        self.plate_reverb_level_mode = mode;
        vec![EffectParam::PlateReverbLevelMode(mode)]
    }

    /// The amplitude the plate reverb holds under on Ceiling.
    pub fn set_plate_reverb_ceiling(&mut self, ceiling: f32) -> Vec<EffectParam> {
        self.plate_reverb_ceiling = ceiling.clamp(0.01, 1.0);
        vec![EffectParam::PlateReverbCeiling(self.plate_reverb_ceiling)]
    }

    /// How much of the ladder filter is heard once it is engaged.
    pub fn set_moog_ladder_mix(&mut self, mix: f32) -> Vec<EffectParam> {
        self.moog_ladder_mix = mix.clamp(0.0, 1.0);
        vec![EffectParam::MoogLadderMix(self.moog_ladder_mix)]
    }

    /// What the ladder filter's slot does about the level it returns.
    pub fn set_moog_ladder_level_mode(
        &mut self,
        mode: crate::music_dsp::LevelMode,
    ) -> Vec<EffectParam> {
        self.moog_ladder_level_mode = mode;
        vec![EffectParam::MoogLadderLevelMode(mode)]
    }

    /// The amplitude the ladder filter holds under on Ceiling.
    pub fn set_moog_ladder_ceiling(&mut self, ceiling: f32) -> Vec<EffectParam> {
        self.moog_ladder_ceiling = ceiling.clamp(0.01, 1.0);
        vec![EffectParam::MoogLadderCeiling(self.moog_ladder_ceiling)]
    }

    /// The policy every effect that has not been pinned follows.
    pub fn set_level_default(&mut self, mode: crate::music_dsp::LevelMode) -> Vec<EffectParam> {
        self.level_default = mode;
        vec![EffectParam::LevelDefault(mode)]
    }

    // ---- stereo width -----------------------------------------------------

    /// The stereo width's on/off switch.
    pub fn toggle_stereo_width(&mut self) -> Vec<EffectParam> {
        self.stereo_width_on = !self.stereo_width_on;
        vec![EffectParam::StereoWidth(self.stereo_width_on)]
    }

    /// Set the stereo width's on/off switch to an explicit value, rather
    /// than flipping whatever it already was -- what a MIX-linked
    /// broadcast needs, the same reason [`Self::set_flanger`] exists.
    pub fn set_stereo_width(&mut self, on: bool) -> Vec<EffectParam> {
        self.stereo_width_on = on;
        vec![EffectParam::StereoWidth(on)]
    }

    /// How far the side signal is scaled: 0 collapses to mono, 1 is the
    /// original image, above 1 widens further.
    pub fn set_stereo_width_amount(&mut self, amount: f32) -> Vec<EffectParam> {
        self.stereo_width_amount = amount.clamp(
            crate::music_dsp::STEREO_WIDTH_MIN,
            crate::music_dsp::STEREO_WIDTH_MAX,
        );
        vec![EffectParam::StereoWidthAmount(self.stereo_width_amount)]
    }

    // ---- plate reverb -----------------------------------------------------

    /// The plate reverb's on/off switch.
    pub fn toggle_plate_reverb(&mut self) -> Vec<EffectParam> {
        self.plate_reverb_on = !self.plate_reverb_on;
        vec![EffectParam::PlateReverb(self.plate_reverb_on)]
    }

    /// Set the plate reverb's on/off switch to an explicit value, rather
    /// than flipping whatever it already was -- what a MIX-linked
    /// broadcast needs, the same reason [`Self::set_flanger`] exists.
    pub fn set_plate_reverb(&mut self, on: bool) -> Vec<EffectParam> {
        self.plate_reverb_on = on;
        vec![EffectParam::PlateReverb(on)]
    }

    /// How long the reverb tank's tail rings.
    pub fn set_plate_reverb_size(&mut self, size: f32) -> Vec<EffectParam> {
        self.plate_reverb_size = size.clamp(
            crate::music_dsp::PLATE_REVERB_SIZE_MIN,
            crate::music_dsp::PLATE_REVERB_SIZE_MAX,
        );
        vec![EffectParam::PlateReverbSize(self.plate_reverb_size)]
    }

    // ---- ladder filter ----------------------------------------------------

    /// The Moog ladder's on/off switch.
    pub fn toggle_moog_ladder(&mut self) -> Vec<EffectParam> {
        self.moog_ladder_on = !self.moog_ladder_on;
        vec![EffectParam::MoogLadder(self.moog_ladder_on)]
    }

    /// Set the Moog ladder's on/off switch to an explicit value, rather
    /// than flipping whatever it already was -- what a MIX-linked
    /// broadcast needs, the same reason [`Self::set_flanger`] exists.
    pub fn set_moog_ladder(&mut self, on: bool) -> Vec<EffectParam> {
        self.moog_ladder_on = on;
        vec![EffectParam::MoogLadder(on)]
    }

    /// Where the ladder starts rolling off, in Hz.
    pub fn set_moog_ladder_cutoff(&mut self, hz: f32) -> Vec<EffectParam> {
        self.moog_ladder_cutoff = hz.clamp(
            crate::music_dsp::MOOG_LADDER_CUTOFF_MIN,
            crate::music_dsp::MOOG_LADDER_CUTOFF_MAX,
        );
        vec![EffectParam::MoogLadderCutoff(self.moog_ladder_cutoff)]
    }

    /// How much of the last stage feeds back into the first.
    pub fn set_moog_ladder_resonance(&mut self, resonance: f32) -> Vec<EffectParam> {
        self.moog_ladder_resonance = resonance.clamp(
            crate::music_dsp::MOOG_LADDER_RESONANCE_MIN,
            crate::music_dsp::MOOG_LADDER_RESONANCE_MAX,
        );
        vec![EffectParam::MoogLadderResonance(self.moog_ladder_resonance)]
    }
}

/// The slots the levels file names, in the order the rack shows them.
/// The words are the file's keys, frozen the way a settings slug is.
pub const LEVEL_SLOTS: [&str; 10] = [
    "echo", "flanger", "bitcrusher", "tremolo", "distortion", "phaser", "autopan",
    "stereo_width", "plate_reverb", "moog_ladder",
];

impl ChainState {
    /// One slot's level policy -- mode, mix, ceiling -- by the file's word
    /// for it. None for a word that is not a slot.
    fn level_trio(&self, slot: &str) -> Option<(LevelMode, f32, f32)> {
        match slot {
            "echo" => Some((self.echo_level_mode, self.echo_mix, self.echo_ceiling)),
            "flanger" => Some((self.flanger_level_mode, self.flanger_mix, self.flanger_ceiling)),
            "bitcrusher" => Some((self.bitcrusher_level_mode, self.bitcrusher_mix, self.bitcrusher_ceiling)),
            "tremolo" => Some((self.tremolo_level_mode, self.tremolo_mix, self.tremolo_ceiling)),
            "distortion" => Some((self.distortion_level_mode, self.distortion_mix, self.distortion_ceiling)),
            "phaser" => Some((self.phaser_level_mode, self.phaser_mix, self.phaser_ceiling)),
            "autopan" => Some((self.autopan_level_mode, self.autopan_mix, self.autopan_ceiling)),
            "stereo_width" => Some((self.stereo_width_level_mode, self.stereo_width_mix, self.stereo_width_ceiling)),
            "plate_reverb" => Some((self.plate_reverb_level_mode, self.plate_reverb_mix, self.plate_reverb_ceiling)),
            "moog_ladder" => Some((self.moog_ladder_level_mode, self.moog_ladder_mix, self.moog_ladder_ceiling)),
            _ => None,
        }
    }

    /// Set one slot's level policy by the file's word for it, and return
    /// what the engine has to be told. A word that is not a slot sets
    /// nothing.
    fn set_level_trio(&mut self, slot: &str, mode: LevelMode, mix: f32, ceiling: f32) -> Vec<EffectParam> {
        let mut params = Vec::new();
        match slot {
            "echo" => {
                params.extend(self.set_echo_level_mode(mode));
                params.extend(self.set_echo_mix(mix));
                params.extend(self.set_echo_ceiling(ceiling));
            }
            "flanger" => {
                params.extend(self.set_flanger_level_mode(mode));
                params.extend(self.set_flanger_mix(mix));
                params.extend(self.set_flanger_ceiling(ceiling));
            }
            "bitcrusher" => {
                params.extend(self.set_bitcrusher_level_mode(mode));
                params.extend(self.set_bitcrusher_mix(mix));
                params.extend(self.set_bitcrusher_ceiling(ceiling));
            }
            "tremolo" => {
                params.extend(self.set_tremolo_level_mode(mode));
                params.extend(self.set_tremolo_mix(mix));
                params.extend(self.set_tremolo_ceiling(ceiling));
            }
            "distortion" => {
                params.extend(self.set_distortion_level_mode(mode));
                params.extend(self.set_distortion_mix(mix));
                params.extend(self.set_distortion_ceiling(ceiling));
            }
            "phaser" => {
                params.extend(self.set_phaser_level_mode(mode));
                params.extend(self.set_phaser_mix(mix));
                params.extend(self.set_phaser_ceiling(ceiling));
            }
            "autopan" => {
                params.extend(self.set_autopan_level_mode(mode));
                params.extend(self.set_autopan_mix(mix));
                params.extend(self.set_autopan_ceiling(ceiling));
            }
            "stereo_width" => {
                params.extend(self.set_stereo_width_level_mode(mode));
                params.extend(self.set_stereo_width_mix(mix));
                params.extend(self.set_stereo_width_ceiling(ceiling));
            }
            "plate_reverb" => {
                params.extend(self.set_plate_reverb_level_mode(mode));
                params.extend(self.set_plate_reverb_mix(mix));
                params.extend(self.set_plate_reverb_ceiling(ceiling));
            }
            "moog_ladder" => {
                params.extend(self.set_moog_ladder_level_mode(mode));
                params.extend(self.set_moog_ladder_mix(mix));
                params.extend(self.set_moog_ladder_ceiling(ceiling));
            }
            _ => {}
        }
        params
    }

    /// This chain's level policies, written under `fxlevel.<tag>.<slot>.<field>`
    /// -- the shape the file has always had, with the deck tags `a` and `b`
    /// and now a tag per target. The panel is a settings surface, and a
    /// setting that does not survive the app is not a setting.
    pub fn write_levels(&self, store: &mut Settings, tag: &str) {
        store.set_usize(&format!("fxlevel.{tag}.all.mode"), self.level_default.as_row() as usize);
        for slot in LEVEL_SLOTS {
            let Some((mode, mix, ceiling)) = self.level_trio(slot) else { continue };
            store.set_usize(&format!("fxlevel.{tag}.{slot}.mode"), mode.as_row() as usize);
            store.set_f64(&format!("fxlevel.{tag}.{slot}.mix"), mix as f64);
            store.set_f64(&format!("fxlevel.{tag}.{slot}.cap"), ceiling as f64);
        }
    }

    /// Read them back. A missing key is the default -- so a file written
    /// when only the decks had a rack carries no answers for the six other
    /// targets and they come up as they always have. Returns what the
    /// engine has to be told.
    pub fn read_levels(&mut self, store: &Settings, tag: &str) -> Vec<EffectParam> {
        let mut params = Vec::new();
        let all = LevelMode::from_row(store.usize(&format!("fxlevel.{tag}.all.mode"), 1) as u32);
        params.extend(self.set_level_default(all));
        for slot in LEVEL_SLOTS {
            let mode = LevelMode::from_row(store.usize(&format!("fxlevel.{tag}.{slot}.mode"), 0) as u32);
            let mix = store.f64(&format!("fxlevel.{tag}.{slot}.mix"), 1.0) as f32;
            let ceiling = store.f64(&format!("fxlevel.{tag}.{slot}.cap"), 1.0) as f32;
            params.extend(self.set_level_trio(slot, mode, mix, ceiling));
        }
        params
    }

    /// Where the bands meet, written as `fx.eq.low_hz` / `fx.eq.high_hz`
    /// -- once, not per chain: the split is the desk's EQ character and
    /// the same on every chain, so whichever chain writes it, writes it
    /// for all.
    pub fn write_crossovers(&self, store: &mut Settings) {
        store.set_f64("fx.eq.low_hz", self.eq_low_hz as f64);
        store.set_f64("fx.eq.high_hz", self.eq_high_hz as f64);
    }

    /// Read them back; a file without them leaves the defaults. Returns
    /// what the engine has to be told.
    pub fn read_crossovers(&mut self, store: &Settings) -> Vec<EffectParam> {
        let low = store.f64("fx.eq.low_hz", crate::music_dsp::EQ_LOW_HZ as f64) as f32;
        let high = store.f64("fx.eq.high_hz", crate::music_dsp::EQ_HIGH_HZ as f64) as f32;
        self.set_crossovers(low, high)
    }
}

#[cfg(test)]
mod levels_file_tests {
    use super::*;

    fn edited() -> ChainState {
        let mut chain = ChainState::default();
        chain.set_level_default(LevelMode::MatchInput);
        chain.set_flanger_level_mode(LevelMode::Ceiling);
        chain.set_flanger_mix(0.4);
        chain.set_flanger_ceiling(0.6);
        chain.set_moog_ladder_mix(0.25);
        chain
    }

    #[test]
    fn write_then_read_levels_is_the_same_chain() {
        let before = edited();
        let mut store = Settings::new();
        before.write_levels(&mut store, "master");
        let text = store.to_text();
        let mut after = ChainState::default();
        let params = after.read_levels(&Settings::from_text(&text), "master");
        assert_eq!(after.level_default, LevelMode::MatchInput);
        assert_eq!(after.flanger_level_mode, LevelMode::Ceiling);
        assert!((after.flanger_mix - 0.4).abs() < 1e-6);
        assert!((after.flanger_ceiling - 0.6).abs() < 1e-6);
        assert!((after.moog_ladder_mix - 0.25).abs() < 1e-6);
        assert_eq!(after.echo_level_mode, LevelMode::Follow, "untouched slots stay put");
        // Everything read is also something the engine was told.
        assert_eq!(params.len(), 1 + 3 * LEVEL_SLOTS.len());
    }

    /// A file written when only the decks had a rack.
    #[test]
    fn a_file_that_knows_only_the_decks_leaves_the_other_six_at_defaults() {
        let mut store = Settings::new();
        edited().write_levels(&mut store, "a");
        edited().write_levels(&mut store, "b");
        let store = Settings::from_text(&store.to_text());
        let mut deck = ChainState::default();
        deck.read_levels(&store, "a");
        assert_eq!(deck.level_default, LevelMode::MatchInput, "the deck reads its own");
        for tag in ["video", "sfx", "piano", "ironfish", "drums", "master"] {
            let mut chain = ChainState::default();
            chain.read_levels(&store, tag);
            let fresh = ChainState::default();
            assert_eq!(chain.level_default, fresh.level_default, "{tag}");
            assert_eq!(chain.flanger_level_mode, fresh.flanger_level_mode, "{tag}");
            assert!((chain.flanger_mix - fresh.flanger_mix).abs() < 1e-6, "{tag}");
        }
    }

    /// The split round-trips, and a file that never heard of it -- every
    /// file written before it was a setting -- leaves the defaults.
    #[test]
    fn write_then_read_crossovers_is_the_same_split() {
        let mut before = ChainState::default();
        before.set_crossovers(320.0, 3_000.0);
        let mut store = Settings::new();
        before.write_crossovers(&mut store);
        let mut after = ChainState::default();
        let params = after.read_crossovers(&Settings::from_text(&store.to_text()));
        assert!((after.eq_low_hz - 320.0).abs() < 1e-3);
        assert!((after.eq_high_hz - 3_000.0).abs() < 1e-3);
        assert_eq!(params.len(), 1, "the engine is told, in one command");
        let mut untouched = ChainState::default();
        untouched.read_crossovers(&Settings::new());
        assert_eq!(untouched.eq_low_hz, crate::music_dsp::EQ_LOW_HZ);
        assert_eq!(untouched.eq_high_hz, crate::music_dsp::EQ_HIGH_HZ);
    }

    /// Every word the file uses names a slot the chain has.
    #[test]
    fn every_level_slot_word_names_a_slot() {
        let chain = ChainState::default();
        for slot in LEVEL_SLOTS {
            assert!(chain.level_trio(slot).is_some(), "{slot}");
        }
        assert!(chain.level_trio("freeze").is_none(), "the freeze has no level row");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The list a load sends is the whole rack, once, in the order the
    /// engine has always had it. Pinned by count and by both ends so a
    /// slot that gains a knob has to come here and say so.
    #[test]
    fn params_says_everything_a_load_sends() {
        let params = ChainState::default().params();
        assert_eq!(params.len(), 70);
        assert_eq!(params.first(), Some(&EffectParam::Echo(None)));
        assert_eq!(
            params.last(),
            Some(&EffectParam::MoogLadderResonance(
                crate::music_dsp::MOOG_LADDER_RESONANCE_DEFAULT
            ))
        );
    }

    #[test]
    fn echo_feedback_clamps_to_the_mixers_own_ceiling() {
        let mut c = ChainState::default();
        assert_eq!(
            c.set_echo_feedback(5.0),
            vec![EffectParam::EchoFeedback(crate::music_dsp::ECHO_FEEDBACK_MAX)]
        );
        assert_eq!(c.set_echo_feedback(-1.0), vec![EffectParam::EchoFeedback(0.0)]);
        assert_eq!(c.echo_feedback, 0.0);
    }

    #[test]
    fn flanger_rate_and_feedback_clamp_to_their_documented_ranges() {
        let mut c = ChainState::default();
        assert_eq!(
            c.set_flanger_rate(100.0),
            vec![EffectParam::FlangerRate(crate::music_dsp::FLANGER_RATE_MAX)]
        );
        assert_eq!(
            c.set_flanger_rate(-1.0),
            vec![EffectParam::FlangerRate(crate::music_dsp::FLANGER_RATE_MIN)]
        );
        assert_eq!(
            c.set_flanger_feedback(5.0),
            vec![EffectParam::FlangerFeedback(crate::music_dsp::FLANGER_FEEDBACK_MAX)]
        );
        assert_eq!(c.set_flanger_depth(5.0), vec![EffectParam::FlangerDepth(1.0)]);
    }

    #[test]
    fn toggle_flanger_flips_and_set_flanger_lands_on_an_explicit_side() {
        let mut c = ChainState::default();
        assert!(!c.flanger_on);
        assert_eq!(c.toggle_flanger(), vec![EffectParam::Flanger(true)]);
        assert!(c.flanger_on);
        // A MIX broadcast lands both decks on the SAME explicit side
        // rather than each toggling its own -- set_flanger is what that
        // needs, distinct from the single-deck toggle.
        let mut other = ChainState::default();
        assert_eq!(other.set_flanger(true), vec![EffectParam::Flanger(true)]);
        assert!(other.flanger_on);
    }

    #[test]
    fn bitcrusher_rate_and_bits_clamp_to_their_documented_ranges() {
        let mut c = ChainState::default();
        assert_eq!(
            c.set_bitcrusher_rate(100_000.0),
            vec![EffectParam::BitcrusherRate(crate::music_dsp::BITCRUSHER_RATE_MAX)]
        );
        assert_eq!(
            c.set_bitcrusher_rate(-1.0),
            vec![EffectParam::BitcrusherRate(crate::music_dsp::BITCRUSHER_RATE_MIN)]
        );
        assert_eq!(
            c.set_bitcrusher_bits(100.0),
            vec![EffectParam::BitcrusherBits(crate::music_dsp::BITCRUSHER_BITS_MAX)]
        );
        assert_eq!(
            c.set_bitcrusher_bits(-1.0),
            vec![EffectParam::BitcrusherBits(crate::music_dsp::BITCRUSHER_BITS_MIN)]
        );
    }

    #[test]
    fn toggle_bitcrusher_flips_and_set_bitcrusher_lands_on_an_explicit_side() {
        let mut c = ChainState::default();
        assert!(!c.bitcrusher_on);
        assert_eq!(c.toggle_bitcrusher(), vec![EffectParam::Bitcrusher(true)]);
        assert!(c.bitcrusher_on);
        let mut other = ChainState::default();
        assert_eq!(other.set_bitcrusher(true), vec![EffectParam::Bitcrusher(true)]);
        assert!(other.bitcrusher_on);
    }

    #[test]
    fn tremolo_rate_and_depth_clamp_to_their_documented_ranges() {
        let mut c = ChainState::default();
        assert_eq!(
            c.set_tremolo_rate(1_000.0),
            vec![EffectParam::TremoloRate(crate::music_dsp::TREMOLO_RATE_MAX)]
        );
        assert_eq!(
            c.set_tremolo_rate(-1.0),
            vec![EffectParam::TremoloRate(crate::music_dsp::TREMOLO_RATE_MIN)]
        );
        assert_eq!(c.set_tremolo_depth(5.0), vec![EffectParam::TremoloDepth(1.0)]);
        assert_eq!(c.set_tremolo_depth(-5.0), vec![EffectParam::TremoloDepth(0.0)]);
    }

    #[test]
    fn toggle_tremolo_flips_and_set_tremolo_lands_on_an_explicit_side() {
        let mut c = ChainState::default();
        assert!(!c.tremolo_on);
        assert_eq!(c.toggle_tremolo(), vec![EffectParam::Tremolo(true)]);
        assert!(c.tremolo_on);
        let mut other = ChainState::default();
        assert_eq!(other.set_tremolo(true), vec![EffectParam::Tremolo(true)]);
        assert!(other.tremolo_on);
    }

    #[test]
    fn distortion_drive_clamps_to_its_documented_range() {
        let mut c = ChainState::default();
        assert_eq!(
            c.set_distortion_drive(1_000.0),
            vec![EffectParam::DistortionDrive(crate::music_dsp::DISTORTION_DRIVE_MAX)]
        );
        assert_eq!(
            c.set_distortion_drive(-1.0),
            vec![EffectParam::DistortionDrive(crate::music_dsp::DISTORTION_DRIVE_MIN)]
        );
    }

    #[test]
    fn toggle_distortion_flips_and_set_distortion_lands_on_an_explicit_side() {
        let mut c = ChainState::default();
        assert!(!c.distortion_on);
        assert_eq!(c.toggle_distortion(), vec![EffectParam::Distortion(true)]);
        assert!(c.distortion_on);
        let mut other = ChainState::default();
        assert_eq!(other.set_distortion(true), vec![EffectParam::Distortion(true)]);
        assert!(other.distortion_on);
    }

    #[test]
    fn phaser_rate_and_feedback_clamp_to_their_documented_ranges() {
        let mut c = ChainState::default();
        assert_eq!(
            c.set_phaser_rate(1_000.0),
            vec![EffectParam::PhaserRate(crate::music_dsp::PHASER_RATE_MAX)]
        );
        assert_eq!(
            c.set_phaser_rate(-1.0),
            vec![EffectParam::PhaserRate(crate::music_dsp::PHASER_RATE_MIN)]
        );
        assert_eq!(
            c.set_phaser_feedback(5.0),
            vec![EffectParam::PhaserFeedback(crate::music_dsp::PHASER_FEEDBACK_MAX)]
        );
        assert_eq!(c.set_phaser_feedback(-5.0), vec![EffectParam::PhaserFeedback(0.0)]);
    }

    #[test]
    fn toggle_phaser_flips_and_set_phaser_lands_on_an_explicit_side() {
        let mut c = ChainState::default();
        assert!(!c.phaser_on);
        assert_eq!(c.toggle_phaser(), vec![EffectParam::Phaser(true)]);
        assert!(c.phaser_on);
        let mut other = ChainState::default();
        assert_eq!(other.set_phaser(true), vec![EffectParam::Phaser(true)]);
        assert!(other.phaser_on);
    }

    #[test]
    fn autopan_rate_clamps_to_its_documented_range() {
        let mut c = ChainState::default();
        assert_eq!(
            c.set_autopan_rate(1_000.0),
            vec![EffectParam::AutopanRate(crate::music_dsp::AUTOPAN_RATE_MAX)]
        );
        assert_eq!(
            c.set_autopan_rate(-1.0),
            vec![EffectParam::AutopanRate(crate::music_dsp::AUTOPAN_RATE_MIN)]
        );
    }

    #[test]
    fn toggle_autopan_flips_and_set_autopan_lands_on_an_explicit_side() {
        let mut c = ChainState::default();
        assert!(!c.autopan_on);
        assert_eq!(c.toggle_autopan(), vec![EffectParam::Autopan(true)]);
        assert!(c.autopan_on);
        let mut other = ChainState::default();
        assert_eq!(other.set_autopan(true), vec![EffectParam::Autopan(true)]);
        assert!(other.autopan_on);
    }

    #[test]
    fn stereo_width_amount_clamps_to_its_documented_range() {
        let mut c = ChainState::default();
        assert_eq!(
            c.set_stereo_width_amount(10.0),
            vec![EffectParam::StereoWidthAmount(crate::music_dsp::STEREO_WIDTH_MAX)]
        );
        assert_eq!(
            c.set_stereo_width_amount(-1.0),
            vec![EffectParam::StereoWidthAmount(crate::music_dsp::STEREO_WIDTH_MIN)]
        );
    }

    #[test]
    fn toggle_stereo_width_flips_and_set_stereo_width_lands_on_an_explicit_side() {
        let mut c = ChainState::default();
        assert!(!c.stereo_width_on);
        assert_eq!(c.toggle_stereo_width(), vec![EffectParam::StereoWidth(true)]);
        assert!(c.stereo_width_on);
        let mut other = ChainState::default();
        assert_eq!(other.set_stereo_width(true), vec![EffectParam::StereoWidth(true)]);
        assert!(other.stereo_width_on);
    }

    #[test]
    fn plate_reverb_size_clamps_to_its_documented_range() {
        let mut c = ChainState::default();
        assert_eq!(
            c.set_plate_reverb_size(10.0),
            vec![EffectParam::PlateReverbSize(crate::music_dsp::PLATE_REVERB_SIZE_MAX)]
        );
        assert_eq!(
            c.set_plate_reverb_size(-1.0),
            vec![EffectParam::PlateReverbSize(crate::music_dsp::PLATE_REVERB_SIZE_MIN)]
        );
    }

    #[test]
    fn toggle_plate_reverb_flips_and_set_plate_reverb_lands_on_an_explicit_side() {
        let mut c = ChainState::default();
        assert!(!c.plate_reverb_on);
        assert_eq!(c.toggle_plate_reverb(), vec![EffectParam::PlateReverb(true)]);
        assert!(c.plate_reverb_on);
        let mut other = ChainState::default();
        assert_eq!(other.set_plate_reverb(true), vec![EffectParam::PlateReverb(true)]);
        assert!(other.plate_reverb_on);
    }

    #[test]
    fn moog_ladder_cutoff_and_resonance_clamp_to_their_documented_ranges() {
        let mut c = ChainState::default();
        assert_eq!(
            c.set_moog_ladder_cutoff(50_000.0),
            vec![EffectParam::MoogLadderCutoff(crate::music_dsp::MOOG_LADDER_CUTOFF_MAX)]
        );
        assert_eq!(
            c.set_moog_ladder_cutoff(-1.0),
            vec![EffectParam::MoogLadderCutoff(crate::music_dsp::MOOG_LADDER_CUTOFF_MIN)]
        );
        assert_eq!(
            c.set_moog_ladder_resonance(5.0),
            vec![EffectParam::MoogLadderResonance(
                crate::music_dsp::MOOG_LADDER_RESONANCE_MAX
            )]
        );
        assert_eq!(
            c.set_moog_ladder_resonance(-5.0),
            vec![EffectParam::MoogLadderResonance(
                crate::music_dsp::MOOG_LADDER_RESONANCE_MIN
            )]
        );
    }

    #[test]
    fn toggle_moog_ladder_flips_and_set_moog_ladder_lands_on_an_explicit_side() {
        let mut c = ChainState::default();
        assert!(!c.moog_ladder_on);
        assert_eq!(c.toggle_moog_ladder(), vec![EffectParam::MoogLadder(true)]);
        assert!(c.moog_ladder_on);
        let mut other = ChainState::default();
        assert_eq!(other.set_moog_ladder(true), vec![EffectParam::MoogLadder(true)]);
        assert!(other.moog_ladder_on);
    }

    /// The corners an operator SEES are the corners the engine RUNS.
    /// They go through one rule, so a push that moves the untouched one
    /// moves it in the stored state too -- a readout that disagreed with
    /// the audio would be worse than no readout.
    #[test]
    fn the_stored_crossovers_are_the_ones_the_engine_keeps() {
        let mut c = ChainState::default();
        // Pushed together: the state has to show the gap the engine keeps.
        let params = c.set_crossovers(800.0, 1_000.0);
        assert!(
            c.eq_high_hz / c.eq_low_hz >= 2.0 - 1e-3,
            "stored {} and {} are too close",
            c.eq_low_hz,
            c.eq_high_hz
        );
        // And the parameter carries exactly what was stored.
        assert_eq!(params, vec![EffectParam::Crossovers(c.eq_low_hz, c.eq_high_hz)]);
        // A value that means nothing moves neither corner.
        let before = (c.eq_low_hz, c.eq_high_hz);
        assert!(c.set_crossovers(f32::NAN, 2_000.0).is_empty());
        assert_eq!((c.eq_low_hz, c.eq_high_hz), before);
    }
}
