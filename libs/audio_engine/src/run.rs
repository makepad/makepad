//! One pass over a prepared schedule: every op in topological order, each
//! reading its inputs' spans and writing its own, with wired controls taking
//! their producer's sample first. Nothing here allocates, locks or frees.

use crate::schedule::{ClockChunk, ControlBank, NodeState, Op, Schedule, SPAN};
use crate::CHUNK;
use makepad_audio_dsp::svf::SvfCoeffs;

/// Per-pass decay of a peak reading: about 300 ms to fall 20 dB at 48 kHz.
pub const PEAK_DECAY: f32 = 0.96;

pub fn spans(arena: &mut [f32], input: Option<u16>, out: u16) -> (Option<&[f32]>, &mut [f32]) {
    let (head, tail) = arena.split_at_mut(out as usize * SPAN);
    (input.map(|input| &head[input as usize * SPAN..][..SPAN]), &mut tail[..SPAN])
}

/// An extension's ops at render time (Stage's decks).
pub trait RunExt<X, Y> {
    #[allow(clippy::too_many_arguments)]
    fn run(&mut self, op: &Y, states: &mut [NodeState<X>], arena: &mut [f32], controls: &ControlBank, probes: &mut [f32], rate: f32, frames: usize);

    /// Whether this op's source is running (a deck playing, a live feed
    /// open): a schedule with one running never rests.
    fn running(&self, op: &Y, states: &[NodeState<X>], controls: &ControlBank) -> bool;
}

/// No extension: a schedule of the engine's own nodes.
pub struct NoExt;

impl<X, Y> RunExt<X, Y> for NoExt {
    fn run(&mut self, _op: &Y, _states: &mut [NodeState<X>], _arena: &mut [f32], _controls: &ControlBank, _probes: &mut [f32], _rate: f32, _frames: usize) {}
    fn running(&self, _op: &Y, _states: &[NodeState<X>], _controls: &ControlBank) -> bool {
        false
    }
}

/// Before a pass: whether the schedule rests and the pass is skipped. A
/// resting schedule wakes (and this returns false) when a control moved or a
/// feed has something waiting or a source of the owner runs. See [`crate::rest`].
pub fn rests<X, Y, B>(schedule: &mut Schedule<X, Y, B>, ext: &dyn RunExt<X, Y>) -> bool {
    let Schedule { ops, states, controls, rest, .. } = schedule;
    rest.skip_pass(
        |snapshot| controls.moved_since(snapshot),
        || ops.iter().any(|op| match op {
            Op::Transport { state, .. } => matches!(&states[*state as usize], NodeState::Transport(this) if this.feed.as_ref().is_some_and(|feed| !feed.is_empty())),
            Op::Loop { state, .. } => matches!(&states[*state as usize], NodeState::Loop(this) if this.feed.as_ref().is_some_and(|feed| !feed.is_empty())),
            Op::Pads { state, .. } => matches!(&states[*state as usize], NodeState::Pads(this) if this.feed.as_ref().is_some_and(|feed| !feed.is_empty())),
            Op::Fx { state, .. } => matches!(&states[*state as usize], NodeState::Fx(this) if this.feed.as_ref().is_some_and(|feed| !feed.is_empty())),
            // A source that started running (a clip loaded on a deck that
            // is set to play) wakes it as a control does.
            Op::Ext(op) => ext.running(op, states, controls),
            Op::Nested { state, .. } => matches!(&states[*state as usize], NodeState::Nested(this) if this.running()),
            _ => false,
        }),
    )
}

/// After a pass: `peak` is the loudest sample the schedule sent to its
/// outputs in it. Returns whether the schedule goes to rest now.
pub fn settle_rest<X, Y, B>(schedule: &mut Schedule<X, Y, B>, peak: f32, frames: usize, ext: &dyn RunExt<X, Y>) -> bool {
    let Schedule { ops, states, controls, rest, rate, .. } = schedule;
    let rate = *rate as f64;
    rest.after_pass(
        peak,
        frames,
        || {
            // Silence between two echoes can last as long as the longest
            // delay line (or an unknown shader) in the graph.
            let echoes = ops.iter().any(|op| matches!(op, Op::Fx { kind: makepad_score_player::FxKind::Delay, .. } | Op::ShaderFx { .. }));
            ((if echoes { crate::rest::ECHO_HOLD_SECONDS } else { crate::rest::HOLD_SECONDS }) * rate) as u64
        },
        || ops.iter().all(|op| match op {
            Op::Transport { state, playing, .. } => {
                controls.get(*playing) < 0.5 && matches!(&states[*state as usize], NodeState::Transport(this) if !this.playing)
            }
            Op::Loop { state, mute, .. } => match &states[*state as usize] {
                // A live input counts while it is open and not muted.
                NodeState::Loop(this) => this.playing_clear() && !(this.input.is_some() && controls.get(*mute) < 0.5),
                _ => true,
            },
            // A shader FX with nothing wired in makes sound of its own.
            Op::ShaderFx { inputs, .. } => !inputs.is_empty(),
            Op::Ext(op) => !ext.running(op, states, controls),
            Op::Nested { state, .. } => !matches!(&states[*state as usize], NodeState::Nested(this) if this.running()),
            _ => true,
        }),
        |snapshot| controls.snapshot(snapshot),
    )
}

/// One pass of `frames` frames (at most [`CHUNK`]) over a schedule. `sink`
/// receives what each output node sends to the device, after its gate.
/// `sink(cue, left, right)`: each output's gated chunk, `cue` for a
/// CueOutput (the headphone cue), else the device's program.
pub fn run<X, Y, B>(schedule: &mut Schedule<X, Y, B>, frames: usize, sink: &mut dyn FnMut(bool, &[f32], &[f32]), ext: &mut dyn RunExt<X, Y>) {
    schedule.last_frames = frames as u16;
    let Schedule { ops, states, arena, clocks, probes, controls, routes, rate, .. } = schedule;
    let rate = *rate as f32;
    let mut route = 0;
    // A soloed loop silences every loop that is not.
    let any_solo = ops.iter().any(|op| matches!(op, Op::Loop { solo, mute, .. } if controls.get(*solo) >= 0.5 && controls.get(*mute) < 0.5));
    for (index, op) in ops.iter().enumerate() {
        // Wired controls take this pass's sample of their producer, which
        // ran earlier in topological order.
        while let Some(next) = routes.get(route).filter(|next| next.op as usize == index) {
            next.apply(probes, controls);
            route += 1;
        }
        match op {
            Op::Transport { state, clock, bpm, playing, probe, mapped } => {
                let NodeState::Transport(this) = &mut states[*state as usize] else { continue };
                let bpm_control = *bpm;
                let bpm = controls.get(*bpm);
                let playing = controls.get(*playing) >= 0.5;
                if let Some(feed) = &mut this.feed {
                    while let Some(event) = feed.pop() {
                        if this.pending.len() < this.pending.capacity() {
                            let at = this.pending.partition_point(|p| p.beat <= event.beat);
                            this.pending.insert(at, event);
                        }
                    }
                }
                let origin = this.frames as f64;
                // The control (or starting and stopping) re-anchors at the
                // beat reached; a changed control sets the tempo.
                if bpm.to_bits() != this.bpm.to_bits() || playing != this.playing {
                    this.anchor = this.beat;
                    this.anchor_frame = origin;
                    if bpm.to_bits() != this.bpm.to_bits() {
                        this.tempo = bpm as f64;
                        this.ramp = None;
                        this.mapped = false;
                    }
                    this.bpm = bpm;
                    this.playing = playing;
                }
                let per = |tempo: f64| if playing { tempo / 60.0 / rate as f64 } else { 0.0 };
                let mut tempo = makepad_score_player::Tempo::linear(this.beat, per(this.tempo));
                if playing {
                    // Tempo changes whose beat falls in this pass split it.
                    loop {
                        let ramp_next = this.ramp.map(|(start, _, _, steps, next)| (start + next as f64 / 16.0, next < steps));
                        let event_next = this.pending.first().map(|e| e.beat);
                        let next = match (ramp_next, event_next) {
                            (Some((r, true)), Some(e)) => r.min(e),
                            (Some((r, true)), None) => r,
                            (_, Some(e)) => e,
                            _ => break,
                        };
                        let per_now = per(this.tempo);
                        let frame = (this.anchor_frame - origin) + (next - this.anchor) / per_now;
                        if frame >= frames as f64 {
                            break;
                        }
                        let frame = frame.max(0.0);
                        let at_beat = next.max(this.anchor + (origin + frame - this.anchor_frame) * per_now);
                        if event_next == Some(next) {
                            let event = this.pending.remove(0);
                            if event.ramp > 0.0 {
                                let steps = (event.ramp * 16.0).round().max(1.0) as u32;
                                this.ramp = Some((event.beat, this.tempo, event.bpm, steps, 0));
                            } else {
                                this.ramp = None;
                                this.tempo = event.bpm;
                            }
                        }
                        if let Some((start, from, to, steps, next_step)) = this.ramp {
                            if start + next_step as f64 / 16.0 <= next + 1e-12 && next_step < steps {
                                this.tempo = from + (to - from) * (next_step + 1) as f64 / steps as f64;
                                this.ramp = if next_step + 1 < steps { Some((start, from, to, steps, next_step + 1)) } else { None };
                            }
                        }
                        this.tempo = this.tempo.clamp(crate::feed::TEMPO_MIN, crate::feed::TEMPO_MAX);
                        // The map's tempo is what the bpm control shows now
                        // (taken as seen, so it is not a new setting).
                        this.mapped = true;
                        this.bpm = this.tempo as f32;
                        controls.set_manual(bpm_control, this.bpm);
                        this.anchor = at_beat;
                        this.anchor_frame = origin + frame;
                        let n = tempo.nsplits as usize;
                        if n < tempo.splits.len() {
                            tempo.splits[n] = (at_beat, frame, per(this.tempo));
                            tempo.nsplits += 1;
                        } else {
                            // More changes than a pass holds: the rest from
                            // here keep their beats, a pass later.
                            break;
                        }
                    }
                }
                let per_frame = per(this.tempo);
                clocks[*clock as usize] = ClockChunk { start: this.beat, per_frame: tempo.per_frame, playing, bpm: this.tempo as f32, tempo };
                this.frames += frames as u64;
                this.beat = this.anchor + (this.frames as f64 - this.anchor_frame) * per_frame;
                probes[*probe as usize] = this.beat as f32;
                probes[*mapped as usize] = if this.mapped { 1.0 } else { 0.0 };
            }
            Op::Filter { state, input, out, cutoff, resonance } => {
                let NodeState::Filter { left, right, cutoff: cutoff_now, q: q_now } = &mut states[*state as usize] else { continue };
                let (input, span) = spans(arena, *input, *out);
                let Some(input) = input else {
                    span.fill(0.0);
                    continue;
                };
                let (cutoff_target, q_target) = (controls.get(*cutoff), controls.get(*resonance));
                // Coefficients follow the control in steps of 32 frames.
                let mut frame = 0;
                while frame < frames {
                    let count = (frames - frame).min(32);
                    *cutoff_now += (cutoff_target - *cutoff_now) * 0.25;
                    *q_now += (q_target - *q_now) * 0.25;
                    let coefficients = SvfCoeffs::new(*cutoff_now, *q_now, rate);
                    for at in frame..frame + count {
                        span[at] = left.process(&coefficients, input[at]).0;
                        span[CHUNK + at] = right.process(&coefficients, input[CHUNK + at]).0;
                    }
                    frame += count;
                }
            }
            Op::Gain { state, input, out, gain, pan, mute, probe } => {
                let NodeState::Gain { left, right, peak } = &mut states[*state as usize] else { continue };
                let (input, span) = spans(arena, *input, *out);
                let Some(input) = input else {
                    span.fill(0.0);
                    *peak = 0.0;
                    probes[*probe as usize] = 0.0;
                    continue;
                };
                let gain = if controls.get(*mute) >= 0.5 { 0.0 } else { controls.get(*gain) };
                let angle = (controls.get(*pan).clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
                let (to_left, to_right) = (gain * angle.cos() * std::f32::consts::SQRT_2, gain * angle.sin() * std::f32::consts::SQRT_2);
                let (step_left, step_right) = ((to_left - *left) / frames as f32, (to_right - *right) / frames as f32);
                // Hold short transients across callback chunks, with the
                // same decay as the output meter, before publishing to UI.
                *peak *= PEAK_DECAY;
                for frame in 0..frames {
                    *left += step_left;
                    *right += step_right;
                    span[frame] = input[frame] * *left;
                    span[CHUNK + frame] = input[CHUNK + frame] * *right;
                    *peak = peak.max(span[frame].abs()).max(span[CHUNK + frame].abs());
                }
                probes[*probe as usize] = *peak;
            }
            Op::Mixer { state, inputs, out, master } => {
                let NodeState::Mixer { master: now } = &mut states[*state as usize] else { continue };
                let (head, tail) = arena.split_at_mut(*out as usize * SPAN);
                let span = &mut tail[..SPAN];
                span.fill(0.0);
                for input in inputs.iter() {
                    let input = &head[*input as usize * SPAN..][..SPAN];
                    for frame in 0..frames {
                        span[frame] += input[frame];
                        span[CHUNK + frame] += input[CHUNK + frame];
                    }
                }
                let step = (controls.get(*master) - *now) / frames as f32;
                for frame in 0..frames {
                    *now += step;
                    span[frame] *= *now;
                    span[CHUNK + frame] *= *now;
                }
            }
            Op::StemMixer { state, input, stems, available, out, gains, mutes, solos } => {
                let NodeState::StemMixer { gains: now } = &mut states[*state as usize] else { continue };
                let any_solo = solos.iter().any(|solo| controls.get(*solo) >= 0.5);
                let targets: [f32; 4] = std::array::from_fn(|lane| {
                    if controls.get(mutes[lane]) >= 0.5 || (any_solo && controls.get(solos[lane]) < 0.5) { 0.0 }
                    else { controls.get(gains[lane]) }
                });
                let steps: [f32; 4] = std::array::from_fn(|lane| (targets[lane] - now[lane]) / frames as f32);
                let separated = available.is_some_and(|probe| probes[probe as usize] >= 0.5) && stems.iter().all(Option::is_some);
                let (head, tail) = arena.split_at_mut(*out as usize * SPAN);
                let span = &mut tail[..SPAN];
                span.fill(0.0);
                for frame in 0..frames {
                    for lane in 0..4 { now[lane] += steps[lane]; }
                    if separated {
                        for lane in 0..4 {
                            if let Some(input) = stems[lane] {
                                let input = &head[input as usize * SPAN..][..SPAN];
                                span[frame] += input[frame] * now[lane];
                                span[CHUNK + frame] += input[CHUNK + frame] * now[lane];
                            }
                        }
                    } else if let Some(input) = input {
                        // A missing optional stem set never changes the full mix.
                        let input = &head[*input as usize * SPAN..][..SPAN];
                        span[frame] = input[frame];
                        span[CHUNK + frame] = input[CHUNK + frame];
                    }
                }
                *now = targets;
            }
            Op::Ext(op) => ext.run(op, states, arena, controls, probes, rate, frames),
            Op::Nested { state, clock, out, probe } => {
                let NodeState::Nested(this) = &mut states[*state as usize] else { continue };
                let chunk = clock.map(|clock| clocks[clock as usize]).unwrap_or_default();
                let (_, span) = spans(arena, None, *out);
                let (left, right) = span.split_at_mut(CHUNK);
                this.run(&chunk, &mut left[..frames], &mut right[..frames]);
                probes[*probe as usize] = left[..frames].iter().chain(&right[..frames]).fold(probes[*probe as usize] * PEAK_DECAY, |peak, x| peak.max(x.abs()));
            }
            Op::Loop { state, clock, out, amp, pan, mute, solo, probe, inst } => {
                let NodeState::Loop(this) = &mut states[*state as usize] else { continue };
                if let Some(feed) = &mut this.feed {
                    while let Some(event) = feed.pop() {
                        this.engine.push(event);
                    }
                }
                // Settings an editor holds: set on the instrument when they
                // moved (no new revision; the lock is only tried).
                if let Some(live) = &this.live {
                    let version = live.version();
                    if this.live_seen != Some(version) {
                        let engine = &mut this.engine;
                        engine.live_begin(inst);
                        if live.try_read(|name, value| engine.live_set(inst, name, value)) {
                            this.live_seen = Some(version);
                        }
                    }
                }
                let chunk = clock.map(|clock| clocks[clock as usize]).unwrap_or_default();
                if this.playing && !chunk.playing {
                    this.engine.release_all();
                }
                this.playing = chunk.playing;
                let (_, span) = spans(arena, None, *out);
                let (left, right) = span.split_at_mut(CHUNK);
                this.engine.render(inst, &mut left[..frames], &mut right[..frames], rate, &if chunk.playing { chunk.tempo } else { makepad_score_player::Tempo::linear(chunk.start, 0.0) });
                let audible = controls.get(*mute) < 0.5 && (!any_solo || controls.get(*solo) >= 0.5);
                if let (makepad_score_player::Inst::Input { gain, .. }, Some(input)) = (inst, &mut this.input) {
                    // The input as it comes (at the graph's rate only).
                    let (l, r) = (&mut left[..frames], &mut right[..frames]);
                    crate::feed::audio_in::read(input, l, r);
                    let gain = if (crate::feed::audio_in::rate() - rate as f64).abs() < 1.0 { *gain } else { 0.0 };
                    for i in 0..frames {
                        l[i] *= gain;
                        r[i] *= gain;
                    }
                }
                if let makepad_score_player::Inst::Midi { port, .. } = inst {
                    // Out as MIDI, due when the chunk's frames are; a muted
                    // loop sends its note offs only; a render's, nothing.
                    let (engine, port, live) = (&mut this.engine, *port, this.midi_live);
                    let now = crate::feed::midi_out::now();
                    crate::feed::midi_out::try_send(|send| {
                        engine.take_midi(|frame, data| {
                            if live && (audible || data[0] & 0xf0 != 0x90) {
                                send(crate::feed::midi_out::MidiOut { at: now + frame as f64 / rate as f64, port, data });
                            }
                        })
                    });
                }
                let gain = if audible { controls.get(*amp).max(0.0) } else { 0.0 };
                let angle = (controls.get(*pan).clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
                let (to_left, to_right) = (gain * angle.cos() * std::f32::consts::SQRT_2, gain * angle.sin() * std::f32::consts::SQRT_2);
                let (step_left, step_right) = ((to_left - this.gain_l) / frames as f32, (to_right - this.gain_r) / frames as f32);
                this.peak *= PEAK_DECAY;
                for frame in 0..frames {
                    this.gain_l += step_left;
                    this.gain_r += step_right;
                    // A voice gone non-finite never reaches the mix (it would
                    // silence everything after it).
                    left[frame] = if left[frame].is_finite() { left[frame] * this.gain_l } else { 0.0 };
                    right[frame] = if right[frame].is_finite() { right[frame] * this.gain_r } else { 0.0 };
                    this.peak = this.peak.max(left[frame].abs()).max(right[frame].abs());
                }
                probes[*probe as usize] = this.peak;
            }
            Op::Pads { state, out, amp, mute, probe } => {
                let NodeState::Pads(this) = &mut states[*state as usize] else { continue };
                if let Some(mut feed) = this.feed.take() {
                    while let Some(hit) = feed.pop() {
                        crate::pads::take_hit(this, hit, rate);
                    }
                    this.feed = Some(feed);
                }
                let (_, span) = spans(arena, None, *out);
                let (left, right) = span.split_at_mut(CHUNK);
                let target = if controls.get(*mute) < 0.5 { controls.get(*amp).max(0.0) } else { 0.0 };
                crate::pads::render(this, &mut left[..frames], &mut right[..frames], target);
                probes[*probe as usize] = this.peak;
            }
            Op::Fx { state, kind, inputs, key, out, params, count, probe, gr } => {
                let NodeState::Fx(this) = &mut states[*state as usize] else { continue };
                let (head, tail) = arena.split_at_mut(*out as usize * SPAN);
                let span = &mut tail[..SPAN];
                span.fill(0.0);
                for input in inputs.iter() {
                    let input = &head[*input as usize * SPAN..][..SPAN];
                    for frame in 0..frames {
                        span[frame] += input[frame];
                        span[CHUNK + frame] += input[CHUNK + frame];
                    }
                }
                // A Duck's key: how loud what pushes it down is in this pass.
                let side = key.map_or(0.0, |key| {
                    let input = &head[key as usize * SPAN..][..SPAN];
                    (0..frames).fold(0.0f32, |m, f| m.max(input[f].abs()).max(input[CHUNK + f].abs()))
                });
                let mut values = [0.0f32; makepad_score_player::fx::MAX_FX_PARAMS];
                for i in 0..*count as usize {
                    values[i] = controls.get(params[i]);
                }
                // The score's automation: ramps start on their beat and hold
                // their value; per pass (a filter sweep, a build-up).
                if let Some(feed) = &mut this.feed {
                    while let Some(event) = feed.pop() {
                        if this.waiting.len() < this.waiting.capacity() {
                            let at = this.waiting.partition_point(|w| w.beat <= event.beat);
                            this.waiting.insert(at, event);
                        }
                    }
                }
                let beat = clocks.first().map_or(0.0, |c| c.start);
                let at = |ramp: &(f32, f32, f64, f64)| {
                    let t = if ramp.3 > 0.0 { ((beat - ramp.2) / ramp.3).clamp(0.0, 1.0) } else { 1.0 };
                    ramp.0 + (ramp.1 - ramp.0) * t as f32
                };
                while this.waiting.first().is_some_and(|w| w.beat <= beat + 1e-9) {
                    let event = this.waiting.remove(0);
                    let i = (event.param as usize).min(makepad_score_player::fx::MAX_FX_PARAMS - 1);
                    let from = this.ramps[i].as_ref().map_or(values[i], at);
                    this.ramps[i] = Some((from, event.value, event.beat, event.ramp as f64));
                }
                for i in 0..*count as usize {
                    if let Some(ramp) = &this.ramps[i] {
                        values[i] = at(ramp);
                    }
                }
                let seconds_per_beat = clocks.first().map_or(0.5, |clock| if clock.bpm > 0.0 { 60.0 / clock.bpm } else { 0.5 });
                let (left, right) = span.split_at_mut(CHUNK);
                // A Comp with a key listens to it (a sidechain).
                let key_span = key.filter(|_| *kind == makepad_score_player::FxKind::Comp).map(|key| &head[key as usize * SPAN..][..SPAN]);
                let key_lr = key_span.map(|s| (&s[..frames], &s[CHUNK..CHUNK + frames]));
                this.fx.process(*kind, &values, &mut left[..frames], &mut right[..frames], rate, seconds_per_beat, (side * 2.5).min(1.0), key_lr);
                probes[*gr as usize] = this.fx.reduction_db();
                this.peak *= PEAK_DECAY;
                for frame in 0..frames {
                    this.peak = this.peak.max(left[frame].abs()).max(right[frame].abs());
                }
                probes[*probe as usize] = this.peak;
            }
            Op::ShaderFx { state, inputs, out, params, probe } => {
                let NodeState::ShaderFx(this) = &mut states[*state as usize] else { continue };
                let (head, tail) = arena.split_at_mut(*out as usize * SPAN);
                let span = &mut tail[..SPAN];
                span.fill(0.0);
                this.input.fill(0.0);
                for input in inputs.iter() {
                    let input = &head[*input as usize * SPAN..][..SPAN];
                    for frame in 0..frames {
                        this.input[frame] += input[frame];
                        this.input[CHUNK + frame] += input[CHUNK + frame];
                    }
                }
                let address = std::sync::Arc::as_ptr(params) as *const u8 as usize;
                if this.params != address {
                    for (index, value) in params.iter() {
                        this.fx.set_param(*index as usize, *value);
                    }
                    this.params = address;
                }
                let (in_l, in_r) = this.input.split_at(CHUNK);
                let (left, right) = span.split_at_mut(CHUNK);
                this.fx.render(&in_l[..frames], &in_r[..frames], &mut left[..frames], &mut right[..frames]);
                this.peak *= PEAK_DECAY;
                for frame in 0..frames {
                    this.peak = this.peak.max(left[frame].abs()).max(right[frame].abs());
                }
                probes[*probe as usize] = this.peak;
            }
            Op::Output { state, input, out, enabled, probes: (probe_l, probe_r), cue } => {
                let NodeState::Output { peak_l, peak_r, gate } = &mut states[*state as usize] else { continue };
                *peak_l *= PEAK_DECAY;
                *peak_r *= PEAK_DECAY;
                let (input, gated) = spans(arena, *input, *out);
                gated.fill(0.0);
                if let Some(span) = input {
                    // The peaks are what arrives, before the gate.
                    for frame in 0..frames {
                        *peak_l = peak_l.max(span[frame].abs());
                        *peak_r = peak_r.max(span[CHUNK + frame].abs());
                    }
                    let target = if controls.get(*enabled) >= 0.5 { 1.0 } else { 0.0 };
                    if *gate > 0.0 || target > 0.0 {
                        // Prepared dedicated output span also feeds the probe.
                        let step = 1.0 / CHUNK as f32;
                        for frame in 0..frames {
                            *gate = if *gate < target { (*gate + step).min(target) } else { (*gate - step).max(target) };
                            gated[frame] = span[frame] * *gate;
                            gated[CHUNK + frame] = span[CHUNK + frame] * *gate;
                        }
                        let (left, right) = gated.split_at(CHUNK);
                        sink(*cue, left, right);
                    }
                }
                probes[*probe_l as usize] = *peak_l;
                probes[*probe_r as usize] = *peak_r;
            }
        }
    }
}


impl crate::schedule::Ext for NoExt {
    type State = ();
    type Op = ();

    fn demand(_type_name: &str) -> Option<(usize, usize, usize)> {
        None
    }

    fn compile(_cx: &mut crate::schedule::ExtCx) -> Result<Option<((), ())>, makepad_flow::live::plan::PlanError> {
        Ok(None)
    }

    fn keeps_across_rates(_type_name: &str) -> bool {
        false
    }

    fn control_range(_kind: &str, _name: &str) -> Option<(f64, f64, bool)> {
        None
    }
}
