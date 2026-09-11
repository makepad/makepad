//! The clock seam's first protocol: MIDI beat clock, both ways.
//!
//! Twenty-four ticks to the beat, and nothing else -- a tick carries no
//! number, so both halves of this are counting. Out, the count is what
//! turns a continuous position into a stream of bytes; in, it is what
//! turns a stream of arrival times back into a tempo and a phase.
//!
//! Deliberately pure: no clock is read here, no byte is sent here, and no
//! device is named. The caller times the sends and hands the arrivals in,
//! which is what makes every rule below testable without hardware.

/// Ticks per beat. The protocol's own number, and the only one it has.
pub const TICKS_PER_BEAT: u32 = 24;

/// Transport bytes, for a slave that has to know when the run starts.
pub const CLOCK_TICK: u8 = 0xF8;
pub const CLOCK_START: u8 = 0xFA;
pub const CLOCK_CONTINUE: u8 = 0xFB;
pub const CLOCK_STOP: u8 = 0xFC;

/// The most ticks one call will ask for.
///
/// A sender that has been asleep, or a clock that has just jumped, would
/// otherwise ask for thousands at once and machine-gun the wire. Half a
/// bar is enough to ride out an ordinary stall and short enough that a
/// jump is heard as a stumble rather than a burst.
const TICK_BURST_CAP: u32 = TICKS_PER_BEAT * 2;

/// Turns a continuous beat position into ticks that are due.
///
/// Holds only how many ticks the run has sent. It never reads a clock:
/// the caller says where the beat position is, and this says how many
/// bytes that means.
#[derive(Clone, Copy, Debug, Default)]
pub struct ClockTicker {
    sent: u64,
    running: bool,
}

impl ClockTicker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn running(&self) -> bool {
        self.running
    }

    /// Begin a run at `position_beats`. Returns the byte to send.
    ///
    /// START rather than CONTINUE, because a slave takes START as "you are
    /// at the top of a bar" and that is what beginning a run means here.
    pub fn start(&mut self, position_beats: f64) -> u8 {
        self.sent = Self::ticks_at(position_beats);
        self.running = true;
        CLOCK_START
    }

    /// End the run. `None` when it was not running -- a stop nobody asked
    /// for is not a message.
    pub fn stop(&mut self) -> Option<u8> {
        self.running.then(|| {
            self.running = false;
            CLOCK_STOP
        })
    }

    /// How many ticks are due now, given where the beat is.
    ///
    /// Zero when the position has not reached the next tick, and zero
    /// when the run is stopped. A position that has gone BACKWARDS -- a
    /// re-anchored clock, a tap -- re-seats the count silently rather
    /// than sending a burst: there is no way to un-send a tick, so the
    /// honest answer is to carry on from where the music is.
    pub fn due(&mut self, position_beats: f64) -> u32 {
        if !self.running || !position_beats.is_finite() {
            return 0;
        }
        let want = Self::ticks_at(position_beats);
        if want <= self.sent {
            self.sent = want;
            return 0;
        }
        let due = (want - self.sent).min(TICK_BURST_CAP as u64) as u32;
        self.sent = want;
        due
    }

    /// How long until the next tick is due, in beats.
    ///
    /// What a sender sleeps on: a thread that wakes on this rather than on
    /// a fixed interval sends each byte within its own jitter of where it
    /// belongs, instead of in a clump once a frame.
    pub fn until_next_beat_fraction(position_beats: f64) -> f64 {
        if !position_beats.is_finite() {
            return 1.0 / TICKS_PER_BEAT as f64;
        }
        let ticks = position_beats * TICKS_PER_BEAT as f64;
        ((ticks.floor() + 1.0) - ticks) / TICKS_PER_BEAT as f64
    }

    fn ticks_at(position_beats: f64) -> u64 {
        if !position_beats.is_finite() || position_beats <= 0.0 {
            return 0;
        }
        (position_beats * TICKS_PER_BEAT as f64).floor() as u64
    }
}

/// How many ticks a reading is averaged over. One beat: long enough that
/// a single late byte cannot move the tempo much, short enough that a
/// real tempo change is followed within a beat.
const LISTEN_WINDOW: usize = TICKS_PER_BEAT as usize;

/// The tempo and phase a stream of incoming ticks implies.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClockReading {
    pub bpm: f64,
    /// Beats since the run started, continuous.
    pub position_beats: f64,
}

/// Turns arriving ticks back into a tempo and a place in the bar.
///
/// A tick carries no number, so the phase is counted from the last START
/// -- which is exactly what the protocol says it means. Without one the
/// count still runs; it simply cannot claim to know where the bar is, and
/// the caller is told so.
#[derive(Clone, Debug, Default)]
pub struct ClockListener {
    /// Arrival times of the last window's ticks, oldest first.
    arrivals: Vec<f64>,
    /// Ticks since the run started, or since the first tick heard.
    count: u64,
    /// Whether a START (or CONTINUE) has placed the count.
    anchored: bool,
    running: bool,
}

impl ClockListener {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn running(&self) -> bool {
        self.running
    }

    /// Whether the count has been placed by a transport message, so the
    /// bar it reports is the sender's bar and not a guess.
    pub fn anchored(&self) -> bool {
        self.anchored
    }

    /// One byte from the wire. Returns a reading when this byte was a tick
    /// and enough of them have arrived to mean a tempo.
    pub fn byte(&mut self, byte: u8, at_secs: f64) -> Option<ClockReading> {
        match byte {
            CLOCK_START => {
                self.reset();
                self.running = true;
                self.anchored = true;
                None
            }
            CLOCK_CONTINUE => {
                self.running = true;
                None
            }
            CLOCK_STOP => {
                self.running = false;
                None
            }
            CLOCK_TICK => self.tick(at_secs),
            _ => None,
        }
    }

    /// A tick arrived. Kept separate so a caller that has already sorted
    /// its bytes does not have to hand them back one at a time.
    pub fn tick(&mut self, at_secs: f64) -> Option<ClockReading> {
        if !at_secs.is_finite() {
            return None;
        }
        // A gap far too long to be this run's next tick means the sender
        // stopped without saying so, or the wire went away: start again
        // rather than averaging across the hole. A gap of NOTHING is the
        // opposite case and must not reset anything: a caller that reads
        // its wire in batches -- a frame pump, say -- hands two ticks the
        // same arrival time whenever both landed between two reads, which
        // at any real tempo is most beats. Resetting on that would mean
        // such a caller never held a tempo at all. Time going BACKWARDS is
        // still a hole.
        if let Some(last) = self.arrivals.last() {
            let gap = at_secs - last;
            if gap < 0.0 || gap > MAX_TICK_GAP_SECS {
                self.reset();
            }
        }
        self.running = true;
        self.arrivals.push(at_secs);
        if self.arrivals.len() > LISTEN_WINDOW + 1 {
            self.arrivals.remove(0);
        }
        self.count += 1;
        if self.arrivals.len() < 3 {
            return None;
        }
        let span = self.arrivals.last()? - self.arrivals.first()?;
        let gaps = (self.arrivals.len() - 1) as f64;
        if !(span > 0.0) {
            return None;
        }
        let bpm = 60.0 * gaps / (span * TICKS_PER_BEAT as f64);
        if !bpm.is_finite() || !(MIN_CLOCK_BPM..=MAX_CLOCK_BPM).contains(&bpm) {
            return None;
        }
        Some(ClockReading {
            bpm,
            position_beats: (self.count - 1) as f64 / TICKS_PER_BEAT as f64,
        })
    }

    fn reset(&mut self) {
        self.arrivals.clear();
        self.count = 0;
        self.anchored = false;
    }
}

/// Longer than this between ticks and the run is over, whatever the sender
/// meant. Two seconds is 30 BPM at 24 ppqn -- far slower than any music,
/// so nothing musical is ever cut in half by it.
pub const MAX_TICK_GAP_SECS: f64 = 2.0;

/// The band a reading is believed inside. Wider than the analyser's,
/// because a sender is a machine and is telling us rather than guessing.
const MIN_CLOCK_BPM: f64 = 20.0;
const MAX_CLOCK_BPM: f64 = 400.0;

/// Whether a byte off the wire is part of a beat clock at all.
pub fn is_clock_byte(byte: u8) -> bool {
    matches!(byte, CLOCK_TICK | CLOCK_START | CLOCK_CONTINUE | CLOCK_STOP)
}

/// Which port's clock is being followed.
///
/// More than one machine on the wire can be sending ticks, and averaging
/// two senders' arrivals gives a tempo neither of them is playing. So the
/// first port to send one owns the clock, and another port's ticks are
/// ignored while it keeps sending. Let it go quiet for longer than a run
/// can be and the next port to speak takes it over -- which is what
/// unplugging one machine and starting another looks like from here.
#[derive(Clone, Debug)]
pub struct ClockInbox<P> {
    owner: Option<P>,
    last_secs: f64,
    listener: ClockListener,
}

impl<P> Default for ClockInbox<P> {
    fn default() -> Self {
        ClockInbox { owner: None, last_secs: f64::NEG_INFINITY, listener: ClockListener::new() }
    }
}

impl<P: Copy + PartialEq> ClockInbox<P> {
    pub fn new() -> Self {
        Self::default()
    }

    /// The port whose clock is being followed, if any.
    pub fn owner(&self) -> Option<P> {
        self.owner
    }

    /// Whether the followed sender is running.
    pub fn running(&self) -> bool {
        self.listener.running()
    }

    /// Whether a transport message has placed the count, so the bar being
    /// reported is the sender's own and not a guess.
    pub fn anchored(&self) -> bool {
        self.listener.anchored()
    }

    /// One byte, with the port it came in on. Returns a reading when this
    /// byte was a tick from the owning port and enough of them have
    /// arrived to mean a tempo.
    pub fn byte(&mut self, port: P, byte: u8, at_secs: f64) -> Option<ClockReading> {
        if !is_clock_byte(byte) || !at_secs.is_finite() {
            return None;
        }
        // A clock going backwards in time is a clock that was restarted.
        let quiet = at_secs < self.last_secs || at_secs - self.last_secs > MAX_TICK_GAP_SECS;
        match self.owner {
            Some(owner) if owner == port => {}
            Some(_) if !quiet => return None,
            _ => {
                self.owner = Some(port);
                self.listener = ClockListener::new();
            }
        }
        self.last_secs = at_secs;
        self.listener.byte(byte, at_secs)
    }

    /// Forget the sender and its count: the switch went off, or the ports
    /// changed under it.
    pub fn forget(&mut self) {
        *self = Self::default();
    }
}

/// What the sender needs to know, written by the pump and read by the
/// thread that does the sending.
///
/// The position is stamped with the instant it was true at, not with the
/// instant it is read: without that the thread would treat every update
/// as "now" and the clock would jump forward by a pump's worth of time
/// twenty times a second.
#[derive(Clone, Debug)]
pub struct ClockShare {
    pub running: bool,
    pub bpm: f64,
    pub position_beats: f64,
    pub at: std::time::Instant,
    pub ports: Vec<makepad_widgets::MidiPortId>,
    /// Set when the app is going away, so the thread can end.
    pub closed: bool,
}

impl ClockShare {
    pub fn new() -> Self {
        ClockShare {
            running: false,
            bpm: 0.0,
            position_beats: 0.0,
            at: std::time::Instant::now(),
            ports: Vec::new(),
            closed: false,
        }
    }

    /// Where the beat is NOW, extrapolated from the last stamp at the
    /// tempo that stamp carried.
    fn position_now(&self, now: std::time::Instant) -> f64 {
        let elapsed = now.saturating_duration_since(self.at).as_secs_f64();
        self.position_beats + elapsed * self.bpm / 60.0
    }
}

impl Default for ClockShare {
    fn default() -> Self {
        Self::new()
    }
}

/// The sender: one thread, sending one byte at a time, on its own clock.
///
/// It has its OWN timing rather than riding the app's pump because that is
/// the whole difference between a clock a machine can follow and a stream
/// of bursts. Twenty-four ticks a beat at 128 BPM is a byte every
/// nineteen milliseconds; a pump at twenty a second would send them three
/// at a time, and everything slaved to it would wobble.
pub struct ClockSender {
    share: std::sync::Arc<std::sync::Mutex<ClockShare>>,
    /// Kept so a shutdown can WAIT for the stop byte. Closing only asks the
    /// thread to stop; the byte goes out when it next wakes, and at quit the
    /// process can be gone by then -- which leaves anything slaved to this
    /// clock hanging on the last tick instead of getting a stop.
    thread: Option<std::thread::JoinHandle<()>>,
}

/// Never sleep longer than this, so a stop or a tempo change is acted on
/// promptly even when the current tempo says the next tick is far away.
const SENDER_MAX_SLEEP: std::time::Duration = std::time::Duration::from_millis(20);
/// Never sleep less than this: a thread spinning on a clock is worse than
/// half a millisecond of jitter on a byte.
const SENDER_MIN_SLEEP: std::time::Duration = std::time::Duration::from_micros(500);

impl ClockSender {
    /// Start the sender. The thread ends when `closed` is set.
    pub fn spawn(
        output: makepad_widgets::MidiOutput,
        share: std::sync::Arc<std::sync::Mutex<ClockShare>>,
    ) -> ClockSender {
        let thread_share = share.clone();
        let thread = std::thread::spawn(move || {
            let mut ticker = ClockTicker::new();
            loop {
                let now = std::time::Instant::now();
                // The lock is held for a copy and nothing else: the pump
                // must never wait on this thread.
                let (closed, running, bpm, position, ports) = {
                    let Ok(share) = thread_share.lock() else { return };
                    (
                        share.closed,
                        share.running,
                        share.bpm,
                        share.position_now(now),
                        share.ports.clone(),
                    )
                };
                if closed {
                    if let Some(byte) = ticker.stop() {
                        send(&output, &ports, byte);
                    }
                    return;
                }
                let alive = running && bpm.is_finite() && bpm > 1.0 && !ports.is_empty();
                match (alive, ticker.running()) {
                    (true, false) => send(&output, &ports, ticker.start(position)),
                    (false, true) => {
                        if let Some(byte) = ticker.stop() {
                            send(&output, &ports, byte);
                        }
                    }
                    _ => {}
                }
                if alive {
                    for _ in 0..ticker.due(position) {
                        send(&output, &ports, CLOCK_TICK);
                    }
                }
                let wait = match alive {
                    true => std::time::Duration::from_secs_f64(
                        ClockTicker::until_next_beat_fraction(position) * 60.0 / bpm,
                    ),
                    false => SENDER_MAX_SLEEP,
                };
                std::thread::sleep(wait.clamp(SENDER_MIN_SLEEP, SENDER_MAX_SLEEP));
            }
        });
        ClockSender { share, thread: Some(thread) }
    }

    /// Tell the sender where the beat is. Cheap enough for every pump: a
    /// lock, five field writes and a port list only when it changed.
    pub fn publish(&self, running: bool, bpm: f64, position_beats: f64, ports: &[makepad_widgets::MidiPortId]) {
        let Ok(mut share) = self.share.lock() else { return };
        share.running = running;
        share.bpm = bpm;
        share.position_beats = position_beats;
        share.at = std::time::Instant::now();
        if share.ports != ports {
            share.ports = ports.to_vec();
        }
    }

    /// Stop sending and let the thread go.
    /// Ask the sender to stop, and WAIT until it has: the thread sends the
    /// stop byte on its way out, and a caller that does not wait cannot know
    /// whether it went. Bounded by the thread's own maximum sleep, so this
    /// costs a few milliseconds at most.
    pub fn close_and_wait(mut self) {
        if let Ok(mut share) = self.share.lock() {
            share.closed = true;
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn send(
    output: &makepad_widgets::MidiOutput,
    ports: &[makepad_widgets::MidiPortId],
    byte: u8,
) {
    // A system-realtime message is one byte. The two that follow are
    // padding, and this comment used to claim every sender path ignored
    // them because the status names the length -- which was simply not
    // true of any of the three backends: each one wrote all three bytes.
    // `MidiData::wire_len` is where the status is read now, below here.
    for port in ports {
        output.send(Some(*port), makepad_widgets::MidiData { data: [byte, 0, 0] });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_sends_twenty_four_ticks_a_beat_and_no_more() {
        let mut ticker = ClockTicker::new();
        assert_eq!(ticker.due(1.0), 0, "a stopped run sends nothing");
        assert_eq!(ticker.start(0.0), CLOCK_START);

        // Walked forward a beat in eight steps: twenty-four ticks, once.
        let mut total = 0;
        for step in 1..=8 {
            total += ticker.due(step as f64 / 8.0);
        }
        assert_eq!(total, 24);
        assert_eq!(ticker.due(1.0), 0, "and not again for standing still");

        // A second beat is another twenty-four.
        assert_eq!(ticker.due(2.0), 24);
        assert_eq!(ticker.stop(), Some(CLOCK_STOP));
        assert_eq!(ticker.stop(), None, "a stop nobody asked for is nothing");
    }

    #[test]
    fn a_clock_that_jumps_is_re_seated_rather_than_machine_gunned() {
        let mut ticker = ClockTicker::new();
        ticker.start(0.0);
        // Forward by a hundred beats: capped, not two thousand bytes.
        let due = ticker.due(100.0);
        assert_eq!(due, TICKS_PER_BEAT * 2);
        // Backwards: silently re-seated, because a tick cannot be un-sent.
        assert_eq!(ticker.due(4.0), 0);
        assert_eq!(ticker.due(4.5), 12, "and it carries on from there");
    }

    #[test]
    fn the_wait_for_the_next_tick_is_what_a_sender_sleeps_on() {
        // Exactly on a tick: a whole tick to the next.
        let whole = ClockTicker::until_next_beat_fraction(0.0);
        assert!((whole - 1.0 / 24.0).abs() < 1e-12, "{whole}");
        // Halfway between two: half of one.
        let half = ClockTicker::until_next_beat_fraction(0.5 / 24.0);
        assert!((half - 0.5 / 24.0).abs() < 1e-12, "{half}");
    }

    /// One sender at a time: two machines ticking at once would average
    /// into a tempo neither is playing.
    #[test]
    fn the_clock_being_followed_is_one_senders_until_that_one_goes_quiet() {
        let mut inbox = ClockInbox::<u8>::new();
        let step = 60.0 / (120.0 * TICKS_PER_BEAT as f64);
        let mut at = 10.0;
        let mut last = None;
        for _ in 0..TICKS_PER_BEAT {
            last = inbox.byte(1, CLOCK_TICK, at).or(last);
            // The other machine in the rig, ticking half as fast.
            assert_eq!(inbox.byte(2, CLOCK_TICK, at + step / 2.0), None, "not this one's clock");
            at += step;
        }
        assert_eq!(inbox.owner(), Some(1));
        let reading = last.expect("a tempo");
        assert!((reading.bpm - 120.0).abs() < 0.5, "{}", reading.bpm);
        // The owner goes quiet for longer than a run can be: the other
        // machine takes the clock over.
        at += MAX_TICK_GAP_SECS + 0.1;
        for _ in 0..TICKS_PER_BEAT {
            inbox.byte(2, CLOCK_TICK, at);
            at += step * 2.0;
        }
        assert_eq!(inbox.owner(), Some(2), "the one still sending");
        let reading = inbox.byte(2, CLOCK_TICK, at).expect("a tempo");
        assert!((reading.bpm - 60.0).abs() < 0.5, "and its own tempo: {}", reading.bpm);
        // Anything that is not a clock byte is not this inbox's business.
        assert_eq!(inbox.byte(2, 0x90, at), None);
        inbox.forget();
        assert_eq!(inbox.owner(), None);
    }

    #[test]
    fn arriving_ticks_become_a_tempo_and_a_place_in_the_bar() {
        let mut listener = ClockListener::new();
        let tick = 60.0 / (128.0 * 24.0);
        assert_eq!(listener.byte(CLOCK_START, 0.0), None);
        assert!(listener.anchored(), "the sender said where the run begins");

        let mut last = None;
        for index in 0..49 {
            last = listener.byte(CLOCK_TICK, 1.0 + index as f64 * tick);
        }
        let reading = last.expect("a tempo");
        assert!((reading.bpm - 128.0).abs() < 1e-6, "{}", reading.bpm);
        // Forty-nine ticks from the START is two beats exactly.
        assert!((reading.position_beats - 2.0).abs() < 1e-9, "{}", reading.position_beats);

        // A stop is heard, and the run is over until it is told otherwise.
        listener.byte(CLOCK_STOP, 3.0);
        assert!(!listener.running());
    }

    #[test]
    fn a_hole_in_the_wire_starts_the_reading_again_rather_than_averaging_over_it() {
        let mut listener = ClockListener::new();
        let tick = 60.0 / (128.0 * 24.0);
        for index in 0..24 {
            listener.tick(index as f64 * tick);
        }
        assert!(listener.tick(24.0 * tick).is_some());

        // Three seconds of nothing, then the sender comes back at 90.
        let slow = 60.0 / (90.0 * 24.0);
        assert!(listener.tick(10.0).is_none(), "one tick is not a tempo");
        let mut last = None;
        for index in 1..25 {
            last = listener.tick(10.0 + index as f64 * slow);
        }
        let reading = last.expect("a tempo");
        assert!((reading.bpm - 90.0).abs() < 1e-6, "{} — averaged over the hole", reading.bpm);
        assert!(!listener.anchored(), "and it no longer claims to know the bar");
    }

    /// A caller that reads its wire in batches stamps everything that
    /// arrived between two reads with the same time. That is what a frame
    /// pump on a loaded machine does -- thirty reads a second against a
    /// hundred and twenty-eight beats a minute is fifty-one ticks a second
    /// -- and the tempo that comes out is still the sender's, give or take
    /// the read it was seen in.
    #[test]
    fn ticks_read_in_batches_still_make_the_senders_tempo() {
        let mut listener = ClockListener::new();
        let tick = 60.0 / (128.0 * TICKS_PER_BEAT as f64);
        let read = 1.0 / 30.0;
        let mut last = None;
        let mut batched = 0;
        let mut previous = f64::NAN;
        for index in 0..(TICKS_PER_BEAT * 4) {
            // The read that first sees this tick: the one it fell before.
            let at = (index as f64 * tick / read).ceil() * read;
            if at == previous {
                batched += 1;
            }
            previous = at;
            last = listener.tick(at).or(last);
        }
        assert!(batched > 10, "the point of the test is a shared read: {batched}");
        let reading = last.expect("a tempo");
        // One read of slack at each end of the window is what the
        // quantisation can cost, and at this tempo that is about six BPM.
        assert!((reading.bpm - 128.0).abs() < 12.0, "{} is not the sender's", reading.bpm);
        assert!(listener.running());
    }

    #[test]
    fn the_share_carries_where_the_beat_was_and_when() {
        let mut share = ClockShare::new();
        share.bpm = 120.0;
        share.position_beats = 4.0;
        // Two beats a second: half a second later is one beat on.
        let later = share.at + std::time::Duration::from_millis(500);
        let at = share.position_now(later);
        assert!((at - 5.0).abs() < 1e-6, "{at}");
        // And a stamp from before the last update never runs the clock
        // backwards.
        let before = share.at - std::time::Duration::from_millis(500);
        assert!((share.position_now(before) - 4.0).abs() < 1e-9);
    }

    #[test]
    fn nonsense_on_the_wire_is_not_a_tempo() {
        let mut listener = ClockListener::new();
        // Absurdly fast ticks: outside any tempo music has.
        for index in 0..24 {
            listener.tick(index as f64 * 1e-5);
        }
        assert!(listener.tick(24e-5).is_none());
        // And a byte that is not the clock's says nothing.
        assert_eq!(listener.byte(0x90, 1.0), None);
    }
}
