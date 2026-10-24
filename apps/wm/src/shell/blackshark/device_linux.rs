//! The receiver's settings interface on Linux, run by the system worker
//! (`system_linux`): no thread of its own, no UI-thread I/O.
//!
//! * Discovery: `/sys/class/hidraw/*` whose HID id is USB 1532:0577 and
//!   whose USB interface is number 5. Opened `O_RDWR | O_NONBLOCK`; the
//!   worker's `poll(2)` wakes on its answers. Only vendor report 02 is ever
//!   written; nothing here resets, detaches or reconfigures the device.
//! * One request in flight. A request is a 64-byte report 02; its answer is
//!   the first complete, checksummed response with the same destination and
//!   function (and slot, for the slot getters). The transaction byte is
//!   always 0x60, so after a timeout or a refusal nothing is sent for a
//!   short quiet period, and whatever arrives without a request in flight
//!   is dropped as late.
//! * Scheduling: one pure decision ([`HeadsetLink::decide`]) says what to
//!   start now or when something next becomes due; the worker's deadline
//!   and the sender both use it, so nothing overdue is left waiting and
//!   nothing is polled early. Every kind of work has its own timer: asking
//!   the receiver (`probe_at`), an owed full read (`refresh_at`, with
//!   backoff), the battery poll or the away recheck (`next_poll`) and
//!   waiting edits (`retry_at`, with backoff). Only the panel opening and
//!   the receiver's link-up report bring a full read forward.
//! * Reads: the receiver first (firmware: does it answer at all), then the
//!   headset's battery and charging; a full read adds every setting. Battery
//!   and charging are re-read every minute; while the headset is away (its
//!   battery reads "unavailable", or it stops answering) only every half
//!   minute. A getter the headset refuses during a full read is skipped
//!   (logged) and the read goes on, leaving the read not fresh.
//! * Values: `snapshot.settings` holds only what was read on this
//!   connection; earlier values move to `snapshot.last_known` for display.
//!   An edit interrupted part way forgets that control's values until a
//!   full read has them again.
//! * Edits: the panel overwrites one entry per setting in [`HeadsetShared`]
//!   (newest value wins, under a lock both sides hold for a copy or a field
//!   write only; no I/O and no other lock under it). When the headset is
//!   connected and nothing is in flight, the worker applies one setting's
//!   whole sequence, every step acknowledged in order, then reads it back.
//!   An entry is cleared only if the person has not changed it since, and
//!   only when the read-back shows the requested value or every step was
//!   accepted and the headset still reports another value (then retrying
//!   would loop, and the panel says so). Failures keep the entry and retry
//!   with a growing delay, also across sleep and replugging; nothing that
//!   was not asked for is ever written.
//! * Writes go through usbhid's SET_REPORT and take a few milliseconds; the
//!   kernel bounds a stalled one by its control timeout.

use super::{
    firmware_label, mixer_linux, query, sidetone_write, sleep_write, Control, Destination, Equalizer, Frame,
    HeadsetEdit, HeadsetEdits, HeadsetSnapshot, LinkState, NoiseControl, ReplyKind, Report, HEADPHONE_EDITABLE_SLOTS,
    HEADPHONE_READ_SLOTS, MIC_CUSTOM_PRESET,
};
use makepad_widgets::log;
use std::{
    collections::VecDeque,
    fs::{self, File, OpenOptions},
    io::{ErrorKind, Read, Write},
    os::{
        fd::{AsRawFd, RawFd},
        unix::fs::OpenOptionsExt,
    },
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};

const O_NONBLOCK: i32 = 0o4000;
const ENODEV: i32 = 19;
const ESHUTDOWN: i32 = 108;
const POLLERR: i16 = 0x008;
const POLLHUP: i16 = 0x010;
const POLLNVAL: i16 = 0x020;
const HID_ID: &str = "HID_ID=0003:00001532:00000577";
const INTERFACE: &str = "05";
/// The receiver answers in ~5 ms, the headset in ~50 ms (captured).
const REPLY_TIMEOUT: f64 = 1.0;
/// After a timeout or a refused step: answers arriving this long are late.
const QUIET_AFTER_FAILURE: f64 = 0.3;
const POLL_INTERVAL: f64 = 60.0;
const AWAY_POLL_INTERVAL: f64 = 30.0;
const RETRY_MIN: f64 = 3.0;
const RETRY_MAX: f64 = 60.0;

/// The panel's edits, shared with the worker. Both sides hold the lock only
/// to copy the record or change a field.
#[derive(Default)]
pub struct HeadsetShared {
    edits: Mutex<HeadsetEdits>,
}

impl HeadsetShared {
    fn lock(&self) -> MutexGuard<'_, HeadsetEdits> {
        self.edits.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn edit(&self, edit: HeadsetEdit) {
        self.lock().apply(edit);
    }

    /// What is still waiting for the headset.
    pub fn pending(&self) -> HeadsetEdits {
        self.lock().clone()
    }

    /// Take a panel's request to read everything again.
    fn take_refresh(&self) -> bool {
        std::mem::take(&mut self.lock().refresh)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Expect {
    /// A getter: the answer carries the value.
    Read,
    /// A setter: the answer must be a success with argument 0.
    Ack,
}

#[derive(Clone, Copy)]
struct Step {
    report: Report,
    expect: Expect,
}

impl Step {
    fn read(destination: Destination, function: u8, slot: Option<u8>) -> Self {
        Self {
            report: query(destination, function, slot),
            expect: Expect::Read,
        }
    }

    fn ack(report: Report) -> Self {
        Self {
            report,
            expect: Expect::Ack,
        }
    }
}

/// One setting being applied, with the value to find on read-back.
#[derive(Clone, Debug, PartialEq)]
enum Applying {
    Noise(NoiseControl),
    Sidetone(u8),
    Sleep(u8),
    MicEq(Equalizer),
    Headphone { slot: u8, eq: Equalizer },
}

impl Applying {
    fn control(&self) -> Control {
        match self {
            Self::Noise(_) => Control::Noise,
            Self::Sidetone(_) => Control::Sidetone,
            Self::Sleep(_) => Control::Sleep,
            Self::MicEq(_) => Control::MicEq,
            Self::Headphone { .. } => Control::Headphone,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Job {
    /// The receiver's firmware: does the receiver answer at all.
    Probe,
    /// Battery and charging; `full` adds every setting.
    Refresh {
        full: bool,
    },
    Apply(Applying),
}

/// What the next waiting edit needs.
enum EditPlan {
    Apply(Applying),
    /// A headphone slot whose curve or metadata has not been read on this
    /// connection: a full read first.
    NeedsRead,
    /// A headphone slot a completed full read did not find: refuse it.
    Unavailable(u8),
}

/// What the sender should do now.
enum Action {
    Start(Job),
    Unavailable(u8),
    /// Nothing is due before this time.
    Wait(f64),
    /// Nothing can be done with the device as it is.
    Idle,
}

#[derive(Clone, Copy, Debug)]
enum Failure {
    Timeout,
    /// Answered with this status instead of success (2).
    Status(u8),
    /// A setter answered success with a nonzero argument.
    Refused,
    /// A getter answered with a value outside the verified format.
    Unexpected,
    Io,
}

impl Failure {
    fn message(self) -> &'static str {
        match self {
            Self::Timeout => "The headset didn't answer; trying again shortly",
            Self::Status(_) | Self::Refused => "The headset didn't accept the change; trying again shortly",
            Self::Unexpected => "The headset gave an unexpected answer; trying again shortly",
            Self::Io => "Couldn't reach the receiver; trying again shortly",
        }
    }

    /// The device answered: the link itself is fine.
    fn answered(self) -> bool {
        matches!(self, Self::Status(_) | Self::Refused | Self::Unexpected)
    }
}

struct Device {
    file: File,
    node: PathBuf,
    /// The receiver's USB device (`/sys/devices/...`), for its sound card.
    usb: PathBuf,
}

struct Inflight {
    step: Step,
    deadline: f64,
}

pub struct HeadsetLink {
    shared: Arc<HeadsetShared>,
    device: Option<Device>,
    snapshot: HeadsetSnapshot,
    changed: bool,
    job: Option<Job>,
    plan: VecDeque<Step>,
    inflight: Option<Inflight>,
    /// No request goes out before this (late answers are drained).
    quiet_until: f64,
    /// The receiver answered since the interface was opened (or since the
    /// last timeout, after which it is asked again first, at `probe_at`).
    receiver_known: bool,
    probe_at: f64,
    silent_delay: f64,
    /// A full read is owed, from `refresh_at` on (connected or checking).
    full_refresh: bool,
    refresh_at: f64,
    refresh_delay: f64,
    /// A full read finished on this connection (some values may have been
    /// skipped; `snapshot.fresh` says whether all were read).
    full_done: bool,
    /// The full read under way skipped a value the headset refused.
    refresh_partial: bool,
    /// Battery poll while connected; the away recheck otherwise.
    next_poll: f64,
    retry_at: f64,
    retry_delay: f64,
    mic_retry_at: f64,
    mic_retry_delay: f64,
    mixer_card: Option<i32>,
    /// The last discovery failure that was logged, so a missing permission
    /// is logged once rather than every scan.
    logged_open_error: Option<String>,
}

impl HeadsetLink {
    pub fn new(shared: Arc<HeadsetShared>) -> Self {
        Self {
            shared,
            device: None,
            snapshot: HeadsetSnapshot::default(),
            changed: false,
            job: None,
            plan: VecDeque::new(),
            inflight: None,
            quiet_until: 0.0,
            receiver_known: false,
            probe_at: 0.0,
            silent_delay: RETRY_MIN,
            full_refresh: true,
            refresh_at: 0.0,
            refresh_delay: RETRY_MIN,
            full_done: false,
            refresh_partial: false,
            next_poll: 0.0,
            retry_at: 0.0,
            retry_delay: RETRY_MIN,
            mic_retry_at: 0.0,
            mic_retry_delay: RETRY_MIN,
            mixer_card: None,
            logged_open_error: None,
        }
    }

    pub fn snapshot(&self) -> &HeadsetSnapshot {
        &self.snapshot
    }

    pub fn poll_fd(&self) -> Option<RawFd> {
        self.device.as_ref().map(|d| d.file.as_raw_fd())
    }

    /// When the worker must run [`Self::service`] again without an event.
    /// The same decision the sender acts on, so a due time is never left
    /// in the past once a pass has run.
    pub fn deadline(&self, now: f64) -> Option<f64> {
        self.device.as_ref()?;
        if let Some(inflight) = &self.inflight {
            return Some(inflight.deadline);
        }
        if self.job.is_some() {
            return Some(self.quiet_until.max(now));
        }
        let edits = self.shared.pending();
        let mut due = if edits.refresh {
            Some(self.quiet_until.max(now))
        } else {
            match self.decide(now, &edits) {
                Action::Start(_) | Action::Unavailable(_) => Some(self.quiet_until.max(now)),
                Action::Wait(at) => Some(at),
                Action::Idle => None,
            }
        };
        if edits.mic_gain_db100.is_some() && self.mixer_card.is_some() {
            let at = self.mic_retry_at.max(now);
            due = Some(due.map_or(at, |d| d.min(at)));
        }
        due
    }

    /// Look for the receiver (the worker's sysfs cadence). Returns whether
    /// the snapshot changed.
    pub fn scan(&mut self, now: f64) -> bool {
        if let Some(device) = &self.device {
            if !device.node.exists() {
                log!("wm: headset receiver {} went away", device.node.display());
                self.close(now);
            }
            return self.take_changed();
        }
        match discover() {
            None => {
                self.logged_open_error = None;
                self.set_state(LinkState::Absent);
            }
            Some((node, usb)) => {
                let opened = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .custom_flags(O_NONBLOCK)
                    .open(&node);
                match opened {
                    Ok(file) => {
                        log!("wm: headset receiver at {}", node.display());
                        self.logged_open_error = None;
                        self.mixer_card = mixer_linux::find_card(&usb);
                        self.device = Some(Device { file, node, usb });
                        self.receiver_known = false;
                        self.probe_at = now;
                        self.silent_delay = RETRY_MIN;
                        self.full_refresh = true;
                        self.refresh_at = now;
                        self.refresh_delay = RETRY_MIN;
                        self.full_done = false;
                        self.next_poll = now;
                        self.retry_at = now;
                        self.quiet_until = now;
                        self.set_state(LinkState::Checking);
                        self.read_mic_gain();
                    }
                    Err(error) => {
                        let text = format!("{}: {error}", node.display());
                        if self.logged_open_error.as_deref() != Some(&text) {
                            log!("wm: headset receiver not opened: {text}");
                            self.logged_open_error = Some(text);
                        }
                        self.set_state(LinkState::NoAccess);
                    }
                }
            }
        }
        self.take_changed()
    }

    /// One worker pass: take what the receiver sent (`revents` from the
    /// worker's `poll`), time out the request in flight, apply a waiting
    /// microphone gain and send the next request. Returns whether the
    /// snapshot changed.
    pub fn service(&mut self, now: f64, revents: i16) -> bool {
        if self.device.is_some() && revents & (POLLERR | POLLHUP | POLLNVAL) != 0 {
            log!("wm: headset receiver closed its settings interface");
            self.close(now);
            return self.take_changed();
        }
        self.read_frames(now);
        if self.inflight.as_ref().is_some_and(|i| now >= i.deadline) {
            self.fail(now, Failure::Timeout);
        }
        self.apply_mic_gain(now);
        self.advance(now);
        let busy = self.job.is_some();
        if self.snapshot.busy != busy {
            self.snapshot.busy = busy;
            self.changed = true;
        }
        self.take_changed()
    }

    fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }

    fn set_state(&mut self, state: LinkState) {
        if self.snapshot.state == state {
            return;
        }
        log!("wm: headset {:?} -> {:?}", self.snapshot.state, state);
        if state != LinkState::Connected {
            // What this connection read becomes last known; a new
            // connection reads everything again before it is editable.
            let settings = std::mem::take(&mut self.snapshot.settings);
            settings.remember_into(&mut self.snapshot.last_known);
            self.snapshot.fresh = false;
            self.full_done = false;
            self.full_refresh = true;
        }
        self.snapshot.state = state;
        self.changed = true;
    }

    fn close(&mut self, now: f64) {
        self.device = None;
        self.job = None;
        self.plan.clear();
        self.inflight = None;
        self.receiver_known = false;
        self.mixer_card = None;
        // The next receiver (or this one replugged) is read afresh.
        self.snapshot.mic_gain = None;
        self.quiet_until = now;
        self.snapshot.busy = false;
        self.set_state(LinkState::Absent);
        self.changed = true;
    }

    /// The panel opened or the radio link came up: read everything now,
    /// whatever backoff was running.
    fn bring_forward(&mut self, now: f64) {
        self.full_refresh = true;
        self.refresh_at = now;
        self.next_poll = now;
        self.probe_at = self.probe_at.min(now);
    }

    fn problem(&mut self, control: Control, message: &str) {
        self.snapshot.problems.retain(|(c, _)| *c != control);
        self.snapshot.problems.push((control, message.to_string()));
        self.snapshot.problems.sort_by_key(|(c, _)| *c);
        self.changed = true;
    }

    fn solved(&mut self, control: Control) {
        let before = self.snapshot.problems.len();
        self.snapshot.problems.retain(|(c, _)| *c != control);
        self.changed |= self.snapshot.problems.len() != before;
    }

    // ---- receiving ------------------------------------------------------

    fn read_frames(&mut self, now: f64) {
        let mut buffer = [0u8; 256];
        loop {
            let Some(device) = self.device.as_mut() else {
                return;
            };
            match device.file.read(&mut buffer) {
                Ok(0) => return,
                Ok(n) => self.on_report(now, &buffer[..n]),
                Err(error) if error.kind() == ErrorKind::WouldBlock => return,
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(error) => {
                    log!("wm: headset receiver read failed: {error}");
                    self.close(now);
                    return;
                }
            }
        }
    }

    fn on_report(&mut self, now: f64, bytes: &[u8]) {
        // Consumer keys and other input reports on this interface are not
        // frames; neither are damaged reports.
        let Some(frame) = Frame::parse(bytes) else {
            return;
        };
        if frame.kind == ReplyKind::Notification {
            self.on_notification(now, frame);
            return;
        }
        let Some(inflight) = &self.inflight else {
            return; // late: its request already failed
        };
        if !frame.answers(&inflight.step.report) {
            return;
        }
        let step = inflight.step;
        self.inflight = None;
        if frame.destination == Destination::Receiver {
            self.receiver_known = true;
        }
        if !frame.succeeded() {
            self.fail_on(now, frame.function, Failure::Status(frame.status));
            return;
        }
        match step.expect {
            Expect::Ack if !frame.acknowledged() => {
                log!(
                    "wm: headset function {:02x} refused: {:02x?}",
                    frame.function,
                    frame.arguments
                );
                self.fail(now, Failure::Refused);
            }
            Expect::Ack => {}
            Expect::Read => self.on_value(now, frame),
        }
    }

    /// Unsolicited reports: the headset's own buttons and the radio link.
    fn on_notification(&mut self, now: f64, frame: Frame<'_>) {
        if !frame.succeeded() {
            return;
        }
        if frame.destination == Destination::Receiver {
            // Function 20 with 01 00 is the only link report verified: the
            // radio link came up. Nothing is assumed about other values.
            if frame.function == 0x20 && frame.arguments == [1, 0] {
                log!("wm: headset radio link up");
                self.bring_forward(now);
            }
            return;
        }
        // A push while not connected is not this connection's state.
        if self.snapshot.state == LinkState::Connected && self.snapshot.settings.observe(frame) {
            if frame.function == 0x21 {
                self.snapshot.battery_at = Some(now);
            }
            self.changed = true;
        }
    }

    fn on_value(&mut self, now: f64, frame: Frame<'_>) {
        match (frame.destination, frame.function, frame.arguments) {
            (Destination::Receiver, 0x02, [a, b, c, d, ..]) => {
                let firmware = Some([*a, *b, *c, *d]);
                self.changed |= self.snapshot.receiver_firmware != firmware;
                self.snapshot.receiver_firmware = firmware;
            }
            (Destination::Headset, 0x02, bytes) if bytes.len() >= 8 => {
                let mut firmware = [0; 8];
                firmware.copy_from_slice(&bytes[..8]);
                self.changed |= self.snapshot.headset_firmware != Some(firmware);
                self.snapshot.headset_firmware = Some(firmware);
            }
            (Destination::Headset, 0x21, [0xff]) => {
                // The receiver's "unavailable": the headset is asleep, off
                // or out of range. Its settings cannot be read now; look
                // again after the away interval, not before.
                self.plan.clear();
                self.job = None;
                self.next_poll = now + AWAY_POLL_INTERVAL;
                self.set_state(LinkState::HeadsetAway);
            }
            _ => {
                // Battery first in every read: a valid one is contact.
                if frame.function == 0x21 && frame.destination == Destination::Headset {
                    if frame.arguments.first().is_some_and(|b| *b <= 100) {
                        self.set_state(LinkState::Connected);
                    }
                }
                if !self.snapshot.settings.observe(frame) {
                    log!(
                        "wm: headset function {:02x} answered {:02x?}",
                        frame.function,
                        frame.arguments
                    );
                    self.fail(now, Failure::Unexpected);
                    return;
                }
                if frame.function == 0x21 {
                    self.snapshot.battery_at = Some(now);
                }
                self.changed = true;
            }
        }
    }

    fn fail_on(&mut self, now: f64, function: u8, failure: Failure) {
        if let Failure::Status(status) = failure {
            log!("wm: headset function {function:02x} answered status {status:02x}");
        }
        self.fail(now, failure);
    }

    fn fail(&mut self, now: f64, failure: Failure) {
        self.inflight = None;
        // A full read skips a value the headset refuses and goes on; the
        // receiver answering anything at all completes the probe.
        if failure.answered() {
            match self.job {
                Some(Job::Refresh { full: true }) => {
                    self.refresh_partial = true;
                    return;
                }
                Some(Job::Probe) => return,
                _ => {}
            }
        }
        let job = self.job.take();
        log!("wm: headset {:?} failed: {:?}", job, failure);
        self.plan.clear();
        self.quiet_until = now + QUIET_AFTER_FAILURE;
        if !failure.answered() {
            // Ask the receiver first next time: it tells a silent receiver
            // from a headset that went away.
            self.receiver_known = false;
            self.probe_at = self.quiet_until;
        }
        match job {
            Some(Job::Probe) => {
                self.probe_at = now + self.silent_delay;
                self.silent_delay = (self.silent_delay * 2.0).min(RETRY_MAX);
                self.set_state(LinkState::ReceiverSilent);
            }
            Some(Job::Refresh { full }) => match failure {
                Failure::Timeout => {
                    // The headset stopped answering: away, rechecked after
                    // the away interval (the receiver is asked once first).
                    self.next_poll = now + AWAY_POLL_INTERVAL;
                    self.set_state(LinkState::HeadsetAway);
                }
                _ => {
                    let at = now + self.refresh_delay;
                    self.refresh_delay = (self.refresh_delay * 2.0).min(RETRY_MAX);
                    if !self.receiver_known {
                        self.probe_at = at;
                    }
                    if full {
                        self.refresh_at = at;
                    } else {
                        self.next_poll = at;
                    }
                }
            },
            Some(Job::Apply(applying)) => {
                let control = applying.control();
                self.problem(control, failure.message());
                self.retry_at = now + self.retry_delay;
                self.retry_delay = (self.retry_delay * 2.0).min(RETRY_MAX);
                // Part of the sequence may have landed: that control is
                // unknown until read again, and so is the snapshot.
                self.snapshot.settings.remember_into(&mut self.snapshot.last_known);
                self.snapshot.settings.forget(control);
                self.snapshot.fresh = false;
                self.full_done = false;
                self.full_refresh = true;
                self.refresh_at = self.refresh_at.max(self.quiet_until);
            }
            None => {}
        }
        self.changed = true;
    }

    // ---- sending --------------------------------------------------------

    fn advance(&mut self, now: f64) {
        while self.device.is_some() && self.inflight.is_none() && now >= self.quiet_until {
            if let Some(step) = self.plan.pop_front() {
                self.send(now, step);
                continue;
            }
            if let Some(job) = self.job.take() {
                self.finish(now, job);
                continue;
            }
            if self.shared.take_refresh() {
                self.bring_forward(now);
            }
            let edits = self.shared.pending();
            match self.decide(now, &edits) {
                Action::Start(job) => self.start(job),
                Action::Unavailable(slot) => self.refuse_slot(slot),
                Action::Wait(_) | Action::Idle => return,
            }
        }
    }

    fn send(&mut self, now: f64, step: Step) {
        let Some(device) = self.device.as_mut() else {
            return;
        };
        match device.file.write(&step.report) {
            Ok(n) if n == step.report.len() => {
                self.inflight = Some(Inflight {
                    step,
                    deadline: now + REPLY_TIMEOUT,
                });
            }
            Ok(n) => {
                log!("wm: headset request written short ({n} bytes)");
                self.fail(now, Failure::Io);
            }
            Err(error) if matches!(error.raw_os_error(), Some(ENODEV | ESHUTDOWN)) => {
                log!("wm: headset receiver gone: {error}");
                self.close(now);
            }
            Err(error) => {
                log!("wm: headset request not written: {error}");
                self.fail(now, Failure::Io);
            }
        }
    }

    /// What to start now, or when something next becomes due. Pure: the
    /// sender and the worker's deadline both ask it. Candidates are in
    /// priority order; the first that is due starts.
    fn decide(&self, now: f64, edits: &HeadsetEdits) -> Action {
        if self.device.is_none() {
            return Action::Idle;
        }
        let mut candidates: Vec<(f64, Action)> = Vec::new();
        if !self.receiver_known {
            candidates.push((self.probe_at, Action::Start(Job::Probe)));
        } else {
            match self.snapshot.state {
                LinkState::Connected => {
                    if self.full_refresh {
                        candidates.push((self.refresh_at, Action::Start(Job::Refresh { full: true })));
                    }
                    match self.edit_plan(edits) {
                        Some(EditPlan::Apply(applying)) => {
                            candidates.push((self.retry_at, Action::Start(Job::Apply(applying))))
                        }
                        Some(EditPlan::NeedsRead) if !self.full_refresh => {
                            candidates.push((self.retry_at, Action::Start(Job::Refresh { full: true })))
                        }
                        Some(EditPlan::NeedsRead) | None => {}
                        Some(EditPlan::Unavailable(slot)) => candidates.push((now, Action::Unavailable(slot))),
                    }
                    candidates.push((self.next_poll, Action::Start(Job::Refresh { full: false })));
                }
                LinkState::Checking | LinkState::ReceiverSilent => {
                    candidates.push((self.refresh_at, Action::Start(Job::Refresh { full: true })));
                }
                // An owed full read waits for the away recheck.
                LinkState::HeadsetAway => {
                    candidates.push((self.next_poll, Action::Start(Job::Refresh { full: true })));
                }
                LinkState::Absent | LinkState::NoAccess => return Action::Idle,
            }
        }
        let mut earliest = f64::INFINITY;
        for (at, action) in candidates {
            let at = at.max(self.quiet_until);
            if at <= now {
                return action;
            }
            earliest = earliest.min(at);
        }
        if earliest.is_finite() {
            Action::Wait(earliest)
        } else {
            Action::Idle
        }
    }

    /// The next waiting edit and what it needs. Band edits of a slot that
    /// is no longer the chosen one go first: the chosen slot is applied
    /// last, since applying a slot activates it.
    fn edit_plan(&self, edits: &HeadsetEdits) -> Option<EditPlan> {
        if let Some(noise) = edits.noise {
            return Some(EditPlan::Apply(Applying::Noise(noise)));
        }
        if let Some(level) = edits.sidetone {
            return Some(EditPlan::Apply(Applying::Sidetone(level)));
        }
        if let Some(minutes) = edits.sleep_minutes {
            return Some(EditPlan::Apply(Applying::Sleep(minutes)));
        }
        if let Some(eq) = edits.mic_eq {
            return Some(EditPlan::Apply(Applying::MicEq(eq)));
        }
        let target = edits.headphone_active;
        let slot = edits
            .headphone_bands
            .keys()
            .copied()
            .find(|s| Some(*s) != target)
            .or(target)?;
        let eq = edits
            .headphone_bands
            .get(&slot)
            .copied()
            .or_else(|| self.preset_eq(slot));
        Some(match (eq, self.preset_metadata(slot)) {
            (Some(eq), Some(_)) => EditPlan::Apply(Applying::Headphone { slot, eq }),
            _ if self.full_done => EditPlan::Unavailable(slot),
            _ => EditPlan::NeedsRead,
        })
    }

    fn refuse_slot(&mut self, slot: u8) {
        log!("wm: headset preset {slot} not found by the full read");
        self.problem(Control::Headphone, "This preset isn't available on the headset");
        let mut shared = self.shared.lock();
        shared.headphone_bands.remove(&slot);
        if shared.headphone_active == Some(slot) {
            shared.headphone_active = None;
        }
    }

    fn preset_eq(&self, slot: u8) -> Option<Equalizer> {
        self.snapshot.settings.headphone_presets.get(&slot).and_then(|p| p.eq)
    }

    fn preset_metadata(&self, slot: u8) -> Option<[u8; 6]> {
        self.snapshot
            .settings
            .headphone_presets
            .get(&slot)
            .and_then(|p| p.metadata)
    }

    fn start(&mut self, job: Job) {
        self.plan.clear();
        match &job {
            Job::Probe => self.plan.push_back(Step::read(Destination::Receiver, 0x02, None)),
            Job::Refresh { full } => {
                self.refresh_partial = false;
                // Battery first: "unavailable" there ends the read early.
                self.plan.push_back(Step::read(Destination::Headset, 0x21, None));
                self.plan.push_back(Step::read(Destination::Headset, 0x2a, None));
                if *full {
                    for function in [0x02, 0x12, 0x19, 0x2c, 0x16, 0x17] {
                        self.plan.push_back(Step::read(Destination::Headset, function, None));
                    }
                    for slot in 0..HEADPHONE_EDITABLE_SLOTS {
                        self.plan.push_back(Step::read(Destination::Headset, 0x15, Some(slot)));
                    }
                    for slot in 0..HEADPHONE_READ_SLOTS {
                        self.plan.push_back(Step::read(Destination::Headset, 0x60, Some(slot)));
                    }
                }
            }
            Job::Apply(applying) => match self.apply_plan(applying) {
                Ok(steps) => {
                    log!("wm: headset applying {applying:?}");
                    self.plan.extend(steps);
                }
                Err(message) => {
                    log!("wm: headset edit {applying:?} refused: {message}");
                    self.problem(applying.control(), message);
                    self.clear_edit(applying);
                    return;
                }
            },
        }
        self.job = Some(job);
    }

    fn apply_plan(&self, applying: &Applying) -> Result<Vec<Step>, &'static str> {
        let read = |function, slot| Step::read(Destination::Headset, function, slot);
        Ok(match *applying {
            Applying::Noise(noise) => vec![Step::ack(noise.write()?), read(0x12, None)],
            Applying::Sidetone(level) => {
                let [enable, set] = sidetone_write(level)?;
                vec![Step::ack(enable), Step::ack(set), read(0x19, None)]
            }
            Applying::Sleep(minutes) => vec![Step::ack(sleep_write(minutes)?), read(0x2c, None)],
            Applying::MicEq(eq) => {
                let [preset, bands] = eq.microphone_write(MIC_CUSTOM_PRESET)?;
                vec![Step::ack(preset), Step::ack(bands), read(0x16, None), read(0x17, None)]
            }
            Applying::Headphone { slot, eq } => {
                let preset = self
                    .snapshot
                    .settings
                    .headphone_presets
                    .get(&slot)
                    .cloned()
                    .unwrap_or_default();
                let mut steps: Vec<Step> = preset.write(eq)?.into_iter().map(Step::ack).collect();
                steps.push(read(0x15, Some(slot)));
                // Every slot's active flag, so no stale flag survives.
                steps.extend((0..HEADPHONE_READ_SLOTS).map(|s| read(0x60, Some(s))));
                steps
            }
        })
    }

    fn finish(&mut self, now: f64, job: Job) {
        match job {
            Job::Probe => {
                self.receiver_known = true;
                self.silent_delay = RETRY_MIN;
                if self.snapshot.state == LinkState::ReceiverSilent {
                    self.set_state(LinkState::Checking);
                }
            }
            Job::Refresh { full } => {
                if self.snapshot.state != LinkState::Connected {
                    // The battery never gave a value (the headset refused
                    // it): treat the headset as away, rechecked later.
                    self.next_poll = now + AWAY_POLL_INTERVAL;
                    self.set_state(LinkState::HeadsetAway);
                    return;
                }
                if full {
                    self.full_refresh = false;
                    self.full_done = true;
                    self.refresh_delay = RETRY_MIN;
                    self.snapshot.fresh = !self.refresh_partial;
                }
                self.next_poll = now + POLL_INTERVAL;
                self.read_mic_gain();
                self.changed = true;
            }
            Job::Apply(applying) => {
                let settings = &self.snapshot.settings;
                let applied = match applying {
                    Applying::Noise(noise) => settings.noise.is_some_and(|read| noise.applied_in(read)),
                    Applying::Sidetone(level) => settings.sidetone == Some(level),
                    Applying::Sleep(minutes) => settings.sleep_minutes == Some(minutes),
                    Applying::MicEq(eq) => {
                        settings.mic_preset == Some(MIC_CUSTOM_PRESET) && settings.mic_eq == Some(eq)
                    }
                    Applying::Headphone { slot, eq } => {
                        self.preset_eq(slot) == Some(eq) && settings.active_headphone_preset() == Some(slot)
                    }
                };
                if applied {
                    log!("wm: headset applied {applying:?}");
                    self.solved(applying.control());
                    self.retry_delay = RETRY_MIN;
                } else {
                    // Every step was accepted and the headset reports
                    // another value: retrying would loop. Say so.
                    log!("wm: headset kept another value after {applying:?}");
                    self.problem(applying.control(), "The headset kept a different setting");
                }
                self.clear_edit(&applying);
                self.changed = true;
            }
        }
    }

    /// Clear the edit just resolved, unless the person changed it again
    /// meanwhile (then the newer value is applied next).
    fn clear_edit(&self, applying: &Applying) {
        let mut shared = self.shared.lock();
        match *applying {
            Applying::Noise(noise) if shared.noise == Some(noise) => shared.noise = None,
            Applying::Sidetone(level) if shared.sidetone == Some(level) => shared.sidetone = None,
            Applying::Sleep(minutes) if shared.sleep_minutes == Some(minutes) => shared.sleep_minutes = None,
            Applying::MicEq(eq) if shared.mic_eq == Some(eq) => shared.mic_eq = None,
            Applying::Headphone { slot, eq } => {
                // Bands for this slot that were applied are done; the
                // choice of active slot is done once no newer bands wait
                // for it.
                let bands = shared.headphone_bands.get(&slot).copied();
                if bands == Some(eq) {
                    shared.headphone_bands.remove(&slot);
                }
                if shared.headphone_active == Some(slot) && bands.is_none_or(|b| b == eq) {
                    shared.headphone_active = None;
                }
            }
            _ => {}
        }
    }

    // ---- microphone gain (ALSA, not a vendor command) --------------------

    fn card(&mut self) -> Option<i32> {
        if self.mixer_card.is_none() {
            self.mixer_card = self.device.as_ref().and_then(|d| mixer_linux::find_card(&d.usb));
        }
        self.mixer_card
    }

    fn read_mic_gain(&mut self) {
        let Some(card) = self.card() else {
            return;
        };
        match mixer_linux::read(card) {
            Ok(gain) => {
                self.changed |= self.snapshot.mic_gain != Some(gain);
                self.snapshot.mic_gain = Some(gain);
            }
            Err(error) => {
                log!("wm: headset microphone gain not read: {error}");
                // The card may have been renumbered: look it up next time,
                // and show nothing rather than an unconfirmed value.
                self.mixer_card = None;
                self.changed |= self.snapshot.mic_gain.is_some();
                self.snapshot.mic_gain = None;
            }
        }
    }

    fn apply_mic_gain(&mut self, now: f64) {
        if now < self.mic_retry_at || self.device.is_none() {
            return;
        }
        let Some(requested) = self.shared.pending().mic_gain_db100 else {
            return;
        };
        let Some(card) = self.card() else {
            return; // no sound card for this receiver yet: it waits
        };
        match mixer_linux::write(card, requested) {
            Ok(gain) => {
                self.snapshot.mic_gain = Some(gain);
                self.mic_retry_delay = RETRY_MIN;
                self.solved(Control::MicGain);
                let mut shared = self.shared.lock();
                if shared.mic_gain_db100 == Some(requested) {
                    shared.mic_gain_db100 = None;
                }
            }
            Err(error) => {
                log!("wm: headset microphone gain not set: {error}");
                self.mixer_card = None;
                self.mic_retry_at = now + self.mic_retry_delay;
                self.mic_retry_delay = (self.mic_retry_delay * 2.0).min(RETRY_MAX);
                self.problem(
                    Control::MicGain,
                    "Couldn't change the microphone gain; trying again shortly",
                );
            }
        }
        self.changed = true;
    }
}

/// The receiver's hidraw node and its USB device: the hidraw device whose
/// HID id is 1532:0577 on USB interface 5. The first by name when there are
/// several receivers.
fn discover() -> Option<(PathBuf, PathBuf)> {
    let mut entries: Vec<_> = fs::read_dir("/sys/class/hidraw").ok()?.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let hid = entry.path().join("device");
        let Ok(uevent) = fs::read_to_string(hid.join("uevent")) else {
            continue;
        };
        if !uevent.lines().any(|line| line.eq_ignore_ascii_case(HID_ID)) {
            continue;
        }
        let Ok(hid) = fs::canonicalize(&hid) else {
            continue;
        };
        let Some(interface) = hid.parent() else {
            continue;
        };
        let number = fs::read_to_string(interface.join("bInterfaceNumber")).unwrap_or_default();
        if number.trim() != INTERFACE {
            continue;
        }
        let Some(usb) = interface.parent() else {
            continue;
        };
        return Some((Path::new("/dev").join(entry.file_name()), usb.to_path_buf()));
    }
    None
}

/// What a remote snapshot reads about the headset (no secrets exist here).
pub fn describe(snapshot: &HeadsetSnapshot, pending: &HeadsetEdits) -> String {
    let s = &snapshot.settings;
    let mut text = format!(
        "headset: state={:?} fresh={} busy={} battery={:?} charging={:?}",
        snapshot.state, snapshot.fresh, snapshot.busy, s.battery_percent, s.charging
    );
    if let Some(firmware) = snapshot.receiver_firmware {
        text.push_str(&format!(" receiver_fw={}", firmware_label(&firmware)));
    }
    if let Some(firmware) = snapshot.headset_firmware {
        text.push_str(&format!(" headset_fw={}", firmware_label(&firmware)));
    }
    text.push_str(&format!(
        "\nheadset: noise={:?} sidetone={:?} sleep={:?} mic_preset={:?} mic_eq={:?}",
        s.noise,
        s.sidetone,
        s.sleep_minutes,
        s.mic_preset,
        s.mic_eq.map(|e| e.bands)
    ));
    text.push_str(&format!("\nheadset: active_preset={:?}", s.active_headphone_preset()));
    for (slot, preset) in &s.headphone_presets {
        text.push_str(&format!(
            "\nheadset: preset {slot} eq={:?} metadata={:02x?}",
            preset.eq.map(|e| e.bands),
            preset.metadata
        ));
    }
    if let Some(battery) = snapshot
        .last_known
        .battery_percent
        .filter(|_| s.battery_percent.is_none())
    {
        text.push_str(&format!("\nheadset: last_known_battery={battery}"));
    }
    if let Some(gain) = snapshot.mic_gain {
        text.push_str(&format!(
            "\nheadset: mic_gain_db={:.2} range={:.2}..{:.2}",
            gain.db100 as f64 / 100.0,
            gain.min_db100 as f64 / 100.0,
            gain.max_db100 as f64 / 100.0
        ));
    }
    if !pending.is_empty() {
        text.push_str(&format!("\nheadset: pending={pending:?}"));
    }
    for (control, message) in &snapshot.problems {
        text.push_str(&format!("\nheadset: problem {control:?}: {message}"));
    }
    text
}
