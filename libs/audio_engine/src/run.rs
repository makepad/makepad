//! One pass over a prepared schedule: every op in topological order, each
//! reading its inputs' spans and writing its own, with wired controls taking
//! their producer's sample first. Nothing here allocates, locks or frees.

use crate::schedule::{ClockChunk, ControlBank, DelayTap, NodeState, Op, Schedule, TransportState, SPAN};
use crate::CHUNK;
use makepad_audio_dsp::svf::SvfCoeffs;

/// Per-pass decay of a peak reading: about 300 ms to fall 20 dB at 48 kHz.
pub const PEAK_DECAY: f32 = 0.96;

impl DelayTap {
    #[inline]
    fn write(&mut self, input: [f32; 2]) {
        self.ring[self.write] = input;
        self.write = (self.write + 1) & (self.ring.len() - 1);
    }

    #[inline]
    fn delayed(&mut self, input: [f32; 2]) -> [f32; 2] {
        let out = self.ring[self.write.wrapping_sub(self.frames) & (self.ring.len() - 1)];
        self.write(input);
        out
    }

    #[inline]
    fn step(&mut self, input: [f32; 2]) -> [f32; 2] {
        if self.frames == 0 { input } else { self.delayed(input) }
    }
}

pub fn spans(arena: &mut [f32], input: Option<u16>, out: u16) -> (Option<&[f32]>, &mut [f32]) {
    let (head, tail) = arena.split_at_mut(out as usize * SPAN);
    (input.map(|input| &head[input as usize * SPAN..][..SPAN]), &mut tail[..SPAN])
}

fn sum_side(arena: &[f32], sources: &[u16], l: &mut [f32], r: &mut [f32]) {
    l.fill(0.0);
    r.fill(0.0);
    for &source in sources {
        let input = &arena[source as usize * SPAN..][..SPAN];
        for i in 0..l.len() {
            l[i] += input[i];
            r[i] += input[CHUNK + i];
        }
    }
}

fn stereo_gain(level: f32, pan: f32) -> (f32, f32) {
    let angle = (pan.clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
    (level * angle.cos() * std::f32::consts::SQRT_2, level * angle.sin() * std::f32::consts::SQRT_2)
}

fn param_target(arena: &[f32], sources: &[u16], at: usize) -> f32 {
    sources.iter().fold(0.0, |sum, &source| {
        let base = source as usize * SPAN + at;
        // Duplicating a finite mono value must not overflow its mean.
        sum + ((f64::from(arena[base]) + f64::from(arena[base + CHUNK])) * 0.5) as f32
    })
}

impl crate::schedule::FxNodeState {
    pub(crate) fn collect_events(&mut self, retired: Option<&[bool; makepad_score_player::fx::MAX_FX_PARAMS]>) {
        let discard = retired.unwrap_or(&self.wired);
        if let Some(feed) = &mut self.feed {
            while let Some(event) = feed.pop() {
                if discard.get(usize::from(event.param)) == Some(&true) { continue; }
                if self.waiting.len() < self.waiting.capacity() {
                    let at = self.waiting.partition_point(|w| w.beat <= event.beat);
                    self.waiting.insert(at, event);
                }
            }
        }
    }

    /// Start any events due by this block and hold their values until the next.
    fn sample_automation(&mut self, beat: f64, params: &[u16], controls: &ControlBank) {
        let value_at = |&(from, to, start, length): &(f32, f32, f64, f64), beat: f64| {
            let t = if length > 0.0 { ((beat - start) / length).clamp(0.0, 1.0) } else { 1.0 };
            from + (to - from) * t as f32
        };
        while self.waiting.first().is_some_and(|w| w.beat <= beat + 1e-9) {
            let event = self.waiting.remove(0);
            let i = usize::from(event.param).min(self.ramps.len() - 1);
            let from = self.ramps[i].as_ref().map_or(self.values[i], |r| value_at(r, event.beat));
            self.ramps[i] = Some((from, event.value, event.beat, event.ramp as f64));
        }
        for ((value, ramp), &index) in self.values.iter_mut().zip(&self.ramps).zip(params) {
            if let Some(r) = ramp {
                *value = value_at(r, beat);
                controls.set_effective(index, *value);
            }
        }
    }
}

fn block_beat(clock: &ClockChunk, at: usize) -> f64 {
    // Use the transport anchor directly: rounding the beat at a host pass
    // start, then adding the offset to the block, would depend on the pass.
    let anchors = clock.time.anchors.iter().take(usize::from(clock.time.count));
    if let Some(&(beat, frame, per_frame, _)) = anchors.rev().find(|a| a.1 <= at as i64) {
        beat + (at as f64 - frame as f64) * per_frame
    } else {
        // A host may supply just the pass's tempo map.
        clock.tempo.beat_at(at as f64)
    }
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
    let skipped = rest.skip_pass(
        |snapshot| controls.moved_since(snapshot),
        || ops.iter().any(|op| match op {
            Op::Transport { state, .. } => matches!(&states[*state as usize], NodeState::Transport(this) if this.feed.as_ref().is_some_and(|feed| !feed.is_empty())),
            Op::Loop { state, .. } => matches!(&states[*state as usize], NodeState::Loop(this) if this.has_input()),
            Op::Pads { state, .. } => matches!(&states[*state as usize], NodeState::Pads(this) if this.feed.as_ref().is_some_and(|feed| !feed.is_empty())),
            Op::Fx { state, .. } => matches!(&states[*state as usize], NodeState::Fx(this) if this.feed.as_ref().is_some_and(|feed| !feed.is_empty())),
            // A source that started running (a clip loaded on a deck that
            // is set to play) wakes it as a control does.
            Op::Ext(op) => ext.running(op, states, controls),
            Op::Nested { state, .. } => matches!(&states[*state as usize], NodeState::Nested(this) if this.running()),
            _ => false,
        }),
    );
    if skipped && schedule.clock_frames != 0 {
        schedule.arena.fill(0.0);
        schedule.clock_frames = 0;
    }
    skipped
}

fn settle_rest<X, Y, B>(schedule: &mut Schedule<X, Y, B>, ext: &dyn RunExt<X, Y>) -> bool {
    let Schedule { ops, states, controls, rest, .. } = schedule;
    rest.settle(
        schedule.frame, schedule.rest_hold,
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
            Op::ShaderFx { inputs, probe, .. } => probe.is_none() || !inputs.is_empty(),
            Op::Ext(op) => !ext.running(op, states, controls),
            Op::Nested { state, .. } => !matches!(&states[*state as usize], NodeState::Nested(this) if this.running()),
            _ => true,
        }),
        |snapshot| controls.snapshot(snapshot),
    )
}

fn take_tempo_feed(this: &mut TransportState, controls: &ControlBank, bpm_control: u16) {
    if let Some(feed) = &mut this.feed {
        while let Some(event) = feed.pop() {
            if event.locate {
                this.pending.clear();
                this.ramp = None;
                this.beat = 0.0;
                this.anchor = 0.0;
                this.anchor_frame = this.frames as f64;
                this.shader_anchor = (0.0, this.frames);
                this.located = this.located.wrapping_add(1);
                if event.bpm > 0.0 {
                    // As the map's tempo: shown on the control, not a new setting.
                    this.tempo = event.bpm.clamp(crate::feed::TEMPO_MIN, crate::feed::TEMPO_MAX);
                    this.bpm = this.tempo as f32;
                    this.mapped = true;
                    controls.set_manual(bpm_control, this.bpm);
                }
                continue;
            }
            if this.pending.len() < this.pending.capacity() {
                let at = this.pending.partition_point(|p| p.beat <= event.beat);
                this.pending.insert(at, event);
            }
        }
    }
}

#[derive(Clone, Copy)]
struct TempoCursor {
    anchor: f64,
    frame: f64,
    bpm: f64,
    ramp: Option<(f64, f64, f64, u32, u32)>,
}

impl TempoCursor {
    fn from_transport(t: &TransportState) -> Self {
        Self { anchor: t.anchor, frame: t.anchor_frame, bpm: t.tempo, ramp: t.ramp }
    }

    fn next(&self, events: &[crate::feed::TempoEvent], rate: f32, origin: f64) -> Option<(f64, f64)> {
        let ramp = self.ramp.map(|(start, _, _, _, step)| start + step as f64 / 16.0);
        let beat = match (ramp, events.first()) {
            (Some(r), Some(e)) => r.min(e.beat),
            (Some(r), None) => r,
            (None, event) => event?.beat,
        };
        let speed = self.bpm / 60.0 / rate as f64;
        Some((beat, (self.frame + (beat - self.anchor).max(0.0) / speed).max(origin)))
    }

    fn apply(&mut self, beat: f64, frame: f64, event: Option<&crate::feed::TempoEvent>, rate: f32) {
        self.anchor = beat.max(self.anchor + (frame - self.frame) * (self.bpm / 60.0 / rate as f64));
        if let Some(event) = event {
            if event.ramp > 0.0 {
                let steps = (event.ramp * 16.0).round().max(1.0) as u32;
                self.ramp = Some((event.beat, self.bpm, event.bpm, steps, 0));
            } else {
                self.ramp = None;
                self.bpm = event.bpm;
            }
        }
        if let Some((start, from, to, steps, step)) = self.ramp {
            if start + step as f64 / 16.0 <= beat + 1e-12 {
                self.bpm = from + (to - from) * (step + 1) as f64 / steps as f64;
                self.ramp = (step + 1 < steps).then(|| (start, from, to, steps, step + 1));
            }
        }
        self.bpm = self.bpm.clamp(crate::feed::TEMPO_MIN, crate::feed::TEMPO_MAX);
        self.frame = frame;
    }
}

fn begin_transport(this: &mut TransportState, bpm: f32, playing: bool) {
    let origin = this.frames as f64;
    // The control (or starting and stopping) re-anchors at the
    // beat reached; a changed control sets the tempo.
    if bpm.to_bits() != this.bpm.to_bits() || playing != this.playing {
        this.anchor = this.beat;
        this.anchor_frame = origin;
        this.shader_anchor = (this.anchor, this.frames);
        if bpm.to_bits() != this.bpm.to_bits() {
            this.tempo = bpm as f64;
            this.ramp = None;
            this.mapped = false;
        }
        this.bpm = bpm;
        this.playing = playing;
    }
}

/// The first split in a sample is retained throughout that sample. A second
/// distinct change makes the segment end at the integrated whole-frame beat.
fn record_tempo_split(
    tempo: &mut makepad_score_player::Tempo, first: &mut Option<usize>,
    beat: f64, frame: f64, speed: f64, whole_beat: f64,
) {
    let mut split = (beat, frame, speed);
    let mut starts_sample = true;
    if let Some(index) = *first {
        let (first_beat, first_frame, _) = tempo.splits[index];
        if frame.floor() == first_frame.floor() && frame != first_frame {
            let end = frame.ceil();
            tempo.splits[index].2 = (whole_beat - first_beat) / (end - first_frame);
            split = (whole_beat, end, speed);
            starts_sample = false;
        }
    }
    // A change at exactly the group's endpoint supersedes that endpoint.
    let n = tempo.nsplits as usize;
    let index = if n != 0 && tempo.splits[n - 1].1 == split.1 {
        n - 1
    } else {
        tempo.nsplits += 1;
        n
    };
    tempo.splits[index] = split;
    if starts_sample { *first = Some(index); }
}

/// Size the next graph pass before any state advances. Follow the same map
/// cursor as the transport; leave the overflowing change for the next pass.
fn tempo_extent<X, Y, B>(schedule: &Schedule<X, Y, B>, mut frames: usize) -> usize {
    for &index in &schedule.transport_ops {
        let Op::Transport { state, .. } = schedule.ops[index] else { continue };
        let NodeState::Transport(t) = &schedule.states[state as usize] else { continue };
        if t.ramp.is_none() && t.pending.len() <= makepad_score_player::loop_engine::MAX_TEMPO_SPLITS
            && t.pending.iter().all(|e| e.ramp <= 0.0) { continue; }
        let routed = schedule.routes.binary_search_by_key(&(index as u16), |r| r.op).is_ok();
        if !routed && !t.playing { continue; }
        // Routed controls can change when their producer runs. The maximum
        // tempo bounds the earliest possible fifth change in that case.
        let origin = t.frames as f64;
        let mut cursor = TempoCursor::from_transport(t);
        if routed {
            cursor.anchor = t.beat;
            cursor.frame = origin;
        }
        let mut consumed = 0;
        let mut sample = None;
        let mut previous = None;
        let mut splits_in_sample = 0;
        let mut count = 0;
        loop {
            if routed { cursor.bpm = crate::feed::TEMPO_MAX; }
            let Some((beat, exact)) = cursor.next(&t.pending[consumed..], schedule.rate as f32, origin) else { break };
            if exact >= origin + frames as f64 { break; }
            if sample != Some(exact.floor()) {
                sample = Some(exact.floor());
                splits_in_sample = 0;
            }
            // For a known clock only the first change and the whole-frame
            // endpoint use slots. Routed clocks can spread these changes over
            // more samples, so charge each event at its earliest possible time.
            if routed || (previous != Some(exact) && splits_in_sample < 2) {
                count += 1;
                splits_in_sample += 1;
                if count > makepad_score_player::loop_engine::MAX_TEMPO_SPLITS {
                    frames = frames.min(((exact - origin).floor() as usize).max(1));
                    break;
                }
                previous = Some(exact);
            }
            let event = t.pending.get(consumed).filter(|e| e.beat == beat);
            consumed += usize::from(event.is_some());
            cursor.apply(beat, exact, event, schedule.rate as f32);
        }
    }
    frames
}

/// One pass of `frames` frames (at most [`CHUNK`]) over a schedule. `sink`
/// receives what each output node sends to the device, after its gate.
/// `sink(cue, left, right)`: each output's gated chunk, `cue` for a
/// CueOutput (the headphone cue), else the device's program.
pub fn run<X, Y, B>(schedule: &mut Schedule<X, Y, B>, frames: usize, sink: &mut dyn FnMut(bool, &[f32], &[f32]), ext: &mut dyn RunExt<X, Y>) {
    run_awake(schedule, frames, sink, ext, false);
}

/// Render a host pass, splitting only at an internal clock or sleep boundary.
/// The caller checks for a wake once at host entry. When a schedule falls
/// asleep inside the pass, its graph and its frame stop at that boundary.
pub fn run_awake<X, Y, B>(schedule: &mut Schedule<X, Y, B>, frames: usize, sink: &mut dyn FnMut(bool, &[f32], &[f32]), ext: &mut dyn RunExt<X, Y>, monitor_rest: bool) {
    schedule.last_frames = frames as u16;
    let mut check_rest = monitor_rest;
    for &index in &schedule.transport_ops {
        if let Op::Transport { state, bpm, playing, .. } = schedule.ops[index] {
            if let NodeState::Transport(t) = &mut schedule.states[state as usize] {
                take_tempo_feed(t, &schedule.controls, bpm);
                if frames != 0 && schedule.routes.binary_search_by_key(&(index as u16), |r| r.op).is_err() {
                    // Latch the unrouted transport for the whole host pass. The
                    // tempo map can change its speed or locate it, never stop it.
                    begin_transport(t, schedule.controls.get(bpm), schedule.controls.get(playing) >= 0.5);
                    if t.playing { check_rest = false; }
                }
            }
        }
    }
    for out in &mut schedule.outputs { out.active = false; }
    for (id, input) in &mut schedule.clock_inputs { *input = schedule.clocks[*id as usize]; }
    let mut offset = 0;
    while offset < frames {
        let mut count = tempo_extent(schedule, frames - offset);
        if check_rest { count = schedule.rest.until_check(schedule.frame, schedule.rest_hold, count); }
        if offset != 0 {
            for (id, input) in &schedule.clock_inputs {
                schedule.clocks[*id as usize] = clock_from(*input, offset);
            }
        }
        render_ops(schedule, count, ext);
        schedule.clock_frames = count;
        if monitor_rest {
            // Also meter each bus before its output gates so a muted tail keeps
            // running. Opposite signals on one bus cancel in each sum.
            for i in 0..count {
                let mut buses = [[0.0f32; 2]; 2];
                let mut pre = [[0.0f32; 2]; 2];
                for out in &schedule.outputs {
                    let bus = out.cue as usize;
                    let j = out.out as usize * SPAN + i;
                    buses[bus][0] += schedule.arena[j];
                    buses[bus][1] += schedule.arena[j + CHUNK];
                    if let Some(input) = out.input {
                        let k = input as usize * SPAN + i;
                        pre[bus][0] += schedule.arena[k];
                        pre[bus][1] += schedule.arena[k + CHUNK];
                    }
                }
                let peak = buses.into_iter().chain(pre).flatten().fold(0.0f32, |p, v| p.max(v.abs()));
                schedule.rest.observe(peak);
            }
            if check_rest { settle_rest(schedule, ext); }
        }
        if count != frames {
            for (src, dst) in schedule.arena.chunks_exact(CHUNK).zip(schedule.assembled.chunks_exact_mut(CHUNK)) {
                dst[offset..offset + count].copy_from_slice(&src[..count]);
                if schedule.rest.asleep() { dst[offset + count..frames].fill(0.0); }
            }
        }
        offset += count;
        if monitor_rest && schedule.rest.asleep() { break; }
    }
    if frames != 0 && schedule.clock_frames != frames {
        std::mem::swap(&mut schedule.arena, &mut schedule.assembled);
    }
    for out in &schedule.outputs {
        if out.active {
            let span = &schedule.arena[out.out as usize * SPAN..][..SPAN];
            let (left, right) = span.split_at(CHUNK);
            sink(out.cue, left, right);
        }
    }
}

/// Rebase an externally supplied clock when its graph needs another pass.
fn clock_from(clock: ClockChunk, offset: usize) -> ClockChunk {
    let f = offset as f64;
    let start = clock.tempo.beat_at(f);
    let mut tempo = makepad_score_player::Tempo::linear(start, clock.tempo.per_frame);
    for &(beat, frame, per) in &clock.tempo.splits[..clock.tempo.nsplits as usize] {
        if frame <= f {
            tempo.per_frame = per;
        } else {
            tempo.splits[tempo.nsplits as usize] = (beat, frame - f, per);
            tempo.nsplits += 1;
        }
    }
    let mut time = clock.time;
    time.count = 0;
    for &(beat, frame, per, bpm) in &clock.time.anchors[..clock.time.count as usize] {
        let local = frame - offset as i64;
        if local <= 0 { time.count = 0; }
        time.anchors[time.count as usize] = (beat, local, per, bpm);
        time.count += 1;
    }
    ClockChunk { start, per_frame: tempo.per_frame, tempo, time, ..clock }
}

fn render_ops<X, Y, B>(schedule: &mut Schedule<X, Y, B>, frames: usize, ext: &mut dyn RunExt<X, Y>) {
    let mut output = 0;
    let pass_frame = schedule.frame as u32;
    let Schedule { ops, states, arena, clocks, probes, controls, routes, rate, .. } = schedule;
    let rate = *rate as f32;
    let mut route = 0;
    // A soloed loop silences every loop that is not.
    let any_solo = ops.iter().any(|op| matches!(op, Op::Loop { solo, mute, .. } if controls.get(*solo) >= 0.5 && controls.get(*mute) < 0.5));
    for (index, op) in ops.iter().enumerate() {
        // Wired controls take this pass's sample of their producer, which
        // ran earlier in topological order.
        let before_routes = route;
        while let Some(next) = routes.get(route).filter(|next| next.op as usize == index) {
            next.apply(probes, controls);
            route += 1;
        }
        match op {
            Op::Delay { state, input, out, .. } => {
                let NodeState::Delay(delay) = &mut states[*state as usize] else { continue };
                let (head, tail) = arena.split_at_mut(*out as usize * SPAN);
                let input = &head[*input as usize * SPAN..][..SPAN];
                let sample = |i: usize| [input[i], input[CHUNK + i]];
                let warm = delay.warmup.min(frames);
                for i in 0..warm {
                    let x = sample(i);
                    delay.current.write(x);
                    [tail[i], tail[CHUNK + i]] = delay.previous.step(x);
                }
                delay.warmup -= warm;
                let end = warm + delay.fade_left.min(frames - warm);
                for i in warm..end {
                    let x = sample(i);
                    let a = delay.previous.step(x);
                    let b = delay.current.step(x);
                    delay.fade_left -= 1;
                    let old_gain = delay.fade_left as f32 / makepad_score_player::shader::FADE as f32;
                    let new_gain = 1.0 - old_gain;
                    tail[i] = a[0] * old_gain + b[0] * new_gain;
                    tail[CHUNK + i] = a[1] * old_gain + b[1] * new_gain;
                }
                delay.report_if_finished();
                if delay.current.frames == 0 {
                    tail[end..frames].copy_from_slice(&input[end..frames]);
                    tail[CHUNK + end..CHUNK + frames].copy_from_slice(&input[CHUNK + end..CHUNK + frames]);
                } else {
                    for i in end..frames {
                        [tail[i], tail[CHUNK + i]] = delay.current.delayed(sample(i));
                    }
                }
            }
            Op::Transport { state, clock, bpm, playing, beats_per_bar, probe, mapped } => {
                let NodeState::Transport(this) = &mut states[*state as usize] else { continue };
                let bpm_control = *bpm;
                if route != before_routes {
                    begin_transport(this, controls.get(*bpm), controls.get(*playing) >= 0.5);
                }
                let playing = this.playing;
                let origin = this.frames as f64;
                let per = |tempo: f64| if playing { tempo / 60.0 / rate as f64 } else { 0.0 };
                let mut tempo = makepad_score_player::Tempo::linear(this.beat, per(this.tempo));
                // The song's time for shaders: the anchor in effect at the
                // pass start, then one entry per tempo change.
                let mut time = makepad_score_player::shader::SongTime { count: 1, beats_per_bar: controls.get(*beats_per_bar), playing, ..Default::default() };
                time.anchors[0] = (this.shader_anchor.0, this.shader_anchor.1 as i64 - this.frames as i64, per(this.tempo), this.tempo as f32);
                if playing {
                    // Tempo changes whose beat falls in this pass split it.
                    let mut cursor = TempoCursor::from_transport(this);
                    let mut consumed = 0;
                    let mut changed = false;
                    let mut first_in_sample = None;
                    while let Some((beat, exact)) = cursor.next(&this.pending[consumed..], rate, origin) {
                        if exact >= origin + frames as f64 { break; }
                        let frame = (exact - origin).max(0.0);
                        let event = this.pending.get(consumed).filter(|e| e.beat == beat);
                        consumed += usize::from(event.is_some());
                        cursor.apply(beat, exact, event, rate);
                        let speed = per(cursor.bpm);
                        let whole = exact.ceil();
                        let whole_beat = cursor.anchor + (whole - exact) * speed;
                        this.shader_anchor = (whole_beat, whole as u64);
                        let record = (whole_beat, whole as i64 - this.frames as i64, speed, cursor.bpm as f32);
                        let last = time.count as usize - 1;
                        if time.anchors[last].1 == record.1 {
                            time.anchors[last] = record;
                        } else if (time.count as usize) < time.anchors.len() {
                            time.anchors[time.count as usize] = record;
                            time.count += 1;
                        }
                        record_tempo_split(&mut tempo, &mut first_in_sample, cursor.anchor, frame, speed, whole_beat);
                        changed = true;
                    }
                    if consumed != 0 { this.pending.drain(..consumed); }
                    this.ramp = cursor.ramp;
                    this.tempo = cursor.bpm;
                    this.anchor = cursor.anchor;
                    this.anchor_frame = cursor.frame;
                    if changed {
                        this.mapped = true;
                        this.bpm = this.tempo as f32;
                        controls.set_manual(bpm_control, this.bpm);
                    }
                }
                let per_frame = per(this.tempo);
                clocks[*clock as usize] = ClockChunk { start: this.beat, per_frame: tempo.per_frame, playing, bpm: this.tempo as f32, tempo, located: this.located, time };
                this.frames += frames as u64;
                this.beat = this.anchor + (this.frames as f64 - this.anchor_frame) * per_frame;
                probes[*probe as usize] = this.beat as f32;
                probes[*mapped as usize] = if this.mapped { 1.0 } else { 0.0 };
            }
            Op::Filter { state, input, out, cutoff, resonance } => {
                let NodeState::Filter { left, right, cutoff: cutoff_now, q: q_now, coeffs } = &mut states[*state as usize] else { continue };
                let (input, span) = spans(arena, *input, *out);
                let Some(input) = input else {
                    span.fill(0.0);
                    continue;
                };
                let mut frame = 0;
                while frame < frames {
                    let phase = pass_frame.wrapping_add(frame as u32) % 32;
                    let count = (frames - frame).min(32 - phase as usize);
                    if phase == 0 {
                        *cutoff_now += (controls.get(*cutoff) - *cutoff_now) * 0.25;
                        *q_now += (controls.get(*resonance) - *q_now) * 0.25;
                        *coeffs = SvfCoeffs::new(*cutoff_now, *q_now, rate);
                    }
                    for at in frame..frame + count {
                        span[at] = left.process(coeffs, input[at]).0;
                        span[CHUNK + at] = right.process(coeffs, input[CHUNK + at]).0;
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
                *peak *= PEAK_DECAY;
                for frame in 0..frames {
                    let absolute = pass_frame.wrapping_add(frame as u32);
                    if absolute % 64 == 0 {
                        let level = if controls.get(*mute) >= 0.5 { 0.0 } else { controls.get(*gain) };
                        let (l, r) = stereo_gain(level, controls.get(*pan));
                        left.target(l, absolute);
                        right.target(r, absolute);
                    }
                    span[frame] = input[frame] * left.advance(false);
                    span[CHUNK + frame] = input[CHUNK + frame] * right.advance(false);
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
                for frame in 0..frames {
                    let absolute = pass_frame.wrapping_add(frame as u32);
                    if absolute % 64 == 0 { now.target(controls.get(*master), absolute); }
                    let gain = now.advance(false);
                    span[frame] *= gain;
                    span[CHUNK + frame] *= gain;
                }
            }
            Op::StemMixer { state, input, stems, available, out, gains, mutes, solos } => {
                let NodeState::StemMixer { gains: now } = &mut states[*state as usize] else { continue };
                let separated = available.is_some_and(|probe| probes[probe as usize] >= 0.5) && stems.iter().all(Option::is_some);
                let (head, tail) = arena.split_at_mut(*out as usize * SPAN);
                let span = &mut tail[..SPAN];
                span.fill(0.0);
                for frame in 0..frames {
                    let absolute = pass_frame.wrapping_add(frame as u32);
                    if absolute % 64 == 0 {
                        let any_solo = solos.iter().any(|solo| controls.get(*solo) >= 0.5);
                        for lane in 0..4 {
                            let muted = controls.get(mutes[lane]) >= 0.5 || (any_solo && controls.get(solos[lane]) < 0.5);
                            now[lane].target(if muted { 0.0 } else { controls.get(gains[lane]) }, absolute);
                        }
                    }
                    let levels = std::array::from_fn::<_, 4, _>(|lane| now[lane].advance(true));
                    if separated {
                        for lane in 0..4 {
                            if let Some(input) = stems[lane] {
                                let input = &head[input as usize * SPAN..][..SPAN];
                                span[frame] += input[frame] * levels[lane];
                                span[CHUNK + frame] += input[CHUNK + frame] * levels[lane];
                            }
                        }
                    } else if let Some(input) = input {
                        // A missing optional stem set never changes the full mix.
                        let input = &head[*input as usize * SPAN..][..SPAN];
                        span[frame] = input[frame];
                        span[CHUNK + frame] = input[CHUNK + frame];
                    }
                }
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
            Op::Loop { state, clock, out, amp, pan, mute, solo, probe, inst, ports } => {
                let NodeState::Loop(this) = &mut states[*state as usize] else { continue };
                this.engine.set_frame(pass_frame);
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
                this.engine.set_song_time(&chunk.time);
                // A loop played from MIDI in plays on while the transport
                // is stopped (on a free clock at its tempo).
                if this.playing && !chunk.playing && this.stream.is_none() {
                    this.engine.release_all();
                }
                this.playing = chunk.playing;
                let tempo = match this.stream.as_deref_mut() {
                    Some(stream) => {
                        let tempo = stream.tempo(&chunk, rate);
                        stream.run(&mut this.engine, inst, &tempo, frames, rate, crate::clock::now());
                        tempo
                    }
                    None if chunk.playing => chunk.tempo,
                    None => makepad_score_player::Tempo::linear(chunk.start, 0.0),
                };
                let (head, tail) = arena.split_at_mut(*out as usize * SPAN);
                let (main, extra) = tail.split_at_mut(SPAN);
                let (left, right) = main.split_at_mut(CHUNK);
                if ports.sides.is_empty() && ports.params.is_empty() && ports.outs.is_empty() {
                    this.engine.render(inst, &mut left[..frames], &mut right[..frames], rate, &tempo);
                } else {
                    if let Some(shader) = this.engine.shader_mut() {
                        for (slot, (_, sources)) in ports.sides.iter().enumerate() {
                            if sources.len() != 1 {
                                let (l, r) = shader.side_buffer(slot);
                                sum_side(head, sources, &mut l[..frames], &mut r[..frames]);
                            }
                        }
                    }
                    let direct = |slot: usize| {
                        let [source] = &*ports.sides[slot].1 else { return None };
                        Some(head[*source as usize * SPAN..][..SPAN].split_at(CHUNK))
                    };
                    this.engine.render_with_ports(inst, &mut left[..frames], &mut right[..frames], rate, &tempo,
                        &direct, &|slot, at| param_target(head, &ports.params[slot].1, at));
                    if let Some(shader) = this.engine.shader_mut() {
                        for (slot, (_, span)) in ports.outs.iter().enumerate() {
                            let (l, r) = shader.output_buffer(slot);
                            let dst = &mut extra[(*span - *out - 1) as usize * SPAN..][..SPAN];
                            dst[..frames].copy_from_slice(&l[..frames]);
                            dst[CHUNK..CHUNK + frames].copy_from_slice(&r[..frames]);
                        }
                    }
                }
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
                    // Out as MIDI, due when the delayed audio is heard; a muted
                    // loop sends its note offs only; a render's, nothing.
                    let (engine, port, live) = (&mut this.engine, *port, this.midi_live);
                    let now = crate::feed::midi_out::now();
                    let latency = schedule.latency;
                    crate::feed::midi_out::try_send(|send| {
                        engine.take_midi(|frame, data| {
                            if live && (audible || data[0] & 0xf0 != 0x90) {
                                send(crate::feed::midi_out::MidiOut { at: now + (u64::from(frame) + latency) as f64 / rate as f64, port, data });
                            }
                        })
                    });
                }
                this.peak *= PEAK_DECAY;
                for frame in 0..frames {
                    let absolute = pass_frame.wrapping_add(frame as u32);
                    if absolute % 64 == 0 {
                        let level = if audible { controls.get(*amp).max(0.0) } else { 0.0 };
                        let (l, r) = stereo_gain(level, controls.get(*pan));
                        this.gain_l.target(l, absolute);
                        this.gain_r.target(r, absolute);
                    }
                    let l = this.gain_l.advance(false);
                    let r = this.gain_r.advance(false);
                    // A voice gone non-finite never reaches the mix (it would
                    // silence everything after it).
                    left[frame] = if left[frame].is_finite() { left[frame] * l } else { 0.0 };
                    right[frame] = if right[frame].is_finite() { right[frame] * r } else { 0.0 };
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
                crate::pads::render(this, &mut left[..frames], &mut right[..frames], pass_frame,
                    &|| if controls.get(*mute) < 0.5 { controls.get(*amp).max(0.0) } else { 0.0 });
                probes[*probe as usize] = this.peak;
            }
            Op::Fx { state, kind, inputs, key, out, params, wires, count, probe, gr } => {
                let NodeState::Fx(this) = &mut states[*state as usize] else { continue };
                if let makepad_score_player::fx::FxState::Delay { tempo, .. } = &mut this.fx {
                    if let Some(clock) = clocks.first() { tempo.begin(&clock.tempo, &clock.time, rate); }
                }
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
                let count = usize::from(*count);
                let mut automated = false;
                for i in 0..count {
                    if this.ramps[i].is_some() {
                        automated = true;
                        if pass_frame % 64 != 0 { controls.set_effective(params[i], this.values[i]); }
                    } else if !this.wired[i] {
                        this.values[i] = controls.get(params[i]);
                    }
                }
                this.collect_events(None);
                let automated = automated || !this.waiting.is_empty();
                let seconds_per_beat = clocks.first().map_or(0.5, |clock| if clock.bpm > 0.0 { 60.0 / clock.bpm } else { 0.5 });
                let (left, right) = span.split_at_mut(CHUNK);
                let duck = *kind == makepad_score_player::FxKind::Duck;
                let key_span = key.map(|key| &head[key as usize * SPAN..][..SPAN]);
                let key_lr = key_span.map(|s| (&s[..frames], &s[CHUNK..CHUNK + frames]));
                if wires.is_empty() && !automated && !duck {
                    this.fx.process(*kind, &this.values, &mut left[..frames], &mut right[..frames], rate, seconds_per_beat, 0.0, key_lr);
                } else {
                    // The same absolute 64-frame grid as shader parameter targets.
                    let mut at = 0;
                    while at < frames {
                        let phase = (pass_frame.wrapping_add(at as u32) % 64) as usize;
                        let end = (at + 64 - phase).min(frames);
                        if phase == 0 {
                            if duck {
                                let previous = this.key_peak.replace(0.0).unwrap_or(0.0);
                                this.previous_key_level = (previous * 2.5).min(1.0);
                            }
                            if automated {
                                let beat = clocks.first().map_or(0.0, |clock| block_beat(clock, at));
                                this.sample_automation(beat, &params[..count], controls);
                            }
                            for (i, sources) in wires.iter() {
                                let target = param_target(head, sources, at);
                                let (_, default, lo, hi) = kind.params()[*i];
                                // A sum that overflows still reaches the corresponding rail.
                                this.values[*i] = if target.is_nan() { default } else { target.clamp(lo, hi) };
                            }
                        }
                        let key = key_lr.map(|(l, r)| (&l[at..end], &r[at..end]));
                        if let (Some(peak), Some((l, r))) = (&mut this.key_peak, key) {
                            *peak = l.iter().zip(r).fold(*peak, |m, (l, r)| m.max(l.abs()).max(r.abs()));
                        }
                        this.fx.process(*kind, &this.values, &mut left[at..end], &mut right[at..end], rate, seconds_per_beat, this.previous_key_level, key);
                        at = end;
                    }
                    // Publish the held value even on a partial block after a swap.
                    for (i, _) in wires.iter() { controls.set_effective(params[*i], this.values[*i]); }
                }
                probes[*gr as usize] = this.fx.reduction_db();
                this.peak *= PEAK_DECAY;
                for frame in 0..frames {
                    this.peak = this.peak.max(left[frame].abs()).max(right[frame].abs());
                }
                probes[*probe as usize] = this.peak;
            }
            Op::ShaderFx { state, inputs, sides, outs, wires, out, params, probe } => {
                let NodeState::ShaderFx(this) = &mut states[*state as usize] else { continue };
                this.fx.sync_frame(pass_frame);
                this.fx.set_song_time(&clocks.first().map(|c| c.time).unwrap_or_default());
                let (head, tail) = arena.split_at_mut(*out as usize * SPAN);
                let (in_l, in_r, left, right) = if probe.is_some() {
                    let span = &mut tail[..SPAN];
                    span.fill(0.0);
                    this.input.fill(0.0);
                    for &source in inputs.iter() {
                        let input = &head[source as usize * SPAN..][..SPAN];
                        for frame in 0..frames {
                            this.input[frame] += input[frame];
                            this.input[CHUNK + frame] += input[CHUNK + frame];
                        }
                    }
                    let (l, r) = this.input.split_at(CHUNK);
                    let (out_l, out_r) = span.split_at_mut(CHUNK);
                    (l, r, out_l, out_r)
                } else {
                    // The stereo host arguments give the pass its length. A control
                    // builds its counted table from the named outputs alone.
                    static SILENCE: [f32; CHUNK] = [0.0; CHUNK];
                    let (l, r) = this.input.split_at_mut(CHUNK);
                    (&SILENCE[..], &SILENCE[..], l, r)
                };
                let address = std::sync::Arc::as_ptr(params) as *const u8 as usize;
                if this.params != address {
                    for (index, value) in params.iter() {
                        this.fx.set_param(*index as usize, *value);
                    }
                    this.params = address;
                }
                if sides.is_empty() && outs.is_empty() && wires.is_empty() {
                    this.fx.render(&in_l[..frames], &in_r[..frames], &mut left[..frames], &mut right[..frames]);
                } else {
                    for (slot, (_, sources)) in sides.iter().enumerate() {
                        let (l, r) = this.fx.side_buffer(slot);
                        sum_side(head, sources, &mut l[..frames], &mut r[..frames]);
                    }
                    this.fx.render_with_params(&in_l[..frames], &in_r[..frames], &mut left[..frames], &mut right[..frames], |slot, at| {
                        param_target(head, &wires[slot].1, at)
                    });
                }
                if let Some(probe) = probe {
                    this.peak *= PEAK_DECAY;
                    for frame in 0..frames {
                        this.peak = this.peak.max(left[frame].abs()).max(right[frame].abs());
                    }
                    probes[*probe as usize] = this.peak;
                }
                for (slot, &(_, span)) in outs.iter().enumerate() {
                    let (l, r) = this.fx.output_buffer(slot);
                    let dst = &mut tail[(span - *out) as usize * SPAN..][..SPAN];
                    dst[..frames].copy_from_slice(&l[..frames]);
                    dst[CHUNK..CHUNK + frames].copy_from_slice(&r[..frames]);
                }
            }
            Op::Output { state, input, out, enabled, probes: (probe_l, probe_r), .. } => {
                let active = &mut schedule.outputs[output].active;
                output += 1;
                let NodeState::Output { peak_l, peak_r, gate, hold } = &mut states[*state as usize] else { continue };
                let skip = (*hold).min(frames as u64);
                *hold -= skip;
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
                        for frame in skip as usize..frames {
                            *gate = if *gate < target { (*gate + step).min(target) } else { (*gate - step).max(target) };
                            gated[frame] = span[frame] * *gate;
                            gated[CHUNK + frame] = span[CHUNK + frame] * *gate;
                        }
                        *active = true;
                    }
                }
                probes[*probe_l as usize] = *peak_l;
                probes[*probe_r as usize] = *peak_r;
            }
        }
    }
    schedule.frame = schedule.frame.wrapping_add(frames as u64);
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
