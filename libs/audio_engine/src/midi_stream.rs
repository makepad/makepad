//! A `Loop` played from MIDI in: its ring of messages (from
//! [`crate::feed::midi_in`]), the note stream it passes them through
//! (`makepad_score_player::stream`) and where each message lands in a pass.
//!
//! Timing: a message is stamped with when it arrived (seconds on
//! [`crate::clock`]). The node keeps an estimate of the clock time of each
//! pass's first frame (the passes' frames counted, pulled slowly toward
//! the clock at each pass) and plays a message `latency` after it arrived,
//! on its frame; one that comes later than that plays at the start of the
//! pass it is read in. So the spacing of what a player plays is kept to the
//! sample, at a fixed delay of `latency`.
//!
//! While the transport is stopped the stream runs on a free clock at the
//! transport's tempo (a keyboard plays, an arp keeps stepping), going on
//! from the beat the transport stopped at. Nothing here allocates.

use crate::feed::midi_in::MidiMsg;
use crate::feed::streams::StreamView;
use crate::schedule::ClockChunk;
use makepad_flow::live::spsc::Consumer;
use makepad_score_player::loop_engine::{Inst, LoopEngine, Tempo};
use makepad_score_player::stream::{Ctl, Msg, Stream, StreamDef};
use std::sync::Arc;

/// Messages read ahead of their pass that a node keeps.
pub const WAITING: usize = 256;
/// Controllers one pass applies.
const CONTROLS: usize = 64;
/// How far the pass-time estimate may be from the clock before it jumps
/// (a device that stalled or restarted), in seconds.
const RESYNC: f64 = 0.1;
/// How much of its distance to the clock the estimate moves each pass.
const SLEW: f64 = 0.05;

pub struct StreamRun {
    pub stream: Stream,
    pub midi: Option<Consumer<MidiMsg>>,
    waiting: Box<[MidiMsg]>,
    nwaiting: usize,
    /// The clock time of the next pass's first frame (NaN: not yet known).
    t_next: f64,
    /// Seconds after arrival a message plays.
    pub latency: f64,
    /// Where the free clock is (the beat the last pass ended on).
    beat: f64,
    pub view: Option<Arc<StreamView>>,
    /// Messages dropped for want of room.
    pub dropped: u32,
}

impl StreamRun {
    /// Off the audio thread.
    pub fn new(def: StreamDef, midi: Option<Consumer<MidiMsg>>, latency: f64, view: Option<Arc<StreamView>>) -> StreamRun {
        StreamRun {
            stream: Stream::new(def),
            midi,
            waiting: vec![MidiMsg { at: 0.0, port: 0, data: [0; 3] }; WAITING].into_boxed_slice(),
            nwaiting: 0,
            t_next: f64::NAN,
            latency: latency.clamp(0.0, 0.25),
            beat: 0.0,
            view,
            dropped: 0,
        }
    }

    /// A new revision of the loop: `old`'s keys, arps, waiting messages and
    /// clocks go on here (this one's settings and ring). No allocation.
    pub fn take_state(&mut self, old: &mut StreamRun) {
        self.stream.take_state(&mut old.stream);
        for i in 0..old.nwaiting.min(WAITING) {
            self.waiting[i] = old.waiting[i];
        }
        self.nwaiting = old.nwaiting;
        old.nwaiting = 0;
        // Messages still in the old ring come along.
        if let Some(midi) = &mut old.midi {
            while let Some(msg) = midi.pop() {
                self.wait(msg);
            }
        }
        self.t_next = old.t_next;
        self.beat = old.beat;
    }

    /// Something to play or take (a schedule holding it does not rest).
    pub fn busy(&self) -> bool {
        self.nwaiting > 0 || self.midi.as_ref().is_some_and(|m| !m.is_empty()) || self.stream.active()
    }

    /// Something arrived (wakes a resting schedule).
    pub fn has_input(&self) -> bool {
        self.nwaiting > 0 || self.midi.as_ref().is_some_and(|m| !m.is_empty())
    }

    fn wait(&mut self, msg: MidiMsg) {
        if self.nwaiting >= WAITING {
            self.dropped += 1;
            return;
        }
        let at = self.waiting[..self.nwaiting].partition_point(|m| m.at <= msg.at);
        self.waiting.copy_within(at..self.nwaiting, at + 1);
        self.waiting[at] = msg;
        self.nwaiting += 1;
    }

    /// The tempo the loop renders this pass on: the transport's while it
    /// plays, else the free clock at its tempo.
    pub fn tempo(&self, chunk: &ClockChunk, rate: f32) -> Tempo {
        if chunk.playing {
            chunk.tempo
        } else {
            let bpm = if chunk.bpm.is_finite() && chunk.bpm > 0.0 { chunk.bpm as f64 } else { 120.0 };
            Tempo::linear(self.beat, bpm / 60.0 / rate as f64)
        }
    }

    /// One pass of `frames` at `rate` on `tempo` (from [`Self::tempo`]),
    /// `now` the clock: the messages due in it through the stream into
    /// `engine` (notes) and onto `inst` (controllers).
    pub fn run(&mut self, engine: &mut LoopEngine, inst: &Inst, tempo: &Tempo, frames: usize, rate: f32, now: f64) {
        let rate_f = rate as f64;
        let span = frames as f64 / rate_f;
        // The clock time of this pass's first frame.
        let t0 = if !self.t_next.is_finite() || (now - self.t_next).abs() > RESYNC { now } else { self.t_next + (now - self.t_next) * SLEW };
        self.t_next = t0 + span;
        if let Some(midi) = self.midi.as_mut() {
            while let Some(msg) = midi.pop() {
                if self.nwaiting >= WAITING {
                    self.dropped += 1;
                    continue;
                }
                let at = self.waiting[..self.nwaiting].partition_point(|m| m.at <= msg.at);
                self.waiting.copy_within(at..self.nwaiting, at + 1);
                self.waiting[at] = msg;
                self.nwaiting += 1;
            }
        }
        // Due in this pass: on their frame (late ones on the first).
        let mut taken = 0;
        while taken < self.nwaiting {
            let msg = self.waiting[taken];
            let due = msg.at + self.latency;
            if due >= t0 + span {
                break;
            }
            taken += 1;
            let frame = if due.is_finite() { ((due - t0) * rate_f).floor().clamp(0.0, (frames - 1) as f64) } else { 0.0 };
            if let Some(m) = Msg::from_midi(msg.data, tempo.beat_at(frame)) {
                self.stream.push(m);
            }
        }
        self.waiting.copy_within(taken..self.nwaiting, 0);
        self.nwaiting -= taken;
        let (b0, b1) = (tempo.beat_at(0.0), tempo.end_beat(frames));
        let def = self.stream.def;
        let mut controls = [(0u8, Ctl::Mod, 0.0f32); CONTROLS];
        let mut ncontrols = 0;
        let key = engine.key;
        self.stream.run(b0, b1, key, 0.0, &mut |event| engine.push(event), &mut |ch, ctl, value| {
            if ncontrols < CONTROLS {
                controls[ncontrols] = (ch, ctl, value);
                ncontrols += 1;
            }
        });
        for (ch, ctl, value) in &controls[..ncontrols] {
            engine.control(inst, *ch, *ctl, *value, def.bend_range, def.mpe);
        }
        self.beat = b1;
        if let Some(view) = &self.view {
            let (recent, started) = self.stream.recent();
            view.store(self.stream.held(), self.stream.sounding(), self.stream.arp_steps(), recent, started);
        }
    }

    /// The loop stops (its transport, a mute that lets go): what sounds ends.
    pub fn release_all(&mut self) {
        self.stream.release_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_flow::live::spsc::ring;
    use makepad_score_player::loop_engine::Inst;

    /// A message plays `latency` after it arrived, on its sample, whatever
    /// pass reads it; one read late plays at the start of its pass.
    #[test]
    fn messages_land_on_their_sample_after_the_latency() {
        let (mut send, recv) = ring(64);
        let mut run = StreamRun::new(StreamDef::default(), Some(recv), 0.010, None);
        let inst = Inst::Midi { key: 1, channel: 0, port: 0 };
        let mut engine = LoopEngine::for_inst(1, &inst, 48_000.0, false).unwrap();
        let tempo = |b0: f64| Tempo::linear(b0, 2.0 / 48_000.0);
        let (mut l, mut r) = ([0.0f32; 256], [0.0f32; 256]);
        let mut pass = |run: &mut StreamRun, engine: &mut LoopEngine, k: usize, now: f64| {
            let t = tempo(k as f64 * 256.0 * 2.0 / 48_000.0);
            run.run(engine, &inst, &t, 256, 48_000.0, now);
            engine.render(&inst, &mut l, &mut r, 48_000.0, &t);
            let mut sent = Vec::new();
            engine.take_midi(|frame, bytes| sent.push((frame, bytes[0] & 0xf0)));
            sent
        };
        let _ = send.push(MidiMsg { at: 1.000, port: 0, data: [0x90, 60, 100] });
        let _ = send.push(MidiMsg { at: 1.0025, port: 0, data: [0x80, 60, 0] });
        // Pass 0 starts at 1.005 s: the key is due at 1.010, frame 240.
        assert_eq!(pass(&mut run, &mut engine, 0, 1.005), [(240, 0x90)]);
        // Pass 1 starts where pass 0 ended (the clock read a little late
        // moves it only a little): its release is due at 1.0125.
        let sent = pass(&mut run, &mut engine, 1, 1.0110);
        assert_eq!(sent.len(), 1);
        let want = ((1.0125 - (1.005 + 256.0 / 48_000.0 + (1.0110 - 1.005 - 256.0 / 48_000.0) * SLEW)) * 48_000.0) as u16;
        assert_eq!(sent[0], (want, 0x80));
        // Read long after it was due: the start of the pass.
        let _ = send.push(MidiMsg { at: 1.0, port: 0, data: [0x90, 62, 100] });
        assert_eq!(pass(&mut run, &mut engine, 2, 1.0160), [(0, 0x90)]);
    }
}
